use anyhow::Result;
use clap::Parser;
use why_locator::{QueryTarget, parse_target};

#[derive(Debug, Parser)]
#[command(name = "why")]
#[command(about = "Ask your codebase why a line exists")]
pub struct Cli {
    /// Positional target: <file>:<line>, <file>:<symbol>, or <file> with --lines.
    pub target: String,

    /// Explicit 1-based line range in START:END form.
    #[arg(long, value_name = "START:END")]
    pub lines: Option<String>,

    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,

    /// Skip LLM synthesis.
    #[arg(long)]
    pub no_llm: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub target: QueryTarget,
    pub json: bool,
    pub no_llm: bool,
}

impl Cli {
    pub fn parse_request(self) -> Result<QueryRequest> {
        Ok(QueryRequest {
            target: parse_target(&self.target, self.lines.as_deref())?,
            json: self.json,
            no_llm: self.no_llm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, QueryRequest};
    use clap::Parser;
    use std::path::PathBuf;
    use why_locator::{QueryKind, QueryTarget};

    #[test]
    fn parses_line_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:42"]);
        let request = cli.parse_request().expect("line target should parse");

        assert_eq!(
            request,
            QueryRequest {
                target: QueryTarget {
                    path: PathBuf::from("src/lib.rs"),
                    start_line: Some(42),
                    end_line: Some(42),
                    symbol: None,
                    query_kind: QueryKind::Line,
                },
                json: false,
                no_llm: false,
            }
        );
    }

    #[test]
    fn parses_range_request() {
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
            request,
            QueryRequest {
                target: QueryTarget {
                    path: PathBuf::from("src/lib.rs"),
                    start_line: Some(40),
                    end_line: Some(45),
                    symbol: None,
                    query_kind: QueryKind::Range,
                },
                json: true,
                no_llm: true,
            }
        );
    }
}
