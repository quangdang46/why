mod cli;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::CommandFactory;
use clap::Parser;
use clap_complete::{Generator, Shell, generate};
use clap_mangen::Man;
use cli::{Cli, CompletionShell, Mode, QueryRequest};
use git2::Repository;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};
use why_annotator::writer::annotate_file;
use why_archaeologist::{
    ArchaeologyResult, BlameChainResult, EvolutionHistoryResult, TeamReport, analyze_blame_chain,
    analyze_evolution_history, analyze_target_with_options, analyze_team,
};
use why_cache::{Cache, HealthSnapshot};
use why_context::{
    LlmConfigLayer, LlmProvider, WhyConfig, WhyConfigLayer, global_config_path, load_config,
    load_config_layer_from_path, local_config_target_path, write_config_layer_to_path,
};
use why_evidence::{
    EvidenceCommit, EvidenceContext, EvidencePack, EvidenceTarget, GitHubClient, GitHubComment,
    GitHubEnrichment, enrich_github_refs, parse_github_ref, parse_github_remote,
    select_single_github_ref,
};
use why_locator::QueryKind;
use why_scanner::{
    CouplingReport, CoverageGapReport, DiffReviewPlan, DiffReviewTarget, GhostFinding,
    HealthBaselineReference, HealthComparison, HealthDelta, HealthGateSummary, HealthReport,
    HealthSignalDelta, HotspotFinding, OnboardFinding, OutageReport, PrTemplateReport,
    RenameSafeReport, Severity, TimeBombFinding, TimeBombKind,
};
use why_splitter::SplitSuggestion;
use why_synthesizer::{
    ConfidenceLevel, DiffReviewFinding, DiffReviewReport, ReportMode, WhyReport,
    build_diff_review_prompt, build_query_prompt, build_system_prompt, client_from_config,
    heuristic_diff_review_report, heuristic_report, prompt_contract,
    synthesize_diff_review as llm_synthesize_diff_review,
    synthesize_report as llm_synthesize_report,
};

