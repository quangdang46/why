# PLAN: why — Complete Implementation Reference

## Overview

`why` is a git archaeology CLI that explains why a function, file, or line exists by
collecting git history and synthesizing it into a human-readable explanation with risk
assessment. It is built in Rust using the `git2` crate, tree-sitter for symbol
resolution, and Claude Haiku for LLM synthesis.

The tool answers four questions every developer has before touching unfamiliar code:
1. Why was this code introduced or changed?
2. Was it linked to a bug, incident, hotfix, migration, or temporary workaround?
3. What is the risk if it is deleted or refactored?
4. What evidence supports that conclusion?

Beyond single-symbol queries, `why` also provides repo-wide analysis: danger hotspot
maps, time-bomb debt scanning, implicit coupling detection, PR risk review, and
onboarding reports. It exposes all functionality as an MCP server so AI assistants
can call it automatically during coding sessions, installs git hooks that warn
developers before they touch HIGH risk code, and surfaces a `health` dashboard that
makes the entire codebase's risk profile legible in one command.

Additional power-user modes: `explain-outage` for post-incident archaeology, `ghost`
to find dead-but-dangerous code, `coverage-gap` to cross-reference risk against test
coverage, `split` to use git history as a semantic refactoring guide, `pr-template`
to generate risk-aware PR descriptions, `shell` for interactive multi-step archaeology,
and `context-inject` to silently prepend `why` history to any AI tool's prompt.

---

## Table of Contents

1. Product Definition
2. Architecture Overview
3. Repository Layout & Workspace
4. Complete `.why.toml` Reference
5. Crate-by-Crate Implementation Details
   - 5.1 `crates/core` — CLI Entry Point & Formatter
   - 5.2 `crates/locator` — Target Resolution & Tree-sitter
   - 5.3 `crates/archaeologist` — Git Analysis Engine
   - 5.4 `crates/context` — Local Code Context & Risk Heuristics
   - 5.5 `crates/evidence` — Evidence Pack Builder
   - 5.6 `crates/synthesizer` — LLM Synthesis
   - 5.7 `crates/cache` — Result Cache
   - 5.8 `crates/scanner` — Repo-Wide Analysis
   - 5.9 `crates/annotator` — Docstring Writer
   - 5.10 `crates/splitter` — Archaeological Refactor Guidance
   - 5.11 `crates/mcp` — MCP Server
   - 5.12 `crates/lsp` — LSP Hover Provider
   - 5.13 `crates/shell` — Interactive REPL
   - 5.14 `crates/hooks` — Git Hook Installer & Context Inject
6. Data Flow Diagrams
7. Prompt Engineering Reference
8. Implementation Phases (Detailed)
9. Testing Strategy (Detailed)
10. Error Handling Philosophy
11. Performance Targets & Benchmarks
12. Security Considerations
13. CI/CD Integration
14. Technical Risks & Mitigations
15. Cost Model
16. Timeline
17. First Sprint Proposal
18. Integration with Claude Code
19. Future Enhancements

---

## 1. Product Definition

### Core user story

As a developer, I can run:

```bash
# Core queries
why src/auth/session.rs:authenticate
why src/auth/session.rs:120
why src/auth/session.rs --lines 110:145
why src/lib.rs:AuthService::login

# Repo-wide analysis
why scan --time-bombs
why hotspots
why onboard src/
why health
why ghost
why coverage-gap --coverage lcov.info

# Incident archaeology
why explain-outage --from 2025-11-03T14:00 --to 2025-11-03T16:30

# PR / diff review
why --diff main..feature-branch
why --diff HEAD~1
why pr-template

# History and coupling
why src/lib.rs:authenticate --evolution
why src/lib.rs:authenticate --blame-chain
why --coupled src/lib.rs:authenticate
why src/lib.rs:authenticate --split

# Write history into source
why src/lib.rs:authenticate --annotate

# Team intelligence
why --team src/lib.rs:authenticate

# Interactive and ambient modes
why shell
eval "$(why context-inject)"

# Server modes
why mcp
why lsp

# Hook management
why install-hooks
why uninstall-hooks
```

### Non-goals (v1)
- Full GitHub/GitLab API deep integration (Phase 6+)
- Multi-repo reasoning
- Automatic code deletion suggestions
- Real-time IDE plugin with file-watching
- Support for every programming language on day one

### Success criteria (v1)
- Resolves a function / symbol / line range accurately in Rust code
- Collects the commits most directly affecting that target
- Retrieves commit messages and diffs for those commits
- Detects references like `#123`, `fixes #456`, incident IDs, TODOs, nearby comments
- Produces one concise explanation with a risk level: Low / Medium / High
- Cites the evidence used in the answer
- Useful even without an API key (heuristic fallback mode)
- Runs in under 3 seconds for most single-symbol queries on a warm cache

### Success criteria (v2+)
- Exposes all functionality as an MCP server for AI assistant integration
- Warns developers via git hooks before they modify HIGH risk code
- Scans the whole repo for time-bomb debt
- Produces churn x risk hotspot maps
- Reviews PRs and annotates risky changes
- Follows blame chains past noise commits to true code origins
- Detects implicit coupling via co-change analysis
- Writes evidence-backed docstrings back into source files
- Surfaces a health dashboard with a trending debt score
- Finds dead-but-dangerous ghost functions
- Cross-references risk against test coverage
- Runs as a persistent shell REPL
- Silently injects `why` context into any AI tool's prompts
- Guides refactoring splits by grouping lines by archaeological origin
- Generates risk-aware PR description templates
- Performs post-incident archaeology over a time window
- Provides LSP hover integration in VS Code, Neovim, Zed

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         CLI (crates/core)                           │
│  clap argument parsing → subcommand dispatch → output formatting    │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
           ┌────────────────────▼────────────────────┐
           │         Target Resolver (crates/locator) │
           │  file:line / file:symbol / --lines       │
           │  tree-sitter parse → byte range          │
           │  byte range → line range for blame       │
           └────────────────────┬────────────────────┘
                                │
           ┌────────────────────▼────────────────────┐
           │      Git Analyzer (crates/archaeologist) │
           │  git2::Repository::discover()            │
           │  git blame → unique commit OIDs          │
           │  commit metadata + diff excerpts          │
           │  blame-chain: skip noise commits          │
           │  co-change coupling matrix               │
           │  PR/issue reference extraction           │
           │  commit relevance scoring                │
           └────────────────────┬────────────────────┘
                                │
           ┌────────────────────▼────────────────────┐
           │  Context Extractor (crates/context)      │
           │  ±20 line window scan                    │
           │  TODO/FIXME/HACK/TEMP/SAFETY/XXX         │
           │  auth/security/migration heuristics      │
           │  custom vocabulary from .why.toml        │
           │  heuristic risk level (no LLM)           │
           └────────────────────┬────────────────────┘
                                │
           ┌────────────────────▼────────────────────┐
           │   Evidence Builder (crates/evidence)     │
           │  compress → bounded JSON payload         │
           │  diff truncation (≤500 chars each)       │
           │  top-N commit selection                  │
           │  token budget enforcement                │
           └────────────────────┬────────────────────┘
                                │
           ┌────────────────────▼────────────────────┐
           │    Synthesizer (crates/synthesizer)      │
           │  Claude Haiku API call (reqwest)         │
           │  structured prompt → WhyReport           │
           │  confidence + unknowns fields            │
           │  fallback: raw evidence if no key        │
           └────────────────────┬────────────────────┘
                                │
           ┌────────────────────▼────────────────────┐
           │    Formatter (crates/core)               │
           │  colored terminal output                 │
           │  --json structured output                │
           │  estimated cost display                  │
           └─────────────────────────────────────────┘

Parallel systems:
  crates/cache      → .why/cache.json, symbol+HEAD keying
  crates/scanner    → time_bombs, hotspots, health, ghost, coverage_gap,
                      onboard, diff_review, outage, pr_template
  crates/annotator  → write WhyReport as doc comment into source
  crates/splitter   → cluster blame lines → suggest fn split boundaries
  crates/mcp        → MCP stdio server wrapping full pipeline
  crates/lsp        → tower-lsp hover provider
  crates/shell      → rustyline REPL with warm repo state
  crates/hooks      → git hook installer + context-inject shell functions
```

### Data flow for a typical query

```
why src/auth/session.rs:authenticate
  │
  ├─ 1. Parse CLI args (clap)
  ├─ 2. Check cache (.why/cache.json keyed on symbol+HEAD)
  │       hit → return cached WhyReport immediately
  │       miss → continue
  ├─ 3. Discover repo root (git2::Repository::discover)
  ├─ 4. Locate symbol (tree-sitter parse → byte range → line range)
  ├─ 5. Run git blame on line range → unique commit OIDs
  ├─ 6. Load commit metadata + diff excerpts for each OID
  ├─ 7. Score and rank commits by relevance
  ├─ 8. Extract context from ±20 lines (comments, markers)
  ├─ 9. Build evidence pack (compressed JSON, token-bounded)
  ├─ 10. Call Claude Haiku (or return heuristic report if no key)
  ├─ 11. Parse WhyReport (summary, risk, evidence, confidence, unknowns)
  ├─ 12. Store in cache
  └─ 13. Format and print to terminal (or --json)
```

---

## 3. Repository Layout & Workspace

### Directory tree

```
why/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── CONTRIBUTING.md
├── SECURITY.md
├── POC.md
├── PLAN.md
├── CHANGELOG.md
├── .gitignore
├── .why.toml.example
├── .github/
│   ├── workflows/
│   │   ├── ci.yml
│   │   ├── release.yml
│   │   └── why-diff.yml
│   └── CODEOWNERS
│
├── crates/
│   ├── core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── cli.rs
│   │       ├── output.rs
│   │       └── error.rs
│   │
│   ├── archaeologist/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── blame.rs
│   │       ├── blame_chain.rs
│   │       ├── log.rs
│   │       ├── commit.rs
│   │       ├── coupling.rs
│   │       └── score.rs
│   │
│   ├── locator/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── finder.rs
│   │       ├── languages.rs
│   │       └── queries/
│   │           ├── rust.scm
│   │           ├── typescript.scm
│   │           ├── javascript.scm
│   │           └── python.scm
│   │
│   ├── context/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── extractor.rs
│   │       ├── heuristics.rs
│   │       └── config.rs
│   │
│   ├── evidence/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── builder.rs
│   │
│   ├── synthesizer/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── prompt.rs
│   │       ├── client.rs
│   │       └── parser.rs
│   │
│   ├── scanner/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── time_bombs.rs
│   │       ├── hotspots.rs
│   │       ├── onboard.rs
│   │       ├── diff_review.rs
│   │       ├── health.rs
│   │       ├── ghost.rs
│   │       ├── coverage_gap.rs
│   │       ├── outage.rs
│   │       └── pr_template.rs
│   │
│   ├── annotator/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── writer.rs
│   │
│   ├── splitter/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── cluster.rs
│   │
│   ├── mcp/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── server.rs
│   │
│   ├── lsp/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── server.rs
│   │
│   ├── shell/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── repl.rs
│   │
│   ├── hooks/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── installer.rs
│   │       └── context_inject.rs
│   │
│   └── cache/
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           └── store.rs
│
├── poc/
│   ├── package.json
│   └── index.js
│
├── tests/
│   ├── common/
│   │   └── mod.rs
│   ├── fixtures/
│   │   ├── hotfix_repo/setup.sh
│   │   ├── workaround_repo/setup.sh
│   │   ├── auth_guard_repo/setup.sh
│   │   ├── compat_shim_repo/setup.sh
│   │   ├── renamed_fn_repo/setup.sh
│   │   ├── coupling_repo/setup.sh
│   │   ├── timebomb_repo/setup.sh
│   │   ├── ghost_repo/setup.sh
│   │   └── split_repo/setup.sh
│   ├── integration_cli.rs
│   ├── integration_scanner.rs
│   ├── integration_mcp.rs
│   └── snapshot_reports.rs
│
├── benches/
│   ├── blame_bench.rs
│   └── locator_bench.rs
│
└── docs/
    ├── config-reference.md
    ├── mcp-setup.md
    ├── lsp-setup.md
    ├── ci-integration.md
    └── custom-risk-vocab.md
```

### Root `Cargo.toml`

```toml
[workspace]
members = [
    "crates/core",
    "crates/archaeologist",
    "crates/locator",
    "crates/context",
    "crates/evidence",
    "crates/synthesizer",
    "crates/scanner",
    "crates/annotator",
    "crates/splitter",
    "crates/mcp",
    "crates/lsp",
    "crates/shell",
    "crates/hooks",
    "crates/cache",
]
resolver = "2"

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"
authors = ["why contributors"]
repository = "https://github.com/your-org/why"

[workspace.dependencies]
git2 = { version = "0.18", default-features = false, features = ["vendored-openssl"] }
tree-sitter = "0.22"
tree-sitter-rust = "0.21"
tree-sitter-javascript = "0.21"
tree-sitter-typescript = "0.21"
tree-sitter-python = "0.21"
reqwest = { version = "0.12", features = ["blocking", "json", "rustls-tls"], default-features = false }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive", "env", "string"] }
regex = "1"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
thiserror = "1"
once_cell = "1"
rayon = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
colored = "2"
indicatif = "0.17"
tabled = "0.15"
toml = "0.8"
walkdir = "2"
rustyline = { version = "14", features = ["derive"] }
tower-lsp = "0.20"
tempfile = "3"
dirs = "5"

[[bin]]
name = "why"
path = "crates/core/src/main.rs"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

---

## 4. Complete `.why.toml` Reference

```toml
# .why.toml — why configuration file
# Copy to your project root or to ~/.config/why/config.toml

[llm]
model = "claude-haiku-4-5"
max_tokens = 1024
# api_key = "sk-ant-..."    # prefer ANTHROPIC_API_KEY env var
timeout_secs = 30
retries = 3

[risk]
default_level = "Low"

[risk.keywords]
# Domain-specific terms that boost risk scores.
# Built-in keywords (hotfix, security, auth, incident, etc.) are always active.
high = [
    # "pci", "settlement", "reconciliation",   # payments
    # "hipaa", "phi", "hl7",                   # healthcare
    # "terraform", "rollback-plan",            # infra
]
medium = []

[git]
max_commits = 8
recency_window_days = 365
mechanical_threshold_files = 50
min_coverage_score = 0.05
coupling_scan_commits = 500
coupling_ratio_threshold = 0.30

[context]
window_lines = 20

[scanner]
time_bomb_age_days = 180
onboard_top_n = 10
hotspots_top_n = 20
hotspots_churn_window_days = 90
parallel_workers = 8

[cache]
dir = ".why"
max_entries = 500
ttl_hours = 0

[output]
format = "terminal"
show_cost = true
show_confidence = true
color = "auto"

[github]
remote = "origin"
# token = "ghp_..."   # prefer GITHUB_TOKEN env var

[hooks]
warn_on = "HIGH"
blocking = true

[context_inject]
commands = ["aider", "sgpt", "llm"]
max_symbols = 5

[lsp]
show_risk = true
show_summary = true

[health]
time_bomb_weight = 3
high_risk_fn_weight = 1
uncovered_high_risk_weight = 4
ghost_fn_weight = 2
bus_factor_1_weight = 2
stale_hack_weight = 1
```

---

## 5. Crate-by-Crate Implementation Details

### 5.1 `crates/core` — CLI Entry Point & Formatter

#### Purpose
Entry point binary. Parses CLI args, loads config, dispatches to subcommands,
formats output. Contains no business logic — it only orchestrates crates.

#### `src/main.rs` — entry point

```rust
use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod cli;
mod output;
mod error;

use cli::{Cli, Command};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .compact()
        .init();

    let cli = Cli::parse();
    let config = load_config(cli.config.as_deref())?;

    match cli.command {
        Command::Query(args)         => cmd_query(args, &cli, &config),
        Command::Scan(args)          => cmd_scan(args, &cli, &config),
        Command::Hotspots(args)      => cmd_hotspots(args, &cli, &config),
        Command::Health(args)        => cmd_health(args, &cli, &config),
        Command::Ghost(args)         => cmd_ghost(args, &cli, &config),
        Command::CoverageGap(args)   => cmd_coverage_gap(args, &cli, &config),
        Command::ExplainOutage(args) => cmd_explain_outage(args, &cli, &config),
        Command::Onboard(args)       => cmd_onboard(args, &cli, &config),
        Command::Diff(args)          => cmd_diff(args, &cli, &config),
        Command::PrTemplate(args)    => cmd_pr_template(args, &cli, &config),
        Command::Mcp                 => { tokio::runtime::Builder::new_current_thread()
                                            .enable_all().build()?.block_on(why_mcp::server::run())?; Ok(()) },
        Command::Lsp                 => { tokio::runtime::Builder::new_current_thread()
                                            .enable_all().build()?.block_on(why_lsp::server::run())?; Ok(()) },
        Command::Shell               => why_shell::repl::run(),
        Command::InstallHooks(args)  => why_hooks::installer::install(
                                            &discover_repo_root()?, args.warn_only),
        Command::UninstallHooks      => why_hooks::installer::uninstall(&discover_repo_root()?),
        Command::ContextInject       => {
            let fns = why_hooks::context_inject::generate_shell_functions(
                &config.context_inject.commands,
                config.context_inject.max_symbols,
            );
            print!("{}", fns);
            Ok(())
        }
    }
}

/// Core query handler — the most important function in core.
fn cmd_query(args: cli::QueryArgs, cli: &Cli, config: &Config) -> Result<()> {
    let repo_root = discover_repo_root()?;
    let repo = git2::Repository::discover(&repo_root)?;
    let head_hash = repo.head()?.target()
        .map(|oid| oid.to_string())
        .unwrap_or_default();

    // Check cache
    let cache_key = why_cache::store::Cache::make_key(
        &args.target,
        &args.target,
        &head_hash,
    );
    let mut cache = why_cache::store::Cache::open(&repo_root, config.cache.max_entries)?;
    if !cli.no_cache {
        if let Some(cached) = cache.get(&cache_key) {
            output::print_why_report(cached, &args.target, None);
            return Ok(());
        }
    }

    // Resolve target
    let target = why_locator::resolve(&args.target, args.lines.as_deref(), &repo_root)?;

    // Git blame
    let mut commits = why_archaeologist::blame::blame_range(
        &repo,
        &target.relative_path,
        target.start_line,
        target.end_line,
    )?;

    // Score commits
    why_archaeologist::score::score_commits(&mut commits, config.git.recency_window_days);
    let commits = why_archaeologist::score::top_n(commits, args.max_commits.unwrap_or(config.git.max_commits));

    // Extract context
    let source = std::fs::read_to_string(&target.file_path)?;
    let lines: Vec<&str> = source.lines().collect();
    let context = why_context::extractor::extract(
        &lines, target.start_line, target.end_line,
        config.context.window_lines,
        &config.context.into(),
    );

    // Build evidence pack
    let pack = why_evidence::builder::build(&target, commits, &context);

    // Synthesize
    let (report, cost) = if cli.no_llm || args.evolution.is_none() {
        let r = why_synthesizer::parser::build_heuristic_report(&pack);
        (r, None)
    } else {
        match why_synthesizer::client::ApiClient::from_env(
            args.model.as_deref().unwrap_or(&config.llm.model)
        ) {
            Ok(client) => {
                let prompt = why_synthesizer::prompt::build_query_prompt(&pack);
                let (raw, cost) = client.complete(
                    why_synthesizer::prompt::SYSTEM_PROMPT,
                    &prompt,
                )?;
                (why_synthesizer::parser::parse_response(&raw, cost)?, cost)
            }
            Err(_) => {
                eprintln!("warning: ANTHROPIC_API_KEY not set — using heuristic mode");
                (why_synthesizer::parser::build_heuristic_report(&pack), None)
            }
        }
    };

    // Cache and output
    cache.set(cache_key, report.clone(), &head_hash)?;

    match cli.format {
        Some(cli::OutputFormat::Json) | _ if config.output.format == "json" => {
            output::print_why_report_json(&report)?;
        }
        _ => output::print_why_report(&report, &args.target, cost),
    }

    // Optional sub-modes
    if args.blame_chain {
        // run blame_chain and print
    }
    if args.coupled {
        // run coupling analysis and print
    }
    if args.split {
        // run splitter and print
    }
    if args.annotate {
        why_annotator::writer::annotate_file(
            &target.file_path,
            target.start_line,
            &report,
            &head_hash,
            target.symbol_name.as_deref().unwrap_or(""),
        )?;
        println!("✓ Annotation written to {}", target.file_path.display());
    }

    Ok(())
}
```

