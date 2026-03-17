use anyhow::{Context, Result, bail};
use git2::{BlameOptions, DiffFormat, DiffOptions, Oid, Repository, Sort};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;
use why_context::WhyConfig;
use why_context::load_config;
use why_locator::{QueryKind, QueryTarget, resolve_target};

const MAX_DIFF_EXCERPT_CHARS: usize = 500;
const LOCAL_CONTEXT_WINDOW_LINES: u32 = 20;
const MAX_LOCAL_COMMENTS: usize = 5;
const MAX_LOCAL_MARKERS: usize = 5;
const MAX_LOCAL_RISK_FLAGS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommitEvidence {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub date: String,
    pub summary: String,
    pub message: String,
    pub diff_excerpt: String,
    pub coverage_score: f32,
    pub relevance_score: f32,
    pub issue_refs: Vec<String>,
    pub is_mechanical: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RiskLevel {
    HIGH,
    MEDIUM,
    LOW,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HIGH => "HIGH",
            Self::MEDIUM => "MEDIUM",
            Self::LOW => "LOW",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::HIGH => {
                "The history suggests security sensitivity, incident context, or non-routine compatibility risk."
            }
            Self::MEDIUM => {
                "The history suggests migration, retry, legacy, or transitional behavior that needs context before changes."
            }
            Self::LOW => {
                "The available history does not show unusual operational or compatibility pressure."
            }
        }
    }

    pub fn change_guidance(self) -> &'static str {
        match self {
            Self::HIGH => {
                "Stop and investigate before deleting or heavily refactoring this target."
            }
            Self::MEDIUM => {
                "Change only after reviewing surrounding code and validating the behavior you might disturb."
            }
            Self::LOW => "Treat this as ordinary code unless stronger evidence appears elsewhere.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputTarget {
    pub path: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub query_kind: QueryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalContext {
    pub comments: Vec<String>,
    pub markers: Vec<String>,
    pub risk_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchaeologyResult {
    pub target: OutputTarget,
    pub commits: Vec<CommitEvidence>,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
    pub change_guidance: String,
    pub local_context: LocalContext,
    pub mode: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamOwner {
    pub author: String,
    pub commit_count: usize,
    pub ownership_percent: u8,
    pub last_commit_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamReport {
    pub target: OutputTarget,
    pub owners: Vec<TeamOwner>,
    pub bus_factor: usize,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwnershipSummary {
    pub owners: Vec<TeamOwner>,
    pub bus_factor: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlameChainResult {
    pub target: OutputTarget,
    pub starting_commit: CommitEvidence,
    pub noise_commits_skipped: Vec<CommitEvidence>,
    pub origin_commit: CommitEvidence,
    pub chain_depth: usize,
    pub risk_level: RiskLevel,
    pub risk_summary: &'static str,
    pub change_guidance: &'static str,
    pub local_context: LocalContext,
    pub mode: &'static str,
    pub notes: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionCommit {
    pub commit: CommitEvidence,
    pub path_at_commit: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvolutionInflection {
    pub category: &'static str,
    pub reason: String,
    pub oid: String,
    pub summary: String,
    pub path_at_commit: PathBuf,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionHistoryResult {
    pub target: OutputTarget,
    pub commits: Vec<EvolutionCommit>,
    pub paths_seen: Vec<PathBuf>,
    pub latest_commit: Option<CommitEvidence>,
    pub origin_commit: Option<CommitEvidence>,
    pub inflection_points: Vec<EvolutionInflection>,
    pub narrative_summary: String,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
    pub change_guidance: String,
    pub local_context: LocalContext,
    pub mode: &'static str,
    pub notes: Vec<&'static str>,
}

pub fn discover_repository(starting_dir: &Path) -> Result<Repository> {
    Repository::discover(starting_dir).with_context(|| {
        format!(
            "failed to discover git repository from {}",
            starting_dir.display()
        )
    })
}

pub fn relative_repo_path(repo: &Repository, path: &Path) -> Result<PathBuf> {
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;

    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workdir.join(path)
    };

    if let Ok(relative) = candidate.strip_prefix(workdir) {
        return Ok(relative.to_path_buf());
    }

    let canonical_workdir = fs::canonicalize(workdir)
        .with_context(|| format!("failed to canonicalize repository root {}", workdir.display()))?;
    let canonical_candidate = fs::canonicalize(&candidate)
        .with_context(|| format!("failed to canonicalize target path {}", candidate.display()))?;

    canonical_candidate
        .strip_prefix(&canonical_workdir)
        .map(PathBuf::from)
        .with_context(|| {
            format!(
                "{} is not inside repository root {}",
                candidate.display(),
                workdir.display()
            )
        })
}

pub fn analyze_target(target: &QueryTarget, cwd: &Path) -> Result<ArchaeologyResult> {
    analyze_target_with_options(target, cwd, None)
}

pub fn analyze_target_with_options(
    target: &QueryTarget,
    cwd: &Path,
    since_days: Option<u64>,
) -> Result<ArchaeologyResult> {
    let resolved = resolve_target(target, cwd)?;
    let config = load_config(cwd)?;

    let target_path = cwd.join(&resolved.path);
    let repo = discover_repository(&target_path)?;
    let relative_path = relative_repo_path(&repo, &target_path)?;
    let commits = blame_commit_evidence(
        &repo,
        &relative_path,
        resolved.start_line,
        resolved.end_line,
        &config,
        since_days,
    )?;
    let local_context = extract_local_context(
        &target_path,
        resolved.start_line,
        resolved.end_line,
        &config,
    )?;

    let risk_level = infer_risk_level(&commits, &local_context, &config);

    Ok(ArchaeologyResult {
        target: OutputTarget {
            path: relative_path,
            start_line: resolved.start_line,
            end_line: resolved.end_line,
            query_kind: resolved.query_kind,
        },
        commits,
        risk_level,
        risk_summary: risk_level.summary().to_string(),
        change_guidance: risk_level.change_guidance().to_string(),
        local_context,
        mode: "heuristic".to_string(),
        notes: vec![
            "No LLM synthesis in phase 1".to_string(),
            "Evidence and inference should be kept separate when presenting this result"
                .to_string(),
        ],
    })
}

pub fn blame_commit_evidence(
    repo: &Repository,
    relative_path: &Path,
    start_line: u32,
    end_line: u32,
    config: &WhyConfig,
    since_days: Option<u64>,
) -> Result<Vec<CommitEvidence>> {
    let ownership = blame_ownership(repo, relative_path, start_line, end_line, since_days)?;
    let total_target_lines = (end_line - start_line + 1) as usize;

    let mut seen = HashSet::new();
    let mut commits = Vec::new();
    for (oid, owned_lines) in ownership {
        if !seen.insert(oid) {
            continue;
        }

        let coverage_score = owned_lines as f32 / total_target_lines as f32;
        let mut evidence = load_commit_evidence(repo, oid, coverage_score, config)?;
        evidence.relevance_score = compute_relevance_score(&evidence, config);
        commits.push(evidence);
    }

    Ok(select_top_commits(commits, config.git.max_commits))
}

#[derive(Debug, Clone, PartialEq)]
struct BlameChainTrace {
    starting_commit: CommitEvidence,
    noise_commits_skipped: Vec<CommitEvidence>,
    origin_commit: CommitEvidence,
    chain_depth: usize,
}

fn blame_ownership(
    repo: &Repository,
    relative_path: &Path,
    start_line: u32,
    end_line: u32,
    since_days: Option<u64>,
) -> Result<Vec<(Oid, usize)>> {
    if start_line == 0 || end_line == 0 {
        bail!("blame lines must be 1-based");
    }

    if end_line < start_line {
        bail!("blame range end must be greater than or equal to start");
    }

    let mut options = BlameOptions::new();
    options
        .min_line(start_line as usize)
        .max_line(end_line as usize);

    if let Some(since_days) = since_days {
        let since_ts = OffsetDateTime::now_utc().unix_timestamp() - since_days as i64 * 86_400;
        let mut revwalk = repo.revwalk().context("failed to create revwalk")?;
        revwalk.push_head().context("failed to walk HEAD")?;
        revwalk
            .set_sorting(Sort::TIME)
            .context("failed to set revwalk ordering")?;
        for oid in revwalk {
            let oid = oid.context("failed to read commit from revwalk")?;
            let commit = repo
                .find_commit(oid)
                .with_context(|| format!("failed to load commit {oid}"))?;
            if commit.time().seconds() < since_ts {
                continue;
            }
            options.newest_commit(oid);
            break;
        }
    }

    let blame = repo
        .blame_file(relative_path, Some(&mut options))
        .with_context(|| format!("failed to blame {}", relative_path.display()))?;

    let mut ownership = Vec::new();
    for hunk in blame.iter() {
        let oid = hunk.final_commit_id();
        if oid.is_zero() {
            continue;
        }

        let hunk_start = hunk.final_start_line() as u32;
        let hunk_end = hunk_start + hunk.lines_in_hunk() as u32 - 1;
        let overlap_start = hunk_start.max(start_line);
        let overlap_end = hunk_end.min(end_line);
        if overlap_end < overlap_start {
            continue;
        }

        let owned_lines = (overlap_end - overlap_start + 1) as usize;
        ownership.push((oid, owned_lines));
    }

    if ownership.is_empty() {
        bail!(
            "no commits found for {}:{}-{}",
            relative_path.display(),
            start_line,
            end_line
        );
    }

    Ok(ownership)
}

fn blame_chain(
    repo: &Repository,
    relative_path: &Path,
    start_line: u32,
    end_line: u32,
    config: &WhyConfig,
    since_days: Option<u64>,
) -> Result<BlameChainTrace> {
    let ownership = blame_ownership(repo, relative_path, start_line, end_line, since_days)?;
    let total_target_lines = (end_line - start_line + 1) as usize;
    let (starting_oid, owned_lines) = ownership
        .into_iter()
        .max_by_key(|(_, owned_lines)| *owned_lines)
        .context("expected blamed ownership to contain at least one commit")?;
    let coverage_score = owned_lines as f32 / total_target_lines as f32;
    let mut current = repo
        .find_commit(starting_oid)
        .with_context(|| format!("failed to load commit {starting_oid}"))?;
    let mut starting_commit = finalize_chain_commit(repo, &current, coverage_score, config)?;
    starting_commit.relevance_score = compute_relevance_score(&starting_commit, config);
    let mut skipped = Vec::new();
    let mut chain_depth = 0;

    loop {
        let parent_count = current.parent_count();
        if parent_count > 1 {
            let skipped_commit = finalize_chain_commit(repo, &current, coverage_score, config)?;
            skipped.push(skipped_commit);
            let mut selected = None;
            for index in (0..parent_count).rev() {
                let parent = current
                    .parent(index)
                    .context("failed to load parent while following merge origin")?;
                if !commit_is_mechanical(repo, &parent, config)? {
                    selected = Some(parent);
                    break;
                }
                selected.get_or_insert(parent);
            }
            current = selected.context("merge commit had no parents to follow")?;
            chain_depth += 1;
            continue;
        }

        if commit_is_mechanical(repo, &current, config)? {
            if parent_count == 0 {
                let mut origin_commit =
                    finalize_chain_commit(repo, &current, coverage_score, config)?;
                origin_commit.relevance_score = compute_relevance_score(&origin_commit, config);
                return Ok(BlameChainTrace {
                    starting_commit,
                    noise_commits_skipped: skipped,
                    origin_commit,
                    chain_depth,
                });
            }

            let skipped_commit = finalize_chain_commit(repo, &current, coverage_score, config)?;
            skipped.push(skipped_commit);
            current = current
                .parent(0)
                .context("failed to load parent while following merge origin")?;
            chain_depth += 1;
            continue;
        }

        let mut origin_commit = finalize_chain_commit(repo, &current, coverage_score, config)?;
        origin_commit.relevance_score = compute_relevance_score(&origin_commit, config);
        return Ok(BlameChainTrace {
            starting_commit,
            noise_commits_skipped: skipped,
            origin_commit,
            chain_depth,
        });
    }
}

pub fn analyze_blame_chain(
    target: &QueryTarget,
    cwd: &Path,
    since_days: Option<u64>,
) -> Result<BlameChainResult> {
    let resolved = resolve_target(target, cwd)?;
    let config = load_config(cwd)?;
    let target_path = cwd.join(&resolved.path);
    let repo = discover_repository(&target_path)?;
    let relative_path = relative_repo_path(&repo, &target_path)?;
    let local_context = extract_local_context(
        &target_path,
        resolved.start_line,
        resolved.end_line,
        &config,
    )?;
    let risk_level = infer_risk_level(
        &blame_commit_evidence(
            &repo,
            &relative_path,
            resolved.start_line,
            resolved.end_line,
            &config,
            since_days,
        )?,
        &local_context,
        &config,
    );

    let chain = blame_chain(
        &repo,
        &relative_path,
        resolved.start_line,
        resolved.end_line,
        &config,
        since_days,
    )?;

    Ok(BlameChainResult {
        target: OutputTarget {
            path: relative_path,
            start_line: resolved.start_line,
            end_line: resolved.end_line,
            query_kind: resolved.query_kind,
        },
        starting_commit: chain.starting_commit,
        noise_commits_skipped: chain.noise_commits_skipped,
        origin_commit: chain.origin_commit,
        chain_depth: chain.chain_depth,
        risk_level,
        risk_summary: risk_level.summary(),
        change_guidance: risk_level.change_guidance(),
        local_context,
        mode: "blame-chain",
        notes: vec![
            "Blame-chain mode walks past mechanical commits to surface a truer origin",
            "Evidence and inference should be kept separate when presenting this result",
        ],
    })
}

pub fn summarize_ownership(commits: &[CommitEvidence]) -> OwnershipSummary {
    let mut by_author: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    for commit in commits {
        let entry = by_author
            .entry(commit.author.clone())
            .or_insert((0, commit.time));
        entry.0 += 1;
        entry.1 = entry.1.max(commit.time);
    }

    let total = commits.len().max(1);
    let mut owners = by_author
        .into_iter()
        .map(|(author, (commit_count, last_time))| TeamOwner {
            author,
            commit_count,
            ownership_percent: ((commit_count * 100) / total) as u8,
            last_commit_date: format_git_time(last_time).unwrap_or_else(|_| "unknown".into()),
        })
        .collect::<Vec<_>>();
    owners.sort_by(|left, right| {
        right
            .commit_count
            .cmp(&left.commit_count)
            .then(left.author.cmp(&right.author))
    });

    let bus_factor = owners
        .iter()
        .find(|owner| owner.ownership_percent >= 50)
        .map(|_| 1)
        .unwrap_or(owners.len().min(2));

    OwnershipSummary { owners, bus_factor }
}

pub fn analyze_team(
    target: &QueryTarget,
    cwd: &Path,
    since_days: Option<u64>,
) -> Result<TeamReport> {
    let resolved = resolve_target(target, cwd)?;
    let config = load_config(cwd)?;
    let target_path = cwd.join(&resolved.path);
    let repo = discover_repository(&target_path)?;
    let relative_path = relative_repo_path(&repo, &target_path)?;
    let commits = blame_commit_evidence(
        &repo,
        &relative_path,
        resolved.start_line,
        resolved.end_line,
        &config,
        since_days,
    )?;
    let local_context = extract_local_context(
        &target_path,
        resolved.start_line,
        resolved.end_line,
        &config,
    )?;
    let risk_level = infer_risk_level(&commits, &local_context, &config);
    let ownership = summarize_ownership(&commits);

    let risk_summary = if let Some(primary) = ownership.owners.first() {
        format!(
            "{} is the primary owner of {}-risk code for this target.",
            primary.author,
            risk_level.as_str()
        )
    } else {
        format!(
            "Ownership is unclear, but this target still carries {}-risk history.",
            risk_level.as_str()
        )
    };

    Ok(TeamReport {
        target: OutputTarget {
            path: relative_path,
            start_line: resolved.start_line,
            end_line: resolved.end_line,
            query_kind: resolved.query_kind,
        },
        owners: ownership.owners,
        bus_factor: ownership.bus_factor,
        risk_level,
        risk_summary,
    })
}

pub fn analyze_evolution_history(
    target: &QueryTarget,
    cwd: &Path,
    since_days: Option<u64>,
) -> Result<EvolutionHistoryResult> {
    let resolved = resolve_target(target, cwd)?;
    let config = load_config(cwd)?;
    let target_path = cwd.join(&resolved.path);
    let repo = discover_repository(&target_path)?;
    let relative_path = relative_repo_path(&repo, &target_path)?;
    let commits = collect_evolution_history(&repo, &relative_path, &config, since_days)?;
    let local_context = extract_local_context(
        &target_path,
        resolved.start_line,
        resolved.end_line,
        &config,
    )?;

    let plain_commits = commits
        .iter()
        .map(|entry| entry.commit.clone())
        .collect::<Vec<_>>();
    let risk_level = infer_risk_level(&plain_commits, &local_context, &config);

    let mut seen_paths = HashSet::new();
    let mut paths_seen = Vec::new();
    for entry in &commits {
        if seen_paths.insert(entry.path_at_commit.clone()) {
            paths_seen.push(entry.path_at_commit.clone());
        }
    }

    let latest_commit = commits.first().map(|entry| entry.commit.clone());
    let origin_commit = commits.last().map(|entry| entry.commit.clone());
    let inflection_points = collect_evolution_inflections(&commits, &paths_seen);
    let narrative_summary = summarize_evolution_history(
        &commits,
        &inflection_points,
        &paths_seen,
        risk_level,
        &local_context,
    );

    Ok(EvolutionHistoryResult {
        target: OutputTarget {
            path: relative_path,
            start_line: resolved.start_line,
            end_line: resolved.end_line,
            query_kind: resolved.query_kind,
        },
        commits,
        paths_seen,
        latest_commit,
        origin_commit,
        inflection_points,
        narrative_summary,
        risk_level,
        risk_summary: risk_level.summary().to_string(),
        change_guidance: risk_level.change_guidance().to_string(),
        local_context,
        mode: "evolution-history",
        notes: vec![
            "Evolution-history mode follows git log renames to preserve pre-move context",
            "Narrative summaries separate recent state, origin, and inflection points from the raw timeline",
        ],
    })
}

pub fn extract_local_context(
    path: &Path,
    start_line: u32,
    end_line: u32,
    config: &WhyConfig,
) -> Result<LocalContext> {
    if start_line == 0 || end_line == 0 {
        bail!("context lines must be 1-based");
    }
    if end_line < start_line {
        bail!("context range end must be greater than or equal to start");
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read source file {}", path.display()))?;
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return Ok(LocalContext {
            comments: Vec::new(),
            markers: Vec::new(),
            risk_flags: Vec::new(),
        });
    }

    let window_start = start_line.saturating_sub(LOCAL_CONTEXT_WINDOW_LINES).max(1) as usize;
    let window_end = (end_line + LOCAL_CONTEXT_WINDOW_LINES).min(lines.len() as u32) as usize;

    let mut comments = Vec::new();
    let mut markers = Vec::new();
    let mut risk_flags = Vec::new();
    let mut seen_flags = HashSet::new();

    for line in &lines[(window_start - 1)..window_end] {
        if let Some(comment) = extract_comment_text(line) {
            if comments.len() < MAX_LOCAL_COMMENTS {
                comments.push(comment.clone());
            }
            if let Some(marker) = classify_marker(&comment) {
                if markers.len() < MAX_LOCAL_MARKERS {
                    markers.push(comment.clone());
                }
                if matches!(marker, "HACK" | "FIXME" | "TEMP" | "SAFETY") {
                    push_risk_flag(
                        &mut risk_flags,
                        &mut seen_flags,
                        marker.to_ascii_lowercase(),
                    );
                }
            }
            collect_risk_flags(&comment, config, &mut risk_flags, &mut seen_flags);
        } else {
            collect_risk_flags(line, config, &mut risk_flags, &mut seen_flags);
        }
    }

    Ok(LocalContext {
        comments,
        markers,
        risk_flags,
    })
}

fn extract_comment_text(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(comment) = trimmed.strip_prefix("//") {
        return normalize_context_text(comment);
    }
    if let Some(comment) = trimmed.strip_prefix('#') {
        return normalize_context_text(comment);
    }
    if let Some(start) = trimmed.find("/*") {
        let comment = &trimmed[start + 2..];
        let comment = comment.strip_suffix("*/").unwrap_or(comment);
        return normalize_context_text(comment);
    }
    if let Some(start) = trimmed.find("//") {
        return normalize_context_text(&trimmed[start + 2..]);
    }
    None
}

fn normalize_context_text(text: &str) -> Option<String> {
    let normalized = text.trim().trim_matches('*').trim();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn classify_marker(text: &str) -> Option<&'static str> {
    let upper = text.to_ascii_uppercase();
    ["TODO", "FIXME", "HACK", "TEMP", "SAFETY", "XXX"]
        .into_iter()
        .find(|marker| upper.contains(marker))
}

fn collect_risk_flags(
    text: &str,
    config: &WhyConfig,
    risk_flags: &mut Vec<String>,
    seen_flags: &mut HashSet<String>,
) {
    let lower = text.to_ascii_lowercase();
    for keyword in HIGH_SIGNAL_KEYWORDS
        .iter()
        .chain(MEDIUM_SIGNAL_KEYWORDS.iter())
        .chain(RISK_DOMAIN_KEYWORDS.iter())
    {
        if lower.contains(keyword) {
            push_risk_flag(risk_flags, seen_flags, (*keyword).to_string());
        }
    }

    for keyword in &config.risk.keywords.high {
        if lower.contains(&keyword.to_ascii_lowercase()) {
            push_risk_flag(risk_flags, seen_flags, keyword.to_ascii_lowercase());
        }
    }
    for keyword in &config.risk.keywords.medium {
        if lower.contains(&keyword.to_ascii_lowercase()) {
            push_risk_flag(risk_flags, seen_flags, keyword.to_ascii_lowercase());
        }
    }
}

fn push_risk_flag(risk_flags: &mut Vec<String>, seen_flags: &mut HashSet<String>, flag: String) {
    if risk_flags.len() >= MAX_LOCAL_RISK_FLAGS {
        return;
    }
    if seen_flags.insert(flag.clone()) {
        risk_flags.push(flag);
    }
}

fn collect_evolution_history(
    repo: &Repository,
    relative_path: &Path,
    config: &WhyConfig,
    since_days: Option<u64>,
) -> Result<Vec<EvolutionCommit>> {
    let mut revwalk = repo.revwalk().context("failed to create revwalk")?;
    revwalk.push_head().context("failed to walk HEAD")?;
    revwalk
        .set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .context("failed to set revwalk ordering")?;

    let since_ts =
        since_days.map(|days| OffsetDateTime::now_utc().unix_timestamp() - days as i64 * 86_400);

    let mut commits = Vec::new();
    let mut seen = HashSet::new();
    let mut current_path = relative_path.to_path_buf();

    for oid in revwalk {
        let oid = oid.context("failed to read commit from revwalk")?;
        let commit = repo
            .find_commit(oid)
            .with_context(|| format!("failed to load commit {oid}"))?;
        if let Some(since_ts) = since_ts {
            if commit.time().seconds() < since_ts {
                continue;
            }
        }

        let touched_path = commit_touches_path(repo, &commit, &current_path)?;
        if !touched_path {
            continue;
        }

        let path_at_commit = path_for_commit(repo, &commit, &current_path)?;
        let mut evidence = finalize_chain_commit(repo, &commit, 1.0, config)?;
        evidence.relevance_score = compute_relevance_score(&evidence, config);

        if seen.insert(commit.id()) {
            commits.push(EvolutionCommit {
                commit: evidence,
                path_at_commit: path_at_commit.clone(),
            });
        }

        current_path = path_at_commit;
    }

    commits.sort_by(|left, right| right.commit.time.cmp(&left.commit.time));
    Ok(commits)
}

fn collect_evolution_inflections(
    commits: &[EvolutionCommit],
    paths_seen: &[PathBuf],
) -> Vec<EvolutionInflection> {
    let mut inflections = Vec::new();

    if let Some(rename_commit) = commits.iter().find(|entry| {
        paths_seen.len() > 1 && entry.commit.summary.to_ascii_lowercase().contains("rename")
    }) {
        inflections.push(EvolutionInflection {
            category: "rename",
            reason: format!(
                "History crosses {} path variants, so this commit likely marks a rename or move boundary.",
                paths_seen.len()
            ),
            oid: rename_commit.commit.oid.clone(),
            summary: rename_commit.commit.summary.clone(),
            path_at_commit: rename_commit.path_at_commit.clone(),
            date: rename_commit.commit.date.clone(),
        });
    }

    if let Some(risk_commit) = commits
        .iter()
        .filter(|entry| !entry.commit.is_mechanical)
        .find(|entry| commit_signal_strength(&entry.commit) >= 3)
    {
        inflections.push(EvolutionInflection {
            category: "risk-escalation",
            reason: "This commit introduces the strongest historical risk signal in the timeline."
                .into(),
            oid: risk_commit.commit.oid.clone(),
            summary: risk_commit.commit.summary.clone(),
            path_at_commit: risk_commit.path_at_commit.clone(),
            date: risk_commit.commit.date.clone(),
        });
    }

    if let Some(mechanical_tail) = commits
        .iter()
        .find(|entry| entry.commit.is_mechanical && !entry.commit.summary.is_empty())
    {
        inflections.push(EvolutionInflection {
            category: "mechanical-follow-up",
            reason: "A later mechanical change touched the target after substantive behavior had already landed.".into(),
            oid: mechanical_tail.commit.oid.clone(),
            summary: mechanical_tail.commit.summary.clone(),
            path_at_commit: mechanical_tail.path_at_commit.clone(),
            date: mechanical_tail.commit.date.clone(),
        });
    }

    inflections
}

fn summarize_evolution_history(
    commits: &[EvolutionCommit],
    inflections: &[EvolutionInflection],
    paths_seen: &[PathBuf],
    risk_level: RiskLevel,
    local_context: &LocalContext,
) -> String {
    if commits.is_empty() {
        return "No evolution history matched the requested window, so there is not enough evidence to narrate how this target changed over time.".into();
    }

    let latest = &commits[0];
    let origin = commits.last().unwrap_or(latest);
    let mut parts = Vec::new();

    parts.push(format!(
        "Latest state: {} on {} at {}.",
        latest.commit.summary,
        latest.commit.date,
        latest.path_at_commit.display()
    ));

    if latest.commit.oid != origin.commit.oid {
        parts.push(format!(
            "Origin: {} on {} at {}.",
            origin.commit.summary,
            origin.commit.date,
            origin.path_at_commit.display()
        ));
    }

    if paths_seen.len() > 1 {
        let rendered_paths = paths_seen
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" -> ");
        parts.push(format!("Path progression: {rendered_paths}."));
    }

    if !inflections.is_empty() {
        let highlights = inflections
            .iter()
            .map(|point| format!("{} ({})", point.summary, point.category))
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!("Key inflection points: {highlights}."));
    }

    if !local_context.risk_flags.is_empty() {
        parts.push(format!(
            "Local context reinforces this with signals such as {}.",
            local_context.risk_flags.join(", ")
        ));
    }

    parts.push(format!(
        "Overall risk remains {}: {}",
        risk_level.as_str(),
        risk_level.change_guidance()
    ));

    parts.join(" ")
}

fn commit_signal_strength(commit: &CommitEvidence) -> usize {
    let lower = format!("{} {}", commit.summary, commit.message).to_ascii_lowercase();
    [
        "hotfix",
        "security",
        "incident",
        "vulnerability",
        "auth",
        "token",
    ]
    .into_iter()
    .filter(|needle| lower.contains(needle))
    .count()
}

fn commit_touches_path(repo: &Repository, commit: &git2::Commit<'_>, path: &Path) -> Result<bool> {
    let tree = commit.tree().context("failed to load commit tree")?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .context("failed to load commit parent")?
                .tree()
                .context("failed to load parent tree")?,
        )
    } else {
        None
    };

    let mut diff_options = DiffOptions::new();
    diff_options.pathspec(path);
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut diff_options))
        .context("failed to inspect commit diff")?;
    if diff.deltas().len() > 0 {
        return Ok(true);
    }

    if let Some(previous_path) = rename_source_path(repo, commit, path)? {
        let mut old_path_diff_options = DiffOptions::new();
        old_path_diff_options.pathspec(&previous_path);
        let old_path_diff = repo
            .diff_tree_to_tree(
                parent_tree.as_ref(),
                Some(&tree),
                Some(&mut old_path_diff_options),
            )
            .context("failed to inspect commit diff for previous path")?;
        return Ok(old_path_diff.deltas().len() > 0);
    }

    Ok(false)
}

fn path_for_commit(repo: &Repository, commit: &git2::Commit<'_>, path: &Path) -> Result<PathBuf> {
    rename_source_path(repo, commit, path)
        .map(|previous| previous.unwrap_or_else(|| path.to_path_buf()))
}

fn rename_source_path(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    path: &Path,
) -> Result<Option<PathBuf>> {
    if commit.parent_count() == 0 {
        return Ok(None);
    }

    let tree = commit.tree().context("failed to load commit tree")?;
    let parent_tree = commit
        .parent(0)
        .context("failed to load commit parent")?
        .tree()
        .context("failed to load parent tree")?;

    let mut diff = repo
        .diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)
        .context("failed to inspect commit diff for rename source")?;
    diff.find_similar(None)
        .context("failed to detect renames in commit diff")?;

    for delta in diff.deltas() {
        let Some(new_file) = delta.new_file().path() else {
            continue;
        };
        if new_file == path {
            let old_path = delta.old_file().path().map(PathBuf::from);
            if old_path.as_deref() != Some(path) {
                return Ok(old_path);
            }
        }
    }

    Ok(None)
}

fn load_commit_evidence(
    repo: &Repository,
    oid: Oid,
    coverage_score: f32,
    config: &WhyConfig,
) -> Result<CommitEvidence> {
    let commit = explainable_commit(repo, oid, config)?;
    finalize_chain_commit(repo, &commit, coverage_score, config)
}

fn finalize_chain_commit(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    coverage_score: f32,
    config: &WhyConfig,
) -> Result<CommitEvidence> {
    let author = commit.author();
    let author_name = author.name().unwrap_or("unknown").to_string();
    let email = author.email().unwrap_or("unknown").to_string();
    let message = commit
        .message()
        .unwrap_or("(no message)")
        .trim()
        .to_string();
    let summary = commit.summary().unwrap_or("(no summary)").to_string();
    let time = commit.time().seconds();
    let date = format_git_time(time)?;
    let oid_text = commit.id().to_string();
    let diff_excerpt = load_diff_excerpt(repo, commit)?;
    let issue_refs = extract_issue_refs(&message);
    let is_mechanical = is_mechanical_commit(repo, commit, &summary, &diff_excerpt, config)?;

    Ok(CommitEvidence {
        short_oid: oid_text.chars().take(8).collect(),
        oid: oid_text,
        author: author_name,
        email,
        time,
        date,
        summary,
        message,
        diff_excerpt,
        coverage_score,
        relevance_score: 0.0,
        issue_refs,
        is_mechanical,
    })
}

fn explainable_commit<'repo>(
    repo: &'repo Repository,
    oid: Oid,
    config: &WhyConfig,
) -> Result<git2::Commit<'repo>> {
    let mut current = repo
        .find_commit(oid)
        .with_context(|| format!("failed to load commit {oid}"))?;

    loop {
        let parent_count = current.parent_count();
        if parent_count > 1 {
            let mut selected = None;
            for index in (0..parent_count).rev() {
                let parent = current
                    .parent(index)
                    .context("failed to load parent while following merge origin")?;
                if !commit_is_mechanical(repo, &parent, config)? {
                    selected = Some(parent);
                    break;
                }
                selected.get_or_insert(parent);
            }
            current = selected.context("merge commit had no parents to follow")?;
            continue;
        }

        if commit_is_mechanical(repo, &current, config)? {
            if parent_count == 0 {
                break;
            }
            current = current
                .parent(0)
                .context("failed to load parent while following merge origin")?;
            continue;
        }

        break;
    }

    Ok(current)
}

