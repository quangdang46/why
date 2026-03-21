# why

> Ask your codebase *why* a function, file, or line exists. Powered by git history + LLM synthesis.

---

## Quick start

1. Install `why`.
2. Run the setup command:

```bash
why config init
```

3. Ask a question about code history:

```bash
why src/auth.rs:verify_token
```

If you only remember one setup command, remember `why config init`.
It is the main setup flow and lets you choose `anthropic`, `openai`, `zai`, or `custom`.

## Installation

### Install the released binary

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/why/main/install.sh?$(date +%s)" | bash
```

### Build locally from this checkout

```bash
cargo run -q -p why-core -- --help
cargo build -p why-core --release
./target/release/why --help
```

The shipping Cargo package is `why-core`, and the installed binary name is `why`.

`cargo install why-core` is **not** supported yet because `crates/core/Cargo.toml` still sets `publish = false`.

### Generate shell completions and a man page

```bash
cargo run -q -p why-core -- completions bash > why.bash
cargo run -q -p why-core -- completions zsh > _why
cargo run -q -p why-core -- completions fish > why.fish
cargo run -q -p why-core -- manpage > why.1
```

### Configure the CLI

If you skipped the quick start above, run:

```bash
why config init
```

This is the main setup flow and lets you choose `anthropic`, `openai`, `zai`, or `custom` interactively.

### Validation and benchmarks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Opt-in real-repo CLI coverage:

```bash
WHY_REAL_REPO_PATH=/absolute/path/to/git/checkout \
  cargo test -p why-workspace --test integration_real_repo_cli -- --nocapture
