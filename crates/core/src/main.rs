mod cli;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Mode, QueryRequest};
use why_archaeologist::{ArchaeologyResult, analyze_target};
use why_locator::QueryKind;
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
        Mode::Query(request) => run_query(request),
    }
}

fn run_query(request: QueryRequest) -> Result<()> {
    let cwd = std::env::current_dir()?;

    if request.split {
        let suggestion = why_splitter::suggest_split(&request.target, &cwd)?;
        if request.json {
            println!("{}", serde_json::to_string_pretty(&suggestion)?);
        } else {
            render_split_terminal(&request.target, suggestion.as_ref());
        }
        return Ok(());
    }

    let result = analyze_target(&request.target, &cwd)?;

    if request.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        render_terminal(&result);
    }

    Ok(())
}

fn render_terminal(result: &ArchaeologyResult) {
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
    println!(
        "No LLM synthesis (--no-llm or no API key). Heuristic risk: {}.",
        result.risk_level.as_str()
    );
    println!("{}", result.risk_summary);
    println!("{}", result.change_guidance);
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