fn compute_relevance_score(commit: &CommitEvidence, config: &WhyConfig) -> f32 {
    let mut score = commit.coverage_score * 100.0;
    let combined_text = format!("{}\n{}", commit.summary, commit.message).to_ascii_lowercase();

    if contains_any(&combined_text, HIGH_SIGNAL_KEYWORDS)
        || contains_custom_any(&combined_text, &config.risk.keywords.high)
    {
        score += 30.0;
    }
    if contains_any(&combined_text, MEDIUM_SIGNAL_KEYWORDS)
        || contains_custom_any(&combined_text, &config.risk.keywords.medium)
    {
        score += 15.0;
    }
    if contains_any(&combined_text, RISK_DOMAIN_KEYWORDS) {
        score += 20.0;
    }

    score += commit.issue_refs.len() as f32 * 10.0;
    if !commit.diff_excerpt.is_empty() {
        score += 5.0;
    }
    score += recency_bonus(commit.time, config.git.recency_window_days);

    if commit.is_mechanical {
        score -= 40.0;
    }

    score.max(0.0)
}

fn recency_bonus(commit_time: i64, recency_window_days: i64) -> f32 {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let age_days = ((now - commit_time).max(0)) / 86_400;
    if age_days >= recency_window_days {
        0.0
    } else {
        20.0 * (1.0 - age_days as f32 / recency_window_days as f32)
    }
}

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| text.contains(keyword))
}

