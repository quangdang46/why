# why

> Ask your codebase *why* a function, file, or line exists. Powered by git history + LLM synthesis.

---

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

3. Synthesize  (LLM call — Claude Haiku)
   └── feed structured git data to Claude
   └── ask: "why does this exist? what risk if removed?"
   └── returns human-readable explanation + risk level
```

Only **one LLM call per query**, with structured git data as input.

## Tech Stack

| Crate | Purpose |
|---|---|
| `git2` | Native git operations — no git binary required |
| `tree-sitter` | Locate function boundaries precisely |
| `anthropic` (HTTP) | Synthesize git data into explanation |
| `clap` | CLI |
| `serde_json` | Structured output |

## Installation

Current repo state:
- GitHub release packaging is checked in via `.github/workflows/release.yml`
- A curl-friendly installer is checked in at `./install.sh`
- The published binary name is `why`
- Crates.io installation is **not** ready yet because the chosen shipping package is `why-core`, but it still has `publish = false` in `crates/core/Cargo.toml`

Current install paths:

```bash
# Install from a GitHub release
curl -sSL https://raw.githubusercontent.com/quangdang46/why/main/install.sh | bash

# Or build locally from this checkout
cargo run -q -p why-core -- --help
cargo build -p why-core --release
./target/release/why --help

# Generate local CLI artifacts from the current build
cargo run -q -p why-core -- completions bash > why.bash
cargo run -q -p why-core -- completions zsh > _why
cargo run -q -p why-core -- completions fish > why.fish
cargo run -q -p why-core -- manpage > why.1

# Set your API key once when you want synthesis
export ANTHROPIC_API_KEY=your_anthropic_api_key_here
```

### Release and publishing readiness checklist

Checked in today:
- CI workflow for fmt, clippy, build, test, audit, optional benches, and packaging-alignment checks in `.github/workflows/ci.yml`
- Tagged GitHub release workflow with cross-platform archives in `.github/workflows/release.yml`
- Installer script with checksum verification and source-build fallback in `install.sh`
- Packaging alignment audit that verifies `why-core` / `why` metadata stays consistent across `crates/core/Cargo.toml`, `.github/workflows/release.yml`, and `install.sh`

Still required before `cargo install ...` is a supported path:
- Keep any future package-name/docs changes aligned with the current shipped package `why-core` and binary `why`
- Remove `publish = false` from the shipping package once crates.io publication is intended
- Verify `cargo install why-core` produces the `why` binary cleanly
- Keep the README installation instructions, release workflow package name, and installer/source-build path aligned with the shipped package

Current artifact-generation support:
- `why completions bash|zsh|fish` emits shell completion scripts to stdout
- `why manpage` emits a roff man page to stdout
- These commands make it possible to check completion/manual artifacts into packaging or release automation later without inventing a separate generator binary

## Usage

Current repo state:
- The checked-in prototype implementation is a Node.js POC under `poc/`
- The checked-in Rust CLI uses positional target syntax: `why <target> [flags]`
- The current Rust implementation supports line, explicit range, symbol, and qualified-symbol queries

Current Rust CLI examples:
```bash
# Why was this specific line written?
why src/auth.js:42

# Why does this line range exist?
why src/auth.js --lines 40:45 --no-llm

# Why does this symbol exist?
why src/auth.js:verifyToken --no-llm

# Machine-readable archaeology output
why src/auth.js:verifyToken --json

# Repo-wide danger hotspots ranked by churn × heuristic risk
why hotspots --limit 10

# Install or remove managed git hooks for high-risk change warnings
why install-hooks --warn-only
why uninstall-hooks
```

Current Rust CLI notes:
- The Rust CLI uses positional target syntax (`why <target> [flags]`), not `fn|file|line` subcommands.
- `--lines <start:end>` supports explicit range queries.
- Symbol queries like `why src/auth.js:verifyToken` are implemented in the current Rust CLI for Rust, Go, JavaScript, TypeScript, Java, and Python.
- Qualified symbol queries like `why src/payment.rs:PaymentService::process_payment` are implemented for Rust impl methods.
- The Rust CLI uses `--json` for machine-readable output; `--raw` is a Node POC flag, not a Rust CLI flag.

Current Node POC examples:
```bash
node poc/index.js fn verifyToken src/auth.js
node poc/index.js file src/legacy/payment_v1.js
node poc/index.js fn verifyToken src/auth.js --raw
```

The Node commands above are prototype-only and do not define the Rust shipping interface.

## Output Example

```bash
$ why src/auth.js:42

why: src/auth.js (line 42)

