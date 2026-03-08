# PLAN: why

## Overview

Git archaeology CLI. Explains why a function, file, or line exists by collecting
git history and synthesizing it into a human-readable explanation with risk assessment.
Rust (`git2` crate) + one Claude Haiku call per query.

---

## Why Workspace + `crates/` (not `src/`)

Same reasoning as the other tools in this suite:
- Workspace root is config, docs, poc only — no source code
- Each concern (git operations, symbol location, LLM synthesis, caching) is its own crate
- `src/` only appears inside each sub-crate
- Entry point: `crates/core/main.rs`

---

## File Structure

```
why/
├── Cargo.toml              # workspace root — no source code here
├── Cargo.lock
├── README.md
├── POC.md
├── PLAN.md
├── .gitignore              # includes .why/
├── .why.toml.example       # user config template
│
├── crates/
│   ├── core/               # binary entry point
│   │   ├── Cargo.toml
│   │   └── main.rs         # CLI: fn / file / line / team
│   │
│   ├── archaeologist/      # git2 data collection
│   │   ├── Cargo.toml
│   │   ├── lib.rs
│   │   ├── blame.rs        # git blame scoped to a function's byte range
│   │   ├── log.rs          # git log -S equivalent for symbol tracking
│   │   └── commit.rs       # parse commit: message, author, date, PR/issue refs
│   │
│   ├── locator/            # tree-sitter function range finder
│   │   ├── Cargo.toml
│   │   ├── lib.rs
│   │   └── finder.rs       # fn name → byte range in file
│   │
│   ├── synthesizer/        # Claude Haiku API call
│   │   ├── Cargo.toml
│   │   ├── lib.rs
│   │   └── prompt.rs       # build structured prompt + call Anthropic API
│   │
│   └── cache/              # .why/cache.json
│       ├── Cargo.toml
│       ├── lib.rs
│       └── store.rs        # key: symbol + HEAD hash, invalidate on new commit
│
├── poc/
│   ├── package.json
│   └── index.js            # Node.js POC: simple-git + Claude Haiku
│
├── tests/
│   └── blame_test.rs
│
└── benches/
    └── blame_bench.rs
```

### Root `Cargo.toml`

```toml
[workspace]
members = [
    "crates/core",
    "crates/archaeologist",
    "crates/locator",
    "crates/synthesizer",
    "crates/cache",
]
resolver = "2"

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"

[[bin]]
name = "why"
path = "crates/core/main.rs"

[workspace.dependencies]
git2 = "0.18"
tree-sitter = "0.22"
tree-sitter-javascript = "0.21"
tree-sitter-typescript = "0.21"
tree-sitter-rust = "0.21"
tree-sitter-python = "0.21"
reqwest = { version = "0.12", features = ["blocking", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
regex = "1"
chrono = "0.4"
anyhow = "1"
```

---

## Phases

### Phase 1 — POC (Node.js, 1 day)

Validate that git data collection + LLM synthesis produces accurate explanations.

- [ ] `simple-git` to run `git log -S <symbol>`
- [ ] Parse commit hash, date, author, subject into structured data
- [ ] Feed to Claude Haiku and receive WHY / HISTORY / RISK / RELATED sections
- [ ] Test on 5 real functions in a known repository
- [ ] `--raw` mode: show raw commits without calling the LLM (for offline testing)

**Success criteria:** explanation is accurate and actionable for a function
with more than 6 months of git history.

---

### Phase 2 — Rust Core (3–4 days)

- [ ] Set up workspace with 5 crates
- [ ] `crates/locator`: tree-sitter → exact byte range of target function per language
- [ ] `crates/archaeologist`:
  - `git2`-based `git log -S` equivalent — commits that introduced or modified the symbol
  - `git blame` scoped to function byte range
  - Parse PR references (`#123`) and issue references (`fixes #456`) from commit messages
- [ ] `crates/synthesizer`:
  - Build structured prompt from collected commit data
  - Call Claude Haiku via reqwest
  - Parse response into sections: WHY / HISTORY / RISK / RELATED
- [ ] `crates/cache`: `.why/cache.json`
  - Cache key: `<symbol>:<repo HEAD hash>`
  - Invalidates automatically when a new commit is made
- [ ] `crates/core`: clap CLI
  - `why fn <n>` — explain a function
  - `why file <path>` — explain a file
  - `why line <file>:<n>` — explain a specific line
  - `--raw` flag — skip LLM, show raw commit data only

---

### Phase 3 — Cache Layer (1 day)

- [ ] `.why/cache.json` keyed on `symbol + HEAD hash`
- [ ] Automatic invalidation when HEAD changes
- [ ] `--no-cache` flag to force refresh

---

### Phase 4 — GitHub Enrichment (2 days)

- [ ] GitHub API: fetch full PR title and description from PR number references in commits
- [ ] Issue resolution: `fixes #123` → fetch issue title from GitHub API
- [ ] `GITHUB_TOKEN` environment variable for authentication
- [ ] Graceful degradation: full functionality without a token, richer output with one

---

### Phase 5 — Polish (1 day)

- [ ] `--since <days>` — focus history analysis on recent changes only
- [ ] `why team <fn>` — bus factor: who has the most commits touching this symbol?
- [ ] Display estimated cost after each query (`~$0.001`)
- [ ] `CLAUDE.md` snippet in README
- [ ] `cargo install why-cli`

---

## Integration with Claude Code

Add to any project's `CLAUDE.md`:

```markdown
## Before deleting or heavily refactoring:
Run `why fn <function_name>` to understand why it exists.
If RISK is HIGH, do not remove it without reading the history first.
```

---

## Cost Model

| Scenario | Estimated cost |
|---|---|
| Function with 5 commits | ~$0.0007 |
| Function with 20 commits | ~$0.0017 |
| 1,000 queries per day | ~$1.00 |

Model: `claude-haiku-4-5` — cheapest tier, fast enough for interactive use.

---

## Timeline

| Phase | Duration |
|---|---|
| 1 — POC | 1 day |
| 2 — Rust core | 3–4 days |
| 3 — Cache layer | 1 day |
| 4 — GitHub enrichment | 2 days |
| 5 — Polish | 1 day |
| **Total** | **~10 days** |
