# why — Implementation Plan (Rust)

## Goal
Build a CLI tool called **`why`** that explains **why a function, file, or specific line exists** by combining:

- precise code targeting
- Git history and blame analysis
- commit / PR / issue context extraction
- one LLM synthesis call

The output should answer:

1. Why was this code introduced or changed?
2. Was it linked to a bug, incident, hotfix, migration, or temporary workaround?
3. What is the risk if it is deleted or refactored?
4. What evidence supports that conclusion?

---

## Product Definition

### Core user story
As a developer, I can run:

```bash
why src/auth/session.rs:authenticate
why src/auth/session.rs:120
why src/auth/session.rs --lines 110:145
```

and get a human-readable explanation of:

- the historical reason this code exists
- the commits that shaped it
- likely deletion/refactor risks
- confidence level based on evidence

### Non-goals (v1)
- Full GitHub API deep integration
- Multi-repo reasoning
- Automatic code deletion suggestions
- Real-time IDE plugin
- Support for every programming language on day one

---

## Success Criteria

A good v1 should:

- resolve a function / symbol / line range accurately
- collect the commits that most directly affected that target
- retrieve commit messages and diffs for those commits
- detect references like `#123`, `fixes #456`, incident IDs, TODOs, and nearby comments
- produce one concise explanation with a risk level:
  - Low
  - Medium
  - High
- cite the evidence used in the answer

---

## High-Level Architecture

```text
CLI
 └── Target Resolver
      ├── file path resolution
      ├── line range resolution
      └── symbol resolution via tree-sitter
           ↓
   Git Analyzer
      ├── git blame on byte/line range
      ├── unique commit collection
      ├── commit metadata extraction
      └── patch/diff extraction
           ↓
   Context Extractor
      ├── nearby comments / TODOs
      ├── PR / issue reference detection
      └── hotfix / incident / migration heuristics
           ↓
   Evidence Pack Builder
      └── structured JSON for LLM
           ↓
   Synthesizer
      ├── single LLM call
      └── explanation + risk assessment
           ↓
   Formatter
      ├── terminal output
      └── optional JSON output
```

---

## Technical Stack (Rust)

### Required crates
- `clap` — CLI parsing
- `anyhow` / `thiserror` — error handling
- `git2` — git repo access, blame, commit lookup, diff extraction
- `tree-sitter` — parse source files and locate symbols
- `tree-sitter-*` language grammars — starting with Rust, maybe TypeScript, Python later
- `serde` / `serde_json` — structured evidence payloads
- `regex` — detect PR refs / issue refs / hotfix patterns
- `reqwest` — LLM API call
- `tokio` — async runtime for network call
- `tracing` / `tracing-subscriber` — logging
- `camino` or `std::path` — path handling

### Optional crates
- `colored` / `owo-colors` — terminal UX
- `ignore` — repo walking if later needed
- `cached` — cache repeated analysis

---

## Module Breakdown

## 1. `cli`
Responsibilities:
- parse user input
- support file, symbol, line, and line-range queries
- configure output format

Example commands:

```bash
why src/lib.rs:my_function
why src/lib.rs:87
why src/lib.rs --lines 80:120
why src/lib.rs:AuthService::login
why src/lib.rs:my_function --json
```

Suggested argument model:

- positional target string
- `--lines <start:end>`
- `--symbol <name>`
- `--json`
- `--max-commits <n>`
- `--model <name>`
- `--no-llm` for raw evidence only

---

## 2. `target_resolver`
Responsibilities:
- parse target syntax
- resolve exact file path
- resolve target into line range and byte range
- for symbols, use tree-sitter queries

### v1 strategy
Support these target kinds:
1. file + line number
2. file + explicit line range
3. file + symbol name

### Rust-specific symbol resolution
For Rust v1, support:
- `fn`
- `impl` methods
- `struct`
- `enum`
- `trait`
- module-level constants/statics (optional)

### Output
A normalized structure:

```rust
pub struct ResolvedTarget {
    pub repo_root: PathBuf,
    pub file_path: PathBuf,
    pub language: LanguageKind,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub symbol_name: Option<String>,
    pub surrounding_context: String,
}
```

### Technical note
`git blame` works line-oriented more naturally than byte-oriented, so tree-sitter should identify byte ranges first, then convert to line ranges for blame.

---

## 3. `git_analyzer`
Responsibilities:
- open repo
- run blame for target lines
- collect unique commits touching those lines
- retrieve commit details and diffs

### Core flow
1. Open repository via `git2::Repository::discover()`
2. Run blame on target file and line range
3. Gather unique commit OIDs from blame hunks
4. Rank commits by coverage / recency / significance
5. Load each commit object
6. Extract:
   - subject
   - body
   - author
   - timestamp
   - parent commits
   - patch affecting target file

### Proposed data model

```rust
pub struct BlamedCommit {
    pub oid: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    pub message: String,
    pub touched_lines: Vec<(usize, usize)>,
    pub diff_excerpt: String,
    pub coverage_score: f32,
}
```

### Important design choice
Do **not** send too many commits to the LLM.
Instead:
- collect all unique commits
- score and compress them
- send top relevant commits plus summary stats