fn main() {
    match run() {
        Ok(ExitStatus::Success) => {}
        Ok(ExitStatus::HealthCiFailure { message }) => {
            eprintln!("why: {message}");
            std::process::exit(3);
        }
        Ok(ExitStatus::HealthRegressionFailure { message }) => {
            eprintln!("why: {message}");
            std::process::exit(4);
        }
        Err(error) => {
            eprintln!("why: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<ExitStatus> {
    let cli = Cli::parse();
    let mode = cli.parse_mode()?;

    match mode {
        Mode::Mcp => {
            why_mcp::run_stdio()?;
            Ok(ExitStatus::Success)
        }
        Mode::ConfigInit {
            local,
            provider,
            model,
            base_url,
            auth_token,
            retries,
            max_tokens,
            timeout,
        } => {
            run_config_init(ConfigInitArgs {
                local,
                provider,
                model,
                base_url,
                auth_token,
                retries,
                max_tokens,
                timeout,
            })?;
            Ok(ExitStatus::Success)
        }
        Mode::ConfigGet { json } => {
            run_config_get(json)?;
            Ok(ExitStatus::Success)
        }
        Mode::Shell => {
            why_shell::run()?;
            Ok(ExitStatus::Success)
        }
        Mode::Lsp => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(why_lsp::run_stdio())?;
            Ok(ExitStatus::Success)
        }
        Mode::ContextInject => {
            run_context_inject()?;
            Ok(ExitStatus::Success)
        }
        Mode::Hotspots { limit, owner, json } => {
            run_hotspots(limit, owner.as_deref(), json)?;
            Ok(ExitStatus::Success)
        }
        Mode::Health {
            json,
            ci,
            baseline_file,
            write_baseline,
            max_regression,
            max_signal_regression,
            require_baseline,
        } => run_health(
            json,
            ci,
            HealthBaselineOptions {
                baseline_file,
                write_baseline,
                max_regression,
                max_signal_regression: max_signal_regression
                    .into_iter()
                    .map(|raw| {
                        let (signal, count) = raw
                            .split_once('=')
                            .expect("health signal budgets should be validated by CLI");
                        (
                            signal.trim().to_string(),
                            count
                                .trim()
                                .parse::<u32>()
                                .expect("health signal budget counts should be valid"),
                        )
                    })
                    .collect(),
                require_baseline,
            },
        ),
        Mode::PrTemplate { json } => {
            run_pr_template(json)?;
            Ok(ExitStatus::Success)
        }
        Mode::ExplainOutage {
            from,
            to,
            limit,
            json,
        } => {
            run_explain_outage(&from, &to, limit, json)?;
            Ok(ExitStatus::Success)
        }
        Mode::DiffReview {
            json,
            no_llm,
            post_github_comment,
            github_ref,
        } => {
            run_diff_review(json, no_llm, post_github_comment, github_ref.as_deref())?;
            Ok(ExitStatus::Success)
        }
        Mode::CoverageGap {
            coverage,
            limit,
            max_coverage,
            json,
        } => {
            run_coverage_gap(&coverage, limit, max_coverage, json)?;
            Ok(ExitStatus::Success)
        }
        Mode::Ghost { limit, json } => {
            run_ghost(limit, json)?;
            Ok(ExitStatus::Success)
        }
        Mode::Onboard { limit, json } => {
            run_onboard(limit, json)?;
            Ok(ExitStatus::Success)
        }
        Mode::TimeBombs { age_days, json } => {
            run_time_bombs(age_days, json)?;
            Ok(ExitStatus::Success)
        }
        Mode::Query(request) => {
            run_query(request)?;
            Ok(ExitStatus::Success)
        }
        Mode::InstallHooks { warn_only } => {
            run_install_hooks(warn_only)?;
            Ok(ExitStatus::Success)
        }
        Mode::UninstallHooks => {
            run_uninstall_hooks()?;
            Ok(ExitStatus::Success)
        }
        Mode::Completions { shell } => {
            run_completions(shell)?;
            Ok(ExitStatus::Success)
        }
        Mode::Manpage => {
            run_manpage()?;
            Ok(ExitStatus::Success)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExitStatus {
    Success,
    HealthCiFailure { message: String },
    HealthRegressionFailure { message: String },
}

#[derive(Debug, Clone)]
struct HealthBaselineOptions {
    baseline_file: Option<PathBuf>,
    write_baseline: Option<PathBuf>,
    max_regression: Option<u32>,
    max_signal_regression: Vec<(String, u32)>,
    require_baseline: bool,
}

#[derive(Debug, Clone)]
struct HealthReportContext {
    report: HealthReport,
    snapshot: HealthSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HealthBaselineFile {
    schema_version: u32,
    snapshot: HealthSnapshot,
}

#[derive(Debug, Clone)]
struct DiffReviewCollectedTarget {
    target: DiffReviewTarget,
    result: ArchaeologyResult,
    evidence_pack: EvidencePack,
}

#[derive(Debug, Clone, Default)]
struct DiffReviewCollected {
    entries: Vec<DiffReviewCollectedTarget>,
    issue_refs: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileWatchState {
    modified_at: Option<std::time::SystemTime>,
    len: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigReport {
    paths: ConfigPathReport,
    risk: ConfigRiskReport,
    git: ConfigGitReport,
    cache: ConfigCacheReport,
    github: ConfigGitHubReport,
    llm: ConfigLlmReport,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigPathReport {
    global: Option<ConfigPathEntry>,
    local: Option<ConfigPathEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigPathEntry {
    path: String,
    exists: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigRiskReport {
    default_level: String,
    high_keywords: Vec<String>,
    medium_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigGitReport {
    max_commits: usize,
    recency_window_days: i64,
    mechanical_threshold_files: usize,
    coupling_scan_commits: usize,
    coupling_ratio_threshold: f64,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigCacheReport {
    max_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigGitHubReport {
    remote: String,
    token_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigLlmReport {
    provider: LlmProvider,
    model: Option<String>,
    base_url: Option<String>,
    retries: u32,
    max_tokens: u32,
    timeout: u64,
    auth_configured: bool,
}

#[derive(Debug, Clone)]
struct ConfigInitArgs {
    local: bool,
    provider: Option<LlmProvider>,
    model: Option<String>,
    base_url: Option<String>,
    auth_token: Option<String>,
    retries: Option<u32>,
    max_tokens: Option<u32>,
    timeout: Option<u64>,
}

#[derive(Debug, Clone)]
struct ConfigInitValues {
    local: bool,
    provider: LlmProvider,
    model: Option<String>,
    base_url: Option<String>,
    auth_token: Option<String>,
    retries: Option<u32>,
    max_tokens: Option<u32>,
    timeout: Option<u64>,
}

fn run_config_init(args: ConfigInitArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let current = load_config(&cwd)?;
    let current_provider = current.llm.provider;
    let values = resolve_config_init_values(&current, args)?;

    let target_path = config_target_path(&cwd, values.local)?;
    let mut layer = load_existing_config_layer(&target_path)?;
    apply_config_init_to_layer(&mut layer, &values, values.provider != current_provider);
    write_config_layer_to_path(&target_path, &layer)?;

    println!(
        "Wrote {} config to {}",
        if values.local { "local" } else { "global" },
        target_path.display()
    );
    println!("Provider: {}", values.provider);

    Ok(())
}

fn resolve_config_init_values(
    current: &WhyConfig,
    args: ConfigInitArgs,
) -> Result<ConfigInitValues> {
    let current_provider = current.llm.provider;
    if args.provider.is_none()
        && args.model.is_none()
        && args.base_url.is_none()
        && args.auth_token.is_none()
        && args.retries.is_none()
        && args.max_tokens.is_none()
        && args.timeout.is_none()
    {
        ensure_interactive_terminal("why config init")?;
        let provider = prompt_provider(current_provider)?;
        let resolved_current = current.resolved_llm_config();
        let keep_provider_defaults = provider != current_provider;
        let model = prompt_optional_text(
            "Model (blank to keep current/default)",
            if keep_provider_defaults {
                provider.default_model()
            } else {
                resolved_current
                    .model
                    .as_deref()
                    .or_else(|| provider.default_model())
            },
        )?;
        let base_url = prompt_optional_text(
            "Base URL (blank to keep current/default)",
            if keep_provider_defaults {
                provider.default_base_url()
            } else {
                resolved_current
                    .base_url
                    .as_deref()
                    .or_else(|| provider.default_base_url())
            },
        )?;
        let auth_token = prompt_optional_text("Auth token (blank to keep current/skip)", None)?;
        let retries = prompt_optional_u32(
            "Retries (blank to keep current/default)",
            Some(resolved_current.retries),
        )?;
        let max_tokens = prompt_optional_u32(
            "Max tokens (blank to keep current/default)",
            Some(resolved_current.max_tokens),
        )?;
        let timeout = prompt_optional_u64(
            "Timeout seconds (blank to keep current/default)",
            Some(resolved_current.timeout),
        )?;

        return Ok(ConfigInitValues {
            provider,
            model,
            base_url,
            auth_token,
            retries,
            max_tokens,
            timeout,
            local: args.local,
        });
    }

    Ok(ConfigInitValues {
        provider: args.provider.unwrap_or(current_provider),
        model: args.model,
        base_url: args.base_url,
        auth_token: args.auth_token,
        retries: args.retries,
        max_tokens: args.max_tokens,
        timeout: args.timeout,
        local: args.local,
    })
}

fn run_config_get(json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let report = build_config_report(&cwd)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_config_report(&report);
    }

    Ok(())
}

fn build_config_report(cwd: &Path) -> Result<ConfigReport> {
    let config = load_config(cwd)?;
    let resolved_llm = config.resolved_llm_config();

    Ok(ConfigReport {
        paths: ConfigPathReport {
            global: config_path_entry(global_config_path()),
            local: config_path_entry(local_config_target_path(cwd)),
        },
        risk: ConfigRiskReport {
            default_level: config.risk.default_level.clone(),
            high_keywords: config.risk.keywords.high.clone(),
            medium_keywords: config.risk.keywords.medium.clone(),
        },
        git: ConfigGitReport {
            max_commits: config.git.max_commits,
            recency_window_days: config.git.recency_window_days,
            mechanical_threshold_files: config.git.mechanical_threshold_files,
            coupling_scan_commits: config.git.coupling_scan_commits,
            coupling_ratio_threshold: config.git.coupling_ratio_threshold,
        },
        cache: ConfigCacheReport {
            max_entries: config.cache.max_entries,
        },
        github: ConfigGitHubReport {
            remote: config.github.remote.clone(),
            token_configured: config.github_token().is_some(),
        },
        llm: ConfigLlmReport {
            provider: resolved_llm.provider,
            model: resolved_llm.model,
            base_url: resolved_llm.base_url,
            retries: resolved_llm.retries,
            max_tokens: resolved_llm.max_tokens,
            timeout: resolved_llm.timeout,
            auth_configured: resolved_llm.auth_token.is_some(),
        },
    })
}

fn render_config_report(report: &ConfigReport) {
    println!("why config");
    println!();
    println!("Paths");
    render_config_path_line("global", report.paths.global.as_ref());
    render_config_path_line("local", report.paths.local.as_ref());
    println!();
    println!("LLM");
    println!("  provider: {}", report.llm.provider);
    println!(
        "  model: {}",
        report.llm.model.as_deref().unwrap_or("<provider default>")
    );
    println!(
        "  base_url: {}",
        report.llm.base_url.as_deref().unwrap_or("<none>")
    );
    println!("  retries: {}", report.llm.retries);
    println!("  max_tokens: {}", report.llm.max_tokens);
    println!("  timeout: {}", report.llm.timeout);
    println!(
        "  auth_configured: {}",
        if report.llm.auth_configured {
            "yes"
        } else {
            "no"
        }
    );
    println!();
    println!("GitHub");
    println!("  remote: {}", report.github.remote);
    println!(
        "  token_configured: {}",
        if report.github.token_configured {
            "yes"
        } else {
            "no"
        }
    );
    println!();
    println!("Risk");
    println!("  default_level: {}", report.risk.default_level);
    println!(
        "  high_keywords: {}",
        comma_or_none(&report.risk.high_keywords)
    );
    println!(
        "  medium_keywords: {}",
        comma_or_none(&report.risk.medium_keywords)
    );
    println!();
    println!("Git");
    println!("  max_commits: {}", report.git.max_commits);
    println!("  recency_window_days: {}", report.git.recency_window_days);
    println!(
        "  mechanical_threshold_files: {}",
        report.git.mechanical_threshold_files
    );
    println!(
        "  coupling_scan_commits: {}",
        report.git.coupling_scan_commits
    );
    println!(
        "  coupling_ratio_threshold: {}",
        report.git.coupling_ratio_threshold
    );
    println!();
    println!("Cache");
    println!("  max_entries: {}", report.cache.max_entries);
}

fn render_config_path_line(label: &str, entry: Option<&ConfigPathEntry>) {
    match entry {
        Some(entry) => println!(
            "  {label}: {} ({})",
            entry.path,
            if entry.exists { "present" } else { "missing" }
        ),
        None => println!("  {label}: <unavailable>"),
    }
}

fn comma_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn config_path_entry(path: Option<PathBuf>) -> Option<ConfigPathEntry> {
    path.map(|path| ConfigPathEntry {
        exists: path.is_file(),
        path: normalize_path(&path),
    })
}

fn config_target_path(cwd: &Path, local: bool) -> Result<PathBuf> {
    if local {
        local_config_target_path(cwd).ok_or_else(|| {
            anyhow!(
                "could not determine a repo-local config path from {}",
                cwd.display()
            )
        })
    } else {
        global_config_path().ok_or_else(|| {
            anyhow!("could not determine a global config path; set XDG_CONFIG_HOME or HOME")
        })
    }
}

fn load_existing_config_layer(path: &Path) -> Result<WhyConfigLayer> {
    if path.is_file() {
        load_config_layer_from_path(path)
    } else {
        Ok(WhyConfigLayer::default())
    }
}

fn apply_config_init_to_layer(
    layer: &mut WhyConfigLayer,
    values: &ConfigInitValues,
    reset_provider_overrides: bool,
) {
    let llm = layer.llm.get_or_insert_with(LlmConfigLayer::default);
    llm.provider = Some(values.provider);
    if reset_provider_overrides {
        llm.model = None;
        llm.base_url = None;
        llm.auth_token = None;
    }
    if let Some(model) = values.model.clone() {
        llm.model = Some(model);
    }
    if let Some(base_url) = values.base_url.clone() {
        llm.base_url = Some(base_url);
    }
    if let Some(auth_token) = values.auth_token.clone() {
        llm.auth_token = Some(auth_token);
    }
    if let Some(retries) = values.retries {
        llm.retries = Some(retries);
    }
    if let Some(max_tokens) = values.max_tokens {
        llm.max_tokens = Some(max_tokens);
    }
    if let Some(timeout) = values.timeout {
        llm.timeout = Some(timeout);
    }
}

fn ensure_interactive_terminal(command: &str) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!(
            "{command} requires an interactive terminal when values are not passed via flags"
        );
    }
    Ok(())
}

fn prompt_provider(default: LlmProvider) -> Result<LlmProvider> {
    loop {
        let raw = prompt_input(
            "Provider (openai, anthropic, zai, custom)",
            Some(default.as_str()),
        )?;
        let candidate = if raw.is_empty() {
            default.as_str().to_string()
        } else {
            raw.to_ascii_lowercase()
        };
        match candidate.as_str() {
            "openai" => return Ok(LlmProvider::Openai),
            "anthropic" => return Ok(LlmProvider::Anthropic),
            "zai" => return Ok(LlmProvider::Zai),
            "custom" => return Ok(LlmProvider::Custom),
            _ => println!("Please choose one of: openai, anthropic, zai, custom."),
        }
    }
}

fn prompt_optional_text(prompt: &str, default: Option<&str>) -> Result<Option<String>> {
    let value = prompt_input(prompt, default)?;
    Ok((!value.is_empty()).then_some(value))
}

fn prompt_optional_u64(prompt: &str, default: Option<u64>) -> Result<Option<u64>> {
    loop {
        let default_value = default.map(|value| value.to_string());
        let raw = prompt_input(prompt, default_value.as_deref())?;
        if raw.is_empty() {
            return Ok(None);
        }
        match raw.parse::<u64>() {
            Ok(value) if value > 0 => return Ok(Some(value)),
            _ => println!("Please enter a positive integer."),
        }
    }
}

fn prompt_optional_u32(prompt: &str, default: Option<u32>) -> Result<Option<u32>> {
    loop {
        let default_value = default.map(|value| value.to_string());
        let raw = prompt_input(prompt, default_value.as_deref())?;
        if raw.is_empty() {
            return Ok(None);
        }
        match raw.parse::<u32>() {
            Ok(value) if value > 0 => return Ok(Some(value)),
            _ => println!("Please enter a positive integer."),
        }
    }
}

fn prompt_input(prompt: &str, default: Option<&str>) -> Result<String> {
    let mut stdout = io::stdout();
    match default {
        Some(default) => write!(stdout, "{prompt} [{default}]: ")?,
        None => write!(stdout, "{prompt}: ")?,
    }
    stdout.flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn run_install_hooks(warn_only: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let repo_root = repo
        .workdir()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| cwd.clone());
    why_hooks::installer::install(&repo_root, warn_only)
}

fn run_uninstall_hooks() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let repo_root = repo
        .workdir()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| cwd.clone());
    why_hooks::installer::uninstall(&repo_root)
}

fn run_context_inject() -> Result<()> {
    print!("{}", why_hooks::context_inject::render_shell_functions());
    Ok(())
}

fn run_hotspots(limit: usize, owner: Option<&str>, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);
    let findings = why_scanner::scan_hotspots(&cwd, limit, owner)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_hotspots_terminal(&findings, limit, owner, Some(&terminal_links));
    }

    Ok(())
}

fn run_health(json: bool, ci: Option<u32>, baseline: HealthBaselineOptions) -> Result<ExitStatus> {
    let context = collect_health_report()?;
    let mut report = context.report;

    let regression_gate_enabled =
        baseline.max_regression.is_some() || !baseline.max_signal_regression.is_empty();

    if let Some(path) = baseline.baseline_file.as_deref() {
        let baseline_snapshot = load_health_baseline(path, baseline.require_baseline)?;
        if let Some(baseline_snapshot) = baseline_snapshot {
            report.comparison = Some(compute_health_comparison(
                &context.snapshot,
                &baseline_snapshot,
            ));
            if regression_gate_enabled || ci.is_some() {
                report.gate = Some(evaluate_health_gate(
                    &context.snapshot,
                    report
                        .comparison
                        .as_ref()
                        .expect("comparison should be set"),
                    ci,
                    baseline.max_regression,
                    &baseline.max_signal_regression,
                ));
            }
        }
    }

    if let Some(path) = baseline.write_baseline.as_deref() {
        write_health_baseline(path, &context.snapshot)?;
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let cwd = std::env::current_dir()?;
        let repo = Repository::discover(&cwd)?;
        let config = load_config(&cwd)?;
        let terminal_links = build_terminal_link_context(&repo, &config);
        render_health_terminal(&report, ci, Some(&terminal_links));
    }

    Ok(determine_health_exit_status(
        &report,
        ci,
        baseline.max_regression,
        !baseline.max_signal_regression.is_empty(),
    ))
}

fn collect_health_report() -> Result<HealthReportContext> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let repo_root = repo
        .workdir()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| cwd.clone());
    let config = load_config(&cwd)?;
    let mut cache = Cache::open(&repo_root, config.cache.max_entries)?;

    let mut report = why_scanner::scan_health(&cwd)?;
    if let Some(previous) = cache.health_snapshots().last() {
        report.delta = Some(compute_health_delta(report.debt_score, previous));
    }

    let snapshot = HealthSnapshot {
        timestamp: current_unix_timestamp(),
        debt_score: report.debt_score,
        signals: report.signals.clone(),
        head_hash: repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok())
            .map(|commit| commit.id().to_string()),
        ref_name: repo
            .head()
            .ok()
            .and_then(|head| head.shorthand().map(str::to_string)),
    };

    cache.insert_health_snapshot(snapshot.clone())?;

    Ok(HealthReportContext { report, snapshot })
}

fn run_pr_template(json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let report = why_scanner::scan_pr_template(&cwd)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_pr_template_markdown(&report));
    }

    Ok(())
}

fn run_explain_outage(from: &str, to: &str, limit: usize, json: bool) -> Result<()> {
    let window_start_ts = parse_outage_timestamp(from, "--from")?;
    let window_end_ts = parse_outage_timestamp(to, "--to")?;
    if window_end_ts < window_start_ts {
        anyhow::bail!("--to must be greater than or equal to --from");
    }

    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);
    let report = why_scanner::scan_outage_window(&cwd, window_start_ts, window_end_ts, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_outage_terminal(&report, limit, Some(&terminal_links));
    }

    Ok(())
}

fn run_diff_review(
    json: bool,
    no_llm: bool,
    post_github_comment: bool,
    github_ref_override: Option<&str>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let plan = why_scanner::scan_diff_review(&cwd)?;
    let collected = collect_diff_review(&plan, &cwd, &repo, &config)?;
    let mut report = synthesize_diff_review_request(&config, &plan, &collected, no_llm)?;

    let markdown = render_diff_review_markdown(&report);
    if post_github_comment {
        let comment = post_diff_review_comment(
            &config,
            &repo,
            github_ref_override,
            collected.issue_refs.clone(),
            &markdown,
        )?;
        report.github_comment_url = Some(comment.html_url.clone());
        if !json {
            println!("Posted GitHub comment: {}", comment.html_url);
            println!();
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_diff_review_markdown(&report));
    }

    Ok(())
}

fn run_coverage_gap(coverage: &str, limit: usize, max_coverage: f32, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);
    let report =
        why_scanner::scan_coverage_gap(&cwd, std::path::Path::new(coverage), limit, max_coverage)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_coverage_gap_terminal(&report, limit, Some(&terminal_links));
    }

    Ok(())
}

fn run_ghost(limit: usize, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);
    let findings = why_scanner::scan_ghosts(&cwd, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_ghost_terminal(&findings, limit, Some(&terminal_links));
    }

    Ok(())
}

fn run_onboard(limit: usize, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);
    let findings = why_scanner::scan_onboard(&cwd, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_onboard_terminal(&findings, limit, Some(&terminal_links));
    }

    Ok(())
}

fn run_time_bombs(age_days: i64, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = Repository::discover(&cwd)?;
    let config = load_config(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);
    let findings = why_scanner::scan_time_bombs(&cwd, age_days)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_time_bombs_terminal(&findings, age_days, Some(&terminal_links));
    }

    Ok(())
}

fn run_rename_safe(request: &QueryRequest, links: &TerminalLinkContext) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let report = why_scanner::scan_rename_safe(&cwd, &request.target, request.since_days)?;

    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_rename_safe_terminal(&report, links);
    }

    Ok(())
}

