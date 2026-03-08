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

```bash
# Why does this function exist?
why fn verifyToken

# Why does this file exist?
why file src/legacy/payment_v1.js

# Why was this specific line written?
why line src/auth.js:42

# Raw mode — show git data without LLM synthesis
why fn verifyToken --raw
```

## Output Example

```bash
$ why fn verifyToken

Analyzing git history for `verifyToken`...
Found 6 commits touching this function (2021–2024)

────────────────────────────────────────────────

WHY IT EXISTS
verifyToken was introduced in April 2021 after a security incident
(commit a3f9b2c) where expired JWT tokens were still being accepted.
The team added explicit expiry checking after a session hijacking
report from a penetration test.

HISTORY
- Apr 2021: Initial implementation after security audit
- Sep 2022: Extended to support refresh tokens (commit 8d2e1f4)  
- Jan 2024: Fixed edge case where iat claim was missing (commit cc91a3b)

RISK IF REMOVED: HIGH
This is the sole validation point for all authenticated routes.
3 routes call this directly; 12 routes call it transitively.

RELATED
- Commit a3f9b2c: "fix: tokens not expiring on logout"
- Issue #234: "Security: session tokens survive password reset"
```

## Integration with Claude Code

Add to your project's `CLAUDE.md`:

```markdown
## Custom Tools

- `why fn <name>` — explain why a function exists and risk if removed
- `why file <path>` — explain why a file exists (especially legacy ones)
- `why line <file>:<line>` — explain why a specific line was written

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
model = "claude-haiku-4-5"
max_commits = 20
```

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
