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

# Raw mode — just show commits without LLM
node /path/to/poc/index.js fn verifyToken --raw

# Explain a file
node /path/to/poc/index.js file src/legacy/payment.js
```

---

## POC Test: Run on a Real Repo

The best POC is running `why` on a real project you know.  
Try on any repo with at least 6 months of history:

```bash
# Clone a known project
git clone https://github.com/expressjs/express /tmp/express-test
cd /tmp/express-test

# Ask why a core function exists
node why/poc/index.js fn finalhandler
node why/poc/index.js fn setCharset
node why/poc/index.js fn compileETag
```

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

## Why This Is The Most Unique Tool

Both `scope` and `smart-grep` have conceptual analogs:
- `scope` ≈ better LSP / dependency analysis
- `smart-grep` ≈ semantic search (GitHub Copilot has this)

**`why` has no analog.** No tool asks "why does this code exist?"  
The combination of `git2` archaeology + LLM synthesis is novel.  
It's the tool that prevents the most damage: deleting code that shouldn't be deleted.

---

## Next Steps (Production Rust)

1. Rewrite with `git2` crate — no system git binary required
2. Tree-sitter integration to locate exact function byte ranges for `git blame`
3. GitHub API integration to fetch full PR descriptions (not just commit messages)
4. Jira/Linear issue resolution from `fixes #123` patterns in commits
5. Cache recent queries in `.why/cache.json` to avoid repeated LLM calls
6. `why --since 30d` to focus on recent changes only
