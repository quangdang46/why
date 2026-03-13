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

    /// Bypass cached results and refresh the query output.
    #[arg(long)]
    pub no_cache: bool,

    /// Show archaeology-guided split suggestions for a symbol target.
    #[arg(long)]
    pub split: bool,

    /// Show file-level co-change coupling for the queried target.
    #[arg(long)]
    pub coupled: bool,

    /// Limit history to commits from the last N days.
    #[arg(long, value_name = "DAYS")]
    pub since: Option<u64>,

    /// Show ownership and bus-factor information for the queried target.
    #[arg(long)]
    pub team: bool,

    /// Walk past mechanical commits to show the likely true origin commit.
    #[arg(long)]
    pub blame_chain: bool,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the MCP stdio server.
    Mcp,
    /// Rank repository hotspots using churn × heuristic risk scoring.
    Hotspots {
        /// Maximum number of findings to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,

        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Aggregate repo-wide scanner signals into a health dashboard.
    Health {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Install managed git hooks that warn on high-risk changes.
    InstallHooks {
        /// Warn instead of blocking when high-risk changes are detected.
        #[arg(long)]
        warn_only: bool,
    },
    /// Remove managed git hooks and restore backups when present.
    UninstallHooks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub target: QueryTarget,
    pub json: bool,
    pub no_llm: bool,
    pub no_cache: bool,
    pub split: bool,
    pub coupled: bool,
    pub since_days: Option<u64>,
    pub team: bool,
    pub blame_chain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Query(QueryRequest),
    Mcp,
    Hotspots { limit: usize, json: bool },
    Health { json: bool },
    InstallHooks { warn_only: bool },
    UninstallHooks,
}

impl Cli {
    pub fn parse_mode(self) -> Result<Mode> {
        match self.command {
            Some(Command::Mcp) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.json
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                {
                    bail!("the mcp subcommand does not accept query flags or a target");
                }
                Ok(Mode::Mcp)
            }
            Some(Command::Hotspots { limit, json }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                {
                    bail!("the hotspots subcommand does not accept query flags or a target");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                Ok(Mode::Hotspots { limit, json })
            }
            Some(Command::Health { json }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                {
                    bail!("the health subcommand does not accept query flags or a target");
                }
                Ok(Mode::Health { json })
            }
            Some(Command::InstallHooks { warn_only }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.json
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                {
                    bail!("the install-hooks subcommand does not accept query flags or a target");
                }
                Ok(Mode::InstallHooks { warn_only })
            }
            Some(Command::UninstallHooks) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.json
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                {
                    bail!("the uninstall-hooks subcommand does not accept query flags or a target");
                }
                Ok(Mode::UninstallHooks)
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
                    no_cache: self.no_cache,
                    split: self.split,
                    coupled: self.coupled,
                    since_days: self.since,
                    team: self.team,
                    blame_chain: self.blame_chain,
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
                no_cache: false,
                split: false,
                coupled: false,
                since_days: None,
                team: false,
                blame_chain: false,
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
                no_cache: false,
                split: false,
                coupled: false,
                since_days: None,
                team: false,
                blame_chain: false,
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
        assert!(!request.no_cache);
        assert!(request.split);
        assert!(!request.coupled);
    }

    #[test]
    fn parses_coupled_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--coupled"]);
        let mode = cli.parse_mode().expect("coupled target should parse");
        let Mode::Query(request) = mode else {
            panic!("expected query mode");
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(!request.json);
        assert!(!request.no_llm);
        assert!(!request.no_cache);
        assert!(!request.split);
        assert!(request.coupled);
        assert_eq!(request.since_days, None);
        assert!(!request.team);
        assert!(!request.blame_chain);
    }

    #[test]
    fn parses_since_and_team_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--since", "30", "--team"]);
        let mode = cli.parse_mode().expect("since/team target should parse");
        let Mode::Query(request) = mode else {
            panic!("expected query mode");
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(!request.no_cache);
        assert_eq!(request.since_days, Some(30));
        assert!(request.team);
        assert!(!request.blame_chain);
    }

    #[test]
    fn parses_blame_chain_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--blame-chain"]);
        let mode = cli.parse_mode().expect("blame-chain target should parse");
        let Mode::Query(request) = mode else {
            panic!("expected query mode");
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(!request.no_cache);
        assert!(request.blame_chain);
        assert!(!request.team);
        assert!(!request.coupled);
        assert!(!request.split);
    }

    #[test]
    fn parses_mcp_subcommand() {
        let cli = Cli::parse_from(["why", "mcp"]);
        assert_eq!(cli.command, Some(Command::Mcp));
        assert_eq!(cli.parse_mode().expect("mcp should parse"), Mode::Mcp);
    }

    #[test]
    fn parses_hotspots_subcommand() {
        let cli = Cli::parse_from(["why", "hotspots", "--limit", "7", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("hotspots should parse"),
            Mode::Hotspots {
                limit: 7,
                json: true,
            }
        );
    }

    #[test]
    fn parses_health_subcommand() {
        let cli = Cli::parse_from(["why", "health", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("health should parse"),
            Mode::Health { json: true }
        );
    }

    #[test]
    fn parses_install_hooks_subcommand() {
        let cli = Cli::parse_from(["why", "install-hooks", "--warn-only"]);
        assert_eq!(
            cli.parse_mode().expect("install-hooks should parse"),
            Mode::InstallHooks { warn_only: true }
        );
    }

    #[test]
    fn parses_uninstall_hooks_subcommand() {
        let cli = Cli::parse_from(["why", "uninstall-hooks"]);
        assert_eq!(
            cli.parse_mode().expect("uninstall-hooks should parse"),
            Mode::UninstallHooks
        );
    }

    #[test]
    fn parses_no_cache_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:42", "--no-cache"]);
        let mode = cli.parse_mode().expect("no-cache target should parse");
        let Mode::Query(request) = mode else {
            panic!("expected query mode");
        };

        assert!(request.no_cache);
        assert!(!request.no_llm);
    }

    #[test]
    fn rejects_positional_target_for_hotspots() {
        let error = Cli::try_parse_from(["why", "hotspots", "--limit", "5", "src/lib.rs:42"])
            .expect_err("clap should reject positional targets for hotspots");
        assert!(
            error
                .to_string()
                .contains("unexpected argument 'src/lib.rs:42' found")
        );
    }

    #[test]
    fn rejects_zero_limit_for_hotspots() {
        let cli = Cli::parse_from(["why", "hotspots", "--limit", "0"]);
        let error = cli
            .parse_mode()
            .expect_err("hotspots should reject a zero limit");
        assert!(
            error
                .to_string()
                .contains("--limit must be greater than zero")
        );
    }

    #[test]
    fn rejects_no_cache_for_hotspots() {
        let error = Cli::try_parse_from(["why", "hotspots", "--no-cache"])
            .expect_err("clap should reject query flags for hotspots");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
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

    #[test]
    fn rejects_query_flags_for_health() {
        let error = Cli::try_parse_from(["why", "health", "--no-cache"])
            .expect_err("clap should reject query flags for health");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }

    #[test]
    fn rejects_query_flags_for_install_hooks() {
        let error = Cli::try_parse_from(["why", "install-hooks", "--json"])
            .expect_err("clap should reject query flags for install-hooks");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--json' found")
        );
    }

    #[test]
    fn rejects_query_flags_for_uninstall_hooks() {
        let error = Cli::try_parse_from(["why", "uninstall-hooks", "--no-cache"])
            .expect_err("clap should reject query flags for uninstall-hooks");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }
}
