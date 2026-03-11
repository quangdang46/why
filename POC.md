# POC: why

> Proof of concept demonstrating git history archaeology + LLM synthesis
> to explain why code exists and assess risk of removal.

---

## POC Goal

Prove that we can:
1. Use `git log -S` and `git blame` to collect relevant commit history for a function
2. Parse commit messages, dates, diffs into structured data
3. Feed that to Claude Haiku and get a useful, accurate explanation
4. Do this in < 3 seconds with < $0.002 per query

---

## POC Scope

Written in **Node.js** using the `simple-git` library.
Production will be Rust with `git2` crate (no git binary dependency).

Important: this Node CLI is a prototype for concept validation only. Its `fn|file` command syntax and `--raw` flag do **not** define the Rust shipping interface. The checked-in Rust CLI uses positional targets (`why <target> [flags]`), supports line/range queries plus implemented symbol queries, and uses `--json` for machine-readable output.

---

## Setup

```bash
cd poc
npm install
export ANTHROPIC_API_KEY=sk-ant-...
```

**Dependencies:**
```json
{
  "simple-git": "^3.22.0",
  "node-fetch": "^3.3.0"
}
```

---

## POC Code

### `poc/index.js`

```javascript
import simpleGit from 'simple-git'
import fetch from 'node-fetch'

const git = simpleGit(process.cwd())

// ─── 1. Find commits that touched a function ──────────────────────────────────
// Uses git log -S to find commits where the function name appeared/disappeared

async function getCommitsForSymbol(symbolName) {
  const log = await git.raw([
    'log',
    '-S', symbolName,
    '--pretty=format:%H|||%ai|||%an|||%s',
    '--diff-filter=AMD',
    '-20'         // max 20 commits
  ])

  if (!log.trim()) return []

  return log.trim().split('\n').map(line => {
    const [hash, date, author, subject] = line.split('|||')
    return { hash, date, author, subject }
  })
}

// ─── 2. Get diff for each commit ──────────────────────────────────────────────

async function getDiffForCommit(hash, symbolName) {
  try {
    const diff = await git.raw(['show', hash, '--stat', '--no-patch'])
    return diff.trim().split('\n').slice(0, 5).join('\n') // first 5 lines of stat
  } catch {
    return ''
  }
}

// ─── 3. Get current file context (comments near function) ────────────────────

async function getFileContext(filePath, fnName) {
  try {
    const { execSync } = await import('child_process')
    // grep for the function definition and surrounding lines
    const result = execSync(
      `grep -n "${fnName}" "${filePath}" | head -5`,
      { encoding: 'utf8' }
    )
    return result.trim()
  } catch {
    return ''
  }
}

// ─── 4. Call Claude Haiku for synthesis ──────────────────────────────────────

async function synthesizeWithClaude(symbolName, commits, fileContext) {
  const commitSummary = commits.map(c =>
    `- ${c.date.substring(0, 10)} [${c.hash.substring(0, 7)}] ${c.author}: "${c.subject}"`
  ).join('\n')

  const prompt = `You are analyzing git history to explain why a piece of code exists.

SYMBOL: ${symbolName}

GIT HISTORY (commits that introduced or modified this symbol):
${commitSummary || '(no commits found — may be very new or renamed)'}

CURRENT CODE CONTEXT:
${fileContext || '(not found in current codebase)'}

Based on this history, provide:
1. WHY IT EXISTS: 2-3 sentences explaining the origin and purpose
2. HISTORY: Key milestones in bullet points
3. RISK IF REMOVED: HIGH / MEDIUM / LOW with one sentence reason
4. RELATED: Any incident, issue, or security concern references from commit messages

Be concise. If history is sparse, say so honestly.`

  const response = await fetch('https://api.anthropic.com/v1/messages', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'x-api-key': process.env.ANTHROPIC_API_KEY,
      'anthropic-version': '2023-06-01'
    },
    body: JSON.stringify({
      model: 'claude-haiku-4-5',
      max_tokens: 500,
      messages: [{ role: 'user', content: prompt }]
    })
  })

  const data = await response.json()
  return data.content[0].text
}

// ─── 5. Main: wire it all together ───────────────────────────────────────────

async function whySymbol(symbolName, filePath) {
  console.log(`\nAnalyzing git history for \`${symbolName}\`...\n`)

  const [commits, fileContext] = await Promise.all([
    getCommitsForSymbol(symbolName),
    filePath ? getFileContext(filePath, symbolName) : Promise.resolve('')
  ])

  console.log(`Found ${commits.length} commits touching this symbol\n`)

  if (process.argv.includes('--raw')) {
    console.log('RAW GIT DATA:')
    commits.forEach(c => console.log(`  ${c.date.substring(0, 10)} ${c.hash.substring(0, 7)} ${c.author}: ${c.subject}`))
    return
  }

  const explanation = await synthesizeWithClaude(symbolName, commits, fileContext)

  console.log('─'.repeat(60))
  console.log(explanation)
  console.log('─'.repeat(60))
}