Commits touching this line:
  a3f9b2c  alice  2024-01-12  fix: tokens not expiring on logout
  8d2e1f4  bob    2022-09-04  extend auth flow for refresh token handling

No LLM synthesis (--no-llm or no API key). Heuristic risk: MEDIUM.
```

A richer narrative explanation for symbol-level queries is planned for later phases after tree-sitter targeting and synthesis land.

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
- `why <file>:<line> --json` — return machine-readable raw archaeology output
- `why <file>:<symbol>` — explain why a supported symbol exists (Rust, Go, JavaScript, TypeScript, Java, Python)
- `why <file>:<symbol> --coupled` — inspect co-change dependencies before a deeper refactor
- `why <file>:<symbol> --team` — identify likely owners before asking for review on risky code

**Always run `why` before deleting or significantly refactoring any function
that exists in git history for more than 6 months.**
```

Recommended Claude Code workflow:

1. Before deleting or rewriting unfamiliar code, run `why` on the exact symbol or line range first.
2. If the reported risk is **HIGH**, treat that as a stop-and-investigate signal rather than a suggestion to proceed quickly.
3. For larger refactors, also run `--coupled` and `--team` so you can spot co-change surfaces and likely reviewers.
4. When working inside an MCP-capable editor, use `why mcp` for tool integration; use the normal CLI when you want the full query/output flow documented in this README. Make sure the MCP server is launched from the repository/workspace you want analyzed, because it operates on its current working directory.

Recommended code review routine:

- include a `why ... --json` or terminal summary when proposing removal of old-looking code
- use `why ... --team` when the change touches operationally sensitive paths and you need to find the best reviewer
- use `why ... --coupled` before splitting or relocating a historically noisy function

For MCP-specific setup examples, see `docs/mcp-setup.md`.

## API Key

`why` only calls Claude Haiku (cheapest model) for synthesis.  
Typical cost: **~$0.001 per query** (one Haiku call with ~2k token input).

Set via environment variable:
```bash
export ANTHROPIC_API_KEY=your_anthropic_api_key_here
```

Or in `.why.toml` at project root:
```toml
[risk]
default_level = "LOW"

[risk.keywords]
high = ["pci", "reconciliation"]
medium = ["terraform"]

[git]
max_commits = 8
recency_window_days = 90
mechanical_threshold_files = 50
coupling_scan_commits = 500
coupling_ratio_threshold = 0.30

[github]
remote = "origin"
# token = "ghp_..."   # optional fallback; prefer GITHUB_TOKEN env var
```

`[risk.keywords]` extends the built-in heuristic vocabulary with team- or domain-specific terms. Matches are case-insensitive and can affect both ranked evidence relevance and the heuristic risk level.

For GitHub enrichment work, set `GITHUB_TOKEN` in the environment when available; `.why.toml` can also carry an optional `[github]` fallback token and remote name. Environment variables take precedence over `.why.toml`, and blank values are ignored. Prefer the environment-variable path because a token stored in `.why.toml` is easier to commit accidentally or leave readable on disk.

See `.why.toml.example` for a fully documented example of the currently implemented config surface.

## Cache and `.why/` directory semantics

Current repo state:
- query results are cached in `.why/cache.json` at the repository root
- cache keys include the target identity plus the current `HEAD` hash prefix, so changing history invalidates prior entries naturally
- terminal output shows `[cached]` when a stored `WhyReport` is reused
- `--no-cache` bypasses cache reads and forces a fresh query
- the cache file also stores rolling health snapshots for future trend-oriented reporting
- up to 52 health snapshots are retained
- CI enforces health regression budgets with `.github/health-baseline.json` and compares pull requests against the base branch's baseline when available

Operator expectations:
- treat `.why/` as local runtime state, not source-controlled project state
- `.why/` should be ignored by git for normal development workflows
- on Unix, the cache directory and file are written with owner-only permissions (`0700` for `.why/`, `0600` for `cache.json`)
- deleting `.why/cache.json` is safe if you want to clear local cached results; `why` will recreate it on the next cached run

## Index Location

No persistent index — `why` reads git history on demand.
Fast enough for interactive use (~1–3 seconds per query).

---

### Health regression gate in CI

GitHub Actions runs `why health` with a checked-in baseline and fails on any debt-score or signal regression:

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

## Roadmap

- [ ] GitHub/GitLab PR title + description integration (via API)
- [ ] Jira/Linear ticket resolution from commit messages
- [ ] `why --since <date>` for recent change context
- [ ] Team blame — who knows the most about this code?
- [ ] VS Code extension with inline `why` on hover