Potential ranking signals:
- how many target lines the commit owns
- whether commit message contains `fix`, `hotfix`, `incident`, `security`, `temporary`, `workaround`, `revert`, `migration`
- whether the diff significantly introduced or deleted logic in the target block

---

## 4. `context_extractor`
Responsibilities:
- inspect local code around the target
- detect comments, TODOs, FIXMEs
- parse commit messages for issue / PR refs
- identify likely risk patterns

### Local code signals
Look at a small window around the target, e.g. ±20 lines:
- comments above function
- inline comments
- TODO / FIXME / HACK / TEMP / SAFETY markers
- feature flags or kill switches
- panic / retry / auth / permission / validation logic

### Commit message signals
Regex examples:
- `#123`
- `fixes #456`
- `closes #789`
- `incident-1234`
- `sev1`, `sev2`, `postmortem`, `hotfix`, `rollback`
- `temporary`, `workaround`, `compat`, `migration`

### Risk heuristics
Raise risk if target appears related to:
- auth / permissions / validation
- retries / backoff / circuit breakers
- data migrations / backward compatibility
- security checks / sanitization
- incident hotfix language in history
- revert chains or repeated fixes in same area

---

## 5. `evidence_builder`
Responsibilities:
- compress analysis into one structured payload for the LLM
- keep input deterministic and bounded

### Output shape

```json
{
  "target": {
    "file": "src/auth/session.rs",
    "symbol": "authenticate",
    "lines": [110, 145]
  },
  "local_context": {
    "comments": ["..."],
    "todos": ["..."]
  },
  "history": {
    "commit_count": 6,
    "top_commits": [
      {
        "oid": "abc123",
        "date": "2025-11-03",
        "summary": "hotfix auth bypass in session refresh",
        "diff_excerpt": "..."
      }
    ]
  },
  "signals": {
    "issue_refs": ["#456"],
    "risk_flags": ["auth", "hotfix", "security"]
  }
}
```

### Constraint
The payload must be small enough for one cheap model call. That means:
- summarized diffs, not full patches
- top N commits only
- truncate verbose commit bodies

---

## 6. `synthesizer`
Responsibilities:
- call LLM once
- ask for explanation and deletion risk
- return structured result

### Prompt contract
Ask the model to produce:
- **Reason this code exists**
- **Historical evidence**
- **What may break if removed**
- **Risk level**
- **Confidence**
- **Unknowns / assumptions**

### Important prompt rule
The model must distinguish between:
- direct evidence from commits/comments
- inferred reasoning

### Suggested response schema

```rust
pub struct WhyReport {
    pub summary: String,
    pub why_it_exists: Vec<String>,
    pub risk_level: RiskLevel,
    pub likely_breakage: Vec<String>,
    pub evidence: Vec<String>,
    pub confidence: String,
    pub unknowns: Vec<String>,
}
```

### Fallback mode
If no LLM key is configured:
- print raw evidence summary
- include a heuristic risk score

This makes the tool still useful offline.

---

## 7. `formatter`
Responsibilities:
- pretty terminal output
- machine-readable JSON output

### Terminal output example

```text
why: src/auth/session.rs:authenticate (lines 110-145)

Why this exists
This function appears to have been hardened after an authentication bypass bug fixed in commit abc123 on 2025-11-03. Later commits adjusted retry/session behavior for backward compatibility.

Risk if removed: HIGH
- May reintroduce auth/session bypass behavior
- May break compatibility with older refresh-token flow

Evidence
- abc123: "hotfix auth bypass in session refresh"
- def456: "preserve legacy mobile token behavior"
- Nearby comment: "temporary guard until all clients rotate"

Confidence: medium-high
Unknowns
- No direct PR description available
```

---

## Implementation Phases

## Phase 1 — CLI + line targeting
Deliverable:
- accept `file:line`
- run blame on that line or line range
- print commits that touched it

Tasks:
- set up cargo workspace or single binary crate
- add `clap`, `git2`, `anyhow`
- implement repo discovery
- implement target parsing for `file:line`
- implement blame and commit listing

Outcome:
A minimal but working history-aware CLI.

---

## Phase 2 — tree-sitter symbol targeting
Deliverable:
- accept `file:symbol`
- resolve symbol to line range

Tasks:
- add `tree-sitter` and `tree-sitter-rust`
- parse Rust source
- locate named function / method / type
- convert byte ranges to line ranges
- handle ambiguous symbol matches with best effort

Outcome:
Useful developer UX for function-level queries.

---

## Phase 3 — commit evidence extraction
Deliverable:
- enrich commits with metadata and diff excerpts

Tasks:
- fetch full commit messages
- compute diff against parent
- isolate file-specific patch snippets
- score commit relevance
- extract issue / PR references via regex

Outcome:
Structured evidence instead of raw blame output.

---

## Phase 4 — local code context + heuristics
Deliverable:
- comment/TODO extraction
- preliminary risk assessment without LLM

Tasks:
- inspect nearby lines
- identify comments and markers
- add domain heuristics for auth/security/migrations/workarounds
- compute heuristic risk level

