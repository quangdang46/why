use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use why_locator::{QueryTarget, parse_target};

#[derive(Debug, Parser)]
#[command(name = "why")]
#[command(
    about = "Ask your codebase why a line, range, symbol, or repo hotspot exists",
    after_help = "Examples:\n  why src/auth.rs:42\n  why src/auth.rs --lines 40:45 --no-llm\n  why src/auth.rs:verify_token --json\n  why src/auth.rs:AuthService::login --team\n  why src/auth.rs:verify_token --blame-chain\n  why src/auth.rs:verify_token --evolution\n  why hotspots --limit 10\n  why health\n  why health --ci 80\n  why pr-template\n  why ghost --limit 10"
)]
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

    /// Show rename-aware target evolution history as a timeline.
    #[arg(long)]
    pub evolution: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the MCP stdio server.
    Mcp,
    /// Start an interactive archaeology shell with completion support.
    Shell,
    /// Run the LSP hover server over stdio.
    Lsp,
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

        /// Exit with code 3 when the debt score exceeds this threshold.
        #[arg(long, value_name = "THRESHOLD", value_parser = clap::value_parser!(u32).range(0..=100))]
        ci: Option<u32>,
    },
    /// Generate a reviewer-friendly PR template from the staged diff.
    PrTemplate {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Find high-risk functions that appear uncalled under static analysis.
    Ghost {
        /// Maximum number of findings to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,

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
    /// Generate shell completion scripts for the why CLI.
    Completions {
        /// Target shell to generate completions for.
        #[arg(value_enum)]
        shell: CompletionShell,
    },
    /// Generate a man page for the why CLI.
    Manpage,
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
    pub evolution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Query(QueryRequest),
    Mcp,
    Shell,
    Lsp,
    Hotspots { limit: usize, json: bool },
    Health { json: bool, ci: Option<u32> },
    PrTemplate { json: bool },
    Ghost { limit: usize, json: bool },
    InstallHooks { warn_only: bool },
    UninstallHooks,
    Completions { shell: CompletionShell },
    Manpage,
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
                    || self.evolution
                {
                    bail!("the mcp subcommand does not accept query flags or a target");
                }
                Ok(Mode::Mcp)
            }
            Some(Command::Shell) => {
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
                    || self.evolution
                {
                    bail!("the shell subcommand does not accept query flags or a target");
                }
                Ok(Mode::Shell)
            }
            Some(Command::Lsp) => {
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
                    || self.evolution
                {
                    bail!("the lsp subcommand does not accept query flags or a target");
                }
                Ok(Mode::Lsp)
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
                    || self.evolution
                {
                    bail!("the hotspots subcommand does not accept query flags or a target");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                Ok(Mode::Hotspots { limit, json })
            }
            Some(Command::Health { json, ci }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                    || self.evolution
                {
                    bail!("the health subcommand does not accept query flags or a target");
                }
                Ok(Mode::Health { json, ci })
            }
            Some(Command::PrTemplate { json }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                    || self.evolution
                {
                    bail!("the pr-template subcommand does not accept query flags or a target");
                }
                Ok(Mode::PrTemplate { json })
            }
            Some(Command::Ghost { limit, json }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.no_llm
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                    || self.evolution
                {
                    bail!("the ghost subcommand does not accept query flags or a target");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                Ok(Mode::Ghost { limit, json })
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
                    || self.evolution
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
                    || self.evolution
                {
                    bail!("the uninstall-hooks subcommand does not accept query flags or a target");
                }
                Ok(Mode::UninstallHooks)
            }
            Some(Command::Completions { shell }) => {
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
                    || self.evolution
                {
                    bail!("the completions subcommand does not accept query flags or a target");
                }
                Ok(Mode::Completions { shell })
            }
            Some(Command::Manpage) => {
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
                    || self.evolution
                {
                    bail!("the manpage subcommand does not accept query flags or a target");
                }
                Ok(Mode::Manpage)
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
                    evolution: self.evolution,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, CompletionShell, Mode, QueryRequest};
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
                evolution: false,
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
                evolution: false,
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
        assert!(!request.evolution);
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
        assert!(!request.evolution);
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
        assert!(!request.evolution);
    }

    #[test]
    fn parses_evolution_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--since", "30", "--evolution"]);
        let mode = cli.parse_mode().expect("evolution target should parse");
        let Mode::Query(request) = mode else {
            panic!("expected query mode");
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(!request.no_cache);
        assert_eq!(request.since_days, Some(30));
        assert!(request.evolution);
        assert!(!request.team);
        assert!(!request.coupled);
        assert!(!request.split);
        assert!(!request.blame_chain);
    }

    #[test]
    fn parses_mcp_subcommand() {
        let cli = Cli::parse_from(["why", "mcp"]);
        assert_eq!(cli.command, Some(Command::Mcp));
        assert_eq!(cli.parse_mode().expect("mcp should parse"), Mode::Mcp);
    }

    #[test]
    fn parses_shell_subcommand() {
        let cli = Cli::parse_from(["why", "shell"]);
        assert_eq!(cli.command, Some(Command::Shell));
        assert_eq!(cli.parse_mode().expect("shell should parse"), Mode::Shell);
    }

    #[test]
    fn parses_lsp_subcommand() {
        let cli = Cli::parse_from(["why", "lsp"]);
        assert_eq!(cli.command, Some(Command::Lsp));
        assert_eq!(cli.parse_mode().expect("lsp should parse"), Mode::Lsp);
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
            Mode::Health {
                json: true,
                ci: None,
            }
        );
    }

    #[test]
    fn parses_health_ci_subcommand() {
        let cli = Cli::parse_from(["why", "health", "--ci", "80", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("health ci should parse"),
            Mode::Health {
                json: true,
                ci: Some(80),
            }
        );
    }

    #[test]
    fn parses_pr_template_subcommand() {
        let cli = Cli::parse_from(["why", "pr-template", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("pr-template should parse"),
            Mode::PrTemplate { json: true }
        );
    }

    #[test]
    fn parses_ghost_subcommand() {
        let cli = Cli::parse_from(["why", "ghost", "--limit", "7", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("ghost should parse"),
            Mode::Ghost {
                limit: 7,
                json: true,
            }
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
    fn parses_completions_subcommand() {
        let cli = Cli::parse_from(["why", "completions", "zsh"]);
        assert_eq!(
            cli.parse_mode().expect("completions should parse"),
            Mode::Completions {
                shell: CompletionShell::Zsh
            }
        );
    }

    #[test]
    fn parses_manpage_subcommand() {
        let cli = Cli::parse_from(["why", "manpage"]);
        assert_eq!(cli.parse_mode().expect("manpage should parse"), Mode::Manpage);
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
    fn rejects_query_flags_for_shell() {
        let error = Cli::try_parse_from(["why", "shell", "--json"])
            .expect_err("clap should reject query flags for shell");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--json' found")
        );
    }

    #[test]
    fn rejects_query_flags_for_lsp() {
        let error = Cli::try_parse_from(["why", "lsp", "--json"])
            .expect_err("clap should reject query flags for lsp");
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
    fn rejects_query_flags_for_ghost() {
        let error = Cli::try_parse_from(["why", "ghost", "--no-cache"])
            .expect_err("clap should reject query flags for ghost");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }

    #[test]
    fn rejects_query_flags_for_pr_template() {
        let error = Cli::try_parse_from(["why", "pr-template", "--no-cache"])
            .expect_err("clap should reject query flags for pr-template");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }

    #[test]
    fn rejects_out_of_range_health_ci_threshold() {
        let error = Cli::try_parse_from(["why", "health", "--ci", "101"])
            .expect_err("health ci should reject thresholds above 100");
        assert!(error.to_string().contains("101"));
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

    #[test]
    fn rejects_query_flags_for_completions() {
        let error = Cli::try_parse_from(["why", "completions", "bash", "--json"])
            .expect_err("clap should reject query flags for completions");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--json' found")
        );
    }

    #[test]
    fn rejects_query_flags_for_manpage() {
        let error = Cli::try_parse_from(["why", "manpage", "--json"])
            .expect_err("clap should reject query flags for manpage");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--json' found")
        );
    }
}
