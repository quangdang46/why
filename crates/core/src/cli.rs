use anyhow::{Result, anyhow, bail};
use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "why")]
#[command(about = "Ask your codebase why a line exists")]
pub struct Cli {
    /// Positional phase-1 target: <file>:<line> or <file> with --lines.
    pub target: String,

    /// Explicit 1-based line range in START:END form.
    #[arg(long, value_name = "START:END")]
    pub lines: Option<String>,

    /// Emit machine-readable phase-1 output.
    #[arg(long)]
    pub json: bool,

    /// Skip LLM synthesis.
    #[arg(long)]
    pub no_llm: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Line,
    Range,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QueryTarget {
    pub path: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub query_kind: QueryKind,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QueryRequest {
    pub target: QueryTarget,
    pub json: bool,
    pub no_llm: bool,
}

impl Cli {
    pub fn parse_request(self) -> Result<QueryRequest> {
        let target = match self.lines {
            Some(lines) => parse_range_target(&self.target, &lines)?,
            None => parse_line_target(&self.target)?,
        };

        Ok(QueryRequest {
            target,
            json: self.json,
            no_llm: self.no_llm,
        })
    }
}

fn parse_line_target(input: &str) -> Result<QueryTarget> {
    let (path, line_text) = input
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("phase 1 requires <file>:<line> or <file> --lines <start:end>"))?;

    if path.is_empty() {
        bail!("target path cannot be empty");
    }

    if line_text.parse::<u32>().is_err() {
        bail!(
            "phase 1 requires <file>:<line> or <file> --lines <start:end>; symbol syntax is reserved for phase 2"
        );
    }

    let start_line = parse_line_number(line_text, "line")?;

    Ok(QueryTarget {
        path: PathBuf::from(path),
        start_line,
        end_line: start_line,
        query_kind: QueryKind::Line,
    })
}

fn parse_range_target(path: &str, lines: &str) -> Result<QueryTarget> {
    if path.is_empty() {
        bail!("target path cannot be empty");
    }

    if path.contains(':') {
        bail!(
            "phase 1 range queries use <file> --lines <start:end>; symbol syntax is reserved for phase 2"
        );
    }

    let (start_text, end_text) = lines
        .split_once(':')
        .ok_or_else(|| anyhow!("--lines must use START:END syntax"))?;

    let start_line = parse_line_number(start_text, "range start")?;
    let end_line = parse_line_number(end_text, "range end")?;

    if end_line < start_line {
        bail!("range end must be greater than or equal to range start");
    }

    Ok(QueryTarget {
        path: PathBuf::from(path),
        start_line,
        end_line,
        query_kind: QueryKind::Range,
    })
}

fn parse_line_number(input: &str, label: &str) -> Result<u32> {
    let line = input
        .parse::<u32>()
        .map_err(|_| anyhow!("invalid {label}: expected a positive integer"))?;

    if line == 0 {
        bail!("invalid {label}: line numbers are 1-based");
    }

    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::{Cli, QueryKind, QueryTarget};
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn parses_line_target() {
        let cli = Cli::parse_from(["why", "src/lib.rs:42"]);
        let request = cli.parse_request().expect("line target should parse");

        assert_eq!(
            request.target,
            QueryTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: 42,
                end_line: 42,
                query_kind: QueryKind::Line,
            }
        );
        assert!(!request.json);
        assert!(!request.no_llm);
    }

    #[test]
    fn parses_range_target() {
        let cli = Cli::parse_from([
            "why",
            "src/lib.rs",
            "--lines",
            "40:45",
            "--json",
            "--no-llm",
        ]);
        let request = cli.parse_request().expect("range target should parse");

        assert_eq!(
            request.target,
            QueryTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: 40,
                end_line: 45,
                query_kind: QueryKind::Range,
            }
        );
        assert!(request.json);
        assert!(request.no_llm);
    }

    #[test]
    fn rejects_symbol_target_in_phase_one() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate"]);
        let error = cli
            .parse_request()
            .expect_err("symbol target should fail in phase 1");

        assert!(
            error
                .to_string()
                .contains("phase 1 requires <file>:<line> or <file> --lines <start:end>")
        );
    }

    #[test]
    fn rejects_mixed_colon_target_with_lines() {
        let cli = Cli::parse_from(["why", "src/lib.rs:42", "--lines", "40:45"]);
        let error = cli
            .parse_request()
            .expect_err("mixed line target with --lines should fail");

        assert!(
            error
                .to_string()
                .contains("phase 1 range queries use <file> --lines <start:end>")
        );
    }

    #[test]
    fn rejects_reversed_range() {
        let cli = Cli::parse_from(["why", "src/lib.rs", "--lines", "45:40"]);
        let error = cli.parse_request().expect_err("reversed range should fail");

        assert!(
            error
                .to_string()
                .contains("range end must be greater than or equal to range start")
        );
    }
}