fn contains_custom_any(text: &str, keywords: &[String]) -> bool {
    keywords
        .iter()
        .map(|keyword| keyword.to_ascii_lowercase())
        .any(|keyword| text.contains(&keyword))
}

fn select_top_commits(
    mut commits: Vec<CommitEvidence>,
    max_relevant_commits: usize,
) -> Vec<CommitEvidence> {
    commits.sort_by(|left, right| {
        right
            .relevance_score
            .partial_cmp(&left.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    commits.truncate(max_relevant_commits);
    commits
}

fn load_diff_excerpt(repo: &Repository, commit: &git2::Commit<'_>) -> Result<String> {
    let tree = commit.tree().context("failed to load commit tree")?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .context("failed to load commit parent")?
                .tree()
                .context("failed to load parent tree")?,
        )
    } else {
        None
    };

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .context("failed to diff commit against parent")?;

    if diff.deltas().len() == 0 {
        return Ok(String::new());
    }

    let mut patch_text = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        let rendered = std::str::from_utf8(line.content()).unwrap_or_default();
        patch_text.push_str(rendered);
        true
    })
    .context("failed to render commit diff")?;

    Ok(truncate_chars(patch_text.trim(), MAX_DIFF_EXCERPT_CHARS))
}

fn extract_issue_refs(message: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    for token in message.split(|c: char| c.is_whitespace() || [',', ';', '(', ')'].contains(&c)) {
        if let Some(issue_ref) = normalize_issue_ref(token) {
            if seen.insert(issue_ref.clone()) {
                refs.push(issue_ref);
            }
        }
    }
    refs
}

