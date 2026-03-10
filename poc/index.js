import simpleGit from 'simple-git'
import fetch from 'node-fetch'
import { readFile } from 'node:fs/promises'
import path from 'node:path'

const git = simpleGit(process.cwd())

async function getCommitsForSymbol(symbolName) {
  const log = await git.raw([
    'log',
    '-S',
    symbolName,
    '--pretty=format:%H|||%ai|||%an|||%s',
    '--diff-filter=AMD',
    '-20'
  ])

  if (!log.trim()) return []

  return log.trim().split('\n').map(line => {
    const [hash, date, author, subject] = line.split('|||')
    return { hash, date, author, subject }
  })
}

async function getDiffForCommit(hash) {
  try {
    const diff = await git.raw(['show', hash, '--stat', '--no-patch'])
    return diff.trim().split('\n').slice(0, 5).join('\n')
  } catch {
    return ''
  }
}

async function getFileContext(filePath, fnName) {
  try {
    const content = await readFile(filePath, 'utf8')
    const lines = content.split('\n')
    const matchIndex = lines.findIndex(line => line.includes(fnName))

    if (matchIndex === -1) return ''

    const start = Math.max(0, matchIndex - 2)
    const end = Math.min(lines.length, matchIndex + 3)

    return lines
      .slice(start, end)
      .map((line, index) => `${start + index + 1}:${line}`)
      .join('\n')
  } catch {
    return ''
  }
}

function buildPrompt(symbolName, commits, fileContext) {
  const commitSummary = commits
    .map(c => {
      const diffStat = c.diffStat ? `\n  Diff/stat:\n${c.diffStat.split('\n').map(line => `    ${line}`).join('\n')}` : ''
      return `- ${c.date.substring(0, 10)} [${c.hash.substring(0, 7)}] ${c.author}: \"${c.subject}\"${diffStat}`
    })
    .join('\n')

  return `You are analyzing git history to explain why a piece of code exists.

SYMBOL: ${symbolName}

GIT HISTORY (commits that introduced or modified this symbol):
${commitSummary || '(no commits found — may be very new or renamed)'}

CURRENT CODE CONTEXT:
${fileContext || '(not found in current codebase)'}

Based only on the evidence above, provide:
1. WHY IT EXISTS: 2-3 sentences explaining the most likely origin and purpose
2. HISTORY: Key milestones in bullet points
3. RISK IF REMOVED: HIGH / MEDIUM / LOW with one sentence reason
4. RELATED: Any incident, issue, compatibility, or security references from commit messages or context

Be concise. If history is sparse, say so honestly and avoid unsupported claims.`
}

async function synthesizeWithClaude(symbolName, commits, fileContext) {
  if (!process.env.ANTHROPIC_API_KEY) {
    throw new Error('ANTHROPIC_API_KEY is required unless you use --raw')
  }

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
      messages: [{ role: 'user', content: buildPrompt(symbolName, commits, fileContext) }]
    })
  })

  if (!response.ok) {
    throw new Error(`Anthropic API request failed: ${response.status} ${response.statusText}`)
  }

  const data = await response.json()
  return data.content?.[0]?.text ?? 'No explanation returned.'
}

async function gatherCommitData(commits) {
  return Promise.all(
    commits.map(async commit => ({
      ...commit,
      diffStat: await getDiffForCommit(commit.hash)
    }))
  )
}

function printRaw(commits, fileContext) {
  console.log('RAW GIT DATA:')
  commits.forEach(c => {
    console.log(`  ${c.date.substring(0, 10)} ${c.hash.substring(0, 7)} ${c.author}: ${c.subject}`)
    if (c.diffStat) {
      console.log(`    ${c.diffStat.split('\n').join('\n    ')}`)
    }
  })

  if (fileContext) {
    console.log('\nCURRENT CODE CONTEXT:')
    console.log(fileContext)
  }
}

async function whySymbol(symbolName, filePath) {
  console.log(`\nAnalyzing git history for \`${symbolName}\`...\n`)

  const normalizedFilePath = filePath ? path.resolve(process.cwd(), filePath) : null
  const [commits, fileContext] = await Promise.all([
    getCommitsForSymbol(symbolName),
    normalizedFilePath ? getFileContext(normalizedFilePath, symbolName) : Promise.resolve('')
  ])
  const commitsWithDiffs = await gatherCommitData(commits)

  console.log(`Found ${commitsWithDiffs.length} commits touching this symbol\n`)

  if (process.argv.includes('--raw')) {
    printRaw(commitsWithDiffs, fileContext)
    return
  }

  const explanation = await synthesizeWithClaude(symbolName, commitsWithDiffs, fileContext)

  console.log('─'.repeat(60))
  console.log(explanation)
  console.log('─'.repeat(60))
}

const [, , command, target, filePath] = process.argv

if (command === 'fn' && target) {
  await whySymbol(target, filePath)
} else if (command === 'file' && target) {
  const symbolName = path.basename(target).replace(/\.[^.]+$/, '')
  await whySymbol(symbolName, target)
} else {
  console.log('Usage: why <fn|file> <name> [filepath] [--raw]')
  process.exitCode = 1
}
