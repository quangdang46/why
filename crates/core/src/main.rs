mod cli;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use why_archaeologist::{ArchaeologyResult, analyze_target};
use why_locator::QueryKind;

fn main() {
    if let Err(error) = run() {
        eprintln!("why: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let request = cli.parse_request()?;
    let cwd = std::env::current_dir()?;
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
}