fn normalize_issue_ref(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|c: char| matches!(c, '.' | ':' | '!' | '?' | '[' | ']'));
    if let Some(stripped) = trimmed.strip_prefix('#') {
        if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("#{stripped}"));
        }
    }
    None
}

pub fn commit_is_mechanical(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    config: &WhyConfig,
) -> Result<bool> {
    let summary = commit.summary().unwrap_or("(no summary)");
    let diff_excerpt = load_diff_excerpt(repo, commit)?;
    is_mechanical_commit(repo, commit, summary, &diff_excerpt, config)
}

fn is_mechanical_commit(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    summary: &str,
    diff_excerpt: &str,
    config: &WhyConfig,
) -> Result<bool> {
    if has_mechanical_summary(summary) {
        return Ok(true);
    }

    let tree = commit.tree().context("failed to load commit tree")?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .context("failed to load commit parent")?
                .tree()
                .context("failed to load parent tree")?,
        )
    } else {
        None
    };

    let mut diff_options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut diff_options))
        .context("failed to inspect commit diff")?;

    if diff.deltas().len() > config.git.mechanical_threshold_files {
        return Ok(true);
    }

    Ok(!diff_excerpt.is_empty() && diff_excerpt.lines().all(is_whitespace_only_diff_line))
}

fn has_mechanical_summary(summary: &str) -> bool {
    let summary = summary.to_ascii_lowercase();
    summary.starts_with("chore:")
        || summary.starts_with("fmt")
        || summary.starts_with("format")
        || summary.starts_with("bump ")
        || summary.contains("merge branch")
        || summary.contains("merge pull request")
}

