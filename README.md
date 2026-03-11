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

```bash
cargo install why-cli

# Set your API key once
export ANTHROPIC_API_KEY=sk-ant-...
```

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
```

Current Rust CLI notes:
- The Rust CLI uses positional target syntax (`why <target> [flags]`), not `fn|file|line` subcommands.
- `--lines <start:end>` supports explicit range queries.
- Symbol queries like `why src/auth.js:verifyToken` are implemented in the current Rust CLI for Rust, JavaScript, TypeScript, and Python.
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
- `why <file>:<symbol>` — explain why a supported symbol exists (Rust, JavaScript, TypeScript, Python)

**Always run `why` before deleting or significantly refactoring any function
that exists in git history for more than 6 months.**
```

## API Key

`why` only calls Claude Haiku (cheapest model) for synthesis.  
Typical cost: **~$0.001 per query** (one Haiku call with ~2k token input).

Set via environment variable:
```bash
export ANTHROPIC_API_KEY=sk-ant-...
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

[github]
remote = "origin"
# token = "ghp_..."   # optional fallback; prefer GITHUB_TOKEN env var
```

For GitHub enrichment work, set `GITHUB_TOKEN` in the environment when available; `.why.toml` can also carry an optional `[github]` fallback token and remote name.

See `.why.toml.example` for a fuller documented example of the currently implemented config surface.

## Index Location

No persistent index — `why` reads git history on demand.  
Fast enough for interactive use (~1–3 seconds per query).

---

## Roadmap

- [ ] GitHub/GitLab PR title + description integration (via API)
- [ ] Jira/Linear ticket resolution from commit messages
- [ ] `why --since <date>` for recent change context
- [ ] Team blame — who knows the most about this code?
- [ ] VS Code extension with inline `why` on hover