fn run_completions(shell: CompletionShell) -> Result<()> {
    let mut command = Cli::command();
    let name = command.get_name().to_string();
    match shell {
        CompletionShell::Bash => generate_completion(Shell::Bash, &mut command, &name),
        CompletionShell::Zsh => generate_completion(Shell::Zsh, &mut command, &name),
        CompletionShell::Fish => generate_completion(Shell::Fish, &mut command, &name),
    }
    Ok(())
}

fn generate_completion<G: Generator>(generator: G, command: &mut clap::Command, name: &str) {
    generate(generator, command, name, &mut std::io::stdout());
}

fn run_manpage() -> Result<()> {
    let command = Cli::command();
    let man = Man::new(command);
    man.render(&mut std::io::stdout())?;
    Ok(())
}

fn run_query(request: QueryRequest) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config = load_config(&cwd)?;
    let repo = Repository::discover(&cwd)?;
    let terminal_links = build_terminal_link_context(&repo, &config);

    if request.split {
        let suggestion = why_splitter::suggest_split(&request.target, &cwd)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&suggestion)?);
        } else {
            render_split_terminal(&request.target, suggestion.as_ref(), Some(&terminal_links));
        }
        return Ok(());
    }

    if request.coupled {
        let report = why_scanner::scan_coupling(&cwd, &request.target, 10)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_coupling_terminal(&report, Some(&terminal_links));
        }
        return Ok(());
    }

    if request.team {
        let report = analyze_team(&request.target, &cwd, request.since_days)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_team_terminal(&report, Some(&terminal_links));
        }
        return Ok(());
    }

    if request.rename_safe {
        run_rename_safe(&request, &terminal_links)?;
        return Ok(());
    }

    if request.blame_chain {
        let report = analyze_blame_chain(&request.target, &cwd, request.since_days)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_blame_chain_terminal(&report, Some(&terminal_links));
        }
        return Ok(());
    }

    if request.evolution {
        let report = analyze_evolution_history(&request.target, &cwd, request.since_days)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_evolution_terminal(&report, Some(&terminal_links));
        }
        return Ok(());
    }

    let repo_root = repo
        .workdir()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    if request.watch {
        return run_watch_query(&request, &cwd, &repo, &config, &terminal_links, &repo_root);
    }

    render_default_query(&request, &cwd, &repo, &config, &terminal_links, &repo_root)?;
    Ok(())
}

fn render_default_query(
    request: &QueryRequest,
    cwd: &Path,
    repo: &Repository,
    config: &why_context::WhyConfig,
    terminal_links: &TerminalLinkContext,
    repo_root: &Path,
) -> Result<()> {
    let rendered =
        render_default_query_output(request, cwd, repo, config, terminal_links, repo_root)?;
    println!("{rendered}");
    Ok(())
}

fn render_default_query_output(
    request: &QueryRequest,
    cwd: &Path,
    repo: &Repository,
    config: &why_context::WhyConfig,
    terminal_links: &TerminalLinkContext,
    repo_root: &Path,
) -> Result<String> {
    let head_hash = repo.head()?.peel_to_commit()?.id().to_string();
    let target_label = match request.target.query_kind {
        QueryKind::Line => request
            .target
            .start_line
            .map(|line| line.to_string())
            .unwrap_or_else(|| "line".to_string()),
        QueryKind::Range => format!(
            "{}:{}",
            request.target.start_line.unwrap_or_default(),
            request.target.end_line.unwrap_or_default()
        ),
        QueryKind::Symbol | QueryKind::QualifiedSymbol => request
            .target
            .symbol
            .clone()
            .unwrap_or_else(|| "symbol".to_string()),
    };
    let cache_key = Cache::make_key(
        &request.target.path.to_string_lossy(),
        &target_label,
        &head_hash,
    );
    let mut cache = Cache::open(repo_root, config.cache.max_entries)?;

    if !request.no_cache {
        if let Some(cached) = cache.get::<WhyReport>(&cache_key) {
            let mut rendered = if request.json {
                serde_json::to_string_pretty(&cached)?
            } else {
                format_why_report(&request.target, &cached, true, Some(terminal_links))
            };

            if request.annotate {
                let result = analyze_target_with_options(&request.target, cwd, request.since_days)?;
                let source_path = repo_root.join(&result.target.path);
                annotate_file(
                    &source_path,
                    result.target.start_line,
                    &result,
                    &head_hash,
                    &format_target_label(&request.target),
                )?;
                if !request.json {
                    rendered.push_str(&format!(
                        "\n\nAnnotation written to {}",
                        source_path.display()
                    ));
                }
            }

            return Ok(rendered);
        }
    }

    let result = analyze_target_with_options(&request.target, cwd, request.since_days)?;
    let report = synthesize_report_query(request, &result, repo, config)?;
    let mut rendered = if request.json {
        serde_json::to_string_pretty(&report)?
    } else {
        format_why_report(&request.target, &report, false, Some(terminal_links))
    };

    if !request.watch {
        cache.set(cache_key, &report, &head_hash)?;
    }

    if request.annotate {
        let source_path = repo_root.join(&result.target.path);
        annotate_file(
            &source_path,
            result.target.start_line,
            &result,
            &head_hash,
            &format_target_label(&request.target),
        )?;
        if !request.json {
            rendered.push_str(&format!(
                "\n\nAnnotation written to {}",
                source_path.display()
            ));
        }
    }

    Ok(rendered)
}

fn run_watch_query(
    request: &QueryRequest,
    cwd: &Path,
    repo: &Repository,
    config: &why_context::WhyConfig,
    terminal_links: &TerminalLinkContext,
    repo_root: &Path,
) -> Result<()> {
    if !std::io::stdout().is_terminal() {
        anyhow::bail!("--watch requires an interactive terminal");
    }

    let absolute_target_path = cwd.join(&request.target.path);
    let mut watch_request = request.clone();
    watch_request.no_cache = true;

    let mut last_state = current_watch_state(&absolute_target_path)?;
    let initial_output =
        render_default_query_output(&watch_request, cwd, repo, config, terminal_links, repo_root)?;
    render_watch_frame(&watch_request, &initial_output, None);

    loop {
        thread::sleep(Duration::from_millis(250));

        let state = current_watch_state(&absolute_target_path)?;
        if state == last_state {
            continue;
        }
        last_state = state;

        let refreshed = render_default_query_output(
            &watch_request,
            cwd,
            repo,
            config,
            terminal_links,
            repo_root,
        );
        match refreshed {
            Ok(output) => render_watch_frame(&watch_request, &output, None),
            Err(error) => {
                let fallback = format!(
                    "why: {}\n\nWaiting for a valid target after file change.\n\nNotes\n  - {error}",
                    format_target_label(&watch_request.target)
                );
                render_watch_frame(&watch_request, &fallback, Some(&error.to_string()));
            }
        }
    }
}

fn format_watch_frame(request: &QueryRequest, body: &str, status: Option<&str>) -> String {
    let mut output = format!(
        "\x1b[2J\x1b[HWatching {}\nRefreshes when {} changes. Press Ctrl-C to stop.\n\n{body}\n\n",
        format_target_label(&request.target),
        request.target.path.display()
    );
    match status {
        Some(status) => output.push_str(&format!("watch status: {status}\n")),
        None => output.push_str("watch status: waiting for changes\n"),
    }
    output
}

fn render_watch_frame(request: &QueryRequest, body: &str, status: Option<&str>) {
    print!("{}", format_watch_frame(request, body, status));
    let _ = std::io::stdout().flush();
}