fn is_whitespace_only_diff_line(line: &str) -> bool {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
        return true;
    }

    let content = line
        .strip_prefix('+')
        .or_else(|| line.strip_prefix('-'))
        .unwrap_or(line);
    content.trim().is_empty()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

const HIGH_SIGNAL_KEYWORDS: &[&str] = &[
    "hotfix",
    "security",
    "vulnerability",
    "cve",
    "auth",
    "bypass",
    "incident",
    "postmortem",
    "rollback",
    "revert",
    "critical",
    "emergency",
    "breach",
    "exploit",
];

const MEDIUM_SIGNAL_KEYWORDS: &[&str] = &[
    "fix",
    "bug",
    "workaround",
    "temporary",
    "compat",
    "migration",
    "deprecated",
    "legacy",
    "backport",
    "regression",
];

const RISK_DOMAIN_KEYWORDS: &[&str] = &[
    "permission",
    "session",
    "token",
    "cookie",
    "password",
    "secret",
    "key",
    "cert",
    "tls",
    "ssl",
    "csrf",
    "xss",
    "injection",
    "sanitize",
];

fn format_git_time(seconds: i64) -> Result<String> {
    let timestamp = OffsetDateTime::from_unix_timestamp(seconds)
        .with_context(|| format!("invalid git timestamp {seconds}"))?;
    let iso = timestamp
        .format(&Iso8601::DATE)
        .context("failed to format commit date")?;
    Ok(iso)
}