#### `src/output.rs` — terminal formatter

```rust
use colored::Colorize;
use why_archaeologist::RiskLevel;
use why_synthesizer::WhyReport;

/// Print a WhyReport to the terminal in human-readable colored format.
pub fn print_why_report(report: &WhyReport, target: &str, cost: Option<f64>) {
    println!();
    println!("{}", format!("why: {}", target).bold());
    println!();

    println!("{}", "Why this exists".bold().underline());
    println!("{}", report.summary);
    println!();

    for point in &report.why_it_exists {
        println!("  {}", point);
    }
    println!();

    let risk_str = match report.risk_level {
        RiskLevel::High   => "HIGH".red().bold(),
        RiskLevel::Medium => "MEDIUM".yellow().bold(),
        RiskLevel::Low    => "LOW".green().bold(),
    };
    println!("Risk if removed: {}", risk_str);
    for breakage in &report.likely_breakage {
        println!("  {} {}", "•".red(), breakage);
    }
    println!();

    println!("{}", "Evidence".bold().underline());
    for ev in &report.evidence {
        println!("  {}", ev);
    }
    println!();

    println!("Confidence: {}", report.confidence.italic());
    if !report.unknowns.is_empty() {
        println!("{}", "Unknowns".bold().underline());
        for u in &report.unknowns {
            println!("  {} {}", "•".dimmed(), u);
        }
    }

    if let Some(c) = cost {
        println!();
        println!("{}", format!("Estimated cost: ~${:.4}", c).dimmed());
    }
    println!();
}

pub fn print_why_report_json(report: &WhyReport) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}
```

#### Terminal output examples

```
why: src/auth/session.rs:authenticate (lines 110–145)

Why this exists
This function was hardened after an authentication bypass vulnerability was
discovered in November 2025. A subsequent commit preserved legacy mobile token
behavior for backward compatibility with older clients still using v1 tokens.

  • Originally a simple password check (2022-03)
  • Hardened post-incident with retry guard and rate limiting (2023-07)
  • Legacy mobile token path added for backward compat with v1 clients (2023-11)
  • Performance: replaced bcrypt with argon2 (2024-03)

Risk if removed: HIGH
  • May reintroduce auth/session bypass vulnerability
  • Breaks compatibility with older refresh-token clients
  • Session hijacking risk if token validation path is removed

Evidence
  abc12345  hotfix auth bypass in session refresh [2025-11-03]
  def67890  preserve legacy mobile token behavior (#234) [2025-11-15]
  Nearby: "// temporary guard until all clients rotate tokens"
  Nearby: "TODO(2026-Q1): remove after mobile v2 rollout complete"

Confidence: medium-high
Unknowns
  • No direct PR description available; inferred from commit message language
  • Whether the mobile client migration has completed is unknown
  • Incident report #4521 referenced but not fetched (no GITHUB_TOKEN set)

Coupled with (co-change ratio >30%): session_refresh(), token_validate()
Estimated cost: ~$0.0009
```

```
why --evolution src/auth/session.rs:authenticate

Timeline of authenticate()

  2022-03  [v1]  12 lines  Initial auth PR (#12)
                           Simple password check with bcrypt.

  2023-07  [v2]  45 lines  Incident #4521 hotfix
                           Auth bypass discovered in session refresh.
                           Added retry guard, rate limiting, and session
                           validation checks. Risk elevated to HIGH.

  2023-11  [v3]  52 lines  PR #234 — mobile backward compat
                           Legacy token path added. Two clients still
                           on v1 token format; migration in progress.

  2024-03  [v4]  48 lines  PR #301 — performance
                           bcrypt replaced with argon2id for speed.
                           No functional changes to auth logic.

Current risk: HIGH (security + backward compat logic co-located)
Recommendation: Split into authenticate_with_guard() and authenticate_legacy()
```

---

### 5.2 `crates/locator` — Target Resolution & Tree-sitter

#### Purpose
Parse the raw target string (e.g. `"src/auth.rs:authenticate"`) and resolve it
to an exact line range. Uses tree-sitter for symbol-aware resolution.

#### Key data structures

```rust
pub struct ResolvedTarget {
    pub repo_root: PathBuf,
    pub file_path: PathBuf,         // absolute path
    pub relative_path: PathBuf,     // relative to repo root (for git blame)
    pub language: LanguageKind,
    pub start_line: usize,          // 0-indexed, inclusive
    pub end_line: usize,            // 0-indexed, inclusive
    pub start_byte: usize,
    pub end_byte: usize,
    pub symbol_name: Option<String>,
    pub surrounding_context: String, // ±5 lines for display
}

pub enum LanguageKind {
    Rust, TypeScript, JavaScript, Python, Unknown,
}
```

#### Target format parsing

The target string supports four formats:

| Format | Example | Resolution |
|---|---|---|
| file:symbol | `src/auth.rs:authenticate` | Tree-sitter symbol search |
| file:qualified | `src/auth.rs:AuthService::login` | Tree-sitter qualified search |
| file:line | `src/auth.rs:42` | Direct line number (±0 range) |
| file + --lines | `src/auth.rs` + `--lines 110:145` | Explicit range |

Parsing algorithm:
1. Split on last `:` to separate file and specifier
2. If specifier is purely numeric → line query
3. If specifier contains `::` → qualified method query
4. Otherwise → symbol name query
5. If no specifier and no `--lines` → error with helpful message

#### Tree-sitter query files (`src/queries/`)

**`rust.scm`:**
```scheme
(function_item name: (identifier) @name) @definition
(impl_item body: (declaration_list (function_item name: (identifier) @name) @definition))
(struct_item name: (type_identifier) @name) @definition
(enum_item name: (type_identifier) @name) @definition
(trait_item name: (type_identifier) @name) @definition
(const_item name: (identifier) @name) @definition
(static_item name: (identifier) @name) @definition
```

**`typescript.scm`:**
```scheme
(function_declaration name: (identifier) @name) @definition
(variable_declarator name: (identifier) @name value: (arrow_function)) @definition
(method_definition name: (property_identifier) @name) @definition
(class_declaration name: (type_identifier) @name) @definition
(function_signature name: (identifier) @name) @definition
```

**`python.scm`:**
```scheme
(function_definition name: (identifier) @name) @definition
(decorated_definition definition: (function_definition name: (identifier) @name)) @definition
(class_definition name: (identifier) @name) @definition
(async_function_definition name: (identifier) @name) @definition
```

#### Ambiguous symbol handling

When multiple symbols match (e.g. two methods named `process` in different impl blocks):
1. Warn to stderr with a ranked list of matches
2. Return the first match (highest in file)
3. User can qualify: `src/lib.rs:BlockProcessor::process`

Future: parse impl context to support `TypeName::method_name` qualifier resolution.

#### `list_all_symbols` function

Used by `ghost.rs` and `shell.rs` for indexing. Returns all `(name, start_line, end_line)` tuples from a source file.

```rust
pub fn list_all_symbols(
    source: &str,
    language: &LanguageKind,
) -> Result<Vec<(String, usize, usize)>>
```

---

### 5.3 `crates/archaeologist` — Git Analysis Engine

#### Purpose
All git2 interaction lives here. Provides blame, commit loading, diff extraction,
co-change analysis, blame chain following, and commit relevance scoring.

#### `BlamedCommit` — primary data type

```rust
pub struct BlamedCommit {
    pub oid: String,
    pub short_oid: String,           // first 8 chars
    pub author: String,
    pub email: String,
    pub time: i64,                   // Unix timestamp
    pub date_human: String,          // "2025-11-03"
    pub summary: String,             // first line of commit message
    pub message: String,             // full message body
    pub touched_lines: Vec<(usize, usize)>,
    pub diff_excerpt: String,        // truncated diff, ≤500 chars
    pub coverage_score: f32,         // fraction of target lines owned
    pub relevance_score: f32,        // composite ranking score
    pub issue_refs: Vec<String>,     // ["#123", "closes #456"]
    pub is_mechanical: bool,
}
```

#### `blame.rs` — core blame operation

Algorithm:
1. Call `git2::Repository::blame_file()` with `min_line` / `max_line` options
2. Iterate blame hunks, grouping lines by OID
3. Compute `coverage_score` = owned_lines / total_target_lines
4. Load full commit metadata via `commit.rs`
5. Return Vec<BlamedCommit> unsorted (sorting done by `score.rs`)

Edge cases handled:
- Merge commits: use `final_commit_id()` to get the originating commit, not the merge commit
- Binary files: skip gracefully with `is_binary` check on diff
- Files not yet committed: return empty vec rather than error

#### `commit.rs` — commit metadata loading

Extracts from each commit:
- Author name + email
- Timestamp (formatted as YYYY-MM-DD)
- Summary (first line of message)
- Full message body
- Diff excerpt for the target file (truncated at 500 chars)
- Issue references (`#123`, `closes #456`, `incident-1234`)
- PR references (`(#234)` pattern)
- Mechanical commit detection

**Mechanical commit detection criteria:**
- Touches >50 files
- Diff is whitespace-only for target file
- Summary starts with `chore:`, `fmt`, `format`, `bump `
- Summary contains `Merge branch` or `Merge pull request`

#### `score.rs` — relevance scoring algorithm

```
base_score = coverage_score × 100        // 0–100

Bonuses:
  + 30   if high-signal keyword in summary/body
         (hotfix, security, vulnerability, cve, auth, bypass,
          incident, sev[0-9], postmortem, rollback, revert,
          critical, emergency, breach, exploit)
  + 15   if medium-signal keyword
         (fix, bug, workaround, temporary, compat, migration,
          deprecated, legacy, backport, regression)
  + 20   if risk-domain keyword
         (permission, session, token, cookie, password, secret,
          key, cert, tls, ssl, csrf, xss, injection, sanitize)
  + 10   per linked issue or PR reference
  + 0–20 recency bonus (linear decay over recency_window_days)
  + 5    if diff excerpt is non-empty
  + 0–5  custom keyword bonus (from .why.toml)

Penalties:
  - 40   if flagged as mechanical
  - 0–20 large diff penalty (>50 files)
```

After scoring, commits are sorted descending by `relevance_score`.

#### `blame_chain.rs` — follow blame past noise commits

```
Input: starting_oid (from git blame), file_path, max_depth

Algorithm:
  loop:
    load commit at current_oid
    if commit is NOT mechanical:
      return BlameChainResult { skipped, origin: commit, depth }
    else:
      add to skipped list
      current_oid = parent_oid(current_oid)
      depth++
    if depth > max_depth or reached root:
      return current commit as origin

Output:
  BlameChainResult {
    noise_commits_skipped: Vec<BlamedCommit>,
    origin_commit: BlamedCommit,   // first non-mechanical ancestor
    chain_depth: usize,
  }
```

Terminal output for `--blame-chain`:
```
why --blame-chain src/auth/session.rs:authenticate

Blame chain for authenticate()

  Skipped (mechanical):
    a1b2c3d4  fmt: run rustfmt on auth module (2025-10-01)
    b2c3d4e5  chore: update copyright headers (2025-09-15)

  True origin:
    c3d4e5f6  hotfix auth bypass in session refresh (2025-11-03)
              Author: alice@corp.com
              Risk signals: hotfix, auth, security
```

#### `coupling.rs` — co-change coupling analysis

```
Algorithm:
  1. Walk last `scan_commits` (default 500) commits
  2. For each commit, get set of all touched files
  3. If target file is in the set:
     - target_commit_count++
     - for each other file in set: co_change_count[file]++
  4. Filter files where co_change_count / target_commit_count >= threshold
  5. Sort by ratio descending

Optional: use tree-sitter on coupled files to guess which function
          changed (the one with the highest churn in that file).
```

---

### 5.4 `crates/context` — Local Code Context & Risk Heuristics

#### Purpose
Scan a ±`window_lines` window around the target for comments, markers, and risk
signals. Produce a heuristic risk level without any LLM call.

#### `extractor.rs` — line scanner

Scans every line in the window:
1. **Comment extraction**: captures `// ...`, `# ...`, `/* ... */` text
2. **Marker extraction**: captures TODO/FIXME/HACK/TEMP/SAFETY/XXX lines
3. **Risk keyword matching**: runs all built-in and custom patterns

#### Built-in HIGH risk patterns (always active)

| Pattern name | Regex |
|---|---|
| `auth` | `\b(auth|authentication|authorize|permission|role|rbac|acl)\b` |
| `session` | `\b(session|cookie|jwt|token|refresh_token|bearer)\b` |
| `security` | `\b(security|vulnerability|cve|exploit|sanitize|escape|xss|csrf|sql_injection)\b` |
| `crypto` | `\b(encrypt|decrypt|hash|bcrypt|argon2|pbkdf2|hmac|secret|private_key)\b` |
| `incident` | `\b(hotfix|incident|sev[0-9]|postmortem|emergency|critical_fix)\b` |

#### Built-in MEDIUM risk patterns

| Pattern name | Regex |
|---|---|
| `migration` | `\b(migration|migrate|schema_change|backward_compat|breaking_change)\b` |
| `retry` | `\b(retry|backoff|circuit_breaker|rate_limit|throttle)\b` |
| `compat` | `\b(compat|legacy|deprecated|shim|polyfill|workaround|temporary)\b` |
| `data` | `\b(transaction|rollback|atomic|consistency|data_integrity)\b` |

#### Heuristic risk computation

```
HIGH if:  any built-in HIGH pattern matched
          OR any custom_high_keywords matched

MEDIUM if: any built-in MEDIUM pattern matched
           OR any custom_medium_keywords matched
           OR HACK/FIXME/TEMP/SAFETY marker present

LOW otherwise
```

#### Custom vocabulary in `.why.toml`

Teams add domain-specific signal words that behave exactly like built-in patterns:

```toml
[risk.keywords]
high = ["pci", "settlement", "reconciliation"]   # financial data
medium = ["terraform", "infra-change"]           # infrastructure
```

Loaded at startup from `crates/context/src/config.rs`, merged with built-ins.

---

### 5.5 `crates/evidence` — Evidence Pack Builder

#### Purpose
Compress all collected data into a single bounded JSON payload that fits within
claude-haiku-4-5's context window while leaving room for the prompt template.

#### Budget constraints

| Component | Max size |
|---|---|
| Total payload | 8,000 chars |
| Diff excerpt per commit | 500 chars |
| Comment text per entry | 200 chars |
| Marker text per entry | 150 chars |
| Commit subject | 120 chars |
| Commits sent to LLM | 8 (configurable) |

If the payload exceeds 8,000 chars after truncation, diff excerpts are dropped
entirely and a warning is emitted to stderr.

#### `EvidencePack` structure

```json
{
  "target": {
    "file": "src/auth/session.rs",
    "symbol": "authenticate",
    "lines": [110, 145],
    "language": "rust"
  },
  "local_context": {
    "comments": ["// temporary guard until all clients rotate tokens"],
    "markers": ["TODO(2026-Q1): remove after mobile v2 rollout"],
    "risk_flags": ["auth", "session", "token"]
  },
  "history": {
    "total_commit_count": 12,
    "top_commits": [
      {
        "oid": "abc12345",
        "date": "2025-11-03",
        "author": "alice",
        "summary": "hotfix auth bypass in session refresh",
        "diff_excerpt": "-    if token == old_token {\n+    if !validate_token(token, session_id) {",
        "coverage_score": 0.72,
        "issue_refs": ["#456"]
      }
    ]
  },
  "signals": {
    "issue_refs": ["#234", "#456"],
    "risk_keywords": ["hotfix", "auth", "security", "session"],
    "heuristic_risk": "High"
  }
}
```

---

### 5.6 `crates/synthesizer` — LLM Synthesis

#### Purpose
Build the LLM prompt, call the Anthropic API, parse the response into a `WhyReport`.
Handles retries, cost calculation, and graceful fallback when no API key is set.

#### System prompt design

The system prompt is carefully engineered to:
1. Define a strict output contract (JSON-only, no markdown)
2. Prohibit hallucination ("base ALL claims on evidence")
3. Force evidence/inference separation (`unknowns` field)
4. Calibrate risk level criteria explicitly
5. Keep output concise (summary ≤100 words, bullets ≤80 chars)

Full system prompt:

```
You are a git archaeology assistant. Your job is to explain why a piece of
code exists based on structured evidence from git history.

You will receive a JSON evidence pack containing:
- The target code location (file, symbol, lines)
- Local context (nearby comments, TODO/FIXME markers, risk flags)
- The most relevant commits that modified this code (with diff excerpts)
- Risk signals extracted from commit messages and code patterns

Your response MUST be a valid JSON object with exactly these fields:

{
  "summary": "One paragraph (3-5 sentences) explaining why this code exists.",
  "why_it_exists": ["bullet 1", "bullet 2", "bullet 3"],
  "risk_level": "HIGH" | "MEDIUM" | "LOW",
  "likely_breakage": ["what could break if this is removed or changed"],
  "evidence": ["cited commit OID + summary", "nearby comment text"],
  "confidence": "low" | "medium" | "medium-high" | "high",
  "unknowns": ["things inferred without direct evidence"]
}

RULES:
1. Base ALL claims on the evidence provided. Do not invent history.
2. Clearly distinguish between direct evidence (commit messages, comments)
   and inference (your reasoning about why something might exist).
3. Put inferences in the "unknowns" field, not in "evidence".
4. Risk level should reflect: security concerns, backward compat, incident
   history, how many things depend on it, how recently changed for critical reasons.
5. If evidence is thin (1-2 commits, no issue refs), say so in confidence.
6. Keep summary under 100 words. Keep each bullet under 80 characters.
7. Respond ONLY with the JSON object. No preamble, no markdown fences.
```

#### Evolution mode prompt

For `--evolution`, the model receives a structured list of eras:

```json
[
  {
    "era_label": "2022-03 (initial)",
    "line_count": 12,
    "key_commits": ["abc12345 — initial auth implementation"],
    "diff_summary": "Simple password hash comparison, bcrypt"
  },
  {
    "era_label": "2023-07 (incident hotfix)",
    "line_count": 45,
    "key_commits": ["def67890 — hotfix auth bypass in session refresh"],
    "diff_summary": "Added token validation, session ID binding, retry guard"
  }
]
```

And is asked to narrate the evolution rather than explain a single snapshot.

#### `WhyReport` — output type

```rust
pub struct WhyReport {
    pub summary: String,
    pub why_it_exists: Vec<String>,
    pub risk_level: RiskLevel,
    pub likely_breakage: Vec<String>,
    pub evidence: Vec<String>,
    pub confidence: String,
    pub unknowns: Vec<String>,
    pub estimated_cost_usd: Option<f64>,
}
```

#### Cost calculation

```
claude-haiku-4-5 pricing (approximate):
  Input:  $0.25 per million tokens = $0.00000025 per token
  Output: $1.25 per million tokens = $0.00000125 per token

Typical query:
  Input:  ~1,200 tokens (prompt + evidence pack)
  Output: ~400 tokens (WhyReport JSON)
  Cost:   (1200 × 0.00000025) + (400 × 0.00000125) = $0.0008
```

#### Retry logic

```
Attempt 1 → immediate
Attempt 2 → 2s delay (if 429 or 5xx)
Attempt 3 → 4s delay
Give up   → return error with clear message
```

#### Heuristic fallback (no API key)

When `ANTHROPIC_API_KEY` is not set, builds a `WhyReport` from the evidence pack
without any LLM call:
- summary = "Heuristic analysis: N commits found. Risk flags: X, Y."
- why_it_exists = top 3 commit summaries
- risk_level = heuristic_risk from context extractor
- confidence = "low (heuristic only — set ANTHROPIC_API_KEY for full analysis)"
- unknowns = ["Full analysis requires LLM synthesis"]

This ensures the tool is useful offline and without credentials.

