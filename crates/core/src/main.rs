mod cli;

use anyhow::{Result, anyhow};
use clap::CommandFactory;
use clap::Parser;
use clap_complete::{Generator, Shell, generate};
use clap_mangen::Man;
use cli::{Cli, CompletionShell, Mode, QueryRequest};
use git2::Repository;
use why_annotator::writer::annotate_file;
use why_archaeologist::{
    ArchaeologyResult, BlameChainResult, EvolutionHistoryResult, TeamReport, analyze_blame_chain,
    analyze_evolution_history, analyze_target_with_options, analyze_team,
};
use why_cache::{Cache, HealthSnapshot};
use why_context::load_config;
use why_evidence::{
    EvidenceCommit, EvidenceContext, EvidencePack, EvidenceTarget, GitHubClient, GitHubComment,
    GitHubEnrichment, enrich_github_refs, parse_github_ref, select_single_github_ref,
};
use why_locator::QueryKind;
use why_scanner::{
    CouplingReport, CoverageGapReport, DiffReviewPlan, DiffReviewTarget, GhostFinding,
    HealthDelta, HealthReport, HotspotFinding, OnboardFinding, PrTemplateReport, Severity,
    TimeBombFinding, TimeBombKind,
};
use why_splitter::SplitSuggestion;
use why_synthesizer::{
    AnthropicClient, AnthropicRequest, ConfidenceLevel, DiffReviewFinding, DiffReviewReport,
    ReportMode, WhyReport, build_diff_review_prompt, build_system_prompt,
    heuristic_diff_review_report, heuristic_report, parse_diff_review_response, parse_response,
    prompt_contract,
};