fn current_watch_state(path: &Path) -> Result<Option<FileWatchState>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(FileWatchState {
            modified_at: metadata.modified().ok(),
            len: metadata.len(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn render_team_terminal(report: &TeamReport, links: Option<&TerminalLinkContext>) {
    let heading = match report.target.query_kind {
        QueryKind::Symbol | QueryKind::QualifiedSymbol => {
            format!(
                "Team ownership for {}",
                format_output_target_heading(&report.target, links)
            )
        }
        _ => format!(
            "Team ownership for {}",
            format_output_target_heading(&report.target, links)
        ),
    };
    println!("{heading}");
    println!();

    for (index, owner) in report.owners.iter().enumerate() {
        let primary = if index == 0 { "  [primary owner]" } else { "" };
        let commits_label = if owner.commit_count == 1 {
            "commit"
        } else {
            "commits"
        };
        println!(
            "  {}    {} {} ({:>2}%)  Last: {}{}",
            owner.author,
            owner.commit_count,
            commits_label,
            owner.ownership_percent,
            owner.last_commit_date,
            primary
        );
    }

    println!();
    println!("Bus factor: {}", report.bus_factor);
    println!("Risk: {}", report.risk_summary);
}

fn render_blame_chain_terminal(report: &BlameChainResult, links: Option<&TerminalLinkContext>) {
    let heading = format!(
        "Blame chain for {}",
        format_output_target_heading(&report.target, links)
    );
    println!("{heading}");
    println!();
    println!(
        "Starting blame tip: {}  {}  {}  {}",
        linked_commit_label(
            links,
            &report.starting_commit.oid,
            &report.starting_commit.short_oid
        ),
        report.starting_commit.author,
        report.starting_commit.date,
        report.starting_commit.summary
    );
    println!();

    if report.noise_commits_skipped.is_empty() {
        println!("  Skipped (mechanical): none");
    } else {
        println!("  Skipped (mechanical):");
        for commit in &report.noise_commits_skipped {
            println!(
                "    {}  {} ({})",
                linked_commit_label(links, &commit.oid, &commit.short_oid),
                commit.summary,
                commit.date
            );
        }
    }

    println!();
    println!("  True origin:");
    println!(
        "    {}  {} ({})",
        linked_commit_label(
            links,
            &report.origin_commit.oid,
            &report.origin_commit.short_oid
        ),
        report.origin_commit.summary,
        report.origin_commit.date
    );
    println!("              Author: {}", report.origin_commit.email);
    if report.local_context.risk_flags.is_empty() {
        println!("              Risk signals: none in local context");
    } else {
        println!(
            "              Risk signals: {}",
            report.local_context.risk_flags.join(", ")
        );
    }
    println!();
    println!("Chain depth: {}", report.chain_depth);
    println!("Heuristic risk: {}.", report.risk_level.as_str());
    println!("{}", report.risk_summary);
    println!("{}", report.change_guidance);
}

fn render_coupling_terminal(report: &CouplingReport, links: Option<&TerminalLinkContext>) {
    println!(
        "Coupled files for {}",
        linked_path_label(
            links,
            &report.target_path,
            None,
            normalize_path(&report.target_path)
        )
    );
    println!();
    if report.results.is_empty() {
        println!("No coupled files met the configured ratio threshold.");
        return;
    }

    println!(
        "Scanned {} commits; {} non-mechanical commits touched the target.",
        report.scan_commits, report.target_commit_count
    );
    println!();

    for finding in &report.results {
        println!(
            "  {:.2}  {} shared  {}",
            finding.coupling_ratio,
            finding.shared_commits,
            linked_path_label(links, &finding.path, None, normalize_path(&finding.path))
        );
    }
}

fn render_evolution_terminal(report: &EvolutionHistoryResult, links: Option<&TerminalLinkContext>) {
    let heading = format!(
        "Evolution history for {}",
        format_output_target_heading(&report.target, links)
    );
    println!("{heading}");
    println!();
    println!("Heuristic risk: {}.", report.risk_level.as_str());
    println!("{}", report.risk_summary);
    println!("{}", report.change_guidance);
    println!();
    println!("Narrative summary:");
    println!("  {}", report.narrative_summary);

    println!();
    match (&report.latest_commit, &report.origin_commit) {
        (Some(latest), Some(origin)) => {
            println!("Current edge:");
            println!(
                "  {}  {}  {}",
                linked_commit_label(links, &latest.oid, &latest.short_oid),
                latest.date,
                latest.summary
            );
            println!("Origin:");
            println!(
                "  {}  {}  {}",
                linked_commit_label(links, &origin.oid, &origin.short_oid),
                origin.date,
                origin.summary
            );
        }
        (Some(latest), None) => {
            println!("Current edge:");
            println!(
                "  {}  {}  {}",
                linked_commit_label(links, &latest.oid, &latest.short_oid),
                latest.date,
                latest.summary
            );
        }
        _ => {}
    }

    println!();
    if report.paths_seen.is_empty() {
        println!("Paths seen: none");
    } else {
        println!("Paths seen:");
        for path in &report.paths_seen {
            println!(
                "  - {}",
                linked_path_label(links, path, None, normalize_path(path))
            );
        }
    }

    println!();
    if report.inflection_points.is_empty() {
        println!("Inflection points: none");
    } else {
        println!("Inflection points:");
        for point in &report.inflection_points {
            println!(
                "  - [{}] {}  {}  {}",
                point.category,
                point.date,
                linked_path_label(
                    links,
                    &point.path_at_commit,
                    None,
                    normalize_path(&point.path_at_commit)
                ),
                point.summary
            );
            println!("      {}", point.reason);
        }
    }

    println!();
    if report.commits.is_empty() {
        println!("Timeline: no commits matched the requested evolution window.");
    } else {
        println!("Timeline:");
        for entry in &report.commits {
            println!(
                "  {}  {}  {}  {}",
                linked_commit_label(links, &entry.commit.oid, &entry.commit.short_oid),
                entry.commit.date,
                linked_path_label(
                    links,
                    &entry.path_at_commit,
                    None,
                    normalize_path(&entry.path_at_commit)
                ),
                entry.commit.summary
            );
        }
    }

    if !report.notes.is_empty() {
        println!();
        println!("Notes:");
        for note in &report.notes {
            println!("  - {note}");
        }
    }
}

fn render_hotspots_terminal(
    findings: &[HotspotFinding],
    limit: usize,
    owner: Option<&str>,
    links: Option<&TerminalLinkContext>,
) {
    match owner {
        Some(owner) => println!("Top {limit} hotspots by churn × risk for owner {owner}"),
        None => println!("Top {limit} hotspots by churn × risk"),
    }
    println!();
    if findings.is_empty() {
        match owner {
            Some(owner) => println!(
                "No source hotspots were found for owner {owner} in the current repository."
            ),
            None => println!("No source hotspots were found in the current repository."),
        }
        return;
    }

    for (index, finding) in findings.iter().enumerate() {
        println!(
            "  {:>2}. {:<30} churn {:>3}  risk {:<6}  score {:.2}",
            index + 1,
            linked_path_label(links, &finding.path, None, normalize_path(&finding.path)),
            finding.churn_commits,
            finding.risk_level.as_str(),
            finding.hotspot_score
        );
        if !finding.top_commit_summaries.is_empty() {
            println!(
                "      top history: {}",
                finding.top_commit_summaries.join(" | ")
            );
        }
        if let Some(primary_owner) = &finding.primary_owner {
            println!(
                "      primary owner: {}  bus factor {}",
                primary_owner, finding.bus_factor
            );
        }
    }
}

fn render_health_terminal(
    report: &HealthReport,
    ci: Option<u32>,
    links: Option<&TerminalLinkContext>,
) {
    println!("Repository health");
    println!();
    println!("Debt score: {}", report.debt_score);
    if let Some(delta) = &report.delta {
        println!(
            "Trend: {} {} (previous {})",
            delta.direction, delta.amount, delta.previous_score
        );
    }
    if let Some(threshold) = ci {
        let status = if report.debt_score > threshold {
            "FAIL"
        } else {
            "PASS"
        };
        println!("CI gate: {status} (threshold {threshold})");
    }
    if let Some(comparison) = &report.comparison {
        println!(
            "Baseline: {} (score {} at {})",
            comparison.baseline.source,
            comparison.baseline.debt_score,
            format_health_timestamp(comparison.baseline.timestamp)
        );
        if let Some(head_hash) = &comparison.baseline.head_hash {
            println!(
                "Baseline head: {}",
                linked_commit_label(links, head_hash, head_hash)
            );
        }
        if let Some(ref_name) = &comparison.baseline.ref_name {
            println!("Baseline ref: {ref_name}");
        }
        println!("Score delta vs baseline: {}", comparison.score_delta);
        if !comparison.signal_deltas.is_empty() {
            println!("Signal deltas");
            for (name, delta) in &comparison.signal_deltas {
                println!(
                    "  - {name}: {} (baseline {}, delta {})",
                    delta.current, delta.baseline, delta.delta
                );
            }
        }
    }
    if let Some(gate) = &report.gate {
        let status = if gate.passed { "PASS" } else { "FAIL" };
        println!("Regression gate: {status}");
        if let Some(threshold) = gate.absolute_threshold {
            println!("  absolute threshold: {threshold}");
        }
        if let Some(max_regression) = gate.max_regression {
            println!("  max regression: {max_regression}");
        }
        if !gate.signal_budgets.is_empty() {
            println!("  signal budgets:");
            for (name, budget) in &gate.signal_budgets {
                println!("    - {name}: {budget}");
            }
        }
        for reason in &gate.reasons {
            println!("  - {reason}");
        }
    }
    println!();
    println!("Signals");
    for (name, count) in sorted_signal_entries(&report.signals) {
        println!("  - {name}: {count}");
    }
    if !report.notes.is_empty() {
        println!();
        println!("Notes");
        for note in &report.notes {
            println!("  - {note}");
        }
    }
}

fn render_pr_template_markdown(report: &PrTemplateReport) -> String {
    let mut lines = vec![format!("# {}", report.title_suggestion), String::new()];

    lines.push("## Summary".into());
    for item in &report.summary {
        lines.push(format!("- {item}"));
    }
    lines.push(String::new());

    lines.push("## Risk notes".into());
    for item in &report.risk_notes {
        lines.push(format!("- {item}"));
    }
    lines.push(String::new());

    lines.push("## Test plan".into());
    for item in &report.test_plan {
        lines.push(format!("- {item}"));
    }
    lines.push(String::new());

    lines.push("## Staged files".into());
    for file in &report.staged_files {
        lines.push(format!(
            "- {} ({})",
            normalize_path(&file.path),
            file.change.as_str()
        ));
    }
    lines.push(String::new());

    lines.join("\n")
}

fn render_outage_terminal(
    report: &OutageReport,
    limit: usize,
    links: Option<&TerminalLinkContext>,
) {
    println!(
        "Top {limit} outage archaeology findings in {} – {}",
        format_outage_timestamp(report.window_start_ts),
        format_outage_timestamp(report.window_end_ts)
    );
    println!();

    if report.findings.is_empty() {
        println!("No source commits were found in the requested outage window.");
    } else {
        for (index, finding) in report.findings.iter().enumerate() {
            let issue_refs = if finding.issue_refs.is_empty() {
                "none".to_string()
            } else {
                finding.issue_refs.join(", ")
            };
            println!(
                "  {:>2}. {}  {}  {}  score {:.2}",
                index + 1,
                linked_commit_label(links, &finding.oid, &finding.short_oid),
                finding.date,
                finding.risk_level.as_str(),
                finding.score
            );
            println!("      summary: {}", finding.summary);
            println!("      author: {}", finding.author);
            println!("      blast radius: {} file(s)", finding.blast_radius_files);
            println!("      issue refs: {issue_refs}");
            if !finding.changed_paths.is_empty() {
                let preview = finding
                    .changed_paths
                    .iter()
                    .take(3)
                    .map(|path| linked_path_label(links, path, None, normalize_path(path)))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("      paths: {preview}");
            }
            println!("      guidance: {}", finding.change_guidance);
            for note in &finding.notes {
                println!("      note: {note}");
            }
        }
    }

    if !report.notes.is_empty() {
        println!();
        println!("Notes");
        for note in &report.notes {
            println!("  - {note}");
        }
    }
}

fn render_diff_review_markdown(report: &DiffReviewReport) -> String {
    let mut lines = vec!["# Diff review".to_string(), String::new()];

    lines.push("## Summary".to_string());
    lines.push(report.summary.clone());
    lines.push(String::new());

    if let Some(url) = &report.github_comment_url {
        lines.push(format!("GitHub comment: {url}"));
        lines.push(String::new());
    }

    if !report.findings.is_empty() {
        lines.push("## Findings".to_string());
        for finding in &report.findings {
            lines.push(format!(
                "- {} — {} ({})",
                finding.target,
                finding.risk_level.as_str(),
                finding.confidence.as_str()
            ));
            lines.push(format!("  - Path: {}", finding.path));
            if let Some(symbol) = &finding.symbol {
                lines.push(format!("  - Symbol: {symbol}"));
            }
            lines.push(format!("  - Why it matters: {}", finding.why_it_matters));
        }
        lines.push(String::new());
    }

    if !report.reviewer_focus.is_empty() {
        lines.push("## Reviewer focus".to_string());
        for item in &report.reviewer_focus {
            lines.push(format!("- {item}"));
        }
        lines.push(String::new());
    }

    if !report.unknowns.is_empty() {
        lines.push("## Unknowns".to_string());
        for item in &report.unknowns {
            lines.push(format!("- {item}"));
        }
        lines.push(String::new());
    }

    if !report.notes.is_empty() {
        lines.push("## Notes".to_string());
        for item in &report.notes {
            lines.push(format!("- {item}"));
        }
        lines.push(String::new());
    }

    lines.push(format!("Mode: {}", diff_review_mode_label(report.mode)));
    if let Some(cost_usd) = report.cost_usd {
        lines.push(format!("Estimated cost: ~${cost_usd:.4}"));
    }

    lines.join("\n")
}

fn render_coverage_gap_terminal(
    report: &CoverageGapReport,
    limit: usize,
    links: Option<&TerminalLinkContext>,
) {
    println!(
        "Top {limit} HIGH-risk functions at or below {:.1}% coverage",
        report.max_coverage
    );
    println!();
    println!(
        "Coverage report: {}",
        linked_path_label(
            links,
            &report.coverage_path,
            None,
            normalize_path(&report.coverage_path)
        )
    );
    println!();
    if report.findings.is_empty() {
        println!("No HIGH-risk coverage gaps were found in the current repository.");
    } else {
        for (index, finding) in report.findings.iter().enumerate() {
            println!(
                "  {:>2}. {}  {}  coverage {:>5.1}%  commits {:>2}",
                index + 1,
                linked_path_label(
                    links,
                    &finding.path,
                    Some(finding.start_line),
                    format!(
                        "{}:{}-{}",
                        normalize_path(&finding.path),
                        finding.start_line,
                        finding.end_line
                    )
                ),
                finding.symbol,
                finding.coverage_pct,
                finding.commit_count
            );
            println!(
                "      instrumented: {} line(s), covered: {}",
                finding.instrumented_lines, finding.covered_lines
            );
            let risk_flags = if finding.risk_flags.is_empty() {
                "none".to_string()
            } else {
                finding.risk_flags.join(", ")
            };
            println!("      risk flags: {risk_flags}");
            println!("      summary: {}", finding.summary);
            if !finding.top_commit_summaries.is_empty() {
                println!(
                    "      top history: {}",
                    finding.top_commit_summaries.join(" | ")
                );
            }
        }
    }

    if !report.notes.is_empty() {
        println!();
        println!("Notes");
        for note in &report.notes {
            println!("  - {note}");
        }
    }
}

fn render_rename_safe_terminal(report: &RenameSafeReport, links: &TerminalLinkContext) {
    let target_label = match &report.target.qualified_name {
        Some(qualified_name) => format!("{} ({qualified_name})", report.target.symbol),
        None => report.target.symbol.clone(),
    };

    println!(
        "Rename-safe review for {}",
        linked_path_label(
            Some(links),
            &report.target.path,
            Some(report.target.start_line),
            format!(
                "{}:{}-{}",
                normalize_path(&report.target.path),
                report.target.start_line,
                report.target.end_line
            )
        )
    );
    println!("Target: {target_label}");
    println!(
        "Risk: {}  commits {}",
        report.target.risk_level.as_str(),
        report.target.commit_count
    );
    println!("Summary: {}", report.target.summary);
    println!("Guidance: {}", report.target.change_guidance);
    if !report.target.top_commit_summaries.is_empty() {
        println!(
            "Top history: {}",
            report.target.top_commit_summaries.join(" | ")
        );
    }

    println!();
    println!("Caller symbols ({})", report.callers.len());
    if report.callers.is_empty() {
        println!("  none");
    } else {
        for (index, caller) in report.callers.iter().enumerate() {
            let caller_label = caller
                .qualified_name
                .as_deref()
                .unwrap_or(&caller.symbol)
                .to_string();
            println!(
                "  {:>2}. {}  {}  call-sites {:>2}  commits {:>2}",
                index + 1,
                linked_path_label(
                    Some(links),
                    &caller.path,
                    Some(caller.start_line),
                    format!(
                        "{}:{}-{}",
                        normalize_path(&caller.path),
                        caller.start_line,
                        caller.end_line
                    )
                ),
                caller_label,
                caller.call_site_count,
                caller.commit_count
            );
            println!("      risk: {}", caller.risk_level.as_str());
            println!("      summary: {}", caller.summary);
            println!("      guidance: {}", caller.change_guidance);
            if !caller.top_commit_summaries.is_empty() {
                println!(
                    "      top history: {}",
                    caller.top_commit_summaries.join(" | ")
                );
            }
        }
    }

    if !report.notes.is_empty() {
        println!();
        println!("Notes");
        for note in &report.notes {
            println!("  - {note}");
        }
    }
}

fn render_ghost_terminal(
    findings: &[GhostFinding],
    limit: usize,
    links: Option<&TerminalLinkContext>,
) {
    println!("Top {limit} ghost functions by risk-aware archaeology");
    println!();
    if findings.is_empty() {
        println!("No high-risk ghost functions were found in the current repository.");
        return;
    }

    for (index, finding) in findings.iter().enumerate() {
        println!(
            "  {:>2}. {}  {}  commits {:>2}  call-sites {:>2}",
            index + 1,
            linked_path_label(
                links,
                &finding.path,
                Some(finding.start_line),
                format!(
                    "{}:{}-{}",
                    normalize_path(&finding.path),
                    finding.start_line,
                    finding.end_line
                )
            ),
            finding.symbol,
            finding.commit_count,
            finding.call_site_count
        );
        println!("      risk: {}", finding.risk_level.as_str());
        println!("      summary: {}", finding.summary);
        if !finding.top_commit_summaries.is_empty() {
            println!(
                "      top history: {}",
                finding.top_commit_summaries.join(" | ")
            );
        }
        for note in &finding.notes {
            println!("      note: {note}");
        }
    }
}

fn render_onboard_terminal(
    findings: &[OnboardFinding],
    limit: usize,
    links: Option<&TerminalLinkContext>,
) {
    println!("Top {limit} symbols to understand first");
    println!();
    if findings.is_empty() {
        println!("No onboarding findings were found in the current repository.");
        return;
    }

    for (index, finding) in findings.iter().enumerate() {
        println!(
            "  {:>2}. {}  {}  risk {:<6}  score {:.2}",
            index + 1,
            linked_path_label(
                links,
                &finding.path,
                Some(finding.start_line),
                format!(
                    "{}:{}-{}",
                    normalize_path(&finding.path),
                    finding.start_line,
                    finding.end_line
                )
            ),
            finding.symbol,
            finding.risk_level.as_str(),
            finding.score
        );
        println!("      summary: {}", finding.summary);
        println!("      guidance: {}", finding.change_guidance);
        if let Some(date) = &finding.last_touched_date {
            println!("      last touched: {date}");
        }
        if !finding.top_commit_summaries.is_empty() {
            println!(
                "      top history: {}",
                finding.top_commit_summaries.join(" | ")
            );
        }
    }
}

fn render_time_bombs_terminal(
    findings: &[TimeBombFinding],
    age_threshold: i64,
    links: Option<&TerminalLinkContext>,
) {
    println!(
        "Time bombs (aged markers with threshold: {} days)",
        age_threshold
    );
    println!();
    if findings.is_empty() {
        println!("No time bombs were found in the current repository.");
        return;
    }

    let by_severity = |severity: Severity| {
        let filtered: Vec<_> = findings.iter().filter(|f| f.severity == severity).collect();
        filtered
    };

    let critical = by_severity(Severity::Critical);
    let warn = by_severity(Severity::Warn);
    let info = by_severity(Severity::Info);

    if !critical.is_empty() {
        println!("CRITICAL:");
        for (index, finding) in critical.iter().enumerate() {
            println!(
                "  {}. {}  {}",
                index + 1,
                linked_path_label(
                    links,
                    &finding.path,
                    Some(finding.line),
                    format!("{}:{}", finding.path.display(), finding.line)
                ),
                kind_emoji(finding.kind)
            );
            println!("      marker: {}", finding.marker);
            println!("      kind: {:?}", finding.kind);
            if let Some(author) = &finding.introduced_by {
                println!("      introduced by: {}", author);
            }
            if let Some(age) = finding.age_days {
                println!("      age: {} days", age);
            }
        }
        println!();
    }

    if !warn.is_empty() {
        println!("WARNING:");
        for (index, finding) in warn.iter().enumerate() {
            println!(
                "  {}. {}  {}",
                index + 1,
                linked_path_label(
                    links,
                    &finding.path,
                    Some(finding.line),
                    format!("{}:{}", finding.path.display(), finding.line)
                ),
                kind_emoji(finding.kind)
            );
            println!("      marker: {}", finding.marker);
            println!("      kind: {:?}", finding.kind);
            if let Some(author) = &finding.introduced_by {
                println!("      introduced by: {}", author);
            }
            if let Some(age) = finding.age_days {
                println!("      age: {} days", age);
            }
        }
        println!();
    }

    if !info.is_empty() {
        println!("INFO:");
        for (index, finding) in info.iter().enumerate() {
            println!(
                "  {}. {}  {}",
                index + 1,
                linked_path_label(
                    links,
                    &finding.path,
                    Some(finding.line),
                    format!("{}:{}", finding.path.display(), finding.line)
                ),
                kind_emoji(finding.kind)
            );
            println!("      marker: {}", finding.marker);
            println!("      kind: {:?}", finding.kind);
            if let Some(author) = &finding.introduced_by {
                println!("      introduced by: {}", author);
            }
            if let Some(age) = finding.age_days {
                println!("      age: {} days", age);
            }
        }
    }

    println!();
    println!("Total: {} finding(s)", findings.len());
    println!(
        "  {} critical, {} warning, {} info",
        critical.len(),
        warn.len(),
        info.len()
    );
}

fn kind_emoji(kind: TimeBombKind) -> &'static str {
    match kind {
        TimeBombKind::PastDueTodo => "📅",
        TimeBombKind::AgedHack => "🔧",
        TimeBombKind::ExpiredRemoveAfter => "⏰",
    }
}

fn parse_outage_timestamp(raw: &str, flag: &str) -> Result<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{flag} must not be empty");
    }

    OffsetDateTime::parse(
        trimmed,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map(|value| value.unix_timestamp())
    .or_else(|_| {
        PrimitiveDateTime::parse(
            trimmed,
            &time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]"),
        )
        .map(|value| value.assume_utc().unix_timestamp())
    })
    .map_err(|_| anyhow!("{flag} must be a valid ISO-8601 timestamp"))
}