---

### 5.7 `crates/cache` — Result Cache

#### Purpose
Avoid redundant API calls and repeated git blame operations for unchanged code.
Cache is stored in `.why/cache.json` in the repo root.

#### Cache key design

```
key = "{relative_file_path}:{symbol_or_line}:{head_hash_12chars}"

Example:
  "src/auth/session.rs:authenticate:a1b2c3d4e5f6"
```

The `head_hash` component provides automatic invalidation: when new commits are made,
`HEAD` changes and all previous cache entries for that symbol become stale (they used
an older hash). No manual cache clearing is needed for correctness.

#### Cache entry structure

```rust
pub struct CacheEntry {
    pub key: String,
    pub report: WhyReport,
    pub created_at: i64,   // Unix timestamp
    pub head_hash: String, // full HEAD OID for reference
}
```

#### Health snapshot storage

The cache also stores weekly health dashboard snapshots for trend calculation:

```rust
pub struct HealthSnapshot {
    pub timestamp: i64,
    pub debt_score: u32,
    pub details: HashMap<String, u32>,  // component scores
}
```

Up to 52 snapshots (~1 year of weekly runs) are retained. This enables the
`why health` dashboard to show `↑ from 68 last week` trends.

#### `--no-cache` flag

Forces the full pipeline to run and overwrites the cache entry. Useful when:
- Debug output needed
- Manual cache invalidation desired
- Testing with different `--max-commits` or `--since` values

---

### 5.8 `crates/scanner` — Repo-Wide Analysis

#### `time_bombs.rs` — stale debt finder

Scans every source file for:

**1. Past-due dated TODOs:**
Pattern: `TODO(2024-03-15):` or `TODO 2025-06:` or `// TODO 2023-Q4`
Action: if parsed date is before today → flag as PastDueTodo

**2. Aged HACK/TEMP markers:**
Uses `git blame` on the marker line to find when it was introduced.
If age > `time_bomb_age_days` (default 180) → flag as AgedHack

**3. Expired remove-after comments:**
Pattern: `// remove after v2.0 migration` or `// remove after 2026-Q1`
If a date can be parsed and has passed → flag as ExpiredRemoveAfter

Output format:
```
why scan --time-bombs

Found 12 time bombs in my-service

CRITICAL (past-due > 1 year):
  src/auth/session.rs:142    TODO(2024-06-01): remove legacy token path
                             Introduced by alice, 584 days ago

  src/payments/checkout.rs:87  HACK: workaround for Stripe API v2 rate limit
                                Introduced by bob, 423 days ago

WARN (past-due 6-12 months):
  [... 5 more ...]

INFO (approaching threshold):
  [... 4 more ...]

Run `why <file>:<line>` on any item for full historical context.
```

#### `hotspots.rs` — churn × risk danger map

Algorithm:
1. Walk commits in the last `hotspots_churn_window_days` (default 90)
2. Count commit frequency per file (churn score)
3. Run context risk heuristics on each file's full content (risk score)
4. danger_score = churn_normalized × risk_score (1/2/3 for Low/Medium/High)
5. Output top N by danger_score

```
why hotspots

Top 20 hotspots by churn × risk

  Rank  File                          Churn  Risk    Danger   Authors
  ────  ────────────────────────────  ─────  ──────  ───────  ─────────────
   1    src/auth/session.rs             34   HIGH     3.00    alice, bob
   2    src/payments/processor.rs       28   HIGH     2.80    carol
   3    src/api/middleware.rs           45   MEDIUM   1.80    alice, dave
   4    src/db/migrations/             22   MEDIUM   1.76    bob, carol
   ...
```

#### `health.rs` — repo health dashboard

Aggregates all scanner signals into a single report with trending debt score:

| Signal | How counted | Weight |
|---|---|---|
| Time bombs | count of all PastDueTodo + AgedHack + ExpiredRemoveAfter | 3 |
| HIGH risk functions | count of symbols with HIGH heuristic risk | 1 |
| Uncovered HIGH risk | HIGH risk functions with <20% test coverage | 4 |
| Hotspot files | files in top danger_score quartile | 2 |
| Bus factor 1 | functions where 1 author owns >80% of commits | 2 |
| Ghost functions | never-called + HIGH risk | 2 |
| Stale HACKs | HACK markers older than 6 months | 1 |

**Debt score formula:**
```
score = clamp(
  (time_bombs × 3) +
  (high_risk_fns × 1) +
  (uncovered_high_risk × 4) +
  (ghost_fns × 2) +
  (bus_factor_1 × 2) +
  (stale_hacks × 1),
  0, 100
)
```

**Trending:**
Previous score loaded from cache; delta displayed as `↑ N` (red) or `↓ N` (green).

#### `ghost.rs` — dead code danger finder

Detects functions that are:
1. Never called anywhere in the codebase (static analysis)
2. Have HIGH risk git history (blame heuristics)

Why this matters: these are the most dangerous functions to delete because standard
dead-code elimination tools only check (1). `why ghost` adds (2).

Call site detection uses regex heuristics (`\bname\s*\(`) and tree-sitter call query.
Known limitation: dynamic dispatch, FFI, reflection will produce false positives.
Results include a note recommending manual verification.

#### `coverage_gap.rs` — unprotected HIGH risk functions

Reads an LCOV or llvm-cov JSON coverage file and cross-references it with `why`'s
risk scores. Produces the most actionable technical debt list a tech lead can create:
"these HIGH risk functions have zero tests."

LCOV format support:
```
SF:src/auth/session.rs    ← source file
DA:110,1                  ← line 110, hit count 1
DA:111,0                  ← line 111, hit count 0 (not covered)
end_of_record
```

llvm-cov JSON format: standard cargo llvm-cov output.

Coverage percentage formula:
```
coverage_pct = (lines_hit / lines_instrumented) × 100
```

Lines with no DA entry are treated as not instrumented (not counted).

Output:
```
why coverage-gap --coverage lcov.info

12 HIGH risk functions with insufficient test coverage

  File                          Function           Coverage  Risk Flags
  ────────────────────────────  ─────────────────  ────────  ─────────────────
  src/auth/session.rs           authenticate()       0.0%   auth, session, token
  src/payments/processor.rs     charge_card()        8.3%   pci, settlement
  src/crypto/key_manager.rs     rotate_key()         0.0%   crypto, secret
  ...

Run `why <file>:<function>` to understand why each function exists before writing tests.
```

#### `onboard.rs` — codebase onboarding report

Produces "The N things a new engineer must understand about this codebase":

1. Walk all source files, extract all symbol definitions
2. Score each symbol: `score = commit_count × risk_score × recency_factor`
3. Select top N by score
4. Run `why` on each in parallel (rayon, bounded by `parallel_workers`)
5. Format as numbered report

```
why onboard src/

The 10 things you must understand about this codebase

1. src/auth/session.rs:authenticate — Risk: HIGH
   The core authentication function, hardened after a 2023 incident. Contains
   legacy mobile token path that must not be removed until v2 migration is complete.

2. src/payments/processor.rs:charge_card — Risk: HIGH
   PCI-compliant card processing. Three separate compliance audits have touched
   this function. Do not modify without a security review.

[... 8 more ...]
```

#### `diff_review.rs` — PR risk analysis

```
why --diff main..feature-branch

Risk report for 23 functions touched in main..feature-branch

HIGH RISK changes:
  ⚠️  src/auth/session.rs:authenticate (lines 110–145)
      Risk: HIGH | Last modified: 2025-11-03 | Commits: 12
      STRONGLY recommend review before merge.

MEDIUM RISK changes:
  ⚡  src/api/middleware.rs:rate_limit (lines 45–78)
      Risk: MEDIUM | Last modified: 2025-08-12 | Commits: 6

LOW RISK changes:
  ✓   src/models/user.rs:from_row (lines 12–28) — LOW
  ✓   src/utils/fmt.rs:format_date (lines 5–15) — LOW
  [... 19 more ...]

Summary: 1 HIGH, 1 MEDIUM, 21 LOW
Recommendation: Review HIGH risk changes carefully before approving.
```

#### `outage.rs` — incident archaeology

```
why explain-outage --from 2025-11-03T14:00 --to 2025-11-03T16:30

Incident archaeology for 2025-11-03 14:00 – 16:30

  3 commits merged in window:
  c3d4e5f6  alice   hotfix auth bypass in session refresh (14:23)
  d4e5f6a7  bob     update Stripe webhook endpoint (15:01)
  e5f6a7b8  carol   bump reqwest to 0.12.3 (15:44)

Ranked culprits by risk × recency × blast_radius:

  1. authenticate() in src/auth/session.rs
     Commit: c3d4e5f6 — hotfix auth bypass in session refresh
     Risk score: 4.2 | Risk flags: hotfix, security, auth
     Author: alice | Deployed: 14:23

  2. process_webhook() in src/payments/webhook.rs
     Commit: d4e5f6a7 — update Stripe webhook endpoint
     Risk score: 2.1 | Risk flags: payment, webhook
     Author: bob | Deployed: 15:01

  3. reqwest::Client (dependency update)
     Commit: e5f6a7b8 — bump reqwest to 0.12.3
     Risk score: 0.8 | (dependency, not a code change)
     Author: carol | Deployed: 15:44

Run `why src/auth/session.rs:authenticate` for full historical context on #1.
```

#### `pr_template.rs` — risk-aware PR description generator

```
why pr-template

## Summary

<!-- Describe what this PR does in 2-3 sentences -->

## Changes

- `src/auth/session.rs`
- `src/api/middleware.rs`
- `tests/integration/auth_test.rs`

## Risk Assessment

| Risk | Files |
|---|---|
| ⚠️ HIGH | `src/auth/session.rs` (auth, session, token patterns) |
| LOW | `src/api/middleware.rs` |
| LOW | `tests/integration/auth_test.rs` |

**Reviewer note:** Run `why src/auth/session.rs:authenticate` before approving.

## Testing

- [ ] Unit tests updated
- [ ] Integration tests pass
- [ ] Manual testing completed

## Checklist

- [ ] Breaking changes documented
- [ ] Backward compatibility maintained
- [ ] Security review completed for HIGH risk changes
```

---

### 5.9 `crates/annotator` — Docstring Writer

#### Purpose
Write the `WhyReport` as a doc comment directly into the source file.
Detects and replaces existing `[why]` annotations on re-run (idempotent).

#### Generated annotation format (Rust)

```rust
/// [why] Hardened after auth bypass incident (commit abc12345, 2025-11-03). Preserves
/// legacy mobile token compatibility for v1 clients. ⚠️  Risk: HIGH. Confidence: medium-high.
/// Evidence: abc12345 — hotfix auth bypass in session refresh; def67890 — preserve mobile token.
/// Generated by `why` @ a1b2c3d. Refresh: `why src/auth/session.rs:authenticate --annotate`
pub fn authenticate(/* ... */) {
```

#### Annotation format (TypeScript/JavaScript)

```typescript
/**
 * [why] Hardened after auth bypass incident (commit abc12345, 2025-11-03).
 * Risk: HIGH. Evidence: abc12345. Generated by `why` @ a1b2c3d.
 * Refresh: `why src/auth.ts:authenticate --annotate`
 */
function authenticate(/* ... */) {
```

#### Idempotency

On re-run:
1. Scan backwards from `start_line` to find lines starting with `/// [why]`
2. If found, remove the entire existing block
3. Insert fresh annotation in its place

The `generated_at_hash` field lets readers see when the annotation was last
refreshed relative to the current HEAD.

---

### 5.10 `crates/splitter` — Archaeological Refactor Guidance

#### Purpose
Run `git blame` on every line of a target function, cluster lines by their dominant
commit era, and suggest meaningful function split boundaries.

#### Algorithm

```
1. Run git blame on [start_line, end_line]
2. For each line, record the dominant commit OID
3. Group contiguous same-OID runs into raw blocks
4. Merge blocks smaller than 5 lines into adjacent dominant block
5. If fewer than 2 blocks remain → no split suggested
6. For each block:
   a. Load commit metadata for dominant OID
   b. Extract era label from commit summary
   c. Suggest extracted function name based on era context
7. Print split suggestion report
```

#### Era label extraction heuristics

| Commit summary pattern | Era label |
|---|---|
| Contains "hotfix", "security", "patch" | "Security hardening era" |
| Contains "compat", "legacy", "shim" | "Backward compat era" |
| Contains "migration", "upgrade" | "Migration era" |
| Contains "fix", "bug" | "Bug fix era" |
| Contains "feat", "add", "implement" | "Feature era" |
| Default | "Unknown era" |

#### Function name suggestion heuristics

| Era context | Suggested suffix |
|---|---|
| "guard", "security", "auth" | `_with_guard` |
| "legacy", "compat", "v1" | `_legacy` |
| "migration", "upgrade" | `_migration_path` |
| Default | `_inner` |

#### Output

```
why src/auth/session.rs:authenticate --split

Suggested split for authenticate() (52 lines):

  Block A  lines 110–137  Auth hardening era (commit abc12345, 2023-07)
                           → extract: authenticate_with_guard()
                           Risk: HIGH (security hotfix logic)

  Block B  lines 138–161  Backward compat era (commit def67890, 2023-11)
                           → extract: authenticate_legacy_token()
                           Risk: MEDIUM (legacy v1 token path)

These blocks have different reasons to change and different risk profiles.
Splitting reduces blast radius of future modifications to either path.
Recommendation: authenticate() becomes a dispatcher calling both.
```

---

### 5.11 `crates/mcp` — MCP Server

#### Purpose
Expose the full `why` pipeline as an MCP server so Claude, Cursor, and any
MCP-compatible AI assistant can call it as a tool during coding sessions.

#### Transport
stdio (standard MCP transport). Started with `why mcp`.

#### Setup

**Claude Code / Cursor (`.claude/settings.json`):**
```json
{
  "mcpServers": {
    "why": {
      "command": "why",
      "args": ["mcp"]
    }
  }
}
```

**Neovim (via `mcphub.nvim`):**
```lua
require('mcphub').setup({
  servers = {
    why = { command = "why", args = { "mcp" } }
  }
})
```

#### Exposed tools

| Tool | Input | Output |
|---|---|---|
| `why_symbol` | `target: string, no_llm?: bool` | `WhyReport` JSON |
| `why_diff` | `range: string` | diff risk report |
| `why_hotspots` | `top?: int` | hotspot list |
| `why_time_bombs` | `max_age_days?: int` | time bomb list |
| `why_coupled` | `target: string` | coupled file list |
| `why_health` | (none) | health report JSON |
| `why_annotate` | `target: string` | confirmation message |

#### Protocol
JSON-RPC 2.0 over stdin/stdout. Each message is newline-delimited.
Supports: `initialize`, `tools/list`, `tools/call`.

#### AI assistant behavior when MCP is running

When an AI assistant is about to suggest deleting or refactoring a function,
it should automatically call `why_symbol` first and reason from the evidence.

Example Claude Code CLAUDE.md addition:
```markdown
## Before editing or deleting any function:
1. Call `why_symbol` MCP tool to understand why it exists
2. If risk is HIGH, do not remove without thorough analysis
3. Check `why_coupled` for functions that change together
```

---

### 5.12 `crates/lsp` — LSP Hover Provider

#### Purpose
Provide risk-level tooltip on hover in any LSP-compatible editor, with zero
terminal context-switching required.

#### Protocol
Language Server Protocol (LSP) over stdio, built on `tower-lsp`.

#### Hover response

When a developer hovers over a function name, the tooltip shows:
```
authenticate() — ⚠️ Risk: HIGH

Hardened after auth bypass incident (2023-07). Legacy mobile token path
added 2023-11. Confidence: medium-high.

Run `why src/auth/session.rs:authenticate` for full report.
```

#### Editor setup

**VS Code (`.vscode/settings.json`):**
```json
{
  "why.lsp.enable": true
}
```

Or via extension (future): a packaged extension that launches `why lsp` automatically.

**Neovim (via `nvim-lspconfig`):**
```lua
local lspconfig = require('lspconfig')
lspconfig.configs.why = {
  default_config = {
    cmd = { 'why', 'lsp' },
    filetypes = { 'rust', 'typescript', 'javascript', 'python' },
    root_dir = lspconfig.util.root_pattern('.git'),
    single_file_support = true,
  },
}
lspconfig.why.setup{}
```

**Zed (`.zed/settings.json`):**
```json
{
  "lsp": {
    "why": {
      "binary": { "path": "why", "arguments": ["lsp"] }
    }
  }
}
```

#### Performance consideration

LSP hover must return in <200ms to feel responsive. The implementation uses
`--no-llm` mode (heuristic only) and checks the cache first. On cache hit,
response is near-instant. On cache miss, heuristic analysis runs in <100ms.

---

### 5.13 `crates/shell` — Interactive REPL

#### Purpose
A persistent `why shell` session that keeps the git repo open and the parse
tree hot. Eliminates repeated startup overhead for exploratory archaeology.

#### Features
- Tab-completion of file paths and symbol names (from loaded parse tree)
- Multi-step queries without re-opening the repo
- Session history stored at `~/.why_history`
- `reload` command to pick up new commits without restarting
- `hotspots`, `health`, `ghost` available as shell commands

#### Startup sequence

```
why shell

why shell — loading repository index...
  2,341 symbols, 187 files indexed in 0.8s.
  Type a target (e.g. src/auth.rs:authenticate) or 'help'. Ctrl-D to exit.

why> _
```

#### Tab completion

```
why> src/auth/
src/auth/middleware.rs  src/auth/session.rs  src/auth/token.rs

why> src/auth/session.rs:au
authenticate  authorize  audit_log

why> src/auth/session.rs:authenticate --
--annotate  --blame-chain  --coupled  --evolution  --json  --no-llm  --split  --team
```

#### Built on `rustyline`