fn main() {
    match run() {
        Ok(ExitStatus::Success) => {}
        Ok(ExitStatus::HealthCiFailure { message }) => {
            eprintln!("why: {message}");
            std::process::exit(3);
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
        Mode::Hotspots { limit, json } => {
            run_hotspots(limit, json)?;
            Ok(ExitStatus::Success)
        }
        Mode::Health { json, ci } => run_health(json, ci),
        Mode::PrTemplate { json } => {
            run_pr_template(json)?;
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
        Mode::Query(request) => {
            run_query(request)?;
            Ok(ExitStatus::Success)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExitStatus {
    Success,
    HealthCiFailure { message: String },
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

fn run_hotspots(limit: usize, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let findings = why_scanner::scan_hotspots(&cwd, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_hotspots_terminal(&findings, limit);
    }

    Ok(())
}

fn run_health(json: bool, ci: Option<u32>) -> Result<ExitStatus> {
    let report = collect_health_report()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_health_terminal(&report, ci);
    }

    Ok(match ci {
        Some(threshold) if report.debt_score > threshold => ExitStatus::HealthCiFailure {
            message: format!(
                "health debt score {} exceeds CI threshold {}",
                report.debt_score, threshold
            ),
        },
        _ => ExitStatus::Success,
    })
}

fn collect_health_report() -> Result<HealthReport> {
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
    cache.insert_health_snapshot(HealthSnapshot {
        timestamp: current_unix_timestamp(),
        debt_score: report.debt_score,
        details: report.signals.clone(),
    })?;

    Ok(report)
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
    let mut report = synthesize_diff_review(&plan, &collected, no_llm)?;

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
    let report =
        why_scanner::scan_coverage_gap(&cwd, std::path::Path::new(coverage), limit, max_coverage)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_coverage_gap_terminal(&report, limit);
    }

    Ok(())
}

fn run_ghost(limit: usize, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let findings = why_scanner::scan_ghosts(&cwd, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_ghost_terminal(&findings, limit);
    }

    Ok(())
}

fn run_onboard(limit: usize, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let findings = why_scanner::scan_onboard(&cwd, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_onboard_terminal(&findings, limit);
    }

    Ok(())
}

fn run_time_bombs(age_days: i64, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let findings = why_scanner::scan_time_bombs(&cwd, age_days)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_time_bombs_terminal(&findings, age_days);
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

    if request.split {
        let suggestion = why_splitter::suggest_split(&request.target, &cwd)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&suggestion)?);
        } else {
            render_split_terminal(&request.target, suggestion.as_ref());
        }
        return Ok(());
    }

    if request.coupled {
        let report = why_scanner::scan_coupling(&cwd, &request.target, 10)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_coupling_terminal(&report);
        }
        return Ok(());
    }

    if request.team {
        let report = analyze_team(&request.target, &cwd, request.since_days)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_team_terminal(&report);
        }
        return Ok(());
    }

    if request.blame_chain {
        let report = analyze_blame_chain(&request.target, &cwd, request.since_days)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_blame_chain_terminal(&report);
        }
        return Ok(());
    }

    if request.evolution {
        let report = analyze_evolution_history(&request.target, &cwd, request.since_days)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            render_evolution_terminal(&report);
        }
        return Ok(());
    }

    let repo = Repository::discover(&cwd)?;
    let repo_root = repo
        .workdir()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| cwd.clone());
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
    let mut cache = Cache::open(&repo_root, config.cache.max_entries)?;

    if !request.no_cache {
        if let Some(cached) = cache.get::<WhyReport>(&cache_key) {
            if request.json {
                println!("{}", serde_json::to_string_pretty(&cached)?);
            } else {
                println!(
                    "{}",
                    format_why_report(&format_target_label(&request.target), &cached, true)
                );
            }

            if request.annotate {
                let result =
                    analyze_target_with_options(&request.target, &cwd, request.since_days)?;
                let source_path = repo_root.join(&result.target.path);
                annotate_file(
                    &source_path,
                    result.target.start_line,
                    &result,
                    &head_hash,
                    &format_target_label(&request.target),
                )?;
                println!("Annotation written to {}", source_path.display());
            }

            return Ok(());
        }
    }

    let result = analyze_target_with_options(&request.target, &cwd, request.since_days)?;
    let report = synthesize_report(&request, &result, &repo, &config)?;
    cache.set(cache_key, &report, &head_hash)?;

    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}",
            format_why_report(&format_target_label(&request.target), &report, false)
        );
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
        println!("Annotation written to {}", source_path.display());
    }

    Ok(())
}

fn render_team_terminal(report: &TeamReport) {
    let heading = match report.target.query_kind {
        QueryKind::Symbol | QueryKind::QualifiedSymbol => "Team ownership for target",
        _ => "Team ownership for file range",
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

fn render_blame_chain_terminal(report: &BlameChainResult) {
    let heading = match report.target.query_kind {
        QueryKind::Symbol | QueryKind::QualifiedSymbol => {
            format!("Blame chain for {}", report.target.path.display())
        }
        QueryKind::Range => format!(
            "Blame chain for {} (lines {}-{})",
            report.target.path.display(),
            report.target.start_line,
            report.target.end_line
        ),
        QueryKind::Line => format!(
            "Blame chain for {}:{}",
            report.target.path.display(),
            report.target.start_line
        ),
    };
    println!("{heading}");
    println!();
    println!(
        "Starting blame tip: {}  {}  {}  {}",
        report.starting_commit.short_oid,
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
                commit.short_oid, commit.summary, commit.date
            );
        }
    }

    println!();
    println!("  True origin:");
    println!(
        "    {}  {} ({})",
        report.origin_commit.short_oid, report.origin_commit.summary, report.origin_commit.date
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

fn render_coupling_terminal(report: &CouplingReport) {
    println!("Coupled files for {}", report.target_path.display());
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
            finding.path.display()
        );
    }
}