fn format_outage_timestamp(timestamp: i64) -> String {
    format_health_timestamp(timestamp)
}

fn sorted_signal_entries(signals: &std::collections::HashMap<String, u32>) -> Vec<(String, u32)> {
    let mut entries = signals
        .iter()
        .map(|(name, count)| (name.clone(), *count))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn build_github_enrichment(
    repo: &Repository,
    config: &why_context::WhyConfig,
    commits: &[why_archaeologist::CommitEvidence],
) -> GitHubEnrichment {
    let Some(remote_name) =
        (!config.github.remote.trim().is_empty()).then(|| config.github.remote.trim())
    else {
        return GitHubEnrichment::default();
    };

    let remote_url = match repo.find_remote(remote_name) {
        Ok(remote) => match remote.url() {
            Some(url) if !url.trim().is_empty() => url.to_string(),
            _ => {
                return GitHubEnrichment {
                    items: Vec::new(),
                    notes: vec![format!(
                        "GitHub enrichment skipped because remote '{remote_name}' has no URL"
                    )],
                };
            }
        },
        Err(error) => {
            return GitHubEnrichment {
                items: Vec::new(),
                notes: vec![format!(
                    "GitHub enrichment skipped because remote '{remote_name}' could not be read: {error}"
                )],
            };
        }
    };

    let client = match GitHubClient::from_config(config, &remote_url) {
        Ok(client) => client,
        Err(error) => {
            return GitHubEnrichment {
                items: Vec::new(),
                notes: vec![format!("GitHub enrichment unavailable: {error}")],
            };
        }
    };

    let issue_refs = commits
        .iter()
        .flat_map(|commit| commit.issue_refs.iter().cloned())
        .collect::<Vec<_>>();
    enrich_github_refs(&client, &issue_refs)
}

fn compute_health_delta(current_score: u32, previous: &HealthSnapshot) -> HealthDelta {
    let amount = current_score as i64 - previous.debt_score as i64;
    let direction = if amount > 0 {
        "↑"
    } else if amount < 0 {
        "↓"
    } else {
        "→"
    };

    HealthDelta {
        direction,
        amount,
        previous_score: previous.debt_score,
    }
}

fn compute_health_comparison(
    current: &HealthSnapshot,
    baseline: &HealthSnapshot,
) -> HealthComparison {
    let mut signal_names = BTreeSet::new();
    signal_names.extend(current.signals.keys().cloned());
    signal_names.extend(baseline.signals.keys().cloned());

    let signal_deltas = signal_names
        .into_iter()
        .map(|name| {
            let current_value = current.signals.get(&name).copied().unwrap_or_default();
            let baseline_value = baseline.signals.get(&name).copied().unwrap_or_default();
            (
                name,
                HealthSignalDelta {
                    current: current_value,
                    baseline: baseline_value,
                    delta: current_value as i64 - baseline_value as i64,
                },
            )
        })
        .collect();

    HealthComparison {
        baseline: HealthBaselineReference {
            source: "file".into(),
            timestamp: baseline.timestamp,
            head_hash: baseline.head_hash.clone(),
            ref_name: baseline.ref_name.clone(),
            debt_score: baseline.debt_score,
        },
        score_delta: current.debt_score as i64 - baseline.debt_score as i64,
        signal_deltas,
    }
}

fn evaluate_health_gate(
    current: &HealthSnapshot,
    comparison: &HealthComparison,
    absolute_threshold: Option<u32>,
    max_regression: Option<u32>,
    signal_budgets: &[(String, u32)],
) -> HealthGateSummary {
    let mut reasons = Vec::new();

    if let Some(threshold) = absolute_threshold {
        if current.debt_score > threshold {
            reasons.push(format!(
                "health debt score {} exceeds CI threshold {}",
                current.debt_score, threshold
            ));
        }
    }

    if let Some(budget) = max_regression {
        if comparison.score_delta > budget as i64 {
            reasons.push(format!(
                "health debt score regressed by {} which exceeds allowed regression {}",
                comparison.score_delta, budget
            ));
        }
    }

    let signal_budgets_map = signal_budgets.iter().cloned().collect::<BTreeMap<_, _>>();
    for (signal, budget) in &signal_budgets_map {
        let delta = comparison
            .signal_deltas
            .get(signal)
            .map(|entry| entry.delta)
            .unwrap_or_default();
        if delta > *budget as i64 {
            reasons.push(format!(
                "health signal {signal} regressed by {delta} which exceeds allowed regression {budget}"
            ));
        }
    }

    HealthGateSummary {
        passed: reasons.is_empty(),
        absolute_threshold,
        max_regression,
        signal_budgets: signal_budgets_map,
        reasons,
    }
}

fn determine_health_exit_status(
    report: &HealthReport,
    ci: Option<u32>,
    max_regression: Option<u32>,
    has_signal_regression_budget: bool,
) -> ExitStatus {
    let ci_failure_message = ci.and_then(|threshold| {
        (report.debt_score > threshold).then(|| {
            format!(
                "health debt score {} exceeds CI threshold {}",
                report.debt_score, threshold
            )
        })
    });
    let regression_gate_enabled = max_regression.is_some() || has_signal_regression_budget;

    if regression_gate_enabled {
        if let Some(gate) = &report.gate {
            if !gate.passed {
                if let Some(message) = gate
                    .reasons
                    .iter()
                    .find(|reason| !reason.contains("exceeds CI threshold"))
                    .cloned()
                {
                    return ExitStatus::HealthRegressionFailure { message };
                }
            }
        }
    }

    if let Some(message) = ci_failure_message {
        return ExitStatus::HealthCiFailure { message };
    }

    ExitStatus::Success
}

fn load_health_baseline(path: &Path, require_baseline: bool) -> Result<Option<HealthSnapshot>> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let baseline: HealthBaselineFile = serde_json::from_str(&contents)?;
            Ok(Some(baseline.snapshot))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !require_baseline => Ok(None),
        Err(error) => Err(anyhow!(
            "failed to load health baseline {}: {error}",
            path.display()
        )),
    }
}

fn write_health_baseline(path: &Path, snapshot: &HealthSnapshot) -> Result<()> {
    let payload = HealthBaselineFile {
        schema_version: 1,
        snapshot: snapshot.clone(),
    };
    fs::write(path, serde_json::to_string_pretty(&payload)?)?;
    Ok(())
}