// ─── 6. CLI ───────────────────────────────────────────────────────────────────

const [,, command, target, filePath] = process.argv

if (command === 'fn') {
  await whySymbol(target, filePath)
} else if (command === 'file') {
  // For file: use the filename itself as search term
  await whySymbol(target.split('/').pop().replace(/\.[^.]+$/, ''), target)
} else {
  console.log('Usage: why <fn|file> <name> [filepath]')
}
```

---

## Run the POC

```bash
# Run on THIS project's own git history (or any git repo)
cd /path/to/any/git/repo

# Explain a function
node /path/to/poc/index.js fn verifyToken src/auth.js

# Raw mode — show bounded evidence without LLM synthesis
node /path/to/poc/index.js fn verifyToken src/auth.js --raw

# Explain a file
node /path/to/poc/index.js file src/legacy/payment.js
```

---

## POC Test: Run on a Real Repo

The best POC is running `why` on a real project you know.
Try on any repo with at least 6 months of history.

Suggested validation pattern:

```bash
# From a real git repo with mature history
cd /path/to/real/repo

# Utility / weak-signal example
node /path/to/why/poc/index.js fn someHelper src/helpers.js --raw

# Stronger-signal example with LLM synthesis (requires ANTHROPIC_API_KEY)
node /path/to/why/poc/index.js fn someCriticalFunction src/core/module.js

# File-oriented query
node /path/to/why/poc/index.js file src/legacy/payment.js
```

Pick targets that match the scenario mix defined below rather than relying on a single repo or only "good demo" functions.

---

## Recommended Validation Targets and Scenarios

Use a small but deliberately varied set of targets so the prototype is tested against different history shapes rather than only "good demos."

### Target mix

Evaluate at least 5 targets covering these scenario types:

| Scenario | Why it matters | What success looks like |
|---|---|---|
| Hotfix / incident response | Tests whether the prototype can recover urgent historical intent | Explanation connects the code to a concrete bug, regression, outage, or security fix when that evidence exists |
| Compatibility shim | Tests whether it can detect code kept for legacy clients, APIs, or platform quirks | Explanation identifies backward-compatibility pressure rather than treating the code as redundant |
| Migration / transitional code | Tests whether it can recognize bridge logic and temporary coexistence | Explanation notes the migration role and any uncertainty about whether the transition is complete |
| Routine utility | Prevents over-dramatization of ordinary code | Explanation stays modest and practical, without inventing a larger story |
| Weak-signal history | Tests honesty under sparse or noisy evidence | Explanation admits uncertainty, lists unknowns, and avoids hallucinated confidence |

### Selection criteria

Choose targets that meet as many of these criteria as possible:

- At least 6 months of history for most targets.
- At least 1 target with only sparse or noisy history.
- At least 1 target whose purpose is already known by the evaluator.
- A mix of functions and files if possible.
- At least 1 target where deletion risk is plausibly high.
- At least 1 target where deletion risk is plausibly low or ambiguous.

### Suggested evaluation set

This initial set is meant to exercise the prototype on a mature public repository such as Express, Rails, Django, or another codebase with long-lived history.

| Slot | Scenario | Target characteristics | Questions to ask |
|---|---|---|---|
| 1 | Hotfix / incident | Function with commit messages mentioning fix, regression, security, crash, outage, or edge case | Does the output identify the operational reason this code exists? |
| 2 | Compatibility shim | Function or file mentioning legacy browser/client/version/protocol support | Does the output explain who or what this code is preserving compatibility for? |
| 3 | Migration / bridge | Code introduced during a rename, API transition, dependency swap, or architecture move | Does the output capture that this code may be transitional rather than foundational? |
| 4 | Routine utility | Small helper with boring, incremental history | Does the output stay restrained instead of inventing a dramatic backstory? |
| 5 | Weak-signal case | Symbol with one noisy refactor commit, rename confusion, or shallow history | Does the output explicitly say the evidence is thin? |

### Anti-bias rules

To keep evaluation honest:

- Do not choose only targets that already have beautifully descriptive commit messages.
- Do not exclude weak or messy cases just because they make the prototype look worse.
- Record at least one case where the prototype fails or produces only partial value.
- Prefer targets where the reviewer can independently assess the likely ground truth from project history.

---

## Expected Output on Real Repo

```bash
$ node poc/index.js fn setCharset