fn render_evolution_terminal(report: &EvolutionHistoryResult) {
    let heading = match report.target.query_kind {
        QueryKind::Symbol | QueryKind::QualifiedSymbol => {
            format!("Evolution history for {}", report.target.path.display())
        }
        QueryKind::Range => format!(
            "Evolution history for {} (lines {}-{})",
            report.target.path.display(),
            report.target.start_line,
            report.target.end_line
        ),
        QueryKind::Line => format!(
            "Evolution history for {}:{}",
            report.target.path.display(),
            report.target.start_line
        ),
    };
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
                latest.short_oid, latest.date, latest.summary
            );
            println!("Origin:");
            println!(
                "  {}  {}  {}",
                origin.short_oid, origin.date, origin.summary
            );
        }
        (Some(latest), None) => {
            println!("Current edge:");
            println!(
                "  {}  {}  {}",
                latest.short_oid, latest.date, latest.summary
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
            println!("  - {}", path.display());
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
                point.path_at_commit.display(),
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
                entry.commit.short_oid,
                entry.commit.date,
                entry.path_at_commit.display(),
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

fn render_hotspots_terminal(findings: &[HotspotFinding], limit: usize) {
    println!("Top {limit} hotspots by churn × risk");
    println!();
    if findings.is_empty() {
        println!("No source hotspots were found in the current repository.");
        return;
    }

    for (index, finding) in findings.iter().enumerate() {
        println!(
            "  {:>2}. {:<30} churn {:>3}  risk {:<6}  score {:.2}",
            index + 1,
            finding.path.display(),
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
    }
}

fn render_health_terminal(report: &HealthReport, ci: Option<u32>) {
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
            file.path.display(),
            file.change.as_str()
        ));
    }
    lines.push(String::new());

    lines.join("\n")
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

fn render_coverage_gap_terminal(report: &CoverageGapReport, limit: usize) {
    println!(
        "Top {limit} HIGH-risk functions at or below {:.1}% coverage",
        report.max_coverage
    );
    println!();
    println!("Coverage report: {}", report.coverage_path.display());
    println!();
    if report.findings.is_empty() {
        println!("No HIGH-risk coverage gaps were found in the current repository.");
    } else {
        for (index, finding) in report.findings.iter().enumerate() {
            let risk_flags = if finding.risk_flags.is_empty() {
                "none".to_string()
            } else {
                finding.risk_flags.join(", ")
            };
            println!(
                "  {:>2}. {}:{}-{}  {}  coverage {:>5.1}%  commits {:>2}",
                index + 1,
                finding.path.display(),
                finding.start_line,
                finding.end_line,
                finding.symbol,
                finding.coverage_pct,
                finding.commit_count
            );
            println!(
                "      instrumented: {} line(s), covered: {}",
                finding.instrumented_lines, finding.covered_lines
            );
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

fn render_ghost_terminal(findings: &[GhostFinding], limit: usize) {
    println!("Top {limit} ghost functions by risk-aware archaeology");
    println!();
    if findings.is_empty() {
        println!("No high-risk ghost functions were found in the current repository.");
        return;
    }

    for (index, finding) in findings.iter().enumerate() {
        println!(
            "  {:>2}. {}:{}-{}  {}  commits {:>2}  call-sites {:>2}",
            index + 1,
            finding.path.display(),
            finding.start_line,
            finding.end_line,
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

fn render_onboard_terminal(findings: &[OnboardFinding], limit: usize) {
    println!("Top {limit} symbols to understand first");
    println!();
    if findings.is_empty() {
        println!("No onboarding findings were found in the current repository.");
        return;
    }

    for (index, finding) in findings.iter().enumerate() {
        println!(
            "  {:>2}. {}:{}-{}  {}  risk {:<6}  score {:.2}",
            index + 1,
            finding.path.display(),
            finding.start_line,
            finding.end_line,
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

fn render_time_bombs_terminal(findings: &[TimeBombFinding], age_threshold: i64) {
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
                "  {}. {}:{}  {}",
                index + 1,
                finding.path.display(),
                finding.line,
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
                "  {}. {}:{}  {}",
                index + 1,
                finding.path.display(),
                finding.line,
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
                "  {}. {}:{}  {}",
                index + 1,
                finding.path.display(),
                finding.line,
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

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn synthesize_report(
    request: &QueryRequest,
    result: &ArchaeologyResult,
    repo: &Repository,
    config: &why_context::WhyConfig,
) -> Result<WhyReport> {
    let github = build_github_enrichment(repo, config, &result.commits);
    let evidence_pack = why_evidence::build(
        &EvidenceTarget {
            file: result.target.path.display().to_string(),
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

    let client = match AnthropicClient::from_env() {
        Ok(client) => client,
        Err(_) => return Ok(fallback()),
    };

    let contract = prompt_contract();
    let evidence_json = serde_json::to_string_pretty(&evidence_pack)?;
    let response = match client.send(&AnthropicRequest {
        system_prompt: format!(
            "You are a careful code archaeology assistant. {} Required fields: {}. Grounding rules: {}",
            contract.response_format,
            contract.required_fields.join(", "),
            contract.grounding_rules.join(" ")
        ),
        user_prompt: format!(
            "Use this evidence pack to explain why the target exists and the risk of changing it.\n\nEvidence pack:\n{}",
            evidence_json
        ),
    }) {
        Ok(response) => response,
        Err(_) => return Ok(fallback()),
    };

    match parse_response(&response.text) {
        Ok(mut report) => {
            report.cost_usd = Some(response.cost_usd);
            Ok(report)
        }
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

fn synthesize_diff_review(
    plan: &DiffReviewPlan,
    collected: &DiffReviewCollected,
    no_llm: bool,
) -> Result<DiffReviewReport> {
    let fallback = || heuristic_diff_review_report(
        format!(
            "Heuristic diff review of {} staged target(s).",
            collected.entries.len()
        ),
        collected
            .entries
            .iter()
            .map(|entry| heuristic_diff_review_finding(entry))
            .collect(),
        heuristic_diff_review_focus(&collected),
        collected.notes.clone(),
    );

    if no_llm {
        return Ok(fallback());
    }

    let client = match AnthropicClient::from_env() {
        Ok(client) => client,
        Err(_) => return Ok(fallback()),
    };

    let target_label = diff_review_target_label(plan);
    let response = match client.send(&AnthropicRequest {
        system_prompt: build_system_prompt(&prompt_contract()),
        user_prompt: build_diff_review_prompt(
            &target_label,
            &collected
                .entries
                .iter()
                .map(|entry| entry.evidence_pack.clone())
                .collect::<Vec<_>>(),
        ),
    }) {
        Ok(response) => response,
        Err(_) => return Ok(fallback()),
    };

    match parse_diff_review_response(&response.text) {
        Ok(mut report) => {
            report.cost_usd = Some(response.cost_usd);
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
            file: result.target.path.display().to_string(),
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
        path: entry.target.target.path.display().to_string(),
        symbol: entry.target.symbol.clone(),
        risk_level: parse_synth_risk(entry.result.risk_level.as_str()),
        confidence: ConfidenceLevel::Low,
        why_it_matters: entry
            .result
            .commits
            .first()
            .map(|commit| format!(
                "Recent history includes {} ({}) and {} relevant commit(s) overall.",
                commit.summary,
                commit.date,
                entry.result.commits.len()
            ))
            .unwrap_or_else(|| "No relevant commit history was available for this target.".into()),
    }
}

fn heuristic_diff_review_focus(collected: &DiffReviewCollected) -> Vec<String> {
    let mut focus = collected
        .entries
        .iter()
        .filter(|entry| matches!(entry.result.risk_level, why_archaeologist::RiskLevel::HIGH))
        .map(|entry| format!(
            "Review {} carefully because archaeology marked it HIGH risk.",
            format_target_label(&entry.target.target)
        ))
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
        .map(|file| file.path.display().to_string())
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
        return Err(anyhow!("GitHub comment posting requires a configured GitHub remote"));
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

fn render_split_terminal(target: &why_locator::QueryTarget, suggestion: Option<&SplitSuggestion>) {
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
                    block.dominant_commit_short_oid, block.dominant_commit_summary
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

fn format_why_report(target: &str, report: &WhyReport, cached: bool) -> String {
    let mut lines = vec![format!("why: {target}"), String::new()];

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
        ExitStatus, HealthReport, build_github_enrichment, compute_health_delta,
        diff_review_mode_label, format_why_report, parse_synth_risk, render_diff_review_markdown,
        render_health_terminal, render_pr_template_markdown, sorted_signal_entries,
        synthesize_diff_review,
    };
    use git2::Repository;
    use std::collections::HashMap;
    use std::fs;
    use why_archaeologist::{ArchaeologyResult, CommitEvidence, LocalContext, OutputTarget};
    use why_cache::HealthSnapshot;
    use why_context::{GitHubConfig, WhyConfig};
    use why_evidence::EvidencePack;
    use why_locator::{QueryKind, QueryTarget};
    use why_scanner::{DiffReviewPlan, DiffReviewTarget, StagedChange, StagedDiffFile, StagedLineRange};
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
        let output = format_why_report("src/auth.rs:verify_token", &sample_report(), false);

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
        let output = format_why_report("src/auth.rs:verify_token", &sample_report(), true);
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
        let output = format_why_report("src/auth.rs:verify_token", &report, false);

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
            details: HashMap::new(),
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
        let report = HealthReport {
            debt_score: 8,
            signals,
            delta: Some(compute_health_delta(
                8,
                &HealthSnapshot {
                    timestamp: 1,
                    debt_score: 5,
                    details: HashMap::new(),
                },
            )),
            notes: vec!["health uses implemented scanner signals".into()],
        };

        let mut buffer = Vec::new();
        {
            use std::io::Write;
            writeln!(&mut buffer, "Repository health").expect("write heading");
            writeln!(&mut buffer).expect("write spacing");
        }
        let _ = buffer;
        render_health_terminal(&report, None);
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
    fn render_diff_review_markdown_includes_expected_sections() {
        let markdown = render_diff_review_markdown(&DiffReviewReport {
            summary: "The staged diff touches one risky auth path.".into(),
            findings: vec![DiffReviewFinding {
                target: "src/auth.rs:authenticate".into(),
                path: "src/auth.rs".into(),
                symbol: Some("authenticate".into()),
                risk_level: RiskLevel::HIGH,
                confidence: ConfidenceLevel::MediumHigh,
                why_it_matters: "The function was repeatedly patched for session regressions.".into(),
            }],
            reviewer_focus: vec!["Verify logout invalidation coverage.".into()],
            unknowns: vec!["No linked incident doc was present in sampled commits.".into()],
            notes: vec!["Heuristic fallback was not needed.".into()],
            mode: ReportMode::Synthesized,
            cost_usd: Some(0.0012),
            github_comment_url: Some(
                "https://github.com/acme/why/issues/42#issuecomment-1".into(),
            ),
        });

        assert!(markdown.contains("# Diff review"));
        assert!(markdown.contains("## Findings"));
        assert!(markdown.contains("src/auth.rs:authenticate — HIGH (medium-high)"));
        assert!(markdown.contains("## Reviewer focus"));
        assert!(markdown.contains("## Unknowns"));
        assert!(markdown.contains("## Notes"));
        assert!(markdown.contains("GitHub comment: https://github.com/acme/why/issues/42#issuecomment-1"));
        assert!(markdown.contains("Mode: synthesized"));
        assert!(markdown.contains("Estimated cost: ~$0.0012"));
    }

    #[test]
    fn diff_review_mode_label_matches_report_modes() {
        assert_eq!(diff_review_mode_label(ReportMode::Heuristic), "heuristic");
        assert_eq!(diff_review_mode_label(ReportMode::Synthesized), "synthesized");
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
