use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use why_evidence::parse_github_ref;
use why_locator::{QueryTarget, parse_target};

#[derive(Debug, Parser)]
#[command(name = "why")]
#[command(
    about = "Ask your codebase why a line, range, symbol, or repo hotspot exists",
    after_help = "Examples:\n  why src/auth.rs:42\n  why src/auth.rs --lines 40:45 --no-llm\n  why src/auth.rs:verify_token --json\n  why src/auth.rs:verify_token --annotate\n  why src/auth.rs:verify_token --rename-safe\n  why src/auth.rs:verify_token --watch --no-llm\n  why src/auth.rs:AuthService::login --team\n  why src/auth.rs:verify_token --blame-chain\n  why src/auth.rs:verify_token --evolution\n  why hotspots --limit 10\n  why health\n  why health --ci 80\n  why pr-template\n  why diff-review --no-llm\n  why explain-outage --from 2025-11-03T14:00 --to 2025-11-03T16:30\n  why coverage-gap --coverage lcov.info\n  why ghost --limit 10\n  why onboard --limit 10\n  why time-bombs --age-days 180\n  eval \"$(why context-inject)\""
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

    /// Write a short evidence-backed doc annotation above the target.
    #[arg(long)]
    pub annotate: bool,

    /// Refresh the default why report whenever the target file changes.
    #[arg(long)]
    pub watch: bool,

    /// Show target risk plus caller risk to assess whether a Rust symbol rename is safe.
    #[arg(long)]
    pub rename_safe: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Subcommand, Clone, PartialEq)]