Analyzing git history for `setCharset`...
Found 4 commits touching this symbol

────────────────────────────────────────────────────────────

WHY IT EXISTS
setCharset was introduced to normalize charset handling in HTTP
response headers. It was added after discovering that some clients
were not correctly reading charset-less content-type headers.

HISTORY
- 2014-03-12: Initial implementation for charset normalization
- 2015-08-01: Fixed edge case with quoted charset values
- 2017-02-20: Refactored to use mime-types library

RISK IF REMOVED: HIGH
This function is called by every response that sets Content-Type.
Removing it would break charset detection for all text responses.

RELATED
- Issue referenced in commit: "fix charset handling for older IE"
────────────────────────────────────────────────────────────
```

---

## Cost Analysis

Typical Haiku call for `why`:
- Input tokens: ~800 (commit history + context)
- Output tokens: ~300 (explanation)
- Cost: ~$0.0008 per query

**1000 queries = < $1.00**

---

## POC Success Rubric

The prototype is only a success if it produces outputs that are useful for change decisions, not merely plausible summaries.

### Evaluation dimensions

Score each run on a 0–2 scale:

| Dimension | 0 — Fail | 1 — Partial | 2 — Pass |
|---|---|---|---|
| Accuracy | Explanation contradicts known history or invents causes unsupported by commits | Core idea is directionally right but misses important nuance or overstates certainty | Explanation matches known history and stays within the evidence |
| Actionability | Output would not change an engineering decision | Output is interesting but does not clearly inform deletion/refactor risk | Output clearly helps decide whether code can be deleted, refactored, or should be investigated further |
| Evidence grounding | Claims are unsupported or detached from the cited commit list/context | Some linkage to commits exists, but key claims are weakly grounded | Major claims can be traced back to specific commits, context, or visible code markers |
| Risk calibration | Risk is obviously too high/low for the target, or no uncertainty is acknowledged | Risk level is plausible but breakage reasoning is thin | Risk level and likely breakage are proportional to the available evidence |
| Confidence honesty | Sparse history still gets a very confident answer | Some uncertainty is mentioned, but confidence is not well calibrated | Thin evidence leads to cautious language, explicit unknowns, and lower confidence |
| Concision | Rambling, repetitive, or hard to scan | Some useful content but too verbose or poorly structured | Easy to scan quickly; summary, history, and risk are all clear |

### Pass / fail threshold

The POC is considered validated only if all of the following are true:

1. At least 5 representative targets are evaluated.
2. At least 4 of 5 targets score **9/12 or better**.
3. No target scores **0** on either **Accuracy** or **Evidence grounding**.
4. At least 1 weak-signal target causes the model to explicitly admit uncertainty rather than hallucinate certainty.
5. A human reviewer concludes the output would have changed or improved a real code-review, deletion, or refactor decision.

If these conditions are not met, the concept is not sufficiently validated for the Rust build and the failure mode should be recorded.

## Evaluation Template

Use this template for each prototype run:

```markdown
### Target
- Repo:
- File:
- Symbol/line:
- Scenario type: hotfix / compatibility shim / migration / utility / weak-signal / other
- Known ground truth:

### Prototype output
- Commits found:
- Latency:
- Estimated cost:
- Raw output saved at:

### Scoring
- Accuracy: 0/1/2
- Actionability: 0/1/2
- Evidence grounding: 0/1/2
- Risk calibration: 0/1/2
- Confidence honesty: 0/1/2
- Concision: 0/1/2
- Total: /12

