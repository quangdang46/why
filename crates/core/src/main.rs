mod cli;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use serde::Serialize;
use why_archaeologist::{blame_range, discover_repository, relative_repo_path};

#[derive(Debug, Serialize)]
struct JsonOutput {
    target: cli::QueryTarget,
    commits: Vec<why_archaeologist::BlameCommit>,
    risk_level: &'static str,
    mode: &'static str,
    notes: Vec<&'static str>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("why: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let request = cli.parse_request()?;
    let repo = discover_repository(&std::env::current_dir()?)?;
    let relative_path = relative_repo_path(&repo, &request.target.path)?;
    let blame = blame_range(
        &repo,
        &relative_path,
        request.target.start_line,
        request.target.end_line,
    )?;

    if request.json {
        let output = JsonOutput {
            target: request.target,
            commits: blame.commits,
            risk_level: "MEDIUM",
            mode: "heuristic",
            notes: vec!["No LLM synthesis in phase 1"],
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        match request.target.query_kind {
            cli::QueryKind::Line => {
                println!(
                    "why: {} (line {})",
                    blame.relative_path.display(),
                    request.target.start_line
                );
            }
            cli::QueryKind::Range => {
                println!(
                    "why: {} (lines {}-{})",
                    blame.relative_path.display(),
                    request.target.start_line,
                    request.target.end_line
                );
            }
        }

        println!();
        println!("Commits touching this target:");
        for commit in &blame.commits {
            println!("  {}", commit.short_oid);
        }
        println!();
        println!("No LLM synthesis (--no-llm or no API key). Heuristic risk: MEDIUM.");
    }

    Ok(())
}