fn format_health_timestamp(timestamp: i64) -> String {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .map(|value| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                value.year(),
                u8::from(value.month()),
                value.day(),
                value.hour(),
                value.minute()
            )
        })
        .unwrap_or_else(|_| timestamp.to_string())
}

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn synthesize_report_query(
    request: &QueryRequest,
    result: &ArchaeologyResult,
    repo: &Repository,
    config: &why_context::WhyConfig,
) -> Result<WhyReport> {
    let github = build_github_enrichment(repo, config, &result.commits);
    let evidence_pack = why_evidence::build(
        &EvidenceTarget {
            file: normalize_path(&result.target.path),
            symbol: request.target.symbol.clone(),
            lines: (
                result.target.start_line as usize,
                result.target.end_line as usize,
            ),
            language: infer_language(&result.target.path),
        },
        &result
            .commits
            .iter()
            .map(|commit| EvidenceCommit {
                oid: commit.oid.clone(),
                date: commit.date.clone(),
                author: commit.author.clone(),
                summary: commit.summary.clone(),
                diff_excerpt: commit.diff_excerpt.clone(),
                coverage_score: commit.coverage_score,
                issue_refs: commit.issue_refs.clone(),
            })
            .collect::<Vec<_>>(),
        &EvidenceContext {
            comments: result.local_context.comments.clone(),
            markers: result.local_context.markers.clone(),
            risk_flags: result.local_context.risk_flags.clone(),
            heuristic_risk: result.risk_level.as_str().to_string(),
        },
        &github,
    );

    let fallback = || {
        let mut notes = result.notes.clone();
        notes.extend(github.notes.iter().cloned());
        heuristic_report(
            format!(
                "Heuristic analysis of {} based on {} relevant commit(s).",
                format_target_label(&request.target),
                result.commits.len()
            ),
            parse_synth_risk(result.risk_level.as_str()),
            result
                .commits
                .iter()
                .map(|commit| format!("{} ({})", commit.summary, commit.date))
                .collect(),
            notes,
        )
    };

    if request.no_llm {
        return Ok(fallback());
    }

    let client = match client_from_config(&config.resolved_llm_config()) {
        Ok(client) => client,
        Err(_) => return Ok(fallback()),
    };

    let contract = prompt_contract();
    let system_prompt = build_system_prompt(&contract);
    let user_prompt = build_query_prompt(&evidence_pack);

    match llm_synthesize_report(&*client, &system_prompt, &user_prompt) {
        Ok(report) => Ok(report),
        Err(_) => Ok(fallback()),
    }
}

fn parse_synth_risk(value: &str) -> why_synthesizer::RiskLevel {
    match value {
        "HIGH" => why_synthesizer::RiskLevel::HIGH,
        "MEDIUM" => why_synthesizer::RiskLevel::MEDIUM,
        _ => why_synthesizer::RiskLevel::LOW,
    }
}

fn diff_review_mode_label(mode: ReportMode) -> &'static str {
    match mode {
        ReportMode::Heuristic => "heuristic",
        ReportMode::Synthesized => "synthesized",
    }
}

fn synthesize_diff_review_request(
    config: &why_context::WhyConfig,
    plan: &DiffReviewPlan,
    collected: &DiffReviewCollected,
    no_llm: bool,
) -> Result<DiffReviewReport> {
    let fallback = || {
        heuristic_diff_review_report(
            format!(
                "Heuristic diff review of {} staged target(s).",
                collected.entries.len()
            ),
            collected
                .entries
                .iter()
                .map(heuristic_diff_review_finding)
                .collect(),
            heuristic_diff_review_focus(collected),
            collected.notes.clone(),
        )
    };

    if no_llm {
        return Ok(fallback());
    }

    let client = match client_from_config(&config.resolved_llm_config()) {
        Ok(client) => client,
        Err(_) => return Ok(fallback()),
    };

    let target_label = diff_review_target_label(plan);
    let user_prompt = build_diff_review_prompt(
        &target_label,
        &collected
            .entries
            .iter()
            .map(|entry| entry.evidence_pack.clone())
            .collect::<Vec<_>>(),
    );
    let system_prompt = build_system_prompt(&prompt_contract());

    match llm_synthesize_diff_review(&*client, &system_prompt, &user_prompt) {
        Ok(mut report) => {
            if report.notes.is_empty() {
                report.notes = collected.notes.clone();
            } else {
                for note in &collected.notes {
                    if !report.notes.contains(note) {
                        report.notes.push(note.clone());
                    }
                }
            }
            Ok(report)
        }
        Err(_) => Ok(fallback()),
    }
}

fn collect_diff_review(
    plan: &DiffReviewPlan,
    cwd: &std::path::Path,
    repo: &Repository,
    config: &why_context::WhyConfig,
) -> Result<DiffReviewCollected> {
    let mut collected = DiffReviewCollected {
        entries: Vec::new(),
        issue_refs: Vec::new(),
        notes: plan.skipped.clone(),
    };

    for target in &plan.targets {
        let result = analyze_target_with_options(&target.target, cwd, None)?;
        collected.issue_refs.extend(
            result
                .commits
                .iter()
                .flat_map(|commit| commit.issue_refs.iter().cloned()),
        );
        let github = build_github_enrichment(repo, config, &result.commits);
        collected.notes.extend(github.notes.iter().cloned());
        let evidence_pack = evidence_pack_from_result(target, &result, &github);
        collected.entries.push(DiffReviewCollectedTarget {
            target: target.clone(),
            result,
            evidence_pack,
        });
    }

    collected.notes.sort();
    collected.notes.dedup();
    collected.issue_refs.sort();
    collected.issue_refs.dedup();
    Ok(collected)
}

fn evidence_pack_from_result(
    target: &DiffReviewTarget,
    result: &ArchaeologyResult,
    github: &GitHubEnrichment,
) -> EvidencePack {
    why_evidence::build(
        &EvidenceTarget {
            file: normalize_path(&result.target.path),
            symbol: target.symbol.clone(),
            lines: (
                result.target.start_line as usize,
                result.target.end_line as usize,
            ),
            language: infer_language(&result.target.path),
        },
        &result
            .commits
            .iter()
            .map(|commit| EvidenceCommit {
                oid: commit.oid.clone(),
                date: commit.date.clone(),
                author: commit.author.clone(),
                summary: commit.summary.clone(),
                diff_excerpt: commit.diff_excerpt.clone(),
                coverage_score: commit.coverage_score,
                issue_refs: commit.issue_refs.clone(),
            })
            .collect::<Vec<_>>(),
        &EvidenceContext {
            comments: result.local_context.comments.clone(),
            markers: result.local_context.markers.clone(),
            risk_flags: result.local_context.risk_flags.clone(),
            heuristic_risk: result.risk_level.as_str().to_string(),
        },
        github,
    )
}

fn heuristic_diff_review_finding(entry: &DiffReviewCollectedTarget) -> DiffReviewFinding {
    DiffReviewFinding {
        target: format_target_label(&entry.target.target),
        path: normalize_path(&entry.target.target.path),
        symbol: entry.target.symbol.clone(),
        risk_level: parse_synth_risk(entry.result.risk_level.as_str()),
        confidence: ConfidenceLevel::Low,
        why_it_matters: entry
            .result
            .commits
            .first()
            .map(|commit| {
                format!(
                    "Recent history includes {} ({}) and {} relevant commit(s) overall.",
                    commit.summary,
                    commit.date,
                    entry.result.commits.len()
                )
            })
            .unwrap_or_else(|| "No relevant commit history was available for this target.".into()),
    }
}

fn heuristic_diff_review_focus(collected: &DiffReviewCollected) -> Vec<String> {
    let mut focus = collected
        .entries
        .iter()
        .filter(|entry| matches!(entry.result.risk_level, why_archaeologist::RiskLevel::HIGH))
        .map(|entry| {
            format!(
                "Review {} carefully because archaeology marked it HIGH risk.",
                format_target_label(&entry.target.target)
            )
        })
        .collect::<Vec<_>>();

    if focus.is_empty() && !collected.entries.is_empty() {
        focus.push("Review changed targets with the thinnest historical evidence first.".into());
    }

    focus
}

fn diff_review_target_label(plan: &DiffReviewPlan) -> String {
    let preview = plan
        .staged_files
        .iter()
        .take(3)
        .map(|file| normalize_path(&file.path))
        .collect::<Vec<_>>()
        .join(", ");
    if preview.is_empty() {
        "staged diff".into()
    } else {
        format!("staged diff touching {preview}")
    }
}

fn post_diff_review_comment(
    config: &why_context::WhyConfig,
    repo: &Repository,
    github_ref_override: Option<&str>,
    issue_refs: Vec<String>,
    body: &str,
) -> Result<GitHubComment> {
    let remote_name = config.github.remote.trim();
    if remote_name.is_empty() {
        return Err(anyhow!(
            "GitHub comment posting requires a configured GitHub remote"
        ));
    }

    let remote = repo
        .find_remote(remote_name)
        .map_err(|error| anyhow!("failed to read GitHub remote '{remote_name}': {error}"))?;
    let remote_url = remote
        .url()
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| anyhow!("GitHub remote '{remote_name}' has no URL"))?;
    let client = GitHubClient::from_config(config, remote_url)?;

    let issue = match github_ref_override {
        Some(value) => parse_github_ref(value)
            .ok_or_else(|| anyhow!("invalid GitHub reference '{value}'; expected #123"))?,
        None => select_single_github_ref(&issue_refs)?,
    };

    client.post_issue_comment(&issue, body)
}

fn infer_language(path: &std::path::Path) -> String {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "rust",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("py") => "python",
        _ => "unknown",
    }
    .to_string()
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn format_target_label(target: &why_locator::QueryTarget) -> String {
    match target.query_kind {
        QueryKind::Line => format!(
            "{}:{}",
            target.path.display(),
            target.start_line.unwrap_or_default()
        ),
        QueryKind::Range => format!(
            "{}:{}-{}",
            target.path.display(),
            target.start_line.unwrap_or_default(),
            target.end_line.unwrap_or_default()
        ),
        QueryKind::Symbol | QueryKind::QualifiedSymbol => format!(
            "{}:{}",
            target.path.display(),
            target.symbol.as_deref().unwrap_or("symbol")
        ),
    }
}

#[derive(Debug, Clone, Default)]
struct TerminalLinkContext {
    remote_url: Option<String>,
    head_rev: Option<String>,
}

fn commit_url(links: Option<&TerminalLinkContext>, oid: &str) -> Option<String> {
    let remote = links?.remote_url.as_deref()?;
    let repo = parse_github_remote(remote).ok()?;
    Some(format!(
        "https://github.com/{}/{}/commit/{oid}",
        repo.owner, repo.name
    ))
}

fn path_url(links: Option<&TerminalLinkContext>, path: &Path, line: Option<u32>) -> Option<String> {
    let remote = links?.remote_url.as_deref()?;
    let repo = parse_github_remote(remote).ok()?;
    let rev = links?.head_rev.as_deref()?;

    let mut url = format!(
        "https://github.com/{}/{}/blob/{}/{}",
        repo.owner,
        repo.name,
        rev,
        normalize_path(path)
    );
    if let Some(line) = line {
        url.push_str(&format!("#L{line}"));
    }
    Some(url)
}

