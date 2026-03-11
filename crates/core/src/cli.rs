use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use why_locator::{QueryTarget, parse_target};

#[derive(Debug, Parser)]
#[command(name = "why")]
#[command(about = "Ask your codebase why a line exists")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Positional target: <file>:<line>, <file>:<symbol>, or <file> with --lines.
    pub target: Option<String>,

    /// Explicit 1-based line range in START:END form.
    #[arg(long, value_name = "START:END")]
    pub lines: Option<String>,

    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,

    /// Skip LLM synthesis.
    #[arg(long)]
    pub no_llm: bool,

    /// Show archaeology-guided split suggestions for a symbol target.
    #[arg(long)]
    pub split: bool,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the MCP stdio server.
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub target: QueryTarget,
    pub json: bool,
    pub no_llm: bool,
    pub split: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Query(QueryRequest),
    Mcp,
}

impl Cli {
    pub fn parse_mode(self) -> Result<Mode> {
        match self.command {
            Some(Command::Mcp) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.json
                    || self.no_llm
                    || self.split
                {
                    bail!("the mcp subcommand does not accept query flags or a target");
                }
                Ok(Mode::Mcp)
            }
            None => {
                let target = self.target.ok_or_else(|| {
                    anyhow::anyhow!(
                        "target must use <file>:<line>, <file>:<symbol>, or <file> --lines <start:end>"
                    )
                })?;

                Ok(Mode::Query(QueryRequest {
                    target: parse_target(&target, self.lines.as_deref())?,
                    json: self.json,
                    no_llm: self.no_llm,
                    split: self.split,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, Mode, QueryRequest};
    use clap::Parser;
    use std::path::PathBuf;
    use why_locator::{QueryKind, QueryTarget};

    #[test]
    fn parses_line_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:42"]);
        let mode = cli.parse_mode().expect("line target should parse");

        assert_eq!(
            mode,
            Mode::Query(QueryRequest {
                target: QueryTarget {
                    path: PathBuf::from("src/lib.rs"),
                    start_line: Some(42),
                    end_line: Some(42),
                    symbol: None,
                    query_kind: QueryKind::Line,
                },
                json: false,
                no_llm: false,
                split: false,
            })
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
        let mode = cli.parse_mode().expect("range target should parse");

        assert_eq!(
            mode,
            Mode::Query(QueryRequest {
                target: QueryTarget {
                    path: PathBuf::from("src/lib.rs"),
                    start_line: Some(40),
                    end_line: Some(45),
                    symbol: None,
                    query_kind: QueryKind::Range,
                },
                json: true,
                no_llm: true,
                split: false,
            })
        );
    }

    #[test]
    fn parses_split_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--split"]);
        let mode = cli.parse_mode().expect("split target should parse");
        let Mode::Query(request) = mode else {
            panic!("expected query mode");
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(!request.json);
        assert!(!request.no_llm);
        assert!(request.split);
    }

    #[test]
    fn parses_mcp_subcommand() {
        let cli = Cli::parse_from(["why", "mcp"]);
        assert_eq!(cli.command, Some(Command::Mcp));
        assert_eq!(cli.parse_mode().expect("mcp should parse"), Mode::Mcp);
    }

    #[test]
    fn rejects_query_flags_for_mcp() {
        let error = Cli::try_parse_from(["why", "mcp", "--json"])
            .expect_err("clap should reject query flags for mcp");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--json' found")
        );
    }
}