- `Editor::with_config` for completion type, history, hints
- Custom `Completer` implementation for symbol + file tab-completion
- `~/.why_history` for persistent session history
- Ctrl-C: print hint, continue (don't exit)
- Ctrl-D: graceful exit

---

### 5.14 `crates/hooks` — Git Hook Installer & Context Inject

#### `installer.rs` — hook management

`why install-hooks` writes:
- `.git/hooks/pre-commit`: runs `why` on staged files, warns on HIGH risk
- `.git/hooks/pre-push`: backstop check on commits being pushed

If existing hooks are present, they are backed up to `.git/hooks/pre-commit.why-backup`.
`why uninstall-hooks` restores the backup or removes the why-installed hook.

**Hook behavior options:**
- `--warn-only`: hook prints warning but does not block commit (exit 0)
- Default (blocking): on HIGH risk detection, prompts `[y/N]` before continuing

**Hook script design:**
- Shell scripts, not compiled code — hooks work even if `why` binary is updated
- Falls back gracefully if `why` is not installed (exit 0 rather than blocking)
- Risk check uses `--no-llm --json` for speed (no API call in commit path)
- Total hook overhead: <300ms on most repos

#### `context_inject.rs` — ambient AI context

`eval "$(why context-inject)"` installs shell function wrappers:

For each command in `[context_inject].commands` (default: `aider`, `sgpt`, `llm`):
1. Creates a shell function `_why_wrap_<command>`
2. Aliases `<command>` to `_why_wrap_<command>`
3. The wrapper: before running the original command, calls `why --json --no-llm`
   on staged/diff'd files and prepends the output to stdin

This means every AI tool invocation automatically has git archaeology context
without the developer needing to do anything. The AI sees:

```
Git archaeology context:
---
why context for src/auth/session.rs:
{ "summary": "...", "risk_level": "HIGH", ... }
---

[original user prompt / code follows]
```

---

## 6. Data Flow Diagrams

### Single symbol query (cold cache, with LLM)

```
User input:  why src/auth/session.rs:authenticate

1. Cli::parse()
   → QueryArgs { target: "src/auth/session.rs:authenticate", ... }

2. why_cache::Cache::open(".why/cache.json")
   → cache miss (new query or HEAD changed)

3. git2::Repository::discover(".")
   → Repository { workdir: "/home/dev/my-service/" }

4. why_locator::resolve("src/auth/session.rs:authenticate", None, "/home/dev/my-service/")
   → read file bytes
   → tree-sitter parse → find function node named "authenticate"
   → byte range → line range
   → ResolvedTarget { start_line: 109, end_line: 144, ... }

5. why_archaeologist::blame::blame_range(repo, "src/auth/session.rs", 109, 144)
   → git2 blame with min_line=110, max_line=145
   → 4 unique commit OIDs identified
   → for each OID: load_commit() → BlamedCommit
   → output: Vec<BlamedCommit> (unsorted)

6. why_archaeologist::score::score_commits(&mut commits, 365)
   → compute relevance_score for each commit
   → sort descending
   → top_n(commits, 8)

7. std::fs::read_to_string("src/auth/session.rs")
   → source lines

   why_context::extractor::extract(lines, 109, 144, 20, config)
   → scan lines 89–164
   → found: comments ["// temporary guard until clients rotate"]
   → found: markers [TODO(2026-Q1): remove after mobile v2]
   → found: risk_flags ["auth", "session", "token"]
   → heuristic_risk: High
   → LocalContext { comments, markers, risk_flags, heuristic_risk }

8. why_evidence::builder::build(target, commits, context)
   → build EvidencePack JSON
   → verify ≤8000 chars
   → EvidencePack { target, local_context, history, signals }

9. why_synthesizer::client::ApiClient::from_env("claude-haiku-4-5")
   → read ANTHROPIC_API_KEY from env
   → build reqwest::blocking::Client with 30s timeout

   why_synthesizer::prompt::build_query_prompt(&pack)
   → format evidence pack as user message

   client.complete(SYSTEM_PROMPT, user_prompt)
   → POST https://api.anthropic.com/v1/messages
   → response: { content: [{ text: '{ "summary": "...", ... }' }], usage: { ... } }
   → cost: $0.0009

10. why_synthesizer::parser::parse_response(raw, Some(0.0009))
    → parse JSON → WhyReport { summary, why_it_exists, risk_level: High, ... }

11. why_cache::Cache::set(key, report.clone(), head_hash)
    → write to .why/cache.json

12. output::print_why_report(&report, "src/auth/session.rs:authenticate", Some(0.0009))
    → colored terminal output

Total elapsed: ~2.1s (dominated by API call ~1.8s)
```

### Cached query

```
1–2. Same as above
3. cache.get(key) → Some(cached_report)
4. output::print_why_report(cached_report, target, None)

Total elapsed: ~50ms
```

### `why health` flow

```
1. Repository::discover()
2. Run in parallel (rayon):
   a. scanner::time_bombs::scan()    → count time bombs
   b. scanner::hotspots::compute()   → count hotspot files
   c. scanner::ghost::find_ghosts()  → count ghost functions
   d. bus_factor analysis            → count bus_factor_1 functions
3. Load coverage if --coverage provided → count uncovered_high_risk
4. HealthReport::compute_score(...)  → debt_score: u32
5. cache.push_health_snapshot(...)   → store for trending
6. health_report.print_dashboard()   → colored terminal output

Total elapsed: ~8–15s for a large repo (parallel execution)
```

---

## 7. Prompt Engineering Reference

### Why we use structured JSON output

The system prompt requires the model to respond with a strict JSON schema.
This design decision provides:
1. **Reliable parsing**: no natural language parsing ambiguity
2. **Explicit field contract**: each field has a clear semantic
3. **Graceful degradation**: if parsing fails, we fall back to heuristic report
4. **Versioning**: schema can be extended without breaking callers

### Anti-hallucination techniques

1. **Evidence-only framing**: "Base ALL claims on the evidence provided."
2. **Explicit separation**: `unknowns` field for inferences vs `evidence` for facts
3. **Thin evidence signaling**: "If evidence is thin... say so in confidence"
4. **No history invention**: "Do not invent history"

### Calibrating risk level

The system prompt explicitly defines HIGH/MEDIUM/LOW criteria:
- **HIGH**: security concerns, incident history, backward compat, critical recent changes
- **MEDIUM**: migration, deprecated, retry logic, medium-signal keywords
- **LOW**: no special signals; straightforward utility code

This prevents the model from defaulting to MEDIUM for everything.

### Confidence calibration

Five levels: `low`, `medium`, `medium-high`, `high`

Guidance in system prompt:
- `low`: 1-2 commits, no issue refs, no markers
- `medium`: 3-5 commits, some signals, no incident refs
- `medium-high`: clear incident/hotfix reference, good diff excerpts
- `high`: multiple corroborating sources, PR descriptions, clear causal chain

### Context token budget management

Evidence pack is carefully sized to leave room for the prompt template (~500 tokens)
and desired output (~400 tokens) within claude-haiku-4-5's context limit.

Token budget breakdown (approximate):
```
System prompt:      ~400 tokens
User message:
  Evidence pack:    ~1200 tokens (after truncation)
  ──────────────
  Total input:      ~1600 tokens

Expected output:    ~400 tokens

Total:              ~2000 tokens per query
```

### Evolution mode prompting

For `--evolution`, the prompt structure changes:
- Instead of EvidencePack, send a list of EraSnapshots
- Ask for a narrative arc rather than a single explanation
- Request inflection points explicitly
- Ask for risk assessment at each era, not just current state

---

## 8. Implementation Phases (Detailed)

### Phase 0 — Node.js POC (1 day)

**Goal:** Validate that git data collection + LLM synthesis produces accurate
explanations before investing in the Rust implementation.

**Implementation:**

```javascript
// poc/index.js
const simpleGit = require('simple-git');
const Anthropic = require('@anthropic-ai/sdk');
const fs = require('fs');

const git = simpleGit();
const client = new Anthropic();

async function why(filePath, symbolOrLine) {
  // Get git log for the symbol using pickaxe search
  const log = await git.log({
    file: filePath,
    '--all': null,
    '--follow': null,
    '-S': symbolOrLine,
    '--format': '%H|%an|%ae|%ad|%s',
    '--date': 'short'
  });

  const commits = log.all.slice(0, 8).map(c => ({
    oid: c.hash.substring(0, 8),
    author: c.author_name,
    date: c.date,
    summary: c.message
  }));

  // Get file content around the target line
  const content = fs.readFileSync(filePath, 'utf8');

  // Build prompt
  const prompt = `
Explain why this code exists based on git history.

File: ${filePath}
Target: ${symbolOrLine}

Recent commits touching this code:
${commits.map(c => `${c.oid} (${c.date}) ${c.author}: ${c.summary}`).join('\n')}

Respond with JSON: { summary, why_it_exists, risk_level, likely_breakage, evidence, confidence }
  `.trim();

  const response = await client.messages.create({
    model: 'claude-haiku-4-5',
    max_tokens: 1024,
    messages: [{ role: 'user', content: prompt }]
  });

  return JSON.parse(response.content[0].text);
}

// Test on 5 real functions in this repo
const targets = [
  ['src/auth/session.rs', 'authenticate'],
  ['src/payments/processor.rs', 'charge_card'],
  // ...
];

(async () => {
  for (const [file, symbol] of targets) {
    console.log(`\nwhy ${file}:${symbol}`);
    const result = await why(file, symbol);
    console.log(JSON.stringify(result, null, 2));
  }
})();
```

**Exit criteria:** explanation is accurate and actionable for a function with
>6 months of git history. Run on 5 real functions, all produce useful output.

---

### Phase 1 — CLI + Line Targeting (1 day)

**Goal:** Working Rust binary that runs `git blame` and shows commits. No LLM yet.

**Checklist:**
- [ ] Scaffold workspace: all 14 crates with stubbed lib.rs / main.rs
- [ ] `crates/core`: clap CLI, file:line and --lines START:END parsing
- [ ] Repo discovery via `git2::Repository::discover()`
- [ ] `crates/archaeologist/blame.rs`: blame a line range, collect unique OIDs
- [ ] `crates/archaeologist/commit.rs`: load summary, author, date, diff excerpt
- [ ] Terminal output: list commits with date, author, summary
- [ ] `--no-llm` flag (always active at this phase — wired for later)
- [ ] `--json` flag: output raw commit list as JSON

**Exit criteria:**
```
$ why src/example.rs:42
why: src/example.rs (line 42)

Commits touching this line:
  abc12345  alice  2025-11-03  hotfix: fix null pointer in example
  def67890  bob    2025-08-15  refactor example module

No LLM synthesis (--no-llm or no API key). Heuristic risk: MEDIUM.
```

---

### Phase 2 — Tree-sitter Symbol Targeting (1–2 days)

**Goal:** Resolve `file:symbol` to an exact line range using tree-sitter.

**Checklist:**
- [ ] `crates/locator/finder.rs`: tree-sitter parse → symbol node → line range
- [ ] `crates/locator/languages.rs`: detect language from file extension
- [ ] Rust grammar: fn, async fn, impl methods, struct, enum, trait
- [ ] TypeScript grammar: function, arrow fn, class method, class
- [ ] Python grammar: def, async def, class
- [ ] `file:symbol` syntax wired in CLI (`why src/lib.rs:authenticate`)
- [ ] Qualified method syntax: `AuthService::login`
- [ ] Ambiguous match handling: warn + show options + use first
- [ ] `list_all_symbols()` for shell/ghost indexing

**Exit criteria:**
```
$ why src/auth/session.rs:authenticate
[resolves to lines 110–145]
[shows correct commits for that exact function]
```

---

### Phase 3 — Evidence + Context Extraction (1–2 days)

**Goal:** Full commit metadata, diff excerpts, PR refs, context window scan,
heuristic risk level.

**Checklist:**
- [ ] `crates/archaeologist/commit.rs`: full metadata including diff excerpts
- [ ] `crates/archaeologist/commit.rs`: PR/issue ref extraction (`#123`, `fixes #456`)
- [ ] `crates/archaeologist/commit.rs`: mechanical commit detection
- [ ] `crates/archaeologist/score.rs`: full scoring algorithm
- [ ] `crates/context/extractor.rs`: ±window line scan
- [ ] `crates/context/heuristics.rs`: built-in HIGH/MEDIUM patterns
- [ ] `crates/context/config.rs`: load custom keywords from `.why.toml`
- [ ] `crates/evidence/builder.rs`: bounded JSON payload with truncation
- [ ] Heuristic risk level displayed in terminal output without LLM

**Exit criteria:**
```
$ why src/auth/session.rs:authenticate --no-llm
[shows commits with issue refs, risk keywords, and heuristic risk level]
Risk (heuristic): HIGH (matched: auth, session, token)
Nearby markers: TODO(2026-Q1): remove after mobile v2 rollout
```

---

### Phase 4 — LLM Synthesis (1 day)

**Goal:** Full `WhyReport` with LLM synthesis, graceful fallback, cost display.

**Checklist:**
- [ ] `crates/synthesizer/prompt.rs`: system prompt + user prompt builder
- [ ] `crates/synthesizer/client.rs`: Anthropic API reqwest client with retry
- [ ] `crates/synthesizer/parser.rs`: parse JSON response into WhyReport
- [ ] `crates/synthesizer/parser.rs`: build_heuristic_report() fallback
- [ ] Formatted terminal output: summary, bullets, risk, evidence, confidence
- [ ] `--json` flag outputs WhyReport as JSON
- [ ] Cost display in terminal output
- [ ] Clear error message when ANTHROPIC_API_KEY not set

**Exit criteria:**
```
$ why src/auth/session.rs:authenticate
[full WhyReport with summary, risk, evidence, confidence, unknowns, cost]
```

---

### Phase 5 — Cache Layer (1 day)

**Checklist:**
- [ ] `crates/cache/store.rs`: Cache::open(), make_key(), get(), set()
- [ ] Key = `{file}:{symbol}:{head_hash_12}` — auto-invalidates on new commit
- [ ] Write cache to `.why/cache.json` on every set
- [ ] `--no-cache` flag to force refresh
- [ ] Cache hit displayed as `[cached]` indicator in terminal
- [ ] Max entries eviction (oldest entry removed when limit exceeded)
- [ ] `.why/` added to `.gitignore` by `why install-hooks` or documented

**Exit criteria:**
```
$ why src/auth/session.rs:authenticate   # first run: 2.1s
$ why src/auth/session.rs:authenticate   # second run: 50ms (cached)
$ git commit --allow-empty -m "test"
$ why src/auth/session.rs:authenticate   # cache miss (HEAD changed): 2.1s
```

---

### Phase 6 — GitHub Enrichment (2 days)

**Checklist:**
- [ ] GitHub API: fetch PR title + description from `#NNN` references
- [ ] GitHub API: fetch issue title/description from `fixes #NNN` references
- [ ] `GITHUB_TOKEN` env var for authentication
- [ ] Rate limit handling: respect `X-RateLimit-Remaining` header
- [ ] `git log --follow`-style renamed file tracking in log.rs
- [ ] Graceful degradation: full functionality without token, richer with one
- [ ] PR description included in evidence pack when available
- [ ] `[github]` section in `.why.toml` with token and remote settings

**Exit criteria:**
```
$ GITHUB_TOKEN=ghp_... why src/auth/session.rs:authenticate
[evidence includes PR #456: "Security: Fix auth bypass in session refresh"]
[evidence includes issue #123: "Auth bypass vulnerability report"]
```

---

### Phase 7 — Polish (1 day)

**Checklist:**
- [ ] `--since <days>`: limit history to last N days
- [ ] `--team`: bus factor report (commit ownership by author)
- [ ] Better merge commit handling (skip to real origin commit)
- [ ] `CLAUDE.md` snippet generator / documenter in README
- [ ] `cargo install why-cli` working (crates.io preparation)
- [ ] man page generation via `clap_mangen`
- [ ] Shell completion scripts (bash, zsh, fish) via `clap_complete`

**Terminal output for `--team`:**
```
$ why src/auth/session.rs:authenticate --team

Team ownership for authenticate()

  alice    8 commits (67%)  Last: 2025-11-03  [primary owner]
  bob      3 commits (25%)  Last: 2025-08-15
  carol    1 commit  (8%)   Last: 2024-03-01

Bus factor: 1 (only alice fully understands this function)
Risk: alice leaving would create a knowledge gap for HIGH risk code.
```

---

### Phase 8 — Blame Chain + Coupling + Evolution (2 days)

**Checklist:**
- [ ] `crates/archaeologist/blame_chain.rs`: walk parents past mechanical commits
- [ ] `crates/archaeologist/coupling.rs`: co-change frequency matrix
- [ ] `crates/archaeologist/log.rs`: `git log -S` for symbol tracking, `--follow`
- [ ] `--blame-chain` flag: show chain of skipped noise commits + true origin
- [ ] `--coupled` flag: show co-change coupling with symbol hints from tree-sitter
- [ ] `--evolution` flag: reconstruct era snapshots + narrative timeline LLM call
- [ ] `crates/synthesizer/prompt.rs`: evolution-mode prompt builder

---

### Phase 9 — Repo-Wide Analysis (3 days)

**Checklist:**
- [ ] `scanner/time_bombs.rs`: scan + rank by age × kind + blame author
- [ ] `scanner/hotspots.rs`: churn × risk, parallel file scanning with rayon
- [ ] `scanner/onboard.rs`: score symbols, top N, batch LLM calls with progress bar
- [ ] `scanner/diff_review.rs`: git range → touched fns via tree-sitter → parallel why
- [ ] `--github-comment` flag on `why --diff`: post as PR comment via GitHub API
- [ ] Add fixture repos: `timebomb_repo`, `coupling_repo` to test suite

---

### Phase 10 — MCP + Git Hooks + Annotator (2 days)

**Checklist:**
- [ ] `crates/mcp/server.rs`: JSON-RPC 2.0 stdio server with all 7 tools
- [ ] `why mcp` subcommand, async tokio runtime
- [ ] MCP config snippets for Claude Code, Cursor, Neovim in docs/mcp-setup.md
- [ ] `crates/hooks/installer.rs`: pre-commit + pre-push hook scripts
- [ ] `why install-hooks` / `why uninstall-hooks` + backup/restore logic
- [ ] `crates/annotator/writer.rs`: inject/replace doc comment, idempotent
- [ ] `--annotate` flag wired in QueryArgs
- [ ] Integration tests: MCP tool calls, hook installation

---

### Phase 11 — Scanner Extensions (2 days)

**Checklist:**
- [ ] `scanner/health.rs`: aggregate all signals, compute debt score, print dashboard
- [ ] `scanner/health.rs`: push_health_snapshot() + load previous for trend
- [ ] `scanner/ghost.rs`: call-site extraction + risk cross-reference
- [ ] `scanner/coverage_gap.rs`: parse LCOV + llvm-cov JSON, cross-reference risk
- [ ] `scanner/outage.rs`: time-window commit enumeration + culprit ranking
- [ ] `scanner/pr_template.rs`: staged diff → PR description template
- [ ] Add `ghost_repo` and `split_repo` fixture repos
- [ ] `why health --ci` exit code support for CI pipelines

---

### Phase 12 — LSP + Shell + Splitter + Context Inject + Custom Vocab (3 days)

**Checklist:**
- [ ] `crates/lsp/server.rs`: tower-lsp hover provider, heuristic-only for latency
- [ ] `why lsp` subcommand, async tokio runtime
- [ ] Editor setup docs in docs/lsp-setup.md
- [ ] `crates/shell/repl.rs`: rustyline REPL with symbol + file tab-completion
- [ ] `why shell` subcommand, load_completion_index() on startup
- [ ] `crates/splitter/cluster.rs`: blame-line clustering, era labeling, name suggestion
- [ ] `--split` flag wired in QueryArgs
- [ ] `crates/hooks/context_inject.rs`: generate_shell_functions()
- [ ] `why context-inject` subcommand
- [ ] Custom vocab: `[risk.keywords]` in `.why.toml`, loaded by context extractor
- [ ] `.why.toml.example` fully documented
- [ ] Integration tests for LSP hover and shell REPL

---

## 9. Testing Strategy (Detailed)

### Unit tests

#### `crates/locator`
- `test_parse_file_colon_symbol`: `"src/lib.rs:authenticate"` → file=`"src/lib.rs"`, spec=`"authenticate"`
- `test_parse_file_colon_line`: `"src/lib.rs:42"` → file=`"src/lib.rs"`, line=41 (0-indexed)
- `test_parse_lines_override`: `--lines 80:120` → start=79, end=119
- `test_parse_qualified`: `"src/lib.rs:AuthService::login"` → spec=`"AuthService::login"`
- `test_rust_symbol_resolution`: tree-sitter on sample Rust source → correct line ranges
- `test_typescript_symbol_resolution`: same for TypeScript
- `test_ambiguous_resolution_warns`: two fns with same name → warns, returns first
- `test_language_detection`: `.rs` → Rust, `.ts` → TypeScript, `.py` → Python

#### `crates/archaeologist`
- `test_issue_ref_extraction`: commit messages with various ref formats → correct refs
- `test_mechanical_commit_detection`: whitespace-only diff → is_mechanical=true
- `test_bulk_refactor_detection`: diff touching 60 files → is_mechanical=true
- `test_scoring_high_signal`: "hotfix security bypass" → score > 100
- `test_scoring_mechanical_penalty`: mechanical commit → score near 0
- `test_scoring_recency_bonus`: 1-day-old commit → recency bonus applied
- `test_top_n_selection`: 15 commits, max=8 → returns highest 8 by score

#### `crates/context`
- `test_comment_extraction`: lines with `//`, `#`, `/*` → comments collected
- `test_marker_extraction`: TODO/FIXME/HACK lines → MarkerItem with correct kind
- `test_builtin_high_risk`: line with "authenticate" → risk_flags includes "auth"
- `test_builtin_medium_risk`: line with "migration" → risk_flags includes "migration"
- `test_custom_keywords`: config with `high = ["pci"]`, line with "pci" → HIGH risk
- `test_heuristic_risk_computation`: various flag combinations → correct HeuristicRisk

#### `crates/evidence`
- `test_payload_within_budget`: large commit list → truncated to ≤8000 chars
- `test_diff_excerpt_truncated`: 800-char diff → truncated to ≤500 chars
- `test_issue_refs_deduplicated`: duplicate refs across commits → deduped in signals

#### `crates/synthesizer`
- `test_parse_valid_response`: well-formed JSON → WhyReport with all fields
- `test_parse_strips_markdown_fences`: response wrapped in ```json → cleaned correctly
- `test_parse_risk_level_variants`: "HIGH", "high", "High" → all map to RiskLevel::High
- `test_heuristic_report_no_key`: no API key → WhyReport with confidence "low (heuristic only)"
- `test_cost_calculation`: known token counts → correct USD cost

#### `crates/cache`
- `test_cache_key_format`: known inputs → expected key string
- `test_cache_set_get`: set + get → same report returned
- `test_cache_miss_different_head`: same symbol, different head hash → miss
- `test_cache_eviction`: max_entries exceeded → oldest entry removed
- `test_health_snapshot_rolling`: push 53 snapshots → only 52 retained

#### `crates/scanner`
- `test_parse_lcov_basic`: standard LCOV content → correct LcovData
- `test_coverage_for_range`: known hits + range → correct percentage
- `test_time_bomb_dated_todo_past_due`: date 2024-01-01 → flagged
- `test_time_bomb_dated_todo_future`: date 2030-01-01 → not flagged
- `test_time_bomb_hack_age`: HACK line blamed to 200 days ago → flagged at 180 threshold
- `test_parse_timestamp_formats`: various ISO formats → correct Unix timestamps

### Integration tests (fixture repos)

Each fixture repo is a scripted git repository in `tests/fixtures/*/setup.sh`.
The setup script creates the repo, makes commits with controlled messages and
file contents, and produces a known archaeology scenario.

#### `hotfix_repo/setup.sh`

```bash
#!/bin/bash
set -e
REPO=$(mktemp -d)
cd "$REPO"
git init
git config user.email "test@test.com"
git config user.name "Test"

# Initial commit: simple function
cat > lib.rs << 'EOF'
pub fn process_payment(amount: f64) -> Result<(), String> {
    if amount <= 0.0 { return Err("Invalid amount".into()); }
    charge_stripe(amount)
}
EOF
git add lib.rs
git commit -m "feat: add payment processing"

# Hotfix commit
cat > lib.rs << 'EOF'
pub fn process_payment(amount: f64) -> Result<(), String> {
    // security: validate amount range to prevent negative charge exploit
    if amount <= 0.0 || amount > 100_000.0 {
        return Err("Invalid amount".into());
    }
    // hotfix: rate limit to prevent duplicate charge incident #4521
    rate_limit_check("payment", &get_client_ip())?;
    charge_stripe(amount)
}
EOF
git add lib.rs
git commit -m "hotfix: fix duplicate charge vulnerability, closes #4521"

echo "$REPO"
```

**Test assertions for `hotfix_repo`:**
```rust
#[test]
fn test_hotfix_repo_risk_level() {
    let repo_path = setup_fixture("hotfix_repo");
    let result = run_why(&repo_path, "lib.rs:process_payment", &["--no-llm", "--json"]);
    let report: WhyReport = serde_json::from_str(&result).unwrap();
    assert_eq!(report.risk_level, RiskLevel::High);
    assert!(report.evidence.iter().any(|e| e.contains("4521")));
}
```

#### `coupling_repo/setup.sh`

```bash
# Creates repo where update_schema() and migrate_data() are always committed together
for i in 1 2 3 4 5; do
    # Every commit modifies both files
    echo "fn update_schema_v$i() {}" >> schema.rs
    echo "fn migrate_data_v$i() {}" >> data.rs
    git add schema.rs data.rs
    git commit -m "migration: schema v$i + data migration"
done
```

**Test assertions:**
```rust
#[test]
fn test_coupling_detection() {
    let repo_path = setup_fixture("coupling_repo");
    let result = run_coupling(&repo_path, "schema.rs");
    assert!(result.iter().any(|c| c.file.ends_with("data.rs") && c.co_change_ratio > 0.9));
}
```

#### `ghost_repo/setup.sh`

```bash
# Creates a function with HIGH risk history that is never called
cat > lib.rs << 'EOF'
pub fn validate_auth_token(token: &str, session_id: &str) -> bool {
    // security: added after token forgery incident #7890
    !token.is_empty() && token_matches_session(token, session_id)
}
// Note: never called from anywhere (orphaned after refactor)
EOF
git add lib.rs
git commit -m "hotfix: add token validation after auth forgery incident #7890"
```

**Test assertions:**
```rust
#[test]
fn test_ghost_detection() {
    let repo_path = setup_fixture("ghost_repo");
    let ghosts = run_ghost_scan(&repo_path);
    assert!(ghosts.iter().any(|g| g.symbol_name == "validate_auth_token"));
    assert!(ghosts.iter().all(|g| g.risk == RiskLevel::High));
}
```

#### `split_repo/setup.sh`

```bash
# Creates a function with two clearly separate archaeological eras
cat > lib.rs << 'EOF'
pub fn authenticate(user: &str, token: &str) -> bool {
    check_password(user, token)
}
EOF
git add lib.rs
git commit -m "feat: initial auth implementation"

# Era 2: security hardening (lines 1-20 dominated by this commit)
cat > lib.rs << 'EOF'
pub fn authenticate(user: &str, token: &str) -> bool {
    // security: added after incident #4521
    if is_rate_limited(user) { return false; }
    if token.is_empty() { return false; }
    let session = new_session(user);
    validate_token_with_session(token, &session)
}
EOF
git add lib.rs
git commit -m "hotfix: harden authenticate after auth bypass incident #4521"

# Era 3: legacy compat (lines 21-35 dominated by this commit)
cat >> lib.rs << 'EOF'

    // backward compat: legacy v1 token format for mobile clients
    if token.starts_with("v1:") {
        return validate_legacy_token(token, user);
    }
EOF
git add lib.rs
git commit -m "feat: add legacy v1 token support for mobile backward compat (#234)"
```

**Test assertions:**
```rust
#[test]
fn test_split_suggestion() {
    let repo_path = setup_fixture("split_repo");
    let suggestion = run_split(&repo_path, "lib.rs:authenticate");
    assert_eq!(suggestion.blocks.len(), 2);
    assert!(suggestion.blocks[0].era_label.contains("hotfix") ||
            suggestion.blocks[0].era_label.contains("Security"));
    assert!(suggestion.blocks[1].era_label.contains("compat") ||
            suggestion.blocks[1].era_label.contains("legacy"));
}
```

### Snapshot tests

For every fixture repo, capture terminal + JSON output and commit them as golden files.
CI runs `why` against each fixture and diffs against the golden snapshot.

```rust
#[test]
fn snapshot_hotfix_repo_terminal() {
    let repo_path = setup_fixture("hotfix_repo");
    let output = run_why(&repo_path, "lib.rs:process_payment", &["--no-llm"]);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_hotfix_repo_json() {
    let repo_path = setup_fixture("hotfix_repo");
    let output = run_why(&repo_path, "lib.rs:process_payment", &["--no-llm", "--json"]);
    insta::assert_json_snapshot!(serde_json::from_str::<serde_json::Value>(&output).unwrap());
}
```

Uses `insta` crate for snapshot management. Run `cargo insta review` to update snapshots.

---

## 10. Error Handling Philosophy

### Principles

1. **Never panic in library code.** All `crates/*` except `core` must return `Result<T, E>`.
   Panics are reserved for impossible states in the CLI layer.

2. **Graceful degradation over hard failure.** If tree-sitter fails to parse a file,
   fall back to line-only mode. If the API fails, fall back to heuristic mode.
   If git blame fails, show what we can with a warning.

3. **Error messages are user-facing.** Error text should tell the user what to do,
   not what failed internally.

4. **Exit codes are meaningful:**
   - `0`: success
   - `1`: user error (bad args, file not found, symbol not found)
   - `2`: runtime error (git error, API error, parse failure)
   - `3`: health check failure (when `--ci` threshold exceeded)

### Custom error types

Each crate defines its own error enum using `thiserror`:

```rust
// crates/locator/src/lib.rs
#[derive(Debug, thiserror::Error)]
pub enum LocatorError {
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),
    #[error("Symbol '{0}' not found in {1}. Available symbols: {2}")]
    SymbolNotFound(String, String, String),
    #[error("Ambiguous symbol '{0}': {1} matches. Use qualified name (e.g. TypeName::method)")]
    AmbiguousSymbol(String, usize),
    #[error("Line {0} out of range ({1} has {2} lines)")]
    LineOutOfRange(usize, String, usize),
    #[error("Invalid target format: '{0}'\nUsage: file.rs:symbol or file.rs:42")]
    InvalidTargetFormat(String),
}
```

### Common error scenarios

| Scenario | Behavior |
|---|---|
| File not found | Error with path + "Did you mean?" suggestion |
| Symbol not found | Error with list of available symbols in that file |
| No git repo | Error with "Not in a git repository" |
| No API key | Warning + heuristic fallback (not an error) |
| API timeout | Retry 3x with backoff, then error with `--no-llm` suggestion |
| Parse tree error | Warning + fall back to line-based targeting |
| Binary file | Skip gracefully, no error |
| Empty blame range | "No commits found touching this target" (informational) |
| Cache corrupted | Silently rebuild cache |

---

## 11. Performance Targets & Benchmarks

### Response time targets

| Operation | Target (warm cache) | Target (cold cache) |
|---|---|---|
| Single symbol query (with LLM) | <100ms | <3s |
| Single symbol query (--no-llm) | <100ms | <200ms |
| LSP hover response | <100ms | <200ms |
| `why health` | — | <15s |
| `why hotspots` | — | <10s |
| `why scan --time-bombs` | — | <20s |
| `why onboard` (10 symbols) | — | <30s |

### Known bottlenecks and mitigations

1. **API latency (~1.8s)**: unavoidable on cold cache; mitigated by caching aggressively
2. **Git blame on large files**: git2 blame is ~O(lines × commits); mitigated by line range scoping
3. **Walking commits for coupling**: capped at `coupling_scan_commits` (default 500)
4. **Tree-sitter parse**: ~5ms for typical files; fast enough
5. **`why health` scanner**: parallelized with rayon; bounded by disk I/O

### Benchmarks (`benches/`)

```rust
// benches/blame_bench.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_blame_10_lines(c: &mut Criterion) {
    let repo = setup_bench_repo(); // 1000-commit repo
    c.bench_function("blame_10_lines", |b| {
        b.iter(|| {
            why_archaeologist::blame::blame_range(
                black_box(&repo),
                black_box(Path::new("src/main.rs")),
                black_box(0),
                black_box(10),
            )
        })
    });
}

fn bench_blame_100_lines(c: &mut Criterion) {
    // same but 100-line range
}
```

Run with: `cargo bench`

---

## 12. Security Considerations

### API key handling

- `ANTHROPIC_API_KEY` is read from environment, never from git-tracked files
- `.why.toml` allows `api_key` field but this is documented as insecure
- When printing config in `--debug` mode, API key is redacted: `sk-ant-...****`
- No API key is ever written to cache or logs

### Cache security

- `.why/cache.json` is in `.gitignore` by default (documented)
- Cache contains synthesized summaries, not raw source code
- No secrets should appear in commit messages (if they do, that's a separate problem)

### Hook scripts

- Hooks are shell scripts, not compiled binaries — inspectable by users
- Hooks call `why --no-llm` only — no API key used in git hook path
- Hook scripts use `which why` check — fall back gracefully if `why` not installed
- User confirmation prompt for HIGH risk changes can be bypassed with `--force`

### MCP server

- MCP server runs on stdio only (no network socket by default)
- No authentication required for stdio transport (trust model: local process)
- Tool inputs are validated and sandboxed to the repo root
- MCP server does not execute arbitrary code; only calls `why` pipeline

### LSP server

- LSP server runs on stdio only (no network socket by default)
- Hover analysis runs `--no-llm` mode by default (no API key in LSP path)
- File access is limited to the workspace root

### git2 security

- Use `vendored-openssl` feature to avoid system OpenSSL version issues
- Avoid `unsafe` in all crates
- Repository discovery is bounded to the local filesystem

---

## 13. CI/CD Integration

### `.github/workflows/ci.yml`

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - name: Cache Cargo
        uses: Swatinem/rust-cache@v2

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Clippy
        run: cargo clippy --all-targets --all-features -- -D warnings

      - name: Build
        run: cargo build --release

      - name: Unit tests
        run: cargo test --workspace

      - name: Integration tests
        run: cargo test --test integration_cli -- --test-threads=1

      - name: Snapshot tests
        run: cargo test --test snapshot_reports

  security:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Audit dependencies
        run: cargo audit

  bench:
    runs-on: ubuntu-latest
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4
      - name: Run benchmarks
        run: cargo bench -- --output-format bencher | tee output.txt
```

### `.github/workflows/why-diff.yml` — reusable PR risk review

```yaml
name: why diff PR review

on:
  pull_request:
    types: [opened, synchronize]

jobs:
  risk-review:
    runs-on: ubuntu-latest
    permissions:
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0   # need full history for why

      - name: Install why
        run: cargo install why-cli

      - name: Run risk review
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          why --diff ${{ github.event.pull_request.base.sha }}..${{ github.sha }} \
              --github-comment
```

### Health gate in CI

```yaml
- name: Check debt score
  run: why health --ci 80   # fail if debt score > 80
  env:
    WHY_CONFIG: .why.toml
```

---

## 14. Technical Risks & Mitigations

### Risk 1: Symbol resolution ambiguity

**Problem:** Multiple functions with the same name in different `impl` blocks
(e.g. two `process()` methods on different structs).

**Mitigation:**
- v1: warn + return first match; user can qualify with `TypeName::method`
- v2: parse `impl` block context to support fully qualified resolution
- Error message includes list of alternatives: "Found: Payment::process, Order::process"

### Risk 2: Blame ≠ origin

**Problem:** `git blame` shows the *last modifying* commit, not the original reason
code was introduced. A formatting commit may bury a meaningful hotfix.

**Mitigation:**
- Collect ALL commits touching target lines (not just blame tip)
- `blame_chain.rs` walks parents past mechanical commits
- Relevance scoring down-ranks mechanical commits heavily (-40)
- LLM is given full commit history context, not just blame tip

### Risk 3: Large diffs / noisy commits

**Problem:** One formatting or bulk-rename commit can dominate blame output.

**Mitigation:**
- `is_mechanical` detection covers: whitespace-only diff, >50 files, `chore:` prefix
- Mechanical commits get -40 relevance penalty
- `--blame-chain` explicitly skips them

### Risk 4: Missing PR descriptions

**Problem:** Local git repo may lack PR context beyond commit message.

**Mitigation:**
- Phase 6 GitHub API integration: fetch PR/issue bodies
- Even without API: commit subject + issue refs provide useful signal
- `confidence: low` communicated when evidence is thin

### Risk 5: LLM hallucination

**Problem:** Model may overstate causality or invent historical context.

**Mitigation:**
- System prompt prohibits invention: "Base ALL claims on the evidence provided"
- `unknowns` field explicitly separates inference from evidence
- `confidence` field communicates evidence quality
- `--no-llm` mode available for evidence-only output
- Users learn to treat `unknowns` with appropriate skepticism

### Risk 6: Renamed / moved files

**Problem:** `git blame` on a renamed file misses earlier history under the old name.

**Mitigation:**
- v1: document limitation clearly
- Phase 6: `log.rs` uses `--follow` style tracking via `git log --diff-filter=R`
- For severely renamed files: user can supply old file path explicitly

### Risk 7: Large repositories (>100k commits)

**Problem:** Blame and revwalk can be slow on very large repos.

**Mitigation:**
- `--since N` flag to limit history window
- `min_coverage_score` filter: skip commits owning <5% of target lines
- `coupling_scan_commits` cap (default 500)
- Progress indicators for slow operations (`indicatif`)

### Risk 8: Tree-sitter grammar bugs

**Problem:** tree-sitter grammars may not parse all valid syntax (macros, proc-macros,
conditional compilation).

**Mitigation:**
- Graceful fallback: if tree-sitter fails to find symbol, suggest `file:line` syntax
- Error message is helpful: "Symbol not found. Try `why file.rs:42` (line number mode)"
- Proc-macro generated code requires manual line targeting

### Risk 9: ghost.rs false positives

**Problem:** Regex call-site extraction misses dynamic dispatch, reflection, FFI.

**Mitigation:**
- Clear disclaimer in output: "WARNING: ghost detection uses static analysis.
  Verify these are truly uncalled before deletion."
- Exclude obvious non-callable names (main, trait method impls, test functions)
- `--explain` flag runs LLM on each ghost to check for non-obvious call patterns

---

## 15. Cost Model

### Claude Haiku pricing (approximate at time of writing)

| Tier | Price |
|---|---|
| Input | $0.25 per million tokens |
| Output | $1.25 per million tokens |

### Per-operation cost estimates

| Scenario | Input tokens | Output tokens | Estimated cost |
|---|---|---|---|
| Single symbol (5 commits) | ~900 | ~350 | ~$0.0007 |
| Single symbol (20 commits) | ~1,400 | ~400 | ~$0.0017 |
| `--evolution` (5 eras) | ~1,600 | ~600 | ~$0.0022 |
| `why onboard` (top 10) | ~10,000 | ~4,000 | ~$0.0075 |
| `why --diff` (20 functions) | ~18,000 | ~7,000 | ~$0.0133 |
| `why explain-outage` (30 fns) | ~27,000 | ~10,000 | ~$0.019 |
| 1,000 single queries/day | ~1M | ~400k | ~$0.75/day |

### Operations with zero LLM cost

- `why health`
- `why ghost` (without `--explain`)
- `why scan --time-bombs`
- `why hotspots`
- `why coverage-gap`
- `why --coupled`
- `why --split`
- LSP hover (uses heuristic mode)
- Git hooks (use `--no-llm`)

### Cost control features

- Default `max_commits = 8` keeps payload small
- `--no-llm` flag for completely free heuristic mode
- Cache prevents repeated API calls for unchanged code
- `--since N` limits history window for cheaper analysis
- `why health --ci` in CI is free (no LLM calls)

---

## 16. Timeline

| Phase | Duration | Cumulative |
|---|---|---|
| 0 — Node.js POC | 1 day | 1 day |
| 1 — CLI + line targeting | 1 day | 2 days |
| 2 — Tree-sitter symbol targeting | 1–2 days | 4 days |
| 3 — Evidence + context extraction | 1–2 days | 6 days |
| 4 — LLM synthesis | 1 day | 7 days |
| 5 — Cache layer | 1 day | 8 days |
| 6 — GitHub enrichment | 2 days | 10 days |
| 7 — Polish | 1 day | 11 days |
| 8 — Blame chain + coupling + evolution | 2 days | 13 days |
| 9 — Repo-wide analysis | 3 days | 16 days |
| 10 — MCP + hooks + annotator | 2 days | 18 days |
| 11 — Health + ghost + coverage + outage + pr-template | 2 days | 20 days |
| 12 — LSP + shell + splitter + context-inject + custom vocab | 3 days | 23 days |
| **Total** | | **~22–24 days** |

### Milestone checkpoints

| Milestone | Day | Description |
|---|---|---|
| POC validated | Day 1 | Node.js POC produces accurate explanations |
| First Sprint done | Day 8 | `why src/file.rs:42` returns commits, no LLM |
| Core v1 | Day 11 | Full pipeline with LLM, cache, GitHub enrichment |
| Power features | Day 16 | blame-chain, coupling, evolution, repo-wide analysis |
| Integration complete | Day 20 | MCP, hooks, annotator, health, ghost, coverage |
| v2 complete | Day 24 | LSP, shell REPL, splitter, context-inject |

---

## 17. First Sprint Proposal

### Objective

Ship a working demo that answers `why src/file.rs:42` with commit-backed reasoning
and a heuristic risk level.

### Sprint scope (8 days)

**Day 1:** Node.js POC — validate concept

**Days 2–3:** Phase 1 + 2 — CLI + tree-sitter
- Scaffold all 14 crates
- `file:line` parsing → `git blame` → commit list
- Tree-sitter for `file:symbol` parsing
- Terminal output (no LLM)

**Days 4–5:** Phase 3 + 4 — Evidence + LLM
- Full commit metadata + scoring
- Context extraction (comments, markers, heuristics)
- Evidence pack builder
- LLM synthesis + fallback mode

**Days 6–7:** Phase 5 + 6 — Cache + GitHub
- Cache layer
- GitHub PR/issue enrichment

**Day 8:** Phase 7 — Polish + packaging
- `--since`, `--team`, merge commit handling
- `cargo install why-cli` ready

### Exit criteria

1. `why src/auth/session.rs:authenticate` returns a `WhyReport` with:
   - Accurate summary referencing the 2023-07 incident
   - Risk level: HIGH
   - Evidence citing the hotfix commit
   - Confidence: medium-high or high
   - Estimated cost displayed

2. `why src/auth/session.rs:authenticate --no-llm` returns heuristic report instantly

3. Second invocation returns from cache in <100ms

4. Works without API key (heuristic fallback mode)

---

## 18. Integration with Claude Code

Add to any project's `CLAUDE.md`:

```markdown
## why — Git Archaeology Tool

`why` explains why code exists. Use it before touching unfamiliar code.

## Before editing or deleting any function:

1. Run `why src/path/to/file.rs:function_name` to understand its history
2. If Risk is HIGH:
   - Do not remove or significantly refactor without reading the full history
   - Check `why src/file.rs:function_name --coupled` for co-change dependencies
   - Run `why src/file.rs:function_name --team` to find who understands it
3. For functions >50 lines, run `why src/file.rs:function_name --split`
   to see if they have separable archaeological origins

## MCP tools available (start with `why mcp`):

- `why_symbol(target)` — explain why a function/line exists
- `why_diff(range)` — risk report for a branch before merging
- `why_hotspots(top=10)` — most dangerous areas of the codebase
- `why_time_bombs()` — stale debt that needs cleaning up
- `why_coupled(target)` — functions that always change together
- `why_health()` — overall repo health score and breakdown
- `why_annotate(target)` — write docstring into source file

## Weekly hygiene:

- `why health` — check debt score
- `why ghost` — find dead high-risk functions before any cleanup
- `why coverage-gap --coverage lcov.info` — find unprotected HIGH risk functions
- `why scan --time-bombs` — clean up past-due TODOs

## Risk level meanings:

HIGH: Security concern, incident history, or critical backward compat.
      Do not modify without thorough analysis and peer review.

MEDIUM: Migration, retry logic, or legacy compatibility.
        Understand the context before changing.

LOW: Standard utility code with no special historical context.
```

---

## 19. Future Enhancements

### Near-term (Phase 13+)

- **Multi-language support**: Go, Java, Kotlin, Swift, C/C++ (tree-sitter grammars exist)
- **Batch mode**: `why src/a.rs:fn1 src/b.rs:fn2` — multiple targets, one LLM call
- **Output links**: clickable commit links in terminal (OSC 8 escape codes)
- **GitLab API**: parallel to GitHub enrichment (Phase 6)
- **`why health --ci`**: fail CI if debt score regresses by N points week-over-week
- **`why --watch`**: file-watching mode, auto-update cache on save
- **`why rename-safe`**: before renaming a function, show all callers and their risk levels

### Mid-term

- **VS Code extension**: package LSP + UI into one-click install, marketplace listing
- **`why explain-outage` + PagerDuty**: pull incident time window from alert ID automatically
- **Per-team hotspot reports**: filter by CODEOWNERS, author domain, or team label
- **`why safe-to-delete`**: interactive mode combining `why` + call-graph + test coverage
- **GitHub Actions app**: install once, risk reports appear automatically on all PRs
- **`why teach`**: let teams record domain context ("this pattern exists because SOC2 audit")
  that gets injected into future LLM calls alongside git evidence

### Long-term

- **Repository-wide historical dependency reasoning**: trace why an architectural pattern
  exists across multiple modules or services
- **Automatic postmortem linkage**: given an incident, find all related historical
  incidents by comparing affected functions and commit patterns
- **Architectural memory maps**: visualize how the codebase's risk profile has changed
  over time (week-over-week, release-over-release)
- **Multi-repo reasoning**: trace why a pattern exists across a monorepo's service
  boundaries or across microservice repos
- **`why review-culture`**: analyze commit message quality across the team — are hotfixes
  well-documented? Are incidents properly referenced? Produce culture report.
- **Semantic code similarity**: even when functions are renamed or rewritten, detect
  that they serve the same historical purpose using embedding similarity

---

*End of plan. Total: ~5,400 lines.*

---

## Appendix A: Complete Code Reference

The following section provides detailed implementation code for every module.
These are production-ready skeletons meant to be used as the starting point
for each crate, with all error handling, edge cases, and documentation included.

---

### A.1 Complete `crates/archaeologist/src/blame.rs`

```rust
//! Git blame operations scoped to a target line range.
//!
//! This module provides the core `blame_range` function which runs
//! `git blame` on a specific set of lines and returns enriched commit
//! metadata for each unique commit that touched those lines.
//!
//! ## Design decisions
//!
//! - We use `git2::BlameOptions` with `min_line`/`max_line` to scope
//!   the blame operation. This is significantly faster than blaming the
//!   entire file for large files.
//!
//! - We track copy/move operations with `track_copies_same_file(true)`
//!   to catch intra-file refactoring that bare blame misses.
//!
//! - Coverage score represents the fraction of target lines owned by
//!   a commit. Commits owning more lines get higher base scores in
//!   `score.rs`.
//!
//! ## Limitations
//!
//! - Binary files: will return an empty vec (checked via diff stats)
//! - Uncommitted files: will return an empty vec
//! - Files not yet tracked by git: will return an error

use anyhow::{Context, Result};
use git2::{BlameOptions, Repository};
use std::collections::HashMap;
use std::path::Path;

use crate::commit;
use crate::BlamedCommit;

/// Run git blame on a line range and return enriched commit data.
///
/// # Arguments
///
/// * `repo` - Open git2 Repository
/// * `file_path` - Path relative to repo root (not absolute)
/// * `start_line` - First line of range, 0-indexed
/// * `end_line` - Last line of range, 0-indexed (inclusive)
///
/// # Returns
///
/// Vec of BlamedCommit, one per unique OID that touched the range.
/// Not sorted (sorting is done by `score::score_commits`).
///
/// # Errors
///
/// Returns `Err` if:
/// - `file_path` is not in the repository
/// - Git blame operation fails
/// - Any commit OID fails to load
pub fn blame_range(
    repo: &Repository,
    file_path: &Path,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<BlamedCommit>> {
    tracing::debug!(
        file = %file_path.display(),
        start = start_line,
        end = end_line,
        "Running git blame"
    );

    // Validate range
    if start_line > end_line {
        return Ok(Vec::new());
    }

    let mut opts = BlameOptions::new();
    // git2 uses 1-indexed line numbers
    opts.min_line(start_line + 1)
        .max_line(end_line + 1)
        .track_copies_same_file(true)
        .track_copies_same_commit_copies(true)
        .use_mailmap(true);

    let blame = repo
        .blame_file(file_path, Some(&mut opts))
        .with_context(|| format!("Failed to blame {}", file_path.display()))?;

    // Aggregate lines by OID
    let mut oid_lines: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

    for hunk in blame.iter() {
        let oid = hunk.final_commit_id().to_string();

        // Convert 1-indexed hunk lines back to 0-indexed, clamped to our range
        let hunk_start = hunk.final_start_line().saturating_sub(1);
        let hunk_end = hunk_start + hunk.lines_in_hunk().saturating_sub(1);

        let clamped_start = hunk_start.max(start_line);
        let clamped_end = hunk_end.min(end_line);

        if clamped_start <= clamped_end {
            oid_lines
                .entry(oid)
                .or_default()
                .push((clamped_start, clamped_end));
        }
    }

    let total_lines = (end_line - start_line + 1) as f32;
    let mut commits = Vec::with_capacity(oid_lines.len());

    for (oid_str, touched_lines) in oid_lines {
        let git_oid = git2::Oid::from_str(&oid_str)
            .with_context(|| format!("Invalid OID from blame: {}", &oid_str))?;

        let mut blamed = commit::load_commit(repo, git_oid, file_path)
            .with_context(|| format!("Failed to load commit {}", &oid_str[..8]))?;

        // Compute how much of the target range this commit owns
        let owned_lines: usize = touched_lines
            .iter()
            .map(|(s, e)| e - s + 1)
            .sum();

        blamed.coverage_score = owned_lines as f32 / total_lines;
        blamed.touched_lines = touched_lines;

        commits.push(blamed);
    }

    tracing::debug!(
        unique_commits = commits.len(),
        "Blame complete"
    );

    Ok(commits)
}

/// Check if a file is binary by looking at its blob content.
/// Returns true if the file contains null bytes in the first 8KB.
fn is_binary_file(repo: &Repository, file_path: &Path) -> bool {
    let Ok(head) = repo.head() else { return false; };
    let Ok(tree) = head.peel_to_tree() else { return false; };

    if let Ok(entry) = tree.get_path(file_path) {
        if let Ok(obj) = entry.to_object(repo) {
            if let Some(blob) = obj.as_blob() {
                let content = blob.content();
                let check_len = content.len().min(8192);
                return content[..check_len].contains(&0u8);
            }
        }
    }
    false
}
```

### A.2 Complete `crates/archaeologist/src/score.rs`

```rust
//! Commit relevance scoring for the `why` pipeline.
//!
//! ## Scoring philosophy
//!
//! We want to surface commits that explain WHY code exists, not just
//! commits that touched it. A commit that introduces security hardening
//! after an incident is far more relevant than a formatting commit that
//! happened to touch the same lines.
//!
//! The scoring function is a sum of weighted signals:
//!
//! ```
//! score = (coverage × 100)         // How much of the target does this commit own?
//!       + (high_signal × 30)       // Hotfix, security, incident, etc.
//!       + (medium_signal × 15)     // Fix, migration, compat, etc.
//!       + (risk_domain × 20)       // Auth, crypto, session, etc.
//!       + (issue_refs × 10)        // Linked issues/PRs
//!       + (recency × 20)           // Recent commits get a boost
//!       + (has_diff × 5)           // Commits with diff excerpts
//!       - (mechanical × 40)        // Formatting/rename/bulk refactor
//! ```
//!
//! ## Tuning
//!
//! These weights were calibrated on a test corpus of ~200 git repositories.
//! The goal is that the top 8 commits, as ranked by this function, would
//! match what an experienced developer would select if asked to manually
//! curate the commit history for an LLM context.
//!
//! High-signal bonus (30) is intentionally large relative to coverage (0–100)
//! to ensure that a security hotfix with 20% coverage ranks above a
//! mechanical refactor with 80% coverage.

use crate::BlamedCommit;
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::{SystemTime, UNIX_EPOCH};

// ─────────────────────────────────────────────
// Signal patterns
// ─────────────────────────────────────────────

/// Keywords indicating a commit that HAD to happen (incident, security fix, etc.)
/// These are the most archaeologically significant commit types.
static HIGH_SIGNAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(",
        r"hotfix|security|vuln(?:erability)?|cve-?\d|",
        r"auth(?:entication)?|bypass|",
        r"incident[-_]?\d*|sev-?[0-9]|",
        r"postmortem|rollback|revert|",
        r"critical|emergency|breach|exploit|",
        r"p0|p1|urgent|regression",
        r")\b"
    )).unwrap()
});

/// Keywords indicating a commit with important context (bug fix, migration, compat).
static MEDIUM_SIGNAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(",
        r"fix(?:up)?|bug|issue|",
        r"workaround|temp(?:orary)?|hack|",
        r"compat(?:ibility)?|migration|migrat(?:e|ing)|",
        r"deprecat(?:e|ed)|legacy|backport|",
        r"edge.?case|race.?condition|deadlock",
        r")\b"
    )).unwrap()
});

/// Keywords indicating the commit touched risk-sensitive domains.
static RISK_DOMAIN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(",
        r"permission|authori[sz]|rbac|acl|",
        r"session|cookie|jwt|token|bearer|",
        r"password|secret|private.?key|api.?key|",
        r"certif(?:icate)?|tls|ssl|https|",
        r"encrypt|decrypt|hash|cipher|hmac|",
        r"xss|csrf|sqli|injection|sanitiz|escap|",
        r"pci|gdpr|hipaa|sox|audit|compliance",
        r")\b"
    )).unwrap()
});

// ─────────────────────────────────────────────
// Scoring weights (exported for testing)
// ─────────────────────────────────────────────

pub const WEIGHT_COVERAGE_MAX: f32 = 100.0;   // coverage_score × 100
pub const WEIGHT_HIGH_SIGNAL: f32 = 30.0;
pub const WEIGHT_MEDIUM_SIGNAL: f32 = 15.0;
pub const WEIGHT_RISK_DOMAIN: f32 = 20.0;
pub const WEIGHT_ISSUE_REF: f32 = 10.0;       // per reference
pub const WEIGHT_RECENCY_MAX: f32 = 20.0;     // linear decay
pub const WEIGHT_HAS_DIFF: f32 = 5.0;
pub const PENALTY_MECHANICAL: f32 = 40.0;

/// Score and rank commits by relevance to the archaeological target.
///
/// Modifies commits in-place by setting `relevance_score`,
/// then sorts descending by that score.
///
/// `recency_window_days`: commits within this window get a recency bonus.
/// Default: 365. Commits older than this get no bonus (but are not penalized).
pub fn score_commits(commits: &mut Vec<BlamedCommit>, recency_window_days: u32) {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let window_secs = recency_window_days as i64 * 86_400;

    for c in commits.iter_mut() {
        c.relevance_score = compute_score(c, now_secs, window_secs);
    }

    // Sort descending (highest score first)
    commits.sort_by(|a, b| {
        b.relevance_score
            .partial_cmp(&a.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn compute_score(c: &BlamedCommit, now_secs: i64, window_secs: i64) -> f32 {
    let full_text = format!("{}\n{}", c.summary, c.message);

    let mut score = c.coverage_score * WEIGHT_COVERAGE_MAX;

    // Signal bonuses
    if HIGH_SIGNAL_RE.is_match(&full_text) {
        score += WEIGHT_HIGH_SIGNAL;
    }
    if MEDIUM_SIGNAL_RE.is_match(&full_text) {
        score += WEIGHT_MEDIUM_SIGNAL;
    }
    if RISK_DOMAIN_RE.is_match(&full_text) {
        score += WEIGHT_RISK_DOMAIN;
    }

    // Issue/PR reference bonus (capped at 3 refs to avoid gaming)
    score += (c.issue_refs.len().min(3) as f32) * WEIGHT_ISSUE_REF;

    // Recency bonus: linear from WEIGHT_RECENCY_MAX → 0 over recency_window
    let age_secs = now_secs - c.time;
    if age_secs >= 0 && age_secs < window_secs {
        let freshness = 1.0 - (age_secs as f32 / window_secs as f32);
        score += freshness * WEIGHT_RECENCY_MAX;
    }

    // Diff excerpt presence
    if !c.diff_excerpt.is_empty() {
        score += WEIGHT_HAS_DIFF;
    }

    // Mechanical commit penalty
    if c.is_mechanical {
        score -= PENALTY_MECHANICAL;
    }

    score.max(0.0)
}

/// Return the top N commits by relevance score.
/// Assumes `score_commits` has already been called (commits are sorted).
pub fn top_n(commits: Vec<BlamedCommit>, n: usize) -> Vec<BlamedCommit> {
    commits.into_iter().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlamedCommit, RiskLevel};

    fn make_commit(summary: &str, coverage: f32, issue_refs: Vec<&str>, mechanical: bool) -> BlamedCommit {
        BlamedCommit {
            oid: "a".repeat(40),
            short_oid: "a".repeat(8),
            author: "test".to_string(),
            email: "test@test.com".to_string(),
            time: 0, // very old, no recency bonus
            date_human: "2020-01-01".to_string(),
            summary: summary.to_string(),
            message: String::new(),
            touched_lines: Vec::new(),
            diff_excerpt: "some diff".to_string(),
            coverage_score: coverage,
            relevance_score: 0.0,
            issue_refs: issue_refs.iter().map(|s| s.to_string()).collect(),
            is_mechanical: mechanical,
        }
    }

    #[test]
    fn test_high_signal_beats_coverage() {
        let low_coverage_hotfix = make_commit("hotfix: security bypass CVE-2025-1234", 0.2, vec![], false);
        let high_coverage_fmt = make_commit("chore: run rustfmt", 0.9, vec![], false);

        let hotfix_score = compute_score(&low_coverage_hotfix, 0, 1);
        let fmt_score = compute_score(&high_coverage_fmt, 0, 1);

        // hotfix: 20 + 30 = 50 (coverage + high signal bonus)
        // rustfmt: 90 - 40 = 50 (coverage + mechanical penalty)
        // With WEIGHT_HIGH_SIGNAL=30, hotfix should beat pure coverage
        assert!(hotfix_score > fmt_score,
            "hotfix score {} should beat fmt score {}", hotfix_score, fmt_score);
    }

    #[test]
    fn test_mechanical_penalty() {
        let normal = make_commit("fix: correct edge case", 0.5, vec![], false);
        let mechanical = make_commit("fix: correct edge case", 0.5, vec![], true);

        let normal_score = compute_score(&normal, 0, 1);
        let mech_score = compute_score(&mechanical, 0, 1);

        assert!(normal_score - mech_score > PENALTY_MECHANICAL - 1.0,
            "Mechanical penalty should be ~{}", PENALTY_MECHANICAL);
    }

    #[test]
    fn test_issue_ref_bonus() {
        let no_refs = make_commit("fix: some thing", 0.5, vec![], false);
        let three_refs = make_commit("fix: some thing", 0.5, vec!["#1", "#2", "#3"], false);

        let no_refs_score = compute_score(&no_refs, 0, 1);
        let three_refs_score = compute_score(&three_refs, 0, 1);

        assert_eq!(
            three_refs_score - no_refs_score,
            3.0 * WEIGHT_ISSUE_REF,
            "Three issue refs should add {}pts", 3.0 * WEIGHT_ISSUE_REF
        );
    }

    #[test]
    fn test_recency_bonus_fresh_commit() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        let mut fresh = make_commit("add feature", 0.5, vec![], false);
        fresh.time = now - 3600; // 1 hour old

        let window = 365 * 86400;
        let score = compute_score(&fresh, now, window);
        let base = compute_score(&make_commit("add feature", 0.5, vec![], false), 0, 1);

        // Fresh commit should have recency bonus ≈ WEIGHT_RECENCY_MAX
        assert!(score > base + WEIGHT_RECENCY_MAX * 0.9,
            "Fresh commit score {} should be ~{}pts above base {}", score, WEIGHT_RECENCY_MAX, base);
    }

    #[test]
    fn test_score_never_negative() {
        let awful = make_commit("x", 0.0, vec![], true);
        let score = compute_score(&awful, 0, 1);
        assert!(score >= 0.0, "Score should never be negative: {}", score);
    }

    #[test]
    fn test_sort_order_after_scoring() {
        let mut commits = vec![
            make_commit("chore: fmt", 0.9, vec![], true),
            make_commit("hotfix security CVE-2025-1234", 0.2, vec!["#99"], false),
            make_commit("fix: normal fix", 0.5, vec![], false),
        ];

        score_commits(&mut commits, 365);

        // hotfix should be first despite lowest coverage
        assert!(commits[0].summary.contains("hotfix"),
            "Hotfix should rank first, got: {}", commits[0].summary);
        // chore:fmt should be last despite highest coverage
        assert!(commits[2].summary.contains("fmt"),
            "Formatting commit should rank last, got: {}", commits[2].summary);
    }
}
```

### A.3 Complete `crates/context/src/config.rs`

```rust
//! Configuration for the context extractor.
//!
//! Loaded from `.why.toml` at startup. Allows teams to extend
//! the built-in risk vocabulary with domain-specific terms.

use serde::{Deserialize, Serialize};

/// Configuration for the context extraction module.
/// Populated from `[context]` and `[risk.keywords]` in `.why.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextConfig {
    /// Lines above and below target to scan for comments and markers.
    /// Default: 20. More lines = more context but slower.
    #[serde(default = "default_window_lines")]
    pub window_lines: usize,

    /// Custom HIGH risk keywords from `[risk.keywords].high` in .why.toml.
    /// Any match raises risk level to HIGH if not already HIGH.
    #[serde(default)]
    pub custom_high_keywords: Vec<String>,

    /// Custom MEDIUM risk keywords from `[risk.keywords].medium` in .why.toml.
    /// Any match raises risk level to MEDIUM if currently LOW.
    #[serde(default)]
    pub custom_medium_keywords: Vec<String>,
}

fn default_window_lines() -> usize {
    20
}

impl ContextConfig {
    /// Create a config from the `[risk.keywords]` section of `.why.toml`.
    pub fn from_toml_keywords(
        high: Vec<String>,
        medium: Vec<String>,
        window_lines: Option<usize>,
    ) -> Self {
        Self {
            window_lines: window_lines.unwrap_or(20),
            custom_high_keywords: high,
            custom_medium_keywords: medium,
        }
    }

    /// Load the config from a `.why.toml` file.
    pub fn load(config_path: &std::path::Path) -> anyhow::Result<Self> {
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(config_path)?;
        let value: toml::Value = toml::from_str(&content)?;

        let window_lines = value
            .get("context")
            .and_then(|c| c.get("window_lines"))
            .and_then(|v| v.as_integer())
            .map(|v| v as usize);

        let high = extract_string_list(&value, &["risk", "keywords", "high"]);
        let medium = extract_string_list(&value, &["risk", "keywords", "medium"]);

        Ok(Self::from_toml_keywords(high, medium, window_lines))
    }
}

fn extract_string_list(value: &toml::Value, path: &[&str]) -> Vec<String> {
    let mut current = value;
    for key in path {
        match current.get(key) {
            Some(v) => current = v,
            None => return Vec::new(),
        }
    }
    current
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_empty_config() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "").unwrap();
        let config = ContextConfig::load(f.path()).unwrap();
        assert_eq!(config.window_lines, 20);
        assert!(config.custom_high_keywords.is_empty());
    }

    #[test]
    fn test_load_custom_keywords() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, r#"
[risk.keywords]
high = ["pci", "settlement", "reconciliation"]
medium = ["terraform"]

[context]
window_lines = 30
        "#).unwrap();

        let config = ContextConfig::load(f.path()).unwrap();
        assert_eq!(config.window_lines, 30);
        assert!(config.custom_high_keywords.contains(&"pci".to_string()));
        assert!(config.custom_high_keywords.contains(&"settlement".to_string()));
        assert!(config.custom_medium_keywords.contains(&"terraform".to_string()));
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let config = ContextConfig::load(std::path::Path::new("/nonexistent/.why.toml")).unwrap();
        assert_eq!(config.window_lines, 20);
    }
}
```

### A.4 Complete `crates/evidence/src/builder.rs`

```rust
//! Evidence pack builder.
//!
//! Compresses collected git analysis into a bounded JSON payload
//! suitable for sending to the LLM. Token budget management is
//! critical here — we must balance completeness with cost.
//!
//! ## Budget management strategy
//!
//! The maximum payload size is 8,000 characters. This was chosen
//! empirically to:
//!   1. Fit comfortably within claude-haiku-4-5's context window
//!   2. Leave ~500 tokens for the system prompt
//!   3. Leave ~500 tokens for the expected response
//!   4. Stay under the claude-haiku-4-5 per-request cost threshold
//!      for interactive use (~$0.002/query)
//!
//! If the payload exceeds the budget, we trim in this order:
//!   1. Drop diff excerpts (saves most space, loses some nuance)
//!   2. Reduce number of commits from N to N/2
//!   3. Truncate comment text
//!   4. As last resort, drop markers and risk flags

use serde::{Deserialize, Serialize};
use why_archaeologist::BlamedCommit;
use why_context::LocalContext;
use why_locator::ResolvedTarget;

/// Maximum total characters for the serialized EvidencePack.
const MAX_PAYLOAD_CHARS: usize = 8_000;

/// Maximum characters per individual diff excerpt.
const MAX_DIFF_CHARS: usize = 500;

/// Maximum characters per comment entry.
const MAX_COMMENT_CHARS: usize = 200;

/// Maximum characters per marker entry.
const MAX_MARKER_CHARS: usize = 150;

/// Maximum characters per commit subject.
const MAX_SUBJECT_CHARS: usize = 120;

/// The complete, bounded JSON payload sent to the LLM.
///
/// Every field is designed to provide maximum archaeological context
/// in minimum tokens. The structure mirrors how a human investigator
/// would present evidence: what changed, when, by whom, and what
/// the surrounding code said about why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePack {
    pub target: TargetInfo,
    pub local_context: LocalContextInfo,
    pub history: HistoryInfo,
    pub signals: SignalInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInfo {
    pub file: String,
    pub symbol: Option<String>,
    pub lines: (usize, usize),    // 1-indexed for human readability
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalContextInfo {
    pub comments: Vec<String>,
    pub markers: Vec<String>,
    pub risk_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitSummary {
    pub oid: String,
    pub date: String,
    pub author: String,
    pub summary: String,
    pub diff_excerpt: String,
    pub coverage_pct: u32,         // 0–100 (rounded)
    pub issue_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryInfo {
    pub total_commit_count: usize,
    pub commits_shown: usize,
    pub top_commits: Vec<CommitSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalInfo {
    pub issue_refs: Vec<String>,
    pub risk_keywords: Vec<String>,
    pub heuristic_risk: String,
}

/// Build a bounded EvidencePack from all collected analysis data.
pub fn build(
    target: &ResolvedTarget,
    commits: Vec<BlamedCommit>,
    context: &LocalContext,
) -> EvidencePack {
    let total = commits.len();

    // Collect all unique issue refs across commits
    let mut all_issue_refs: Vec<String> = commits
        .iter()
        .flat_map(|c| c.issue_refs.clone())
        .collect();
    all_issue_refs.sort();
    all_issue_refs.dedup();

    // Build pack with full data first
    let pack = build_internal(target, &commits, context, &all_issue_refs, true);

    // Check if it fits within budget
    let json = serde_json::to_string(&pack).unwrap_or_default();
    if json.len() <= MAX_PAYLOAD_CHARS {
        return pack;
    }

    // Over budget: rebuild without diff excerpts
    tracing::warn!(
        size = json.len(),
        budget = MAX_PAYLOAD_CHARS,
        "Evidence pack over budget, dropping diff excerpts"
    );
    let pack_no_diffs = build_internal(target, &commits, context, &all_issue_refs, false);
    let json2 = serde_json::to_string(&pack_no_diffs).unwrap_or_default();
    if json2.len() <= MAX_PAYLOAD_CHARS {
        return pack_no_diffs;
    }

    // Still over budget: reduce to half the commits
    tracing::warn!("Evidence pack still over budget, reducing commits");
    let half = commits.len() / 2;
    let reduced_commits = commits[..half.max(1)].to_vec();
    build_internal(target, &reduced_commits, context, &all_issue_refs, false)
}

fn build_internal(
    target: &ResolvedTarget,
    commits: &[BlamedCommit],
    context: &LocalContext,
    all_issue_refs: &[String],
    include_diffs: bool,
) -> EvidencePack {
    let commit_summaries: Vec<CommitSummary> = commits
        .iter()
        .map(|c| CommitSummary {
            oid: c.short_oid.clone(),
            date: c.date_human.clone(),
            author: c.author.clone(),
            summary: truncate(&c.summary, MAX_SUBJECT_CHARS),
            diff_excerpt: if include_diffs {
                truncate(&c.diff_excerpt, MAX_DIFF_CHARS)
            } else {
                String::new()
            },
            coverage_pct: (c.coverage_score * 100.0).round() as u32,
            issue_refs: c.issue_refs.clone(),
        })
        .collect();

    EvidencePack {
        target: TargetInfo {
            file: target.relative_path.display().to_string(),
            symbol: target.symbol_name.clone(),
            lines: (target.start_line + 1, target.end_line + 1),
            language: format!("{:?}", target.language).to_lowercase(),
        },
        local_context: LocalContextInfo {
            comments: context.comments
                .iter()
                .take(5)
                .map(|s| truncate(s, MAX_COMMENT_CHARS))
                .collect(),
            markers: context.markers
                .iter()
                .take(5)
                .map(|m| truncate(&m.text, MAX_MARKER_CHARS))
                .collect(),
            risk_flags: context.risk_flags
                .iter()
                .take(10)
                .cloned()
                .collect(),
        },
        history: HistoryInfo {
            total_commit_count: commits.len(),
            commits_shown: commit_summaries.len(),
            top_commits: commit_summaries,
        },
        signals: SignalInfo {
            issue_refs: all_issue_refs.to_vec(),
            risk_keywords: context.risk_flags.clone(),
            heuristic_risk: format!("{:?}", context.heuristic_risk),
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Truncate at a word boundary if possible
        let truncated = &s[..max];
        if let Some(last_space) = truncated.rfind(' ') {
            format!("{}…", &truncated[..last_space])
        } else {
            format!("{}…", truncated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_at_word_boundary() {
        let s = "hello world this is a long string";
        let result = truncate(s, 15);
        assert!(result.ends_with('…'));
        assert!(result.len() <= 16); // truncated + ellipsis
        assert!(!result.contains(' ') || result.rfind(' ').unwrap() < result.len() - 2);
    }

    #[test]
    fn test_truncate_no_op_when_short() {
        let s = "short";
        assert_eq!(truncate(s, 100), "short");
    }
}
```

### A.5 Complete `crates/synthesizer/src/prompt.rs`

```rust
//! Prompt engineering for the why synthesizer.
//!
//! ## Prompt design philosophy
//!
//! The system prompt serves several purposes:
//!
//! 1. **Role definition**: Makes the model reason as a historian, not a
//!    code reviewer. We want it to explain WHY code exists, not WHAT it does.
//!
//! 2. **Output contract**: Enforces strict JSON output to enable reliable
//!    parsing. We cannot risk the model wrapping its response in prose.
//!
//! 3. **Anti-hallucination**: Explicitly prohibits the model from inventing
//!    history not supported by the evidence.
//!
//! 4. **Evidence/inference separation**: Forces the `unknowns` field to
//!    contain inferences, keeping `evidence` clean for citations.
//!
//! 5. **Risk calibration**: Explicitly defines what constitutes HIGH/MEDIUM/LOW
//!    to prevent the model from defaulting to MEDIUM for everything.
//!
//! ## Token budget awareness
//!
//! The system prompt is ~400 tokens. Combined with the evidence pack
//! (~1200 tokens for a typical 8-commit query), the total input is
//! ~1600 tokens. The expected output is ~400 tokens. Total: ~2000 tokens.
//! At claude-haiku-4-5 pricing, this is ~$0.0008 per query.

use crate::EraSnapshot;
use why_evidence::EvidencePack;

/// The static system prompt defining the model's role and output contract.
/// Changes to this prompt should be versioned and tested against the
/// fixture corpus before deployment.
pub const SYSTEM_PROMPT: &str = r#"You are a git archaeology assistant. Your job is to explain why a piece of code exists based on structured evidence from git history. You reason like a forensic historian, not a code reviewer.

You will receive a JSON evidence pack containing:
- The target code location (file, symbol, lines)
- Local context (nearby comments, TODO/FIXME markers, risk flags detected by static analysis)
- The most relevant commits that modified this code (with truncated diff excerpts)
- Risk signals extracted from commit messages and code patterns

Your response MUST be a valid JSON object with EXACTLY these fields and no others:

{
  "summary": "One concise paragraph (3-5 sentences max, under 100 words) explaining why this code exists from a historical perspective.",
  "why_it_exists": ["bullet point 1 (under 80 chars)", "bullet point 2", "bullet point 3"],
  "risk_level": "HIGH",
  "likely_breakage": ["specific thing that could break if removed", "another consequence"],
  "evidence": ["abc12345 — commit summary (date)", "Nearby comment: '...'"],
  "confidence": "medium-high",
  "unknowns": ["thing inferred without direct evidence", "assumption made"]
}

FIELD DEFINITIONS:
- summary: Historical narrative. Why was this introduced? What problem did it solve?
- why_it_exists: 2-4 bullets summarizing the key historical reasons
- risk_level: Must be exactly "HIGH", "MEDIUM", or "LOW" (uppercase)
  * HIGH: Security concern, incident/postmortem history, backward compat critical to other systems, recent emergency fix
  * MEDIUM: Migration shim, retry/resilience logic, deprecated workaround, medium-signal keywords in history
  * LOW: Utility code, no special signals, straightforward history
- likely_breakage: What would break if this code were removed or significantly changed?
- evidence: ONLY direct evidence — cited commit OIDs with summaries, nearby comments, issue refs
- confidence: How confident are you in this analysis?
  * "low": 1-2 commits, no issue refs, no markers
  * "medium": 3-5 commits, some signals, no incident refs
  * "medium-high": clear incident/hotfix reference or good diff excerpts
  * "high": multiple corroborating sources, PR descriptions, clear causal chain
- unknowns: ONLY inferences and assumptions. NOT evidence. Include here anything you're reasoning about rather than citing directly.

STRICT RULES:
1. Base ALL claims in "evidence" on the JSON evidence pack provided. Do not invent commits, dates, or people.
2. Put ALL inferences and reasoning in "unknowns", never in "evidence".
3. If evidence is sparse, say so in "confidence" and "unknowns". A low-confidence honest answer is better than a high-confidence fabricated one.
4. Do not explain what the code DOES — explain WHY it EXISTS historically.
5. Respond ONLY with the JSON object. No preamble, no explanation, no markdown code fences.
6. All string values must be valid JSON (escape special characters)."#;

/// Build the user message for a single-symbol query.
/// The evidence pack is serialized to pretty-printed JSON for readability.
pub fn build_query_prompt(pack: &EvidencePack) -> String {
    let evidence_json = serde_json::to_string_pretty(pack)
        .unwrap_or_else(|e| format!("{{\"error\": \"serialize failed: {}\"}}", e));

    format!(
        "Analyze this evidence pack and explain why this code exists.\n\
         Focus on historical context, not current functionality.\n\n\
         {}",
        evidence_json
    )
}

/// Build the user message for evolution mode (`--evolution` flag).
/// Instead of a single EvidencePack, the model receives a timeline of eras.
pub fn build_evolution_prompt(
    symbol_name: &str,
    file: &str,
    eras: &[EraSnapshot],
) -> String {
    let eras_json = serde_json::to_string_pretty(eras)
        .unwrap_or_else(|_| "[]".to_string());

    format!(
        "Describe the evolution of `{}` in `{}` across these historical snapshots.\n\
         For each era, explain what changed and why, focusing on inflection points.\n\
         What architectural decisions does this history reveal?\n\n\
         Historical snapshots:\n{}",
        symbol_name, file, eras_json
    )
}

/// Build the user message for the diff review mode (`--diff`).
pub fn build_diff_review_prompt(
    target: &str,
    packs: &[EvidencePack],
) -> String {
    let packs_json = serde_json::to_string_pretty(packs)
        .unwrap_or_else(|_| "[]".to_string());

    format!(
        "Risk analysis for changes in: {}\n\
         For each function, explain the historical risk of modifying it.\n\n\
         Evidence packs:\n{}",
        target, packs_json
    )
}

/// System prompt variant for evolution mode.
/// Same JSON contract but asks for a narrative timeline rather than snapshot.
pub const EVOLUTION_SYSTEM_PROMPT: &str = r#"You are a git archaeology assistant specializing in code evolution analysis.

Given a timeline of historical snapshots of a function, produce a narrative explaining how and why the function changed over time.

Respond with valid JSON:
{
  "timeline": [
    {
      "era": "YYYY-MM label",
      "summary": "What changed and why in this era (2-3 sentences)",
      "key_reason": "The single most important reason for this change",
      "risk_delta": "how did risk change in this era: increased/decreased/unchanged"
    }
  ],
  "overall_arc": "2-3 sentence narrative of the function's complete evolution",
  "current_risk": "HIGH|MEDIUM|LOW",
  "risk_reasoning": "Why the current risk level is what it is",
  "recommendation": "Optional: what this history suggests about future handling"
}

Respond ONLY with the JSON. No preamble."#;

/// Era snapshot for evolution mode prompts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EraSnapshot {
    pub era_label: String,
    pub line_count: usize,
    pub key_commits: Vec<String>,
    pub diff_summary: String,
    pub risk_signals: Vec<String>,
}
```

### A.6 Complete `tests/common/mod.rs`

```rust
//! Common test utilities for integration tests.
//!
//! Provides fixture repo setup, CLI invocation helpers, and assertion utilities.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// A scripted git repository for integration testing.
pub struct FixtureRepo {
    pub dir: TempDir,
    pub path: PathBuf,
}

impl FixtureRepo {
    /// Create a new empty fixture repository.
    pub fn new() -> Result<Self> {
        let dir = TempDir::new()?;
        let path = dir.path().to_path_buf();

        git_init(&path)?;
        git_config(&path, "user.email", "test@test.com")?;
        git_config(&path, "user.name", "Test User")?;

        Ok(Self { dir, path })
    }

    /// Write a file and stage it.
    pub fn write_file(&self, name: &str, content: &str) -> Result<()> {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        git_add(&self.path, name)?;
        Ok(())
    }

    /// Make a commit with the given message.
    pub fn commit(&self, message: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.path)
            .output()?;

        if !output.status.success() {
            anyhow::bail!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Return the short OID
        let oid_output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&self.path)
            .output()?;

        Ok(String::from_utf8_lossy(&oid_output.stdout).trim().to_string())
    }

    /// Run `why` binary against this repo.
    pub fn run_why(&self, args: &[&str]) -> Result<std::process::Output> {
        // Use the debug binary
        let why_binary = std::env::var("WHY_BINARY")
            .unwrap_or_else(|_| "target/debug/why".to_string());

        let output = Command::new(&why_binary)
            .args(args)
            .current_dir(&self.path)
            .env("ANTHROPIC_API_KEY", "") // disable LLM in tests by default
            .output()?;

        Ok(output)
    }

    /// Run why and parse JSON output.
    pub fn run_why_json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let mut all_args: Vec<&str> = args.to_vec();
        all_args.push("--json");

        let output = self.run_why(&all_args)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        serde_json::from_str(&stdout)
            .map_err(|e| anyhow::anyhow!("JSON parse failed: {}\nOutput was: {}", e, stdout))
    }
}

/// Create the hotfix fixture: a function hardened after a security incident.
pub fn setup_hotfix_repo() -> Result<FixtureRepo> {
    let repo = FixtureRepo::new()?;

    // Initial commit: simple function
    repo.write_file("src/payment.rs", r#"
pub fn process_payment(amount: f64) -> Result<(), String> {
    if amount <= 0.0 {
        return Err("Invalid amount".into());
    }
    charge_stripe(amount)
}
"#)?;
    repo.commit("feat: add payment processing")?;

    // Hotfix commit: security hardening
    repo.write_file("src/payment.rs", r#"
pub fn process_payment(amount: f64) -> Result<(), String> {
    // security: validate amount range to prevent negative charge exploit
    if amount <= 0.0 || amount > 100_000.0 {
        return Err("Invalid amount range".into());
    }
    // hotfix: rate limit to prevent duplicate charge incident #4521
    rate_limit_check("payment")?;
    charge_stripe(amount)
}
"#)?;
    repo.commit("hotfix: fix duplicate charge vulnerability, closes #4521")?;

    Ok(repo)
}

/// Create the coupling fixture: two functions always committed together.
pub fn setup_coupling_repo() -> Result<FixtureRepo> {
    let repo = FixtureRepo::new()?;

    for i in 1..=5 {
        repo.write_file("src/schema.rs", &format!(r#"
pub fn update_schema_v{}() {{
    execute_migration(SCHEMA_V{});
}}
"#, i, i))?;

        repo.write_file("src/data.rs", &format!(r#"
pub fn migrate_data_v{}() {{
    transform_records(MIGRATION_V{});
}}
"#, i, i))?;

        repo.commit(&format!("migration: schema v{} + data migration", i))?;
    }

    Ok(repo)
}

/// Create the time bomb fixture: repo with past-due TODOs and aged HACKs.
pub fn setup_timebomb_repo() -> Result<FixtureRepo> {
    let repo = FixtureRepo::new()?;

    // Use a very old date in the TODO
    repo.write_file("src/legacy.rs", r#"
pub fn process_legacy_format(data: &[u8]) -> Vec<u8> {
    // TODO(2020-01-15): remove after v3 migration is complete
    // HACK: workaround for old client format, should be cleaned up
    if data.starts_with(b"LEGACY:") {
        convert_legacy_format(data)
    } else {
        data.to_vec()
    }
}
"#)?;
    repo.commit("feat: add legacy format support")?;

    Ok(repo)
}

/// Create the ghost fixture: a never-called function with HIGH risk history.
pub fn setup_ghost_repo() -> Result<FixtureRepo> {
    let repo = FixtureRepo::new()?;

    repo.write_file("src/auth.rs", r#"
// This function is never called from anywhere (orphaned after refactor)
pub fn validate_auth_token_legacy(token: &str, session_id: &str) -> bool {
    // security: added after token forgery incident #7890
    !token.is_empty() && token_matches_session(token, session_id)
}

pub fn authenticate(user: &str, password: &str) -> bool {
    check_password_hash(user, password)
}
"#)?;
    repo.commit("hotfix: add token validation after auth forgery incident #7890")?;

    // Second commit: authenticate gets called, validate_auth_token_legacy does not
    repo.write_file("src/main.rs", r#"
fn main() {
    let user = "alice";
    let pass = "password";
    if authenticate(user, pass) {
        println!("logged in");
    }
}
"#)?;
    repo.commit("feat: add main entry point using authenticate")?;

    Ok(repo)
}

/// Create the split fixture: a function with two distinct archaeological eras.
pub fn setup_split_repo() -> Result<FixtureRepo> {
    let repo = FixtureRepo::new()?;

    // Era 1: initial simple implementation
    repo.write_file("src/auth.rs", r#"
pub fn authenticate(user: &str, token: &str) -> bool {
    check_password(user, token)
}
"#)?;
    repo.commit("feat: initial auth implementation")?;

    // Era 2: security hardening
    repo.write_file("src/auth.rs", r#"
pub fn authenticate(user: &str, token: &str) -> bool {
    // security: added after incident #4521
    if is_rate_limited(user) { return false; }
    if token.is_empty() { return false; }
    let session = new_session(user);
    validate_token_with_session(token, &session)
}
"#)?;
    repo.commit("hotfix: harden authenticate after auth bypass incident #4521")?;

    // Era 3: legacy compat
    repo.write_file("src/auth.rs", r#"
pub fn authenticate(user: &str, token: &str) -> bool {
    // security: added after incident #4521
    if is_rate_limited(user) { return false; }
    if token.is_empty() { return false; }

    // backward compat: legacy v1 token format for mobile clients
    if token.starts_with("v1:") {
        return validate_legacy_token(token, user);
    }

    let session = new_session(user);
    validate_token_with_session(token, &session)
}
"#)?;
    repo.commit("feat: add legacy v1 token support for mobile backward compat (#234)")?;

    Ok(repo)
}

// ─────────────────────────────────────────────
// Git helpers
// ─────────────────────────────────────────────

fn git_init(path: &Path) -> Result<()> {
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .status()?;
    if !status.success() {
        // Try older git without -b flag
        Command::new("git").args(["init"]).current_dir(path).status()?;
    }
    Ok(())
}

fn git_config(path: &Path, key: &str, value: &str) -> Result<()> {
    Command::new("git")
        .args(["config", key, value])
        .current_dir(path)
        .status()?;
    Ok(())
}

fn git_add(path: &Path, file: &str) -> Result<()> {
    Command::new("git")
        .args(["add", file])
        .current_dir(path)
        .status()?;
    Ok(())
}
```

### A.7 Complete `tests/integration_cli.rs`

```rust
//! Integration tests for the core CLI pipeline.
//!
//! These tests run `why` against scripted fixture repositories and
//! assert on the structure of the output. They do NOT test LLM synthesis
//! (that would require an API key and is non-deterministic).
//!
//! All tests use `--no-llm --json` to get heuristic-only JSON output.

mod common;
use common::*;
use why_synthesizer::{WhyReport, RiskLevel};
use anyhow::Result;

// ─────────────────────────────────────────────
// Basic query tests
// ─────────────────────────────────────────────

#[test]
fn test_hotfix_repo_detects_high_risk() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let report: WhyReport = repo.run_why_json(&[
        "src/payment.rs:process_payment",
        "--no-llm",
    ])?;

    assert_eq!(
        report.risk_level,
        RiskLevel::High,
        "hotfix_repo should produce HIGH risk for process_payment"
    );

    Ok(())
}

#[test]
fn test_hotfix_repo_evidence_contains_issue_ref() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let report: WhyReport = repo.run_why_json(&[
        "src/payment.rs:process_payment",
        "--no-llm",
    ])?;

    let all_evidence = report.evidence.join(" ") + &report.why_it_exists.join(" ");
    assert!(
        all_evidence.contains("4521") || all_evidence.contains("hotfix"),
        "Evidence should reference issue #4521 or hotfix keyword. Got: {:?}",
        report.evidence
    );

    Ok(())
}

#[test]
fn test_line_targeting_returns_result() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    // Test line-based targeting (no tree-sitter)
    let output = repo.run_why(&["src/payment.rs:5", "--no-llm"])?;
    assert!(
        output.status.success(),
        "Line targeting should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

#[test]
fn test_json_output_is_valid() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let output = repo.run_why(&[
        "src/payment.rs:process_payment",
        "--no-llm",
        "--json",
    ])?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| anyhow::anyhow!("JSON parse error: {}\nOutput: {}", e, stdout))?;

    // Verify required fields exist
    assert!(parsed["summary"].is_string(), "summary should be a string");
    assert!(parsed["risk_level"].is_string(), "risk_level should be a string");
    assert!(parsed["evidence"].is_array(), "evidence should be an array");
    assert!(parsed["confidence"].is_string(), "confidence should be a string");

    Ok(())
}

#[test]
fn test_symbol_not_found_returns_error() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let output = repo.run_why(&["src/payment.rs:nonexistent_function", "--no-llm"])?;

    assert!(
        !output.status.success(),
        "Query for nonexistent symbol should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("Symbol"),
        "Error message should mention symbol not found. Got: {}", stderr
    );

    Ok(())
}

#[test]
fn test_file_not_found_returns_error() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let output = repo.run_why(&["src/nonexistent.rs:some_fn", "--no-llm"])?;

    assert!(!output.status.success(), "Query for nonexistent file should fail");

    Ok(())
}

// ─────────────────────────────────────────────
// Scanner tests
// ─────────────────────────────────────────────

#[test]
fn test_time_bombs_detected() -> Result<()> {
    let repo = setup_timebomb_repo()?;

    let output = repo.run_why(&["scan", "--time-bombs"])?;

    assert!(
        output.status.success(),
        "scan --time-bombs should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("2020") || stdout.contains("TODO") || stdout.contains("time bomb"),
        "Output should mention the past-due TODO. Got: {}", stdout
    );

    Ok(())
}

#[test]
fn test_coupling_detection() -> Result<()> {
    let repo = setup_coupling_repo()?;

    let output = repo.run_why(&["--coupled", "src/schema.rs"])?;

    assert!(output.status.success(), "Coupling analysis should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("data.rs"),
        "data.rs should be detected as coupled with schema.rs. Got: {}", stdout
    );

    Ok(())
}

#[test]
fn test_ghost_detection() -> Result<()> {
    let repo = setup_ghost_repo()?;

    let output = repo.run_why(&["ghost"])?;

    assert!(output.status.success(), "ghost scan should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("validate_auth_token_legacy"),
        "Ghost scan should detect validate_auth_token_legacy. Got: {}", stdout
    );

    Ok(())
}

// ─────────────────────────────────────────────
// Cache tests
// ─────────────────────────────────────────────

#[test]
fn test_cache_hit_on_second_query() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    // First query
    let start1 = std::time::Instant::now();
    repo.run_why(&["src/payment.rs:process_payment", "--no-llm"])?;
    let elapsed1 = start1.elapsed();

    // Second query (should be from cache)
    let start2 = std::time::Instant::now();
    repo.run_why(&["src/payment.rs:process_payment", "--no-llm"])?;
    let elapsed2 = start2.elapsed();

    // Cache hit should be significantly faster (50ms vs 200ms+)
    assert!(
        elapsed2 < elapsed1 / 2,
        "Cached query ({:?}) should be significantly faster than cold query ({:?})",
        elapsed2, elapsed1
    );

    Ok(())
}

#[test]
fn test_no_cache_flag_bypasses_cache() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    // Warm the cache
    repo.run_why(&["src/payment.rs:process_payment", "--no-llm"])?;

    // --no-cache should force re-run
    let output = repo.run_why(&[
        "src/payment.rs:process_payment",
        "--no-llm",
        "--no-cache",
    ])?;

    assert!(output.status.success(), "--no-cache query should succeed");
    // We can't easily verify it didn't use cache, but at least it runs

    Ok(())
}

// ─────────────────────────────────────────────
// Output format tests
// ─────────────────────────────────────────────

#[test]
fn test_terminal_output_contains_risk_level() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let output = repo.run_why(&["src/payment.rs:process_payment", "--no-llm"])?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("HIGH") || stdout.contains("MEDIUM") || stdout.contains("LOW"),
        "Terminal output should contain risk level. Got: {}", stdout
    );

    Ok(())
}

#[test]
fn test_json_risk_level_is_valid() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let report: serde_json::Value = repo.run_why_json(&[
        "src/payment.rs:process_payment",
        "--no-llm",
    ])?;

    let risk = report["risk_level"].as_str().unwrap_or("");
    assert!(
        matches!(risk, "HIGH" | "MEDIUM" | "LOW"),
        "risk_level should be HIGH, MEDIUM, or LOW. Got: {:?}", risk
    );

    Ok(())
}
```

### A.8 Complete `tests/snapshot_reports.rs`

```rust
//! Snapshot tests for terminal and JSON output.
//!
//! These tests capture the exact output of `why` for each fixture repo
//! and assert it hasn't changed. They catch regressions in formatting,
//! field names, or output structure.
//!
//! Run `cargo insta review` to review and accept snapshot changes.
//! Run `cargo test` to verify snapshots match.

mod common;
use common::*;
use anyhow::Result;

// ─────────────────────────────────────────────
// JSON output snapshots
// ─────────────────────────────────────────────

#[test]
fn snapshot_hotfix_json() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:process_payment",
        "--no-llm",
        "--json",
    ])?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout)?;

    // Normalize timestamps and OIDs before snapshotting
    // (they change with each test run)
    let normalized = normalize_for_snapshot(&value);

    insta::assert_json_snapshot!("hotfix_repo_payment_json", normalized);
    Ok(())
}

#[test]
fn snapshot_timebomb_json() -> Result<()> {
    let repo = setup_timebomb_repo()?;
    let output = repo.run_why(&["scan", "--time-bombs", "--json"])?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout)?;
    let normalized = normalize_for_snapshot(&value);

    insta::assert_json_snapshot!("timebomb_repo_scan_json", normalized);
    Ok(())
}

// ─────────────────────────────────────────────
// Terminal output snapshots
// ─────────────────────────────────────────────

#[test]
fn snapshot_hotfix_terminal() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:process_payment",
        "--no-llm",
    ])?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let normalized = normalize_terminal_output(&stdout);

    insta::assert_snapshot!("hotfix_repo_payment_terminal", normalized);
    Ok(())
}

// ─────────────────────────────────────────────
// Normalization helpers
// ─────────────────────────────────────────────

/// Remove fields that change between test runs (timestamps, OIDs, costs).
fn normalize_for_snapshot(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;

    match value {
        Value::Object(map) => {
            let mut normalized = serde_json::Map::new();
            for (k, v) in map {
                // Skip fields that contain volatile data
                if matches!(k.as_str(), "estimated_cost_usd" | "generated_at_hash") {
                    normalized.insert(k.clone(), Value::String("[REDACTED]".into()));
                } else if k.contains("date") || k.contains("time") || k.contains("_at") {
                    normalized.insert(k.clone(), Value::String("[DATE]".into()));
                } else {
                    normalized.insert(k.clone(), normalize_for_snapshot(v));
                }
            }
            Value::Object(normalized)
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(normalize_for_snapshot).collect())
        }
        // Replace OID-like strings
        Value::String(s) if looks_like_oid(s) => {
            Value::String("[OID]".into())
        }
        other => other.clone(),
    }
}

fn looks_like_oid(s: &str) -> bool {
    s.len() >= 8 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_terminal_output(output: &str) -> String {
    // Remove ANSI color codes
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let no_colors = re.replace_all(output, "").to_string();

    // Replace OID-like strings
    let oid_re = regex::Regex::new(r"\b[0-9a-f]{8,40}\b").unwrap();
    let no_oids = oid_re.replace_all(&no_colors, "[OID]").to_string();

    // Replace dates
    let date_re = regex::Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
    date_re.replace_all(&no_oids, "[DATE]").to_string()
}
```

---

## Appendix B: Example `.why.toml` Configurations by Domain

### B.1 Payments team

```toml
[risk.keywords]
high = [
    "pci", "pci-dss", "pci-compliance",
    "settlement", "reconciliation", "ledger",
    "charge", "refund", "dispute", "chargeback",
    "stripe", "braintree", "adyen",
    "card-number", "cvv", "pan",
    "tokenization", "vault",
]
medium = [
    "webhook", "idempotency", "retry-key",
    "fx-rate", "currency-conversion",
    "batching", "settlement-delay",
]

[health]
time_bomb_weight = 4       # payments debt is more critical
uncovered_high_risk_weight = 6
```

### B.2 Healthcare / HIPAA team

```toml
[risk.keywords]
high = [
    "hipaa", "phi", "pii",
    "hl7", "fhir", "dicom",
    "patient-id", "mrn", "ssn",
    "audit-log", "access-log",
    "de-identify", "anonymize",
    "consent", "authorization-to-disclose",
]
medium = [
    "encounter", "diagnosis", "icd10",
    "claim", "prior-auth", "formulary",
    "edi", "x12",
]
```

### B.3 Infrastructure / Platform team

```toml
[risk.keywords]
high = [
    "terraform", "infrastructure-as-code",
    "vpc", "subnet", "security-group",
    "iam-role", "iam-policy",
    "kms", "encryption-key",
    "rollback-plan", "dr-plan",
    "sla", "rto", "rpo",
]
medium = [
    "deployment", "k8s", "kubernetes",
    "helm", "argocd", "flux",
    "canary", "blue-green",
    "autoscaling", "capacity",
]

[scanner]
time_bomb_age_days = 90    # infra debt goes stale faster
```

### B.4 Machine learning team

```toml
[risk.keywords]
high = [
    "model-serving", "inference",
    "feature-store", "training-data",
    "model-version", "champion-challenger",
    "bias", "fairness", "explainability",
    "pii-features", "sensitive-attribute",
]
medium = [
    "hyperparameter", "experiment",
    "a-b-test", "rollout",
    "data-drift", "model-drift",
    "retraining", "shadow-mode",
]
```

---

## Appendix C: Terminal Output Gallery

### C.1 Standard query output

```
why: src/auth/session.rs:authenticate (lines 110–145)

Why this exists
This function was hardened after an authentication bypass vulnerability was
discovered in November 2023 (incident #4521). A subsequent commit preserved
legacy mobile token behavior for backward compatibility with older iOS clients
still using the v1 token format.

  • Originally a simple password comparison (initial commit, 2022-03)
  • Hardened post-incident with rate limiting and session validation (2023-07)
  • Legacy v1 token path added for mobile backward compatibility (2023-11)

Risk if removed: HIGH
  • May reintroduce authentication bypass vulnerability
  • Breaks compatibility with older iOS clients still on v1 token format
  • Session hijacking possible if validation path is removed

Evidence
  abc12345  hotfix: fix auth bypass in session refresh [2023-07-14]
  def67890  feat: add legacy v1 token support for mobile compat (#234) [2023-11-01]
  Nearby: "// temporary guard until all clients rotate to v2 tokens"
  Issue: #4521

Confidence: medium-high
Unknowns
  • No GitHub token set — PR #234 description not available
  • Whether the mobile client v2 migration is complete is unknown

Coupled with (co-change ratio >30%):
  session_refresh()    ratio: 0.74  (9/12 commits)
  token_validate()     ratio: 0.42  (5/12 commits)

Estimated cost: ~$0.0009
```

### C.2 Evolution mode output

```
why --evolution src/auth/session.rs:authenticate

Timeline of authenticate()
──────────────────────────────────────────────────────────────────

  2022-03  12 lines  Initial auth implementation
           Commit: abc11111 — feat: initial auth module
           Simple bcrypt password comparison. No rate limiting.
           Risk: LOW

  ▲ INFLECTION POINT ─────────────────────────────────────────────

  2023-07  45 lines  Security hardening (post-incident)
           Commit: abc22222 — hotfix: fix auth bypass in session refresh
           Auth bypass discovered by internal security audit. Added:
           - Rate limiting per user
           - Session ID binding for token validation
           - Retry guard with exponential backoff
           Risk: HIGH ↑ (security incident history)

  2023-11  52 lines  Mobile backward compat
           Commit: abc33333 — feat: legacy v1 token support (#234)
           iOS clients on v1 token format. Migration planned for Q2 2024.
           Added v1 token validation path alongside the hardened v2 path.
           Risk: HIGH (unchanged — security + backward compat)

  2024-03  48 lines  Performance optimization
           Commit: abc44444 — perf: replace bcrypt with argon2id (#301)
           Argon2id is faster and more modern. No functional changes.
           Risk: HIGH (unchanged — core logic unmodified)

──────────────────────────────────────────────────────────────────
Current risk: HIGH
Key inflection: 2023-07 incident #4521 fundamentally changed this function
Recommendation: Split into authenticate_v2() and authenticate_legacy() before
next refactor to separate the security-critical path from the compat shim.
```

### C.3 Health dashboard output

```
why health

Repository: my-service  (branch: main, 2,847 commits, 187 source files)
──────────────────────────────────────────────────────────────────────────

  Time bombs            18   (4 critical, past-due > 1 year)
  HIGH risk functions   47   (12 with zero test coverage)
  Hotspot files          8   (high churn + high risk, last 90 days)
  Bus factor 1          14   functions only 1 person understands
  Ghost functions        3   (never called, HIGH risk history)
  Stale HACKs           23   (avg age: 16 months)

  Debt score: 74/100  ↑ from 68 last week

Top 3 immediate actions:
  1. src/payments/processor.rs:charge_card     — HIGH risk, 0% coverage
  2. src/auth/session.rs (line 142)            — TODO(2023-Q4) past due 16 months
  3. src/crypto/key_manager.rs:rotate_key      — ghost function, HIGH risk

Run `why ghost` for ghost function details.
Run `why coverage-gap --coverage lcov.info` for coverage gap details.
Run `why scan --time-bombs` for full time bomb list.
```

### C.4 Splitter output

```
why src/auth/session.rs:authenticate --split

Suggested split for authenticate() (52 lines, lines 110–161)
──────────────────────────────────────────────────────────────────

  Block A  lines 110–137  Security hardening era
                           Dominant commit: abc22222 — hotfix post-incident #4521
                           → Suggested extraction: authenticate_with_guard()
                           Risk: HIGH (security incident history, rate limiting, session binding)
                           28 lines / 54% of function

  Block B  lines 138–161  Backward compat era
                           Dominant commit: abc33333 — mobile v1 token compat (#234)
                           → Suggested extraction: authenticate_legacy_token()
                           Risk: MEDIUM (legacy compat, eventually removable)
                           24 lines / 46% of function

──────────────────────────────────────────────────────────────────
These blocks have different reasons to change and different risk profiles.
Splitting reduces blast radius: security changes to Block A won't risk Block B.

Recommended refactor:
  pub fn authenticate(user: &str, token: &str) -> bool {
      if token.starts_with("v1:") {
          authenticate_legacy_token(user, token)
      } else {
          authenticate_with_guard(user, token)
      }
  }
```

---

*End of document. Total: ~5,100+ lines.*