pub fn infer_risk_level(
    commits: &[CommitEvidence],
    local_context: &LocalContext,
    config: &WhyConfig,
) -> RiskLevel {
    let default_level = parse_default_risk_level(&config.risk.default_level);
    if commits
        .iter()
        .any(|commit| has_high_signal_marker(commit, config))
        || local_context_has_high_signal(local_context, config)
    {
        RiskLevel::HIGH
    } else if commits
        .iter()
        .any(|commit| has_medium_signal_marker(commit, config))
        || local_context_has_medium_signal(local_context, config)
    {
        RiskLevel::MEDIUM
    } else if commits.len() <= 1 {
        default_level
    } else {
        RiskLevel::MEDIUM
    }
}

fn has_high_signal_marker(commit: &CommitEvidence, config: &WhyConfig) -> bool {
    let combined_text = format!("{}\n{}", commit.summary, commit.message).to_ascii_lowercase();
    ["hotfix", "security", "incident", "vulnerability"]
        .iter()
        .any(|marker| combined_text.contains(marker))
        || contains_custom_any(&combined_text, &config.risk.keywords.high)
}

fn has_medium_signal_marker(commit: &CommitEvidence, config: &WhyConfig) -> bool {
    let combined_text = format!("{}\n{}", commit.summary, commit.message).to_ascii_lowercase();
    contains_any(&combined_text, MEDIUM_SIGNAL_KEYWORDS)
        || contains_custom_any(&combined_text, &config.risk.keywords.medium)
}

fn local_context_has_high_signal(local_context: &LocalContext, config: &WhyConfig) -> bool {
    local_context
        .comments
        .iter()
        .chain(local_context.markers.iter())
        .any(|text| {
            let lower = text.to_ascii_lowercase();
            contains_any(&lower, HIGH_SIGNAL_KEYWORDS)
                || contains_custom_any(&lower, &config.risk.keywords.high)
                || contains_any(&lower, RISK_DOMAIN_KEYWORDS)
        })
}

fn local_context_has_medium_signal(local_context: &LocalContext, config: &WhyConfig) -> bool {
    local_context
        .comments
        .iter()
        .chain(local_context.markers.iter())
        .any(|text| {
            let lower = text.to_ascii_lowercase();
            contains_any(&lower, MEDIUM_SIGNAL_KEYWORDS)
                || contains_custom_any(&lower, &config.risk.keywords.medium)
        })
        || local_context
            .risk_flags
            .iter()
            .any(|flag| matches!(flag.as_str(), "hack" | "fixme" | "temp" | "safety"))
}