pub enum Command {
    /// Run the MCP stdio server.
    Mcp,
    /// Start an interactive archaeology shell with completion support.
    Shell,
    /// Run the LSP hover server over stdio.
    Lsp,
    /// Emit shell wrappers that prepend archaeology context to supported AI tools.
    ContextInject,
    /// Rank repository hotspots using churn × heuristic risk scoring.
    Hotspots {
        /// Maximum number of findings to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,

        /// Only include hotspots whose ownership history includes this author.
        #[arg(long, value_name = "AUTHOR")]
        owner: Option<String>,

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

        /// Load a baseline snapshot from a JSON file for regression checks.
        #[arg(long, value_name = "PATH")]
        baseline_file: Option<PathBuf>,

        /// Write the current health snapshot to a JSON file.
        #[arg(long, value_name = "PATH")]
        write_baseline: Option<PathBuf>,

        /// Fail when the debt score increases by more than this amount.
        #[arg(long, value_name = "POINTS")]
        max_regression: Option<u32>,

        /// Fail when a specific signal increases by more than the allowed amount.
        #[arg(long, value_name = "SIGNAL=COUNT")]
        max_signal_regression: Vec<String>,

        /// Fail if a requested baseline file cannot be loaded.
        #[arg(long)]
        require_baseline: bool,
    },
    /// Generate a reviewer-friendly PR template from the staged diff.
    PrTemplate {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Rank suspicious commits inside a bounded incident window.
    ExplainOutage {
        /// Inclusive window start timestamp in ISO-8601 form (for example 2025-11-03T14:00).
        #[arg(long, value_name = "TIMESTAMP")]
        from: String,

        /// Inclusive window end timestamp in ISO-8601 form (for example 2025-11-03T16:30).
        #[arg(long, value_name = "TIMESTAMP")]
        to: String,

        /// Maximum number of findings to return.
        #[arg(long, default_value_t = 10)]
        limit: usize,

        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Review the staged diff with archaeology-backed risk findings.
    DiffReview {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,

        /// Skip LLM synthesis and use heuristic review output only.
        #[arg(long)]
        no_llm: bool,

        /// Post the rendered review as a GitHub issue/PR comment.
        #[arg(long)]
        post_github_comment: bool,

        /// Explicit GitHub issue/PR reference to use when posting (for example #42).
        #[arg(long, value_name = "#123")]
        github_ref: Option<String>,
    },
    /// Cross-reference HIGH-risk functions against LCOV or llvm-cov JSON coverage.
    CoverageGap {
        /// Path to an LCOV file or llvm-cov JSON report.
        #[arg(long, value_name = "PATH")]
        coverage: String,

        /// Maximum number of findings to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,

        /// Only include findings at or below this coverage percentage.
        #[arg(long, value_name = "PERCENT", default_value_t = 20.0)]
        max_coverage: f32,

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
    /// Rank the symbols a new engineer should understand first.
    Onboard {
        /// Maximum number of findings to return.
        #[arg(long, default_value_t = 10)]
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
    /// Find stale TODOs, HACK/TEMP markers, and expired remove-after dates.
    TimeBombs {
        /// Age threshold in days for aged markers (default: 180).
        #[arg(long, default_value_t = 180)]
        age_days: i64,

        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
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
    pub annotate: bool,
    pub watch: bool,
    pub rename_safe: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Query(QueryRequest),
    Mcp,
    Shell,
    Lsp,
    ContextInject,
    Hotspots {
        limit: usize,
        owner: Option<String>,
        json: bool,
    },
    Health {
        json: bool,
        ci: Option<u32>,
        baseline_file: Option<PathBuf>,
        write_baseline: Option<PathBuf>,
        max_regression: Option<u32>,
        max_signal_regression: Vec<String>,
        require_baseline: bool,
    },
    PrTemplate {
        json: bool,
    },
    ExplainOutage {
        from: String,
        to: String,
        limit: usize,
        json: bool,
    },
    DiffReview {
        json: bool,
        no_llm: bool,
        post_github_comment: bool,
        github_ref: Option<String>,
    },
    CoverageGap {
        coverage: String,
        limit: usize,
        max_coverage: f32,
        json: bool,
    },
    Ghost {
        limit: usize,
        json: bool,
    },
    Onboard {
        limit: usize,
        json: bool,
    },
    InstallHooks {
        warn_only: bool,
    },
    UninstallHooks,
    Completions {
        shell: CompletionShell,
    },
    Manpage,
    TimeBombs {
        age_days: i64,
        json: bool,
    },
}

fn parse_signal_budget(raw: &str) -> Result<(String, u32)> {
    let (signal, count) = raw
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("signal budgets must use SIGNAL=COUNT format"))?;
    let signal = signal.trim();
    if signal.is_empty() {
        bail!("signal budget names must not be empty");
    }
    let count = count
        .trim()
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("signal budget counts must be non-negative integers"))?;
    Ok((signal.to_string(), count))
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the lsp subcommand does not accept query flags or a target");
                }
                Ok(Mode::Lsp)
            }
            Some(Command::ContextInject) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the context-inject subcommand does not accept query flags or a target");
                }
                Ok(Mode::ContextInject)
            }
            Some(Command::Hotspots { limit, owner, json }) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the hotspots subcommand does not accept query flags or a target");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                let owner = owner.map(|owner| owner.trim().to_string());
                if owner.as_deref().is_some_and(str::is_empty) {
                    bail!("--owner must not be empty");
                }
                Ok(Mode::Hotspots { limit, owner, json })
            }
            Some(Command::Health {
                json,
                ci,
                baseline_file,
                write_baseline,
                max_regression,
                max_signal_regression,
                require_baseline,
            }) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the health subcommand does not accept query flags or a target");
                }
                if baseline_file.is_none() && require_baseline {
                    bail!("--require-baseline requires --baseline-file");
                }
                if baseline_file.is_none()
                    && (max_regression.is_some() || !max_signal_regression.is_empty())
                {
                    bail!("health regression budgets require --baseline-file");
                }
                for budget in &max_signal_regression {
                    parse_signal_budget(budget)?;
                }
                Ok(Mode::Health {
                    json,
                    ci,
                    baseline_file,
                    write_baseline,
                    max_regression,
                    max_signal_regression,
                    require_baseline,
                })
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the pr-template subcommand does not accept query flags or a target");
                }
                Ok(Mode::PrTemplate { json })
            }
            Some(Command::ExplainOutage {
                from,
                to,
                limit,
                json,
            }) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the explain-outage subcommand does not accept query flags or a target");
                }
                if from.trim().is_empty() {
                    bail!("--from must not be empty");
                }
                if to.trim().is_empty() {
                    bail!("--to must not be empty");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                Ok(Mode::ExplainOutage {
                    from: from.trim().to_string(),
                    to: to.trim().to_string(),
                    limit,
                    json,
                })
            }
            Some(Command::DiffReview {
                json,
                no_llm,
                post_github_comment,
                github_ref,
            }) => {
                if self.target.is_some()
                    || self.lines.is_some()
                    || self.no_cache
                    || self.split
                    || self.coupled
                    || self.since.is_some()
                    || self.team
                    || self.blame_chain
                    || self.evolution
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                    || self.json
                {
                    bail!("the diff-review subcommand does not accept query flags or a target");
                }
                let github_ref = match github_ref {
                    Some(github_ref) => {
                        let trimmed = github_ref.trim();
                        if trimmed.is_empty() {
                            bail!("--github-ref must not be empty");
                        }
                        if !post_github_comment {
                            bail!("--github-ref requires --post-github-comment");
                        }
                        if parse_github_ref(trimmed).is_none() {
                            bail!("--github-ref must use #123 syntax");
                        }
                        Some(trimmed.to_string())
                    }
                    None => None,
                };
                Ok(Mode::DiffReview {
                    json,
                    no_llm,
                    post_github_comment,
                    github_ref,
                })
            }
            Some(Command::CoverageGap {
                coverage,
                limit,
                max_coverage,
                json,
            }) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the coverage-gap subcommand does not accept query flags or a target");
                }
                if coverage.trim().is_empty() {
                    bail!("--coverage must not be empty");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                if !(0.0..=100.0).contains(&max_coverage) {
                    bail!("--max-coverage must be between 0 and 100");
                }
                Ok(Mode::CoverageGap {
                    coverage,
                    limit,
                    max_coverage,
                    json,
                })
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the ghost subcommand does not accept query flags or a target");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                Ok(Mode::Ghost { limit, json })
            }
            Some(Command::Onboard { limit, json }) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the onboard subcommand does not accept query flags or a target");
                }
                if limit == 0 {
                    bail!("--limit must be greater than zero");
                }
                Ok(Mode::Onboard { limit, json })
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the manpage subcommand does not accept query flags or a target");
                }
                Ok(Mode::Manpage)
            }
            Some(Command::TimeBombs { age_days, json }) => {
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
                    || self.annotate
                    || self.watch
                    || self.rename_safe
                {
                    bail!("the time-bombs subcommand does not accept query flags or a target");
                }
                Ok(Mode::TimeBombs { age_days, json })
            }
            None => {
                let target = self.target.ok_or_else(|| {
                    anyhow::anyhow!(
                        "target must use <file>:<line>, <file>:<symbol>, or <file> --lines <start:end>"
                    )
                })?;

                if self.watch {
                    if self.json {
                        bail!("--watch does not support --json");
                    }
                    if self.annotate {
                        bail!("--watch does not support --annotate");
                    }
                    if self.split
                        || self.coupled
                        || self.team
                        || self.blame_chain
                        || self.evolution
                        || self.rename_safe
                    {
                        bail!(
                            "--watch supports only the default why report (no specialty query flags)"
                        );
                    }
                }

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
                    annotate: self.annotate,
                    watch: self.watch,
                    rename_safe: self.rename_safe,
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
                annotate: false,
                watch: false,
                rename_safe: false,
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
                annotate: false,
                watch: false,
                rename_safe: false,
            })
        );
    }

    #[test]
    fn parses_split_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--split"]);
        let mode = cli.parse_mode().expect("split target should parse");
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
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
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
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
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
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
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
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
        let cli = Cli::parse_from([
            "why",
            "src/lib.rs:authenticate",
            "--since",
            "30",
            "--evolution",
        ]);
        let mode = cli.parse_mode().expect("evolution target should parse");
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
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
        assert!(!request.annotate);
    }

    #[test]
    fn parses_annotate_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--annotate"]);
        let mode = cli.parse_mode().expect("annotate target should parse");
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(request.annotate);
        assert!(!request.watch);
        assert!(!request.rename_safe);
        assert!(!request.split);
        assert!(!request.coupled);
        assert!(!request.team);
        assert!(!request.blame_chain);
        assert!(!request.evolution);
    }

    #[test]
    fn parses_rename_safe_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--rename-safe"]);
        let mode = cli.parse_mode().expect("rename-safe target should parse");
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(request.rename_safe);
        assert!(!request.annotate);
        assert!(!request.watch);
        assert!(!request.split);
        assert!(!request.coupled);
        assert!(!request.team);
        assert!(!request.blame_chain);
        assert!(!request.evolution);
    }

    #[test]
    fn parses_watch_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--watch"]);
        let mode = cli.parse_mode().expect("watch target should parse");
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
        };

        assert_eq!(request.target.path, PathBuf::from("src/lib.rs"));
        assert_eq!(request.target.symbol.as_deref(), Some("authenticate"));
        assert_eq!(request.target.query_kind, QueryKind::Symbol);
        assert!(request.watch);
        assert!(!request.annotate);
        assert!(!request.rename_safe);
        assert!(!request.split);
        assert!(!request.coupled);
        assert!(!request.team);
        assert!(!request.blame_chain);
        assert!(!request.evolution);
    }

    #[test]
    fn rejects_watch_with_json() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--watch", "--json"]);
        let error = cli.parse_mode().expect_err("watch should reject json mode");
        assert!(
            error
                .to_string()
                .contains("--watch does not support --json")
        );
    }

    #[test]
    fn rejects_watch_with_annotate() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--watch", "--annotate"]);
        let error = cli
            .parse_mode()
            .expect_err("watch should reject annotate mode");
        assert!(
            error
                .to_string()
                .contains("--watch does not support --annotate")
        );
    }

    #[test]
    fn rejects_watch_with_specialty_query_flags() {
        let cli = Cli::parse_from(["why", "src/lib.rs:authenticate", "--watch", "--rename-safe"]);
        let error = cli
            .parse_mode()
            .expect_err("watch should reject specialty query modes");
        assert!(
            error
                .to_string()
                .contains("--watch supports only the default why report")
        );
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
    fn parses_context_inject_subcommand() {
        let cli = Cli::parse_from(["why", "context-inject"]);
        assert_eq!(cli.command, Some(Command::ContextInject));
        assert_eq!(
            cli.parse_mode().expect("context-inject should parse"),
            Mode::ContextInject
        );
    }

    #[test]
    fn parses_hotspots_subcommand() {
        let cli = Cli::parse_from(["why", "hotspots", "--limit", "7", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("hotspots should parse"),
            Mode::Hotspots {
                limit: 7,
                owner: None,
                json: true,
            }
        );
    }

    #[test]
    fn parses_hotspots_owner_filter() {
        let cli = Cli::parse_from(["why", "hotspots", "--limit", "7", "--owner", "Fixture Bot"]);
        assert_eq!(
            cli.parse_mode()
                .expect("hotspots owner filter should parse"),
            Mode::Hotspots {
                limit: 7,
                owner: Some("Fixture Bot".into()),
                json: false,
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
                baseline_file: None,
                write_baseline: None,
                max_regression: None,
                max_signal_regression: Vec::new(),
                require_baseline: false,
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
                baseline_file: None,
                write_baseline: None,
                max_regression: None,
                max_signal_regression: Vec::new(),
                require_baseline: false,
            }
        );
    }

    #[test]
    fn parses_health_regression_subcommand() {
        let cli = Cli::parse_from([
            "why",
            "health",
            "--json",
            "--baseline-file",
            "baseline.json",
            "--write-baseline",
            "next.json",
            "--max-regression",
            "2",
            "--max-signal-regression",
            "time_bombs=0",
            "--max-signal-regression",
            "stale_hacks=1",
            "--require-baseline",
        ]);
        assert_eq!(
            cli.parse_mode().expect("health regression should parse"),
            Mode::Health {
                json: true,
                ci: None,
                baseline_file: Some(std::path::PathBuf::from("baseline.json")),
                write_baseline: Some(std::path::PathBuf::from("next.json")),
                max_regression: Some(2),
                max_signal_regression: vec!["time_bombs=0".into(), "stale_hacks=1".into()],
                require_baseline: true,
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
    fn parses_explain_outage_subcommand() {
        let cli = Cli::parse_from([
            "why",
            "explain-outage",
            "--from",
            "2025-11-03T14:00",
            "--to",
            "2025-11-03T16:30",
            "--limit",
            "7",
            "--json",
        ]);
        assert_eq!(
            cli.parse_mode().expect("explain-outage should parse"),
            Mode::ExplainOutage {
                from: "2025-11-03T14:00".into(),
                to: "2025-11-03T16:30".into(),
                limit: 7,
                json: true,
            }
        );
    }

    #[test]
    fn parses_explain_outage_with_defaults() {
        let cli = Cli::parse_from([
            "why",
            "explain-outage",
            "--from",
            "2025-11-03T14:00",
            "--to",
            "2025-11-03T16:30",
        ]);
        assert_eq!(
            cli.parse_mode().expect("explain-outage should parse"),
            Mode::ExplainOutage {
                from: "2025-11-03T14:00".into(),
                to: "2025-11-03T16:30".into(),
                limit: 10,
                json: false,
            }
        );
    }

    #[test]
    fn parses_diff_review_subcommand() {
        let cli = Cli::parse_from([
            "why",
            "diff-review",
            "--json",
            "--no-llm",
            "--post-github-comment",
            "--github-ref",
            "#42",
        ]);
        assert_eq!(
            cli.parse_mode().expect("diff-review should parse"),
            Mode::DiffReview {
                json: true,
                no_llm: true,
                post_github_comment: true,
                github_ref: Some("#42".into()),
            }
        );
    }

    #[test]
    fn parses_diff_review_without_optional_flags() {
        let cli = Cli::parse_from(["why", "diff-review"]);
        assert_eq!(
            cli.parse_mode().expect("diff-review should parse"),
            Mode::DiffReview {
                json: false,
                no_llm: false,
                post_github_comment: false,
                github_ref: None,
            }
        );
    }

    #[test]
    fn rejects_query_flags_for_diff_review() {
        let error = Cli::try_parse_from(["why", "diff-review", "--no-cache"])
            .expect_err("clap should reject query flags for diff-review");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }

    #[test]
    fn rejects_positional_target_for_diff_review() {
        let error = Cli::try_parse_from(["why", "diff-review", "src/lib.rs:42"])
            .expect_err("clap should reject positional targets for diff-review");
        assert!(
            error
                .to_string()
                .contains("unexpected argument 'src/lib.rs:42' found")
        );
    }

    #[test]
    fn rejects_empty_github_ref_for_diff_review() {
        let cli = Cli::parse_from(["why", "diff-review", "--github-ref", "   "]);
        let error = cli
            .parse_mode()
            .expect_err("diff-review should reject empty github refs");
        assert!(error.to_string().contains("--github-ref must not be empty"));
    }

    #[test]
    fn rejects_github_ref_without_comment_posting() {
        let cli = Cli::parse_from(["why", "diff-review", "--github-ref", "#42"]);
        let error = cli
            .parse_mode()
            .expect_err("diff-review should require comment posting when github ref is supplied");
        assert!(
            error
                .to_string()
                .contains("--github-ref requires --post-github-comment")
        );
    }

    #[test]
    fn rejects_invalid_github_ref_for_diff_review() {
        let cli = Cli::parse_from([
            "why",
            "diff-review",
            "--post-github-comment",
            "--github-ref",
            "42",
        ]);
        let error = cli
            .parse_mode()
            .expect_err("diff-review should reject invalid github refs");
        assert!(
            error
                .to_string()
                .contains("--github-ref must use #123 syntax")
        );
    }

    #[test]
    fn rejects_empty_github_ref_after_trim_for_diff_review() {
        let cli = Cli::parse_from([
            "why",
            "diff-review",
            "--post-github-comment",
            "--github-ref",
            "   ",
        ]);
        let error = cli
            .parse_mode()
            .expect_err("diff-review should reject blank github refs");
        assert!(error.to_string().contains("--github-ref must not be empty"));
    }

    #[test]
    fn parses_coverage_gap_subcommand() {
        let cli = Cli::parse_from([
            "why",
            "coverage-gap",
            "--coverage",
            "lcov.info",
            "--limit",
            "7",
            "--max-coverage",
            "15",
            "--json",
        ]);
        assert_eq!(
            cli.parse_mode().expect("coverage-gap should parse"),
            Mode::CoverageGap {
                coverage: "lcov.info".into(),
                limit: 7,
                max_coverage: 15.0,
                json: true,
            }
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
    fn parses_onboard_subcommand() {
        let cli = Cli::parse_from(["why", "onboard", "--limit", "7", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("onboard should parse"),
            Mode::Onboard {
                limit: 7,
                json: true,
            }
        );
    }

    #[test]
    fn parses_time_bombs_subcommand() {
        let cli = Cli::parse_from(["why", "time-bombs", "--age-days", "365", "--json"]);
        assert_eq!(
            cli.parse_mode().expect("time-bombs should parse"),
            Mode::TimeBombs {
                age_days: 365,
                json: true,
            }
        );
    }

    #[test]
    fn parses_time_bombs_default_age() {
        let cli = Cli::parse_from(["why", "time-bombs"]);
        assert_eq!(
            cli.parse_mode().expect("time-bombs should parse"),
            Mode::TimeBombs {
                age_days: 180,
                json: false,
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
        assert_eq!(
            cli.parse_mode().expect("manpage should parse"),
            Mode::Manpage
        );
    }

    #[test]
    fn parses_no_cache_request() {
        let cli = Cli::parse_from(["why", "src/lib.rs:42", "--no-cache"]);
        let mode = cli.parse_mode().expect("no-cache target should parse");
        assert!(matches!(mode, Mode::Query(_)), "expected query mode");
        let Mode::Query(request) = mode else {
            return;
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
    fn rejects_zero_limit_for_onboard() {
        let cli = Cli::parse_from(["why", "onboard", "--limit", "0"]);
        let error = cli
            .parse_mode()
            .expect_err("onboard should reject a zero limit");
        assert!(
            error
                .to_string()
                .contains("--limit must be greater than zero")
        );
    }

    #[test]
    fn rejects_invalid_max_coverage_for_coverage_gap() {
        let cli = Cli::parse_from([
            "why",
            "coverage-gap",
            "--coverage",
            "lcov.info",
            "--max-coverage",
            "101",
        ]);
        let error = cli
            .parse_mode()
            .expect_err("coverage-gap should reject percentages above 100");
        assert!(
            error
                .to_string()
                .contains("--max-coverage must be between 0 and 100")
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
    fn rejects_query_flags_for_coverage_gap() {
        let error = Cli::try_parse_from([
            "why",
            "coverage-gap",
            "--coverage",
            "lcov.info",
            "--no-cache",
        ])
        .expect_err("clap should reject query flags for coverage-gap");
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
    fn rejects_query_flags_for_explain_outage() {
        let error = Cli::try_parse_from([
            "why",
            "explain-outage",
            "--from",
            "2025-11-03T14:00",
            "--to",
            "2025-11-03T16:30",
            "--no-cache",
        ])
        .expect_err("clap should reject query flags for explain-outage");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }

    #[test]
    fn rejects_zero_limit_for_explain_outage() {
        let cli = Cli::parse_from([
            "why",
            "explain-outage",
            "--from",
            "2025-11-03T14:00",
            "--to",
            "2025-11-03T16:30",
            "--limit",
            "0",
        ]);
        let error = cli
            .parse_mode()
            .expect_err("explain-outage should reject a zero limit");
        assert!(
            error
                .to_string()
                .contains("--limit must be greater than zero")
        );
    }

    #[test]
    fn rejects_empty_from_for_explain_outage() {
        let cli = Cli::parse_from([
            "why",
            "explain-outage",
            "--from",
            "   ",
            "--to",
            "2025-11-03T16:30",
        ]);
        let error = cli
            .parse_mode()
            .expect_err("explain-outage should reject blank from values");
        assert!(error.to_string().contains("--from must not be empty"));
    }

    #[test]
    fn rejects_empty_to_for_explain_outage() {
        let cli = Cli::parse_from([
            "why",
            "explain-outage",
            "--from",
            "2025-11-03T14:00",
            "--to",
            "   ",
        ]);
        let error = cli
            .parse_mode()
            .expect_err("explain-outage should reject blank to values");
        assert!(error.to_string().contains("--to must not be empty"));
    }

    #[test]
    fn rejects_query_flags_for_onboard() {
        let error = Cli::try_parse_from(["why", "onboard", "--no-cache"])
            .expect_err("clap should reject query flags for onboard");
        assert!(
            error
                .to_string()
                .contains("unexpected argument '--no-cache' found")
        );
    }

    #[test]
    fn rejects_query_flags_for_time_bombs() {
        let error = Cli::try_parse_from(["why", "time-bombs", "--no-cache"])
            .expect_err("clap should reject query flags for time-bombs");
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
    fn rejects_require_baseline_without_baseline_file() {
        let error = Cli::parse_from(["why", "health", "--require-baseline"])
            .parse_mode()
            .expect_err("health should reject orphaned require-baseline");
        assert!(
            error
                .to_string()
                .contains("--require-baseline requires --baseline-file")
        );
    }

    #[test]
    fn rejects_invalid_health_signal_budget() {
        let error = Cli::parse_from([
            "why",
            "health",
            "--baseline-file",
            "baseline.json",
            "--max-signal-regression",
            "time_bombs",
        ])
        .parse_mode()
        .expect_err("health should reject invalid signal budgets");
        assert!(error.to_string().contains("SIGNAL=COUNT"));
    }

    #[test]
    fn rejects_health_regression_budget_without_baseline_file() {
        let error = Cli::parse_from(["why", "health", "--max-regression", "1"])
            .parse_mode()
            .expect_err("health should reject orphaned regression budgets");
        assert!(
            error
                .to_string()
                .contains("health regression budgets require --baseline-file")
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