Outcome:
The tool is useful even without AI.

---

## Phase 5 — LLM synthesis
Deliverable:
- one API call for final explanation

Tasks:
- define evidence JSON schema
- build concise prompt
- implement provider client
- parse response into report structure
- add `--json` output

Outcome:
Human-readable “why” reports.

---

## Phase 6 — polish and reliability
Deliverable:
- stable v1

Tasks:
- snapshot tests
- fixture repos for history scenarios
- better error messages
- truncation logic for diffs
- support for merge commits and renamed files where practical

Outcome:
Production-quality CLI.

---

## Data Flow Example

For:

```bash
why src/auth/session.rs:authenticate
```

Flow:
1. Parse CLI target
2. Detect repo root
3. Parse Rust file with tree-sitter
4. Find `authenticate`
5. Convert its node span into lines 110-145
6. Run `git blame` on those lines
7. Collect unique OIDs
8. Load commit metadata + patch excerpts
9. Extract nearby comments and TODOs
10. Build compact evidence JSON
11. Call LLM once
12. Print report

---

## Technical Risks

## 1. Symbol resolution ambiguity
Problem:
- duplicate function names
- methods with same name across impl blocks

Mitigation:
- require file path in v1
- later support fully qualified paths
- if multiple matches, choose exact match with warning or present options

## 2. Blame is not the same as origin
Problem:
- blame shows last modifying commit, not necessarily the first reason code was introduced

Mitigation:
- inspect multiple commits across target lines
- optionally trace oldest relevant commit among collected history
- make inference explicit in output

## 3. Large diffs / noisy commits
Problem:
- one formatting or refactor commit can dominate

Mitigation:
- relevance scoring
- ignore whitespace-only changes when possible
- detect broad mechanical refactors and down-rank them

## 4. Missing PR descriptions
Problem:
- pure local git repo may not include PR context

Mitigation:
- rely on commit message refs
- keep GitHub/GitLab API lookup as future enhancement

## 5. LLM hallucination risk
Problem:
- model may overstate causality

Mitigation:
- structured evidence only
- require explicit evidence vs inference separation
- return confidence + unknowns

---

## Testing Strategy

### Unit tests
- target parser
- symbol resolution in sample Rust files
- regex extraction of refs and risk flags
- evidence payload truncation

### Integration tests
Create fixture repos with scripted histories for scenarios like:
- hotfix commit
- temporary workaround that became permanent
- auth/security guard
- backward compatibility shim
- renamed function with stable behavior

Then assert:
- correct commit collection
- correct symbol range resolution
- sensible risk classification

### Snapshot tests
Snapshot terminal report output and JSON output.

---

## Suggested Repo Layout

```text
why/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── target.rs
│   ├── treesitter.rs
│   ├── git.rs
│   ├── context.rs
│   ├── evidence.rs
│   ├── synth.rs
│   ├── report.rs
│   └── output.rs
├── tests/
│   ├── fixtures/
│   ├── integration_cli.rs
│   └── snapshot_reports.rs
└── docs/
    └── plan.md
```

---

## MVP Definition

A strong MVP is:
- Rust-only language support
- target by `file:line` and `file:symbol`
- git blame + commit summaries + diff excerpts
- comment/TODO extraction
- one LLM synthesis call
- terminal + JSON output

This is enough to prove the core value: **historical reasoning before code deletion or refactoring**.

---

## Future Enhancements

### Near-term
- GitHub / GitLab API enrichment for PR titles and descriptions
- support renamed files (`git log --follow`-style behavior)
- config file for provider/model
- caching analysis results
- better handling of merge commits

### Mid-term
- multi-language support:
  - TypeScript
  - Python
  - Go
- IDE/editor integration
- output links to commits / PRs
- batch mode for multiple symbols

### Long-term
- “safe to delete?” mode
- repository-wide historical dependency reasoning
- automatic incident / postmortem linkage
- architectural memory maps

---

## Recommended Build Order

1. `file:line` support
2. blame + commit metadata
3. diff excerpts
4. Rust tree-sitter symbol resolution
5. comment/TODO extraction
6. heuristic risk scoring
7. LLM synthesis
8. JSON output
9. tests and fixtures

This order minimizes risk and gets to a useful CLI quickly.

---

## Final Recommendation

Build **v1 narrowly and deeply**:
- one language: Rust
- one excellent use case: “why does this function/line exist?”
- one strong output: explanation + deletion risk + evidence

Do not overbuild integrations first. The defensible technical moat is:

1. precise code targeting
2. high-signal commit evidence extraction
3. disciplined synthesis that separates evidence from inference

That is the core product.

---

## First Sprint Proposal

### Sprint objective
Ship a demo that answers `why file.rs:123` with commit-backed reasoning.

### Sprint tasks
- scaffold CLI
- implement repo discovery
- implement line target parsing
- run blame for target range
- collect commit summaries
- render terminal report
- add one fixture repo test

### Sprint exit criteria
Running:

```bash
why src/example.rs:42
```

returns:
- the relevant commits
- basic explanation of historical intent
- preliminary risk level

That is enough to validate the concept before tree-sitter and LLM polish.