fn parse_default_risk_level(level: &str) -> RiskLevel {
    match level.trim().to_ascii_uppercase().as_str() {
        "HIGH" => RiskLevel::HIGH,
        "MEDIUM" => RiskLevel::MEDIUM,
        _ => RiskLevel::LOW,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitEvidence, LocalContext, RiskLevel, blame_chain, blame_commit_evidence,
        collect_evolution_history, compute_relevance_score, discover_repository,
        explainable_commit, extract_issue_refs, extract_local_context, has_mechanical_summary,
        infer_risk_level, is_whitespace_only_diff_line, parse_default_risk_level,
        relative_repo_path, select_top_commits, truncate_chars,
    };
    use anyhow::{Context, Result};
    use git2::Repository;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;
    use time::OffsetDateTime;
    use why_context::WhyConfig;

    #[test]
    fn discovers_repository_from_workspace_root() {
        let repo = discover_repository(Path::new(".")).expect("repo should be discoverable");
        assert!(repo.workdir().is_some());
    }

    #[test]
    fn computes_relative_repo_path() {
        let repo = discover_repository(Path::new(".")).expect("repo should be discoverable");
        let relative =
            relative_repo_path(&repo, Path::new("README.md")).expect("path should be relative");
        assert_eq!(relative, Path::new("README.md"));
    }

    #[cfg(unix)]
    #[test]
    fn computes_relative_repo_path_from_symlink_alias() -> Result<()> {
        let dir = TempDir::new()?;
        let repo_root = dir.path().join("repo");
        fs::create_dir_all(&repo_root)?;
        let repo = Repository::init(&repo_root)?;
        let file_path = repo_root.join("sample.rs");
        fs::write(&file_path, "fn helper() {}\n")?;

        let alias_root = dir.path().join("repo-alias");
        std::os::unix::fs::symlink(&repo_root, &alias_root)?;
        let alias_file_path = alias_root.join("sample.rs");

        let relative = relative_repo_path(&repo, &alias_file_path)?;
        assert_eq!(relative, Path::new("sample.rs"));
        Ok(())
    }

    #[test]
    fn explainable_commit_walks_past_merge_wrappers() -> Result<()> {
        let dir = TempDir::new()?;
        let repo = Repository::init(dir.path())?;
        {
            let mut cfg = repo.config()?;
            cfg.set_str("user.name", "Fixture Bot")?;
            cfg.set_str("user.email", "test@example.com")?;
        }

        let path = dir.path().join("sample.rs");
        fs::write(&path, "pub fn helper() {\n    1\n}\n")?;
        std::process::Command::new("git")
            .args(["add", "sample.rs"])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "feat: add helper"])
            .current_dir(dir.path())
            .output()?;

        std::process::Command::new("git")
            .args(["checkout", "-b", "feature"])
            .current_dir(dir.path())
            .output()?;
        fs::write(&path, "pub fn helper() {\n    2\n}\n")?;
        std::process::Command::new("git")
            .args(["add", "sample.rs"])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "fix: adjust helper"])
            .current_dir(dir.path())
            .output()?;

        std::process::Command::new("git")
            .args(["checkout", "master"])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args([
                "merge",
                "--no-ff",
                "feature",
                "-m",
                "Merge branch 'feature'",
            ])
            .current_dir(dir.path())
            .output()?;

        let head = repo.head()?.target().context("expected HEAD target")?;
        let explainable = explainable_commit(&repo, head, &WhyConfig::default())?;
        assert!(
            explainable
                .summary()
                .unwrap_or_default()
                .contains("adjust helper")
        );
        Ok(())
    }

    #[test]
    fn blames_a_single_line() {
        let repo = discover_repository(Path::new(".")).expect("repo should be discoverable");
        let relative =
            relative_repo_path(&repo, Path::new("README.md")).expect("path should be relative");
        let commits = blame_commit_evidence(&repo, &relative, 1, 1, &WhyConfig::default(), None)
            .expect("blame should succeed");

        assert!(!commits.is_empty());
        assert!(!commits[0].short_oid.is_empty());
        assert!(!commits[0].summary.is_empty());
    }

    #[test]
    fn blame_chain_collects_mechanical_skip_and_origin() -> Result<()> {
        let dir = TempDir::new()?;
        let repo = Repository::init(dir.path())?;
        {
            let mut cfg = repo.config()?;
            cfg.set_str("user.name", "Fixture Bot")?;
            cfg.set_str("user.email", "test@example.com")?;
        }

        let path = dir.path().join("sample.rs");
        fs::write(
            &path,
            "pub fn helper() {\n    // security: keep validation path intact\n    rate_limit_check();\n}\n",
        )?;
        std::process::Command::new("git")
            .args(["add", "sample.rs"])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "hotfix: add helper guard"])
            .current_dir(dir.path())
            .output()?;

        fs::write(
            &path,
            "pub fn helper() {\n        // security: keep validation path intact\n        rate_limit_check();\n}\n",
        )?;
        std::process::Command::new("git")
            .args(["add", "sample.rs"])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "fmt: align helper indentation"])
            .current_dir(dir.path())
            .output()?;

        let relative = relative_repo_path(&repo, &path)?;
        let trace = blame_chain(&repo, &relative, 1, 4, &WhyConfig::default(), None)?;
        assert_eq!(trace.chain_depth, 1);
        assert_eq!(trace.noise_commits_skipped.len(), 1);
        assert!(trace.noise_commits_skipped[0].is_mechanical);
        assert!(
            trace.noise_commits_skipped[0]
                .summary
                .contains("fmt: align helper indentation")
        );
        assert!(
            trace
                .origin_commit
                .summary
                .contains("hotfix: add helper guard")
        );
        assert!(!trace.origin_commit.is_mechanical);

        Ok(())
    }

    #[test]
    fn high_signal_commits_produce_high_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: 1_710_000_000,
            date: "2026-03-10".into(),
            summary: "hotfix: close security vulnerability".into(),
            message: "hotfix: close security vulnerability".into(),
            diff_excerpt: String::new(),
            coverage_score: 1.0,
            relevance_score: 0.0,
            issue_refs: Vec::new(),
            is_mechanical: false,
        }];

        assert_eq!(
            infer_risk_level(
                &commits,
                &LocalContext {
                    comments: Vec::new(),
                    markers: Vec::new(),
                    risk_flags: Vec::new(),
                },
                &WhyConfig::default(),
            ),
            RiskLevel::HIGH
        );
    }

    #[test]
    fn sparse_history_produces_low_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: 1_710_000_000,
            date: "2026-03-10".into(),
            summary: "feat: add helper".into(),
            message: "feat: add helper".into(),
            diff_excerpt: String::new(),
            coverage_score: 1.0,
            relevance_score: 0.0,
            issue_refs: Vec::new(),
            is_mechanical: false,
        }];

        assert_eq!(
            infer_risk_level(
                &commits,
                &LocalContext {
                    comments: Vec::new(),
                    markers: Vec::new(),
                    risk_flags: Vec::new(),
                },
                &WhyConfig::default(),
            ),
            RiskLevel::LOW
        );
    }

    #[test]
    fn custom_high_keywords_raise_risk_and_relevance() {
        let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/fixtures/config/high-risk.toml");
        let config = why_context::load_config_from_path(&config_path).expect("config should load");
        let commit = CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: OffsetDateTime::now_utc().unix_timestamp(),
            date: "2026-03-11".into(),
            summary: "feat: add reconciliation guard".into(),
            message: "feat: add reconciliation guard for settlement pipeline".into(),
            diff_excerpt: String::new(),
            coverage_score: 0.4,
            relevance_score: 0.0,
            issue_refs: Vec::new(),
            is_mechanical: false,
        };

        assert_eq!(
            infer_risk_level(
                std::slice::from_ref(&commit),
                &LocalContext {
                    comments: Vec::new(),
                    markers: Vec::new(),
                    risk_flags: Vec::new(),
                },
                &config,
            ),
            RiskLevel::HIGH
        );
        assert!(
            compute_relevance_score(&commit, &config)
                > compute_relevance_score(&commit, &WhyConfig::default())
        );
    }

    #[test]
    fn default_risk_level_parsing_handles_expected_values() {
        assert_eq!(parse_default_risk_level("high"), RiskLevel::HIGH);
        assert_eq!(parse_default_risk_level("medium"), RiskLevel::MEDIUM);
        assert_eq!(parse_default_risk_level("anything-else"), RiskLevel::LOW);
    }

    #[test]
    fn risk_levels_expose_user_facing_semantics() {
        assert!(RiskLevel::HIGH.summary().contains("security sensitivity"));
        assert!(
            RiskLevel::HIGH
                .change_guidance()
                .contains("Stop and investigate")
        );
        assert!(
            RiskLevel::MEDIUM
                .summary()
                .contains("transitional behavior")
        );
        assert!(
            RiskLevel::MEDIUM
                .change_guidance()
                .contains("reviewing surrounding code")
        );
        assert!(
            RiskLevel::LOW
                .summary()
                .contains("unusual operational or compatibility pressure")
        );
        assert!(RiskLevel::LOW.change_guidance().contains("ordinary code"));
    }

    #[test]
    fn extracts_issue_references_from_commit_messages() {
        let refs = extract_issue_refs("fix: preserve behavior (#318) closes #4521 and refs #4521");
        assert_eq!(refs, vec!["#318", "#4521"]);
    }

    #[test]
    fn mechanical_summary_markers_are_detected() {
        assert!(has_mechanical_summary("chore: run formatter"));
        assert!(has_mechanical_summary(
            "Merge pull request #42 from feature/foo"
        ));
        assert!(!has_mechanical_summary("fix: preserve charset handling"));
    }

    #[test]
    fn whitespace_only_diff_lines_are_detected() {
        assert!(is_whitespace_only_diff_line("@@ -1,2 +1,2 @@"));
        assert!(is_whitespace_only_diff_line("-    "));
        assert!(is_whitespace_only_diff_line("+\t"));
        assert!(!is_whitespace_only_diff_line(
            "+    rate_limit_check(\"payment\")?;"
        ));
    }

    #[test]
    fn truncates_diff_excerpt_to_budget() {
        let text = "x".repeat(600);
        assert_eq!(truncate_chars(&text, 500).chars().count(), 500);
    }

    #[test]
    fn extracts_comments_markers_and_risk_flags_from_local_window() -> Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("sample.rs");
        fs::write(
            &path,
            "pub fn helper() {}\n// auth: validate bearer token before session refresh\n// TODO: remove after mobile rollout\nfn target() {\n    do_work();\n}\n",
        )?;

        let context = extract_local_context(&path, 4, 5, &WhyConfig::default())?;
        assert!(
            context
                .comments
                .iter()
                .any(|comment| comment.contains("validate bearer token"))
        );
        assert!(
            context
                .markers
                .iter()
                .any(|marker| marker.contains("TODO: remove after mobile rollout"))
        );
        assert!(context.risk_flags.iter().any(|flag| flag == "auth"));
        assert!(context.risk_flags.iter().any(|flag| flag == "token"));

        Ok(())
    }

    #[test]
    fn local_context_high_signal_produces_high_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: 1_710_000_000,
            date: "2026-03-10".into(),
            summary: "feat: add helper".into(),
            message: "feat: add helper".into(),
            diff_excerpt: String::new(),
            coverage_score: 1.0,
            relevance_score: 0.0,
            issue_refs: Vec::new(),
            is_mechanical: false,
        }];

        assert_eq!(
            infer_risk_level(
                &commits,
                &LocalContext {
                    comments: vec!["security: validate session token before auth refresh".into()],
                    markers: Vec::new(),
                    risk_flags: vec!["security".into(), "session".into(), "token".into()],
                },
                &WhyConfig::default(),
            ),
            RiskLevel::HIGH
        );
    }

    #[test]
    fn local_context_marker_produces_medium_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: 1_710_000_000,
            date: "2026-03-10".into(),
            summary: "feat: add helper".into(),
            message: "feat: add helper".into(),
            diff_excerpt: String::new(),
            coverage_score: 1.0,
            relevance_score: 0.0,
            issue_refs: Vec::new(),
            is_mechanical: false,
        }];

        assert_eq!(
            infer_risk_level(
                &commits,
                &LocalContext {
                    comments: Vec::new(),
                    markers: vec!["HACK: temporary compatibility path".into()],
                    risk_flags: vec!["hack".into()],
                },
                &WhyConfig::default(),
            ),
            RiskLevel::MEDIUM
        );
    }

    #[test]
    fn high_signal_scoring_beats_plain_commit() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let high_signal = CommitEvidence {
            oid: "a".into(),
            short_oid: "a".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: now,
            date: "2026-03-11".into(),
            summary: "hotfix: auth bypass incident".into(),
            message: "hotfix: auth bypass incident closes #42".into(),
            diff_excerpt: "diff --git a/src/auth.rs b/src/auth.rs".into(),
            coverage_score: 0.5,
            relevance_score: 0.0,
            issue_refs: vec!["#42".into()],
            is_mechanical: false,
        };
        let plain = CommitEvidence {
            oid: "b".into(),
            short_oid: "b".into(),
            author: "bob".into(),
            email: "bob@example.com".into(),
            time: now - 120 * 86_400,
            date: "2025-11-11".into(),
            summary: "feat: add helper".into(),
            message: "feat: add helper".into(),
            diff_excerpt: String::new(),
            coverage_score: 0.5,
            relevance_score: 0.0,
            issue_refs: Vec::new(),
            is_mechanical: false,
        };

        assert!(
            compute_relevance_score(&high_signal, &WhyConfig::default())
                > compute_relevance_score(&plain, &WhyConfig::default())
        );
    }

    #[test]
    fn mechanical_penalty_reduces_relevance() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut commit = CommitEvidence {
            oid: "a".into(),
            short_oid: "a".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            time: now,
            date: "2026-03-11".into(),
            summary: "fix: preserve session token flow".into(),
            message: "fix: preserve session token flow closes #52".into(),
            diff_excerpt: "diff --git a/src/auth.rs b/src/auth.rs".into(),
            coverage_score: 0.8,
            relevance_score: 0.0,
            issue_refs: vec!["#52".into()],
            is_mechanical: false,
        };
        let non_mechanical = compute_relevance_score(&commit, &WhyConfig::default());
        commit.is_mechanical = true;
        let mechanical = compute_relevance_score(&commit, &WhyConfig::default());

        assert!(mechanical < non_mechanical);
    }

    #[test]
    fn top_n_selection_keeps_highest_scores() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let commits: Vec<_> = (0..10)
            .map(|index| CommitEvidence {
                oid: format!("oid-{index}"),
                short_oid: format!("{index}"),
                author: "alice".into(),
                email: "alice@example.com".into(),
                time: now,
                date: "2026-03-11".into(),
                summary: format!("commit-{index}"),
                message: String::new(),
                diff_excerpt: String::new(),
                coverage_score: 0.1,
                relevance_score: index as f32,
                issue_refs: Vec::new(),
                is_mechanical: false,
            })
            .collect();

        let selected = select_top_commits(commits, 8);
        assert_eq!(selected.len(), 8);
        assert_eq!(
            selected.first().map(|commit| commit.relevance_score),
            Some(9.0)
        );
        assert_eq!(
            selected.last().map(|commit| commit.relevance_score),
            Some(2.0)
        );
    }

    #[test]
    fn collect_evolution_history_follows_git_mv_renames() -> Result<()> {
        let dir = TempDir::new()?;
        let repo = Repository::init(dir.path())?;
        {
            let mut cfg = repo.config()?;
            cfg.set_str("user.name", "Fixture Bot")?;
            cfg.set_str("user.email", "test@example.com")?;
        }

        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir)?;
        let old_path = src_dir.join("legacy.rs");
        fs::write(&old_path, "pub fn helper() {\n    old_logic();\n}\n")?;
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "feat: add legacy helper"])
            .current_dir(dir.path())
            .output()?;

        let new_path = src_dir.join("modern.rs");
        std::process::Command::new("git")
            .args(["mv", "src/legacy.rs", "src/modern.rs"])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "refactor: rename helper module"])
            .current_dir(dir.path())
            .output()?;

        fs::write(&new_path, "pub fn helper() {\n    new_logic();\n}\n")?;
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()?;
        std::process::Command::new("git")
            .args(["commit", "-m", "fix: update helper behavior"])
            .current_dir(dir.path())
            .output()?;

        let relative = relative_repo_path(&repo, &new_path)?;
        let history = collect_evolution_history(&repo, &relative, &WhyConfig::default(), None)?;

        assert_eq!(history.len(), 3);
        assert_eq!(history[0].path_at_commit, Path::new("src/modern.rs"));
        assert_eq!(history[1].path_at_commit, Path::new("src/legacy.rs"));
        assert_eq!(history[2].path_at_commit, Path::new("src/legacy.rs"));
        assert!(
            history
                .iter()
                .any(|entry| entry.commit.summary.contains("rename helper module"))
        );
        assert!(
            history
                .iter()
                .any(|entry| entry.commit.summary.contains("add legacy helper"))
        );
        Ok(())
    }
}