fn terminal_link(label: impl AsRef<str>, url: &str) -> String {
    let label = label.as_ref();
    if !std::io::stdout().is_terminal() {
        return label.to_string();
    }
    format!("\u{1b}]8;;{url}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
}

fn linked_commit_label(links: Option<&TerminalLinkContext>, oid: &str, short_oid: &str) -> String {
    commit_url(links, oid)
        .map(|url| terminal_link(short_oid, &url))
        .unwrap_or_else(|| short_oid.to_string())
}

fn linked_path_label(
    links: Option<&TerminalLinkContext>,
    path: &Path,
    line: Option<u32>,
    label: impl AsRef<str>,
) -> String {
    let label = label.as_ref();
    path_url(links, path, line)
        .map(|url| terminal_link(label, &url))
        .unwrap_or_else(|| label.to_string())
}

fn format_target_heading(
    target: &why_locator::QueryTarget,
    links: Option<&TerminalLinkContext>,
) -> String {
    match target.query_kind {
        QueryKind::Line => linked_path_label(
            links,
            &target.path,
            target.start_line,
            format!(
                "{}:{}",
                target.path.display(),
                target.start_line.unwrap_or_default()
            ),
        ),
        QueryKind::Range => linked_path_label(
            links,
            &target.path,
            target.start_line,
            format!(
                "{}:{}-{}",
                target.path.display(),
                target.start_line.unwrap_or_default(),
                target.end_line.unwrap_or_default()
            ),
        ),
        QueryKind::Symbol | QueryKind::QualifiedSymbol => linked_path_label(
            links,
            &target.path,
            None,
            format!(
                "{}:{}",
                target.path.display(),
                target.symbol.as_deref().unwrap_or("symbol")
            ),
        ),
    }
}

fn format_output_target_heading(
    target: &why_archaeologist::OutputTarget,
    links: Option<&TerminalLinkContext>,
) -> String {
    match target.query_kind {
        QueryKind::Line => linked_path_label(
            links,
            &target.path,
            Some(target.start_line),
            format!("{}:{}", target.path.display(), target.start_line),
        ),
        QueryKind::Range => linked_path_label(
            links,
            &target.path,
            Some(target.start_line),
            format!(
                "{}:{}-{}",
                target.path.display(),
                target.start_line,
                target.end_line
            ),
        ),
        QueryKind::Symbol | QueryKind::QualifiedSymbol => {
            linked_path_label(links, &target.path, None, normalize_path(&target.path))
        }
    }
}

fn github_remote_url(repo: &Repository, config: &why_context::WhyConfig) -> Option<String> {
    let remote_name = config.github.remote.trim();
    if remote_name.is_empty() {
        return None;
    }
    let remote = repo.find_remote(remote_name).ok()?;
    let url = remote.url()?.trim();
    if url.is_empty() {
        return None;
    }
    parse_github_remote(url).ok()?;
    Some(url.to_string())
}

fn build_terminal_link_context(
    repo: &Repository,
    config: &why_context::WhyConfig,
) -> TerminalLinkContext {
    TerminalLinkContext {
        remote_url: github_remote_url(repo, config),
        head_rev: repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok())
            .map(|commit| commit.id().to_string())
            .or_else(|| {
                repo.head()
                    .ok()
                    .and_then(|head| head.shorthand().map(str::to_string))
            }),
    }
}

fn render_split_terminal(
    target: &why_locator::QueryTarget,
    suggestion: Option<&SplitSuggestion>,
    links: Option<&TerminalLinkContext>,
) {
    match suggestion {
        Some(suggestion) => {
            println!(
                "Suggested split for {}() ({} lines, lines {}-{})",
                suggestion.symbol,
                suggestion.total_lines,
                suggestion.start_line,
                suggestion.end_line
            );
            println!();
            for (index, block) in suggestion.blocks.iter().enumerate() {
                let label = ((b'A' + index as u8) as char).to_string();
                println!(
                    "  Block {}  lines {}-{}  {}",
                    label, block.start_line, block.end_line, block.era_label
                );
                println!(
                    "                           Dominant commit: {} — {}",
                    linked_commit_label(
                        links,
                        &block.dominant_commit_oid,
                        &block.dominant_commit_short_oid
                    ),
                    block.dominant_commit_summary
                );
                println!(
                    "                           -> Suggested extraction: {}()",
                    block.suggested_name
                );
                println!(
                    "                           Risk: {}",
                    block.risk_level.as_str()
                );
                println!(
                    "                           {} lines / {}% of function",
                    block.line_count, block.percentage_of_function
                );
                println!();
            }
            println!("These blocks have different reasons to change and different risk profiles.");
            println!("Splitting reduces blast radius by separating historically distinct paths.");
        }
        None => {
            let symbol = target.symbol.as_deref().unwrap_or("target");
            println!(
                "No split suggested for {symbol}. The target appears archaeologically cohesive."
            );
        }
    }
}

