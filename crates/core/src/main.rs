mod cli;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Mode, QueryRequest};
use git2::Repository;
use why_archaeologist::{
    ArchaeologyResult, BlameChainResult, TeamReport, analyze_blame_chain,
    analyze_target_with_options, analyze_team,
};
use why_cache::Cache;
use why_context::load_config;
use why_evidence::{EvidenceCommit, EvidenceContext, EvidenceTarget};
use why_locator::QueryKind;
use why_scanner::{CouplingReport, HotspotFinding};
use why_splitter::SplitSuggestion;
use why_synthesizer::{AnthropicClient, AnthropicRequest, WhyReport, heuristic_report, parse_response, prompt_contract};

fn main() {
    if let Err(error) = run() {
        eprintln!("why: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let mode = cli.parse_mode()?;

    match mode {
        Mode::Mcp => why_mcp::run_stdio(),
        Mode::Hotspots { limit, json } => run_hotspots(limit, json),
        Mode::Query(request) => run_query(request),
    }
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
                println!("{}", format_why_report(&format_target_label(&request.target), &cached, true));
            }
            return Ok(());
        }
    }

    let result = analyze_target_with_options(&request.target, &cwd, request.since_days)?;
    let report = synthesize_report(&request, &result)?;
    cache.set(cache_key, &report, &head_hash)?;

    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", format_why_report(&format_target_label(&request.target), &report, false));
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

fn synthesize_report(request: &QueryRequest, result: &ArchaeologyResult) -> Result<WhyReport> {
    let evidence_pack = why_evidence::build(
        &EvidenceTarget {
            file: result.target.path.display().to_string(),
            symbol: request.target.symbol.clone(),
            lines: (result.target.start_line as usize, result.target.end_line as usize),
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
    );

    let fallback = || {
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
            result.notes.clone(),
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
    use super::format_why_report;
    use why_synthesizer::{ConfidenceLevel, ReportMode, RiskLevel, WhyReport};

    fn sample_report() -> WhyReport {
        WhyReport {
            summary: "This guard exists because a logout hotfix preserved token invalidation.".into(),
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
}