### Notes
- Strong evidence cited:
- Missing evidence:
- Hallucinations or overreach:
- Would this affect a real engineering decision?
- Keep / revise / reject for production assumptions:
```

## Examples of acceptable vs unacceptable output

### Acceptable
- Correctly identifies that history is sparse and says the reason is uncertain.
- Ties a high-risk judgment to clear evidence such as a hotfix commit, security wording, compatibility note, or repeated maintenance around the same code.
- Gives a modest summary for routine utility code instead of inventing a dramatic origin story.

### Unacceptable
- Claims a security or incident origin with no such evidence in commit messages or local context.
- States `RISK IF REMOVED: HIGH` without explaining likely breakage.
- Presents a polished explanation for a target with only one noisy refactor commit and no contextual support.
- Confuses "this code changed often" with "this code is important" without further evidence.

---

## Why This Is The Most Unique Tool

Both `scope` and `smart-grep` have conceptual analogs:
- `scope` ≈ better LSP / dependency analysis
- `smart-grep` ≈ semantic search (GitHub Copilot has this)

**`why` has no analog.** No tool asks "why does this code exist?"  
The combination of `git2` archaeology + LLM synthesis is novel.  
It's the tool that prevents the most damage: deleting code that shouldn't be deleted.

---

## Evaluation Findings (Current Repo Check)

A light phase-0 evaluation was run against the current `why` repository using the implemented Node prototype in `poc/index.js`.

### What was tested

Commands run:

```bash
node poc/index.js fn getCommitsForSymbol poc/index.js --raw
node poc/index.js fn whySymbol poc/index.js --raw
node poc/index.js file poc/index.js --raw
```

### Observed results

- The prototype runs successfully in `--raw` mode and can enumerate commits via `git log -S`.
- Symbol-level queries on this repository returned only a single documentation-oriented commit for both `getCommitsForSymbol` and `whySymbol`.
- File-mode queries produced a broader set of commits, but the search term (`index`) is noisy and not specific enough to establish real historical intent.
- The current environment did not provide `ANTHROPIC_API_KEY`, so the synthesis path could not be evaluated here.

### Assessment against the rubric

| Dimension | Score | Notes |
|---|---:|---|
| Accuracy | 1/2 | Raw commit enumeration is mechanically correct, but symbol matching is too weak to support confident archaeology claims on this repo |
| Actionability | 1/2 | Useful as a signal-gathering prototype, not yet strong enough for deletion/refactor decisions |
| Evidence grounding | 2/2 | Raw mode shows the actual commits used, which is good for inspection |
| Risk calibration | 0/2 | No meaningful risk judgment can be validated from raw mode alone in this run |
| Confidence honesty | 1/2 | The prompt asks for honesty under sparse history, but the non-raw path was not exercised here |
| Concision | 2/2 | Output is small and easy to inspect |
| Total | 7/12 | Below the success threshold |

### Go / no-go decision

**Decision: conditional go for the Rust implementation, but not because the POC is fully validated.**

Reasoning:

- The prototype successfully proves the basic plumbing: pickaxe search, local context capture, and one-call synthesis wiring are feasible.
- It does **not** yet prove that symbol targeting is reliable enough for trustworthy archaeology. The current `git log -S <symbol>` approach is too noisy and brittle for production semantics.
- The missing LLM-path evaluation means the most important product claim — useful explanation quality — remains only partially validated in this repository.

### Product learnings for the Rust build

1. `git log -S <symbol>` is acceptable for a POC but insufficient as the core production strategy.
2. Exact target resolution via tree-sitter plus line-range blame is necessary, not optional.
3. Raw evidence mode is valuable and should remain a first-class product surface.
4. The product must explicitly handle weak-signal cases by lowering confidence and surfacing unknowns.
5. Validation should be repeated on mature external repositories with known historical ground truth before treating the concept as fully proven.

### Recommended follow-up before calling the concept fully validated

- Run the POC on at least 5 curated external targets from a mature repository.
- Evaluate one weak-signal case and verify the explanation admits uncertainty.
- Evaluate at least one hotfix/security case and verify the explanation cites the incident evidence.
- Record full rubric scores for each run rather than relying on intuition.

---

## Next Steps (Production Rust)

1. Rewrite with `git2` crate — no system git binary required
2. Tree-sitter integration to locate exact function byte ranges for `git blame`
3. GitHub API integration to fetch full PR descriptions (not just commit messages)
4. Jira/Linear issue resolution from `fixes #123` patterns in commits
5. Cache recent queries in `.why/cache.json` to avoid repeated LLM calls
6. `why --since 30d` to focus on recent changes only