fn format_why_report(
    target: &why_locator::QueryTarget,
    report: &WhyReport,
    cached: bool,
    links: Option<&TerminalLinkContext>,
) -> String {
    let mut lines = vec![
        format!("why: {}", format_target_heading(target, links)),
        String::new(),
    ];

    if cached {
        lines.push("[cached]".to_string());
        lines.push(String::new());
    }

    lines.push("Summary".to_string());
    lines.push(report.summary.clone());
    lines.push(String::new());

    lines.push(format!(
        "Risk: {} ({})",
        report.risk_level.as_str(),
        report.confidence.as_str()
    ));
    lines.push(report.risk_summary.clone());
    lines.push(report.change_guidance.clone());
    lines.push(String::new());

    if !report.evidence.is_empty() {
        lines.push("Evidence".to_string());
        for item in &report.evidence {
            lines.push(format!("  - {item}"));
        }
        lines.push(String::new());
    }

    if !report.inference.is_empty() {
        lines.push("Inference".to_string());
        for item in &report.inference {
            lines.push(format!("  - {item}"));
        }
        lines.push(String::new());
    }

    if !report.unknowns.is_empty() {
        lines.push("Unknowns".to_string());
        for item in &report.unknowns {
            lines.push(format!("  - {item}"));
        }
        lines.push(String::new());
    }

    if !report.notes.is_empty() {
        lines.push("Notes".to_string());
        for item in &report.notes {
            lines.push(format!("  - {item}"));
        }
        lines.push(String::new());
    }

    if let Some(cost_usd) = report.cost_usd {
        lines.push(format!("Estimated cost: ~${cost_usd:.4}"));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        ExitStatus, HealthBaselineReference, HealthComparison, HealthGateSummary, HealthReport,
        HealthSignalDelta, build_github_enrichment, compute_health_comparison,
        compute_health_delta, current_watch_state, determine_health_exit_status,
        diff_review_mode_label, evaluate_health_gate, format_outage_timestamp, format_watch_frame,
        format_why_report, parse_outage_timestamp, parse_synth_risk, render_diff_review_markdown,
        render_health_terminal, render_outage_terminal, render_pr_template_markdown,
        sorted_signal_entries,
    };
    use crate::QueryRequest;
    use git2::Repository;
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use why_archaeologist::CommitEvidence;
    use why_cache::HealthSnapshot;
    use why_context::{GitHubConfig, WhyConfig};
    use why_scanner::{OutageFinding, OutageReport};
    use why_synthesizer::{
        ConfidenceLevel, DiffReviewFinding, DiffReviewReport, ReportMode, RiskLevel, WhyReport,
    };

    fn sample_report() -> WhyReport {
        WhyReport {
            summary: "This guard exists because a logout hotfix preserved token invalidation."
                .into(),
            evidence: vec![
                "fix: tokens not expiring on logout".into(),
                "comment references incident #4521".into(),
            ],
            inference: vec!["Removing the guard could reopen session invalidation bugs.".into()],
            unknowns: vec!["No incident postmortem was linked in history.".into()],
            risk_level: RiskLevel::HIGH,
            risk_summary: RiskLevel::HIGH.summary().into(),
            change_guidance: RiskLevel::HIGH.change_guidance().into(),
            confidence: ConfidenceLevel::MediumHigh,
            mode: ReportMode::Synthesized,
            notes: vec!["Keep evidence separate from inference.".into()],
            cost_usd: Some(0.0008),
        }
    }

    #[test]
    fn why_report_terminal_output_includes_all_sections() {
        let output = format_why_report(
            &why_locator::parse_target("src/auth.rs:verify_token", None)
                .expect("target should parse"),
            &sample_report(),
            false,
            None,
        );

        assert!(output.contains("why: src/auth.rs:verify_token"));
        assert!(output.contains("Summary"));
        assert!(output.contains("Risk: HIGH (medium-high)"));
        assert!(output.contains("Evidence"));
        assert!(output.contains("Inference"));
        assert!(output.contains("Unknowns"));
        assert!(output.contains("Notes"));
        assert!(output.contains("Estimated cost: ~$0.0008"));
    }

    #[test]
    fn why_report_terminal_output_shows_cached_marker_when_requested() {
        let output = format_why_report(
            &why_locator::parse_target("src/auth.rs:verify_token", None)
                .expect("target should parse"),
            &sample_report(),
            true,
            None,
        );
        assert!(output.contains("[cached]"));
    }

    #[test]
    fn why_report_terminal_output_omits_empty_optional_sections() {
        let report = WhyReport {
            evidence: Vec::new(),
            inference: Vec::new(),
            unknowns: Vec::new(),
            notes: Vec::new(),
            cost_usd: None,
            ..sample_report()
        };
        let output = format_why_report(
            &why_locator::parse_target("src/auth.rs:verify_token", None)
                .expect("target should parse"),
            &report,
            false,
            None,
        );

        assert!(!output.contains("Evidence\n"));
        assert!(!output.contains("Inference\n"));
        assert!(!output.contains("Unknowns\n"));
        assert!(!output.contains("Notes\n"));
        assert!(!output.contains("Estimated cost:"));
    }

    #[test]
    fn why_report_json_output_round_trips_expected_fields() {
        let report = sample_report();
        let json = serde_json::to_string_pretty(&report).expect("report should serialize");

        assert!(json.contains("\"summary\""));
        assert!(json.contains("\"evidence\""));
        assert!(json.contains("\"inference\""));
        assert!(json.contains("\"unknowns\""));
        assert!(json.contains("\"risk_level\": \"HIGH\""));
        assert!(json.contains("\"risk_summary\""));
        assert!(json.contains("\"change_guidance\""));
        assert!(json.contains("\"confidence\": \"medium-high\""));
        assert!(json.contains("\"mode\": \"synthesized\""));
        assert!(json.contains("\"notes\""));
        assert!(json.contains("\"cost_usd\": 0.0008"));
    }

    #[test]
    fn health_delta_marks_increasing_scores() {
        let previous = HealthSnapshot {
            timestamp: 1,
            debt_score: 5,
            signals: HashMap::new(),
            head_hash: None,
            ref_name: None,
        };
        let delta = compute_health_delta(9, &previous);
        assert_eq!(delta.direction, "↑");
        assert_eq!(delta.amount, 4);
        assert_eq!(delta.previous_score, 5);
    }

    #[test]
    fn sorted_signal_entries_are_stable_for_terminal_output() {
        let mut signals = HashMap::new();
        signals.insert("zeta".into(), 1);
        signals.insert("alpha".into(), 2);
        let names = sorted_signal_entries(&signals)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn render_health_terminal_includes_trend_signals_and_notes() {
        let mut signals = HashMap::new();
        signals.insert("time_bombs".into(), 2);
        let mut signal_deltas = BTreeMap::new();
        signal_deltas.insert(
            "time_bombs".into(),
            HealthSignalDelta {
                current: 2,
                baseline: 1,
                delta: 1,
            },
        );
        let mut signal_budgets = BTreeMap::new();
        signal_budgets.insert("time_bombs".into(), 0);
        let report = HealthReport {
            debt_score: 8,
            signals,
            delta: Some(compute_health_delta(
                8,
                &HealthSnapshot {
                    timestamp: 1,
                    debt_score: 5,
                    signals: HashMap::new(),
                    head_hash: None,
                    ref_name: None,
                },
            )),
            comparison: Some(HealthComparison {
                baseline: HealthBaselineReference {
                    source: "file".into(),
                    timestamp: 1,
                    head_hash: Some("abc123".into()),
                    ref_name: Some("main".into()),
                    debt_score: 7,
                },
                score_delta: 1,
                signal_deltas,
            }),
            gate: Some(HealthGateSummary {
                passed: false,
                absolute_threshold: Some(7),
                max_regression: Some(0),
                signal_budgets,
                reasons: vec!["health debt score 8 exceeds CI threshold 7".into()],
            }),
            notes: vec!["health uses implemented scanner signals".into()],
        };

        render_health_terminal(&report, Some(7), None);
    }

    #[test]
    fn compute_health_comparison_tracks_score_and_signal_deltas() {
        let mut current_signals = HashMap::new();
        current_signals.insert("time_bombs".into(), 2);
        current_signals.insert("hotspot_files".into(), 1);
        let mut baseline_signals = HashMap::new();
        baseline_signals.insert("time_bombs".into(), 1);
        baseline_signals.insert("stale_hacks".into(), 3);

        let comparison = compute_health_comparison(
            &HealthSnapshot {
                timestamp: 10,
                debt_score: 7,
                signals: current_signals,
                head_hash: Some("newhead".into()),
                ref_name: Some("main".into()),
            },
            &HealthSnapshot {
                timestamp: 5,
                debt_score: 4,
                signals: baseline_signals,
                head_hash: Some("oldhead".into()),
                ref_name: Some("origin/main".into()),
            },
        );

        assert_eq!(comparison.baseline.source, "file");
        assert_eq!(comparison.baseline.debt_score, 4);
        assert_eq!(comparison.score_delta, 3);
        assert_eq!(comparison.signal_deltas["time_bombs"].delta, 1);
        assert_eq!(comparison.signal_deltas["hotspot_files"].delta, 1);
        assert_eq!(comparison.signal_deltas["stale_hacks"].delta, -3);
    }

    #[test]
    fn evaluate_health_gate_collects_regression_reasons() {
        let mut signal_deltas = BTreeMap::new();
        signal_deltas.insert(
            "time_bombs".into(),
            HealthSignalDelta {
                current: 2,
                baseline: 1,
                delta: 1,
            },
        );

        let gate = evaluate_health_gate(
            &HealthSnapshot {
                timestamp: 10,
                debt_score: 8,
                signals: HashMap::new(),
                head_hash: None,
                ref_name: Some("main".into()),
            },
            &HealthComparison {
                baseline: HealthBaselineReference {
                    source: "file".into(),
                    timestamp: 1,
                    head_hash: None,
                    ref_name: Some("main".into()),
                    debt_score: 4,
                },
                score_delta: 4,
                signal_deltas,
            },
            Some(7),
            Some(2),
            &[("time_bombs".into(), 0)],
        );

        assert!(!gate.passed);
        assert_eq!(gate.absolute_threshold, Some(7));
        assert_eq!(gate.max_regression, Some(2));
        assert_eq!(gate.signal_budgets["time_bombs"], 0);
        assert_eq!(gate.reasons.len(), 3);
    }

    #[test]
    fn determine_health_exit_status_prefers_regression_failures() {
        let status = determine_health_exit_status(
            &HealthReport {
                debt_score: 9,
                signals: HashMap::new(),
                delta: None,
                comparison: Some(HealthComparison {
                    baseline: HealthBaselineReference {
                        source: "file".into(),
                        timestamp: 1,
                        head_hash: None,
                        ref_name: None,
                        debt_score: 5,
                    },
                    score_delta: 4,
                    signal_deltas: BTreeMap::new(),
                }),
                gate: Some(HealthGateSummary {
                    passed: false,
                    absolute_threshold: Some(8),
                    max_regression: Some(0),
                    signal_budgets: BTreeMap::new(),
                    reasons: vec![
                        "health debt score 9 exceeds CI threshold 8".into(),
                        "health debt score regressed by 4 which exceeds allowed regression 0"
                            .into(),
                    ],
                }),
                notes: Vec::new(),
            },
            Some(8),
            Some(0),
            false,
        );

        assert_eq!(
            status,
            ExitStatus::HealthRegressionFailure {
                message: "health debt score regressed by 4 which exceeds allowed regression 0"
                    .into()
            }
        );
    }

    #[test]
    fn render_pr_template_markdown_includes_expected_sections() {
        let markdown = render_pr_template_markdown(&why_scanner::PrTemplateReport {
            title_suggestion: "update staged changes".into(),
            summary: vec!["Touched crates/core and crates/scanner.".into()],
            risk_notes: vec!["No existing hotspot warnings matched the staged files.".into()],
            test_plan: vec!["[ ] Run targeted tests.".into()],
            staged_files: vec![why_scanner::StagedFile {
                path: std::path::PathBuf::from("crates/core/src/main.rs"),
                change: why_scanner::StagedChange::Modified,
            }],
        });

        assert!(markdown.contains("# update staged changes"));
        assert!(markdown.contains("## Summary"));
        assert!(markdown.contains("## Risk notes"));
        assert!(markdown.contains("## Test plan"));
        assert!(markdown.contains("## Staged files"));
        assert!(markdown.contains("crates/core/src/main.rs (modified)"));
    }

    #[test]
    fn parse_outage_timestamp_accepts_partial_iso8601() {
        let ts =
            parse_outage_timestamp("2025-11-03T14:00", "--from").expect("timestamp should parse");
        assert_eq!(format_outage_timestamp(ts), "2025-11-03 14:00");
    }

    #[test]
    fn parse_outage_timestamp_rejects_invalid_values() {
        let error = parse_outage_timestamp("not-a-time", "--from")
            .expect_err("invalid timestamps should fail");
        assert!(
            error
                .to_string()
                .contains("--from must be a valid ISO-8601 timestamp")
        );
    }

    #[test]
    fn render_outage_terminal_handles_findings() {
        let report = OutageReport {
            window_start_ts: parse_outage_timestamp("2025-11-03T14:00", "--from").unwrap(),
            window_end_ts: parse_outage_timestamp("2025-11-03T16:30", "--to").unwrap(),
            findings: vec![OutageFinding {
                oid: "abcdef1234567890".into(),
                short_oid: "abcdef1".into(),
                author: "alice".into(),
                date: "2025-11-03".into(),
                summary: "hotfix: rollback auth guard after outage (#42)".into(),
                risk_level: why_archaeologist::RiskLevel::HIGH,
                risk_summary: why_archaeologist::RiskLevel::HIGH.summary().into(),
                change_guidance: why_archaeologist::RiskLevel::HIGH.change_guidance().into(),
                blast_radius_files: 2,
                changed_paths: vec![
                    std::path::PathBuf::from("src/auth.rs"),
                    std::path::PathBuf::from("src/util.rs"),
                ],
                issue_refs: vec!["#42".into()],
                score: 4.2,
                notes: vec!["Representative touched paths: src/auth.rs, src/util.rs.".into()],
            }],
            notes: vec!["Scores are suggestive only.".into()],
        };

        render_outage_terminal(&report, 10, None);
    }

    #[test]
    fn render_diff_review_markdown_includes_expected_sections() {
        let markdown = render_diff_review_markdown(&DiffReviewReport {
            summary: "The staged diff touches one risky auth path.".into(),
            findings: vec![DiffReviewFinding {
                target: "src/auth.rs:authenticate".into(),
                path: "src/auth.rs".into(),
                symbol: Some("authenticate".into()),
                risk_level: RiskLevel::HIGH,
                confidence: ConfidenceLevel::MediumHigh,
                why_it_matters: "The function was repeatedly patched for session regressions."
                    .into(),
            }],
            reviewer_focus: vec!["Verify logout invalidation coverage.".into()],
            unknowns: vec!["No linked incident doc was present in sampled commits.".into()],
            notes: vec!["Heuristic fallback was not needed.".into()],
            mode: ReportMode::Synthesized,
            cost_usd: Some(0.0012),
            github_comment_url: Some("https://github.com/acme/why/issues/42#issuecomment-1".into()),
        });

        assert!(markdown.contains("# Diff review"));
        assert!(markdown.contains("## Findings"));
        assert!(markdown.contains("src/auth.rs:authenticate — HIGH (medium-high)"));
        assert!(markdown.contains("## Reviewer focus"));
        assert!(markdown.contains("## Unknowns"));
        assert!(markdown.contains("## Notes"));
        assert!(
            markdown
                .contains("GitHub comment: https://github.com/acme/why/issues/42#issuecomment-1")
        );
        assert!(markdown.contains("Mode: synthesized"));
        assert!(markdown.contains("Estimated cost: ~$0.0012"));
    }

    #[test]
    fn diff_review_mode_label_matches_report_modes() {
        assert_eq!(diff_review_mode_label(ReportMode::Heuristic), "heuristic");
        assert_eq!(
            diff_review_mode_label(ReportMode::Synthesized),
            "synthesized"
        );
    }

    #[test]
    fn parse_synth_risk_maps_unknown_values_to_low() {
        assert_eq!(parse_synth_risk("HIGH"), RiskLevel::HIGH);
        assert_eq!(parse_synth_risk("MEDIUM"), RiskLevel::MEDIUM);
        assert_eq!(parse_synth_risk("anything-else"), RiskLevel::LOW);
    }

    #[test]
    fn health_ci_failure_exit_status_is_distinct() {
        let status = ExitStatus::HealthCiFailure {
            message: "health debt score 9 exceeds CI threshold 4".into(),
        };
        assert_eq!(
            status,
            ExitStatus::HealthCiFailure {
                message: "health debt score 9 exceeds CI threshold 4".into()
            }
        );
    }

    #[test]
    fn health_regression_failure_exit_status_is_distinct() {
        let status = ExitStatus::HealthRegressionFailure {
            message: "health debt score regressed by 4 which exceeds allowed regression 0".into(),
        };
        assert_eq!(
            status,
            ExitStatus::HealthRegressionFailure {
                message: "health debt score regressed by 4 which exceeds allowed regression 0"
                    .into()
            }
        );
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "why-core-{name}-{}-{}",
            std::process::id(),
            super::current_unix_timestamp()
        ));
        fs::create_dir_all(&path).expect("test dir should be created");
        path
    }

    #[test]
    fn watch_frame_formats_expected_sections() {
        let request = QueryRequest {
            target: why_locator::parse_target("src/auth.rs:verify_token", None)
                .expect("target should parse"),
            json: false,
            no_llm: false,
            no_cache: true,
            split: false,
            coupled: false,
            since_days: None,
            team: false,
            blame_chain: false,
            evolution: false,
            annotate: false,
            watch: true,
            rename_safe: false,
        };

        let frame = format_watch_frame(&request, "body text", Some("boom"));
        assert!(frame.starts_with("\u{1b}[2J\u{1b}[HWatching src/auth.rs:verify_token\n"));
        assert!(frame.contains("Refreshes when src/auth.rs changes. Press Ctrl-C to stop."));
        assert!(frame.contains("\n\nbody text\n\n"));
        assert!(frame.contains("watch status: boom\n"));
    }

    #[test]
    fn watch_frame_defaults_to_waiting_status() {
        let request = QueryRequest {
            target: why_locator::parse_target("src/auth.rs:42", None).expect("target should parse"),
            json: false,
            no_llm: false,
            no_cache: true,
            split: false,
            coupled: false,
            since_days: None,
            team: false,
            blame_chain: false,
            evolution: false,
            annotate: false,
            watch: true,
            rename_safe: false,
        };

        let frame = format_watch_frame(&request, "body text", None);
        assert!(frame.contains("watch status: waiting for changes\n"));
    }

    #[test]
    fn current_watch_state_tracks_file_size_and_missing_files() {
        let dir = unique_test_dir("watch-state-existing");
        let path = dir.join("watched.rs");
        fs::write(&path, "fn watched() {}\n").expect("file should write");

        let state = current_watch_state(&path).expect("watch state should load");
        let state = state.expect("file should exist");
        assert_eq!(state.len, 16);
        assert!(state.modified_at.is_some());

        let missing =
            current_watch_state(&dir.join("missing.rs")).expect("missing files should not error");
        assert_eq!(missing, None);

        fs::remove_dir_all(&dir).expect("test dir should clean up");
    }

    #[test]
    fn current_watch_state_changes_after_file_update() {
        let dir = unique_test_dir("watch-state-updated");
        let path = dir.join("watched.rs");
        fs::write(&path, "fn watched() {}\n").expect("file should write");

        let initial = current_watch_state(&path)
            .expect("watch state should load")
            .expect("file should exist");
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&path, "fn watched() { println!(\"changed\"); }\n").expect("file should rewrite");

        let updated = current_watch_state(&path)
            .expect("watch state should reload")
            .expect("file should still exist");
        assert_ne!(updated, initial);
        assert!(updated.len > initial.len);

        fs::remove_dir_all(&dir).expect("test dir should clean up");
    }

    #[test]
    fn build_github_enrichment_reports_missing_remote() {
        let dir = unique_test_dir("missing-remote");
        let repo = Repository::init(&dir).expect("repo should initialize");
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: None,
            },
            ..WhyConfig::default()
        };

        let enrichment = build_github_enrichment(&repo, &config, &[]);
        assert!(enrichment.items.is_empty());
        assert_eq!(enrichment.notes.len(), 1);
        assert!(enrichment.notes[0].contains("origin"));
    }

    #[test]
    fn build_github_enrichment_reports_invalid_remote_url() {
        let dir = unique_test_dir("invalid-remote");
        let repo = Repository::init(&dir).expect("repo should initialize");
        repo.remote("origin", "https://gitlab.com/acme/why.git")
            .expect("remote should be created");
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: None,
            },
            ..WhyConfig::default()
        };
        let commits = vec![CommitEvidence {
            short_oid: "abcdef12".into(),
            oid: "abcdef1234567890".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: 0,
            date: "2026-03-13".into(),
            summary: "fix: close auth hole (#42)".into(),
            message: "fix: close auth hole (#42)".into(),
            diff_excerpt: String::new(),
            coverage_score: 1.0,
            relevance_score: 0.0,
            issue_refs: vec!["#42".into()],
            is_mechanical: false,
        }];

        let enrichment = build_github_enrichment(&repo, &config, &commits);
        assert!(enrichment.items.is_empty());
        assert_eq!(enrichment.notes.len(), 1);
        assert!(enrichment.notes[0].contains("unsupported GitHub remote"));
    }
}
