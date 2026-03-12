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
use why_locator::QueryKind;
use why_scanner::{CouplingReport, HotspotFinding};
use why_splitter::SplitSuggestion;

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
        if let Some(cached) = cache.get::<ArchaeologyResult>(&cache_key) {
            if request.json {
                println!("{}", serde_json::to_string_pretty(&cached)?);
            } else {
                render_terminal(&cached, true);
            }
            return Ok(());
        }
    }

    let result = analyze_target_with_options(&request.target, &cwd, request.since_days)?;
    cache.set(cache_key, &result, &head_hash)?;

    if request.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        render_terminal(&result, false);
    }

    Ok(())
}

fn render_terminal(result: &ArchaeologyResult, cached: bool) {
    match result.target.query_kind {
        QueryKind::Line => {
            println!(
                "why: {} (line {})",
                result.target.path.display(),
                result.target.start_line
            );
            println!();
            println!("Commits touching this line:");
        }
        QueryKind::Range => {
            println!(
                "why: {} (lines {}-{})",
                result.target.path.display(),
                result.target.start_line,
                result.target.end_line
            );
            println!();
            println!("Commits touching this range:");
        }
        QueryKind::Symbol | QueryKind::QualifiedSymbol => {
            println!("why: {}", result.target.path.display());
            println!();
            println!("Commits touching this target:");
        }
    }

    for commit in &result.commits {
        println!(
            "  {}  {}  {}  {}",
            commit.short_oid, commit.author, commit.date, commit.summary
        );
    }

    println!();
    if cached {
        println!("[cached]");
    }
    println!(
        "No LLM synthesis (--no-llm or no API key). Heuristic risk: {}.",
        result.risk_level.as_str()
    );
    println!("{}", result.risk_summary);
    println!("{}", result.change_guidance);
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