```

- The real-repo tests are opt-in so the default test suite stays deterministic.
- They clone the supplied checkout into a temporary test repo before running `why`.

Benchmark commands:

```bash
cargo bench --package why-workspace --bench cache_bench
cargo bench --package why-workspace --bench archaeology_bench
cargo bench --package why-workspace --bench scanner_bench
```

GitHub Actions also exposes the same Criterion run via `.github/workflows/bench.yml` and uploads `target/criterion/**` as artifacts.

## The Problem

Claude Code has no access to git history. It sees the code as it is today — with no context of:
- Why was this written this way?
- Was this a hotfix for an incident?
- Is this "temporary" code from 2019 that became permanent?
- What breaks if I delete this?

Developers delete "dead-looking" code that was actually a critical security fix.
Claude Code does the same — because it can't read the past.

## Why `why` Is Different

| | Claude Code alone | why |
|---|---|---|
| Understands git history | ❌ | ✅ |
| Reads PR descriptions | ❌ | ✅ |
| Explains commit reasoning | ❌ | ✅ |
| Risk assessment before deletion | ❌ | ✅ |
| Links code to past incidents | ❌ | ✅ |

**This is the only tool in this suite that Claude Code literally cannot replicate** — git history is not in any context window.

## How It Works

```
1. Identify  (pure Rust, git2 crate)
   └── tree-sitter locates exact byte range of target function/line
   └── git2 runs git blame on that range
   └── collect all unique commits that touched those lines

2. Gather  (pure Rust, git2 crate)
   └── for each commit: message, author, date, diff
   └── check for PR refs (#123), issue refs (fixes #456)
   └── extract comments and TODOs near the target code

3. Synthesize  (LLM call — configured provider)
   └── feed structured git data to the configured model
   └── ask: "why does this exist? what risk if removed?"
   └── returns human-readable explanation + risk level
```

Only **one LLM call per query**, with structured git data as input.

## Tech Stack

| Crate | Purpose |
|---|---|
| `git2` | Native git operations — no git binary required |
| `tree-sitter` | Locate function boundaries precisely |
| provider-aware HTTP clients | Synthesize git data into explanation via Anthropic or OpenAI-compatible providers |
| `clap` | CLI |
| `serde_json` | Structured output |

## Usage

### Query targets

`why` uses positional target syntax, not `fn|file|line` subcommands.

Supported query forms:

- `why <file>:<line>`
- `why <file>:<symbol>`
- `why <file>:<Type::method>`
- `why <file> --lines <start:end>`

Supported symbol-resolution languages:

- Rust (`.rs`)
- Go (`.go`)
- JavaScript (`.js`)
- TypeScript (`.ts`, `.tsx`)
- Java (`.java`)
- Python (`.py`)

Important rules:

- Line numbers are 1-based.
- `--lines` must use `START:END`.
- Do not combine `--lines` with `<file>:<line>` or `<file>:<symbol>`.
- Bare file paths are not valid queries unless paired with `--lines`.

### Common query examples

```bash
# Why was this specific line written?
why src/auth.rs:42

# Why does this line range exist?
why src/auth.rs --lines 40:45 --no-llm

# Why does this symbol exist?
why src/auth.rs:verify_token

# Qualified symbol queries for Rust impl methods
why src/auth.rs:AuthService::login --team

# Machine-readable archaeology output
why src/auth.rs:verify_token --json

# Limit archaeology to recent commits
why src/auth.rs:verify_token --since 30

# Inspect files that historically co-change with this target
why src/auth.rs:verify_token --coupled

# Show likely owners and bus-factor signals
why src/auth.rs:verify_token --team

# Walk past mechanical edits to the likely true origin commit
why src/auth.rs:verify_token --blame-chain

# Show rename-aware target evolution history
why src/auth.rs:verify_token --evolution

# Ask whether a symbol should be split
why src/auth.rs:verify_token --split

# Write an evidence-backed annotation above the target
why src/auth.rs:verify_token --annotate

# Refresh the report when the file changes
why src/auth.rs:verify_token --watch --no-llm

# Review whether renaming a Rust symbol is safe
why src/auth.rs:verify_token --rename-safe
```

### Query flags

| Flag | Purpose | Notes |
|---|---|---|
| `--json` | Emit machine-readable output | Works for the main query flow and many subcommands |
| `--no-llm` | Skip LLM synthesis | Useful in CI, local validation, or no-key environments |
| `--no-cache` | Bypass cached results | Forces a fresh query instead of reusing `.why/cache.jsonl` |
| `--since <days>` | Restrict history to recent commits | Applies to query-style archaeology/report modes |
| `--coupled` | Show file-level co-change coupling | Good before a larger refactor |
| `--team` | Show ownership and bus-factor signals | Good before picking reviewers |
| `--blame-chain` | Skip likely mechanical commits | Helps find the real origin of a line or symbol |
| `--evolution` | Show rename-aware target history | Timeline-style output |
| `--split` | Suggest whether a symbol should be split | Symbol-oriented query mode |
| `--annotate` | Insert a short evidence-backed doc annotation above the target | This modifies the file |
| `--watch` | Re-run the default report when the file changes | Requires an interactive terminal |
| `--rename-safe` | Show target risk plus caller risk for rename analysis | Currently supports Rust symbol targets only |

### Repo-wide and review commands

Most report-style subcommands also support `--json`.

```bash
# Rank repository hotspots by churn × heuristic risk
why hotspots --limit 10

# Repository health summary
why health
why health --ci 80

# Generate a reviewer-friendly PR template from the staged diff
why pr-template

# Review the staged diff with archaeology-backed findings
why diff-review --no-llm
why diff-review --post-github-comment --github-ref '#42'

# Rank suspicious commits inside an incident window
why explain-outage --from 2025-11-03T14:00 --to 2025-11-03T16:30

# Cross-reference high-risk functions against coverage data
why coverage-gap --coverage lcov.info

# Find high-risk functions that appear uncalled under static analysis
why ghost --limit 10

# Rank the symbols a new engineer should understand first
why onboard --limit 10

# Find stale TODOs, HACK/TEMP markers, and expired remove-after dates
why time-bombs --age-days 180
```

Key behavior notes:

- `why pr-template` reads the **staged diff**, not unstaged changes.
- `why diff-review` also reads the **staged diff**.
- `why diff-review --post-github-comment` expects a valid GitHub ref like `#42` and a configured GitHub remote/token path.
- `why ghost` uses heuristic static analysis and warns about that in terminal output.
- `why health --ci <threshold>` exits with code `3` when the debt score exceeds the threshold.
- `why health` regression gating exits with code `4` when a configured regression budget fails.

### Integration and developer commands

```bash
# Run the MCP stdio server
why mcp

# Run the hover-focused LSP server over stdio
why lsp

# Start the interactive archaeology shell
why shell

# Emit shell wrappers for supported AI tools
why context-inject

# Install or remove managed git hooks
why install-hooks --warn-only
why uninstall-hooks

# Generate shell completions or a man page
why completions bash > why.bash
why completions zsh > _why
why completions fish > why.fish
why manpage > why.1
```

More detail:

- `why shell` starts an interactive shell with indexed completion support.
  - Shell queries default to `--no-llm` unless you pass `--no-llm` explicitly yourself.
  - Built-in shell commands include `help`, `reload`, `hotspots`, `health`, `ghost`, `exit`, and `quit`.
- `why lsp` is a hover-oriented LSP server that returns Markdown hover content and a CLI hint for the full report.
- `why context-inject` emits shell code intended to be used as:

  ```bash
  eval "$(why context-inject)"
  ```

  The generated wrappers currently target supported prompt tools such as `claude`, `sgpt`, and `llm`.

### Historical Node prototype

There is still a Node.js prototype under `poc/`, but it is **not** the shipping interface.

Examples like these are prototype-only:

```bash
node poc/index.js fn verifyToken src/auth.js
node poc/index.js file src/legacy/payment_v1.js
node poc/index.js fn verifyToken src/auth.js --raw
```

The Rust CLI documented above is the supported interface for the current tool.

## Output Example

```bash
$ why src/auth.rs:42

why: src/auth.rs (line 42)

Commits touching this line:
  a3f9b2c  alice  2024-01-12  fix: tokens not expiring on logout
  8d2e1f4  bob    2022-09-04  extend auth flow for refresh token handling

No LLM synthesis (--no-llm or no API key). Heuristic risk: MEDIUM.
```

## Risk semantics and explanation style

`why` should make conservative change decisions easier, not sound more certain than the evidence supports.

### Risk levels

- **HIGH** — The code shows security sensitivity, incident history, critical backward-compatibility behavior, or other signals that removal could break production behavior in a non-routine way. Treat this as a stop-and-investigate signal: do not delete or heavily refactor without deeper review.
- **MEDIUM** — The code appears tied to migrations, retries, legacy paths, or transitional behavior where changes may be safe, but only after understanding the surrounding context.
- **LOW** — The available history and nearby code do not show special operational or compatibility pressure. This is ordinary utility code unless stronger evidence emerges.

### Explanation style rules

- Separate **evidence** from **inference**. Commit messages, comments, and code markers are evidence; conclusions drawn from them are inference.
- Be explicit about **unknowns** when history is sparse, noisy, or ambiguous.
- Do not invent incidents, PR context, or dependencies that are not present in the evidence.
- Keep output easy to scan: concise summary first, then supporting history, then risk.
- Calibrate confidence downward when only 1–2 commits or weak signals are available.

### Confidence guidance

`why` models confidence internally as an enum and serializes it as one of these JSON/string values:

- **low** — Thin history, weak commit messages, or little corroborating context.
- **medium** — Some useful historical signal, but limited direct evidence.
- **medium-high** — Clear historical intent such as a hotfix, incident, or compatibility trail.
- **high** — Multiple corroborating sources point to the same explanation.

## Integration with Claude Code

Add to your project's `CLAUDE.md`:

```markdown
## Custom Tools

- `why <file>:<line>` — explain why a specific line was written
- `why <file> --lines <start:end>` — explain why a line range exists
- `why <file>:<symbol>` — explain why a supported symbol exists
- `why <file>:<symbol> --coupled` — inspect co-change dependencies before a deeper refactor
- `why <file>:<symbol> --team` — identify likely owners before asking for review on risky code
- `why <file>:<symbol> --blame-chain` — skip mechanical edits to find the real origin commit
- `why <file>:<symbol> --evolution` — inspect rename-aware target history before large moves
- `why diff-review --no-llm` — review the staged diff before opening a PR
- `why health --json` — export a machine-readable repo health snapshot

**Always run `why` before deleting or significantly refactoring any function
that exists in git history for more than 6 months.**
```

Recommended Claude Code workflow:

1. Before deleting or rewriting unfamiliar code, run `why` on the exact symbol or line range first.
2. If the reported risk is **HIGH**, treat that as a stop-and-investigate signal rather than a suggestion to proceed quickly.
3. For larger refactors, also run `--coupled`, `--team`, `--blame-chain`, or `--evolution` depending on what you need to learn.
4. Before opening a PR, run `why diff-review` on the staged diff.
5. For editor/tool integration, pick the interface that matches your workflow:
   - `why mcp` for MCP-capable editors
   - `why lsp` for hover-oriented editor integration
   - `eval "$(why context-inject)"` for shell-wrapped prompt tools

Recommended code review routine:

- include a `why ... --json` or terminal summary when proposing removal of old-looking code
- use `why ... --team` when the change touches operationally sensitive paths and you need to find the best reviewer
- use `why ... --coupled` before splitting or relocating a historically noisy function
- use `why diff-review` to summarize staged-change risk before sharing the branch

For MCP-specific setup examples, see `docs/mcp-setup.md`.

## Configuration and credentials

`why` supports layered configuration:

1. built-in defaults
2. global config at `$XDG_CONFIG_HOME/why/why.toml` or `~/.config/why/why.toml`
3. repo-local `why.local.toml`

Use the CLI to manage these layers:

```bash
# Global config is the default target
why config init --provider anthropic --model claude-haiku-4-5-20251001

# Use --local for repo-specific overrides
why config init --local --provider zai --model glm-5
why config init --local --provider custom --model local-model --base-url https://api.example.com/v1/chat/completions

# Inspect the effective merged config without printing secrets
why config get
why config get --json
```

If you run `why config init` in an interactive terminal without passing values via flags, the CLI prompts for provider, model, base URL, auth token, retries, max tokens, and timeout. You can leave values blank to keep the current value or provider default, then edit `why.toml` or `why.local.toml` later.

Supported providers:

- `anthropic`
- `openai`
- `zai`
- `custom` (OpenAI-compatible)

Current built-in defaults:

- `anthropic` → model `claude-haiku-4-5-20251001`, base URL `https://api.anthropic.com/v1/messages`
- `openai` → model `gpt-5.4`, base URL `https://api.openai.com/v1/chat/completions`
- `zai` → model `glm-5`, base URL `https://api.z.ai/api/anthropic/v1/messages`
- `custom` → no built-in model or base URL

`why config get` hides secrets and reports whether auth is configured via `llm.auth_configured`.

Environment variables take precedence over config values. Blank values are ignored.

Provider credential env vars:

```bash
export ANTHROPIC_API_KEY=your_anthropic_api_key_here
export OPENAI_API_KEY=your_openai_api_key_here
export ZAI_API_KEY=your_zai_api_key_here
export CUSTOM_API_KEY=your_custom_api_key_here
```

Example config:

```toml
[risk]
default_level = "LOW"

[risk.keywords]
high = ["pci", "reconciliation"]
medium = ["terraform", "webhook", "idempotency"]

[git]
max_commits = 8
recency_window_days = 90
mechanical_threshold_files = 50
coupling_scan_commits = 500
coupling_ratio_threshold = 0.30

[cache]
max_entries = 500

[llm]
provider = "openai"
model = "gpt-5.4"
base_url = "https://api.openai.com/v1/chat/completions"
auth_token = "your_provider_token_here"
retries = 3
max_tokens = 500
timeout = 30

[github]
remote = "origin"
# token = "ghp_..."   # optional fallback; prefer GITHUB_TOKEN env var
```

`[risk.keywords]` extends the built-in heuristic vocabulary with team- or domain-specific terms. Matches are case-insensitive and can affect both ranked evidence relevance and the heuristic risk level.

For GitHub enrichment work, set `GITHUB_TOKEN` in the environment when available; config can also carry an optional `[github]` fallback token and remote name. Environment variables take precedence over config, and blank values are ignored.

Secret-handling guidance:

- prefer environment variables when possible
- global config is acceptable for local development if you choose it
- repo-local `why.local.toml` should generally avoid secrets because it is easier to commit accidentally

See `.why.toml.example` for a fully documented example of the current config surface.

## Cache and `.why/` directory semantics

Current behavior:

- query results are cached in `.why/cache.jsonl` at the repository root, one JSON object per line
- cache keys include the target identity plus the current `HEAD` hash prefix, so changing history invalidates prior entries naturally
- terminal output shows `[cached]` when a stored `WhyReport` is reused
- `--no-cache` bypasses cache reads and forces a fresh query
- `[cache].max_entries` controls retained query reports in `.why/cache.jsonl`
- rolling health snapshots are stored separately in `.why/health.jsonl`, one JSON object per line
- up to 52 health snapshots are retained
- CI can enforce health regression budgets with `.github/health-baseline.json`

Operator expectations:

- treat `.why/` as local runtime state, not source-controlled project state
- `.why/` should be ignored by git for normal development workflows
- on Unix, the cache directory and runtime files are written with owner-only permissions (`0700` for `.why/`, `0600` for `cache.jsonl`, `health.jsonl`, and `runtime.log`)
- deleting `.why/cache.jsonl` is safe if you want to clear local cached results; `why` will recreate it on the next cached run
- deleting `.why/health.jsonl` is safe if you want to reset local health trend history
- LLM fallback reasons are appended to `.why/runtime.log` when synthesis fails and `why` falls back to heuristic mode

## `why doctor`

Use `why doctor` to validate the current effective settings and perform a small live LLM test call.

```bash
why doctor
why doctor --json
```

It reports:
- the effective config paths and resolved LLM settings,
- whether auth is configured,
- whether the LLM client can be initialized,
- whether a live LLM call succeeds.

If the live call fails, `why doctor` reports the error directly and the runtime log remains available at `.why/runtime.log`.

## Index Location

No persistent index — `why` reads git history on demand.
Fast enough for interactive use (~1–3 seconds per query).

---

### Health regression gate

Use `why health` with a checked-in baseline to fail on any debt-score or signal regression:

```bash
cargo run -p why-core --bin why -- health \
  --baseline-file .github/health-baseline.json \
  --require-baseline \
  --max-regression 0 \
  --max-signal-regression time_bombs=0 \
  --max-signal-regression high_risk_files=0 \
  --max-signal-regression hotspot_files=0 \
  --max-signal-regression stale_hacks=0
```

Update `.github/health-baseline.json` intentionally after a known-good mainline shift by re-running:

```bash
cargo run -p why-core --bin why -- health --json --write-baseline .github/health-baseline.json
```

Exit-code summary:

- `0` — checks passed
- `3` — CI threshold failed (`--ci`)
- `4` — regression gate failed (`--max-regression` / `--max-signal-regression`)

## Roadmap

- [ ] GitHub/GitLab PR title + description integration (via API)
- [ ] Jira/Linear ticket resolution from commit messages
- [ ] `why --since <date>` for recent change context
- [ ] Team blame — who knows the most about this code?
- [ ] VS Code extension with inline `why` on hover
