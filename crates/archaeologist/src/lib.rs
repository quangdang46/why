use anyhow::{Context, Result, bail};
use git2::{BlameOptions, DiffFormat, DiffOptions, Oid, Repository};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;
use why_locator::{QueryKind, QueryTarget, resolve_target};

const MAX_DIFF_EXCERPT_CHARS: usize = 500;
const MECHANICAL_FILE_THRESHOLD: usize = 50;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommitEvidence {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub summary: String,
    pub message: String,
    pub diff_excerpt: String,
    pub issue_refs: Vec<String>,
    pub is_mechanical: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OutputTarget {
    pub path: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub query_kind: QueryKind,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchaeologyResult {
    pub target: OutputTarget,
    pub commits: Vec<CommitEvidence>,
    pub risk_level: RiskLevel,
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

    candidate
        .strip_prefix(workdir)
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
    let resolved = resolve_target(target, cwd)?;

    let target_path = cwd.join(&resolved.path);
    let repo = discover_repository(&target_path)?;
    let relative_path = relative_repo_path(&repo, &target_path)?;
    let commits = blame_commit_evidence(
        &repo,
        &relative_path,
        resolved.start_line,
        resolved.end_line,
    )?;

    Ok(ArchaeologyResult {
        target: OutputTarget {
            path: relative_path,
            start_line: resolved.start_line,
            end_line: resolved.end_line,
            query_kind: resolved.query_kind,
        },
        risk_level: infer_risk_level(&commits),
        commits,
        mode: "heuristic",
        notes: vec!["No LLM synthesis in phase 1"],
    })
}

pub fn blame_commit_evidence(
    repo: &Repository,
    relative_path: &Path,
    start_line: u32,
    end_line: u32,
) -> Result<Vec<CommitEvidence>> {
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

    let blame = repo
        .blame_file(relative_path, Some(&mut options))
        .with_context(|| format!("failed to blame {}", relative_path.display()))?;

    let mut seen = HashSet::new();
    let mut ordered_oids = Vec::new();

    for hunk in blame.iter() {
        let oid = hunk.final_commit_id();
        if oid.is_zero() || !seen.insert(oid) {
            continue;
        }
        ordered_oids.push(oid);
    }

    if ordered_oids.is_empty() {
        bail!(
            "no commits found for {}:{}-{}",
            relative_path.display(),
            start_line,
            end_line
        );
    }

    ordered_oids
        .into_iter()
        .map(|oid| load_commit_evidence(repo, oid))
        .collect()
}

fn load_commit_evidence(repo: &Repository, oid: Oid) -> Result<CommitEvidence> {
    let commit = repo
        .find_commit(oid)
        .with_context(|| format!("failed to load commit {oid}"))?;
    let author = commit.author();
    let author_name = author.name().unwrap_or("unknown").to_string();
    let email = author.email().unwrap_or("unknown").to_string();
    let message = commit
        .message()
        .unwrap_or("(no message)")
        .trim()
        .to_string();
    let summary = commit.summary().unwrap_or("(no summary)").to_string();
    let date = format_git_time(commit.time().seconds())?;
    let oid_text = oid.to_string();
    let diff_excerpt = load_diff_excerpt(repo, &commit)?;
    let issue_refs = extract_issue_refs(&message);
    let is_mechanical = is_mechanical_commit(repo, &commit, &summary, &diff_excerpt)?;

    Ok(CommitEvidence {
        short_oid: oid_text.chars().take(8).collect(),
        oid: oid_text,
        author: author_name,
        email,
        date,
        summary,
        message,
        diff_excerpt,
        issue_refs,
        is_mechanical,
    })
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

fn is_mechanical_commit(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    summary: &str,
    diff_excerpt: &str,
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

    if diff.deltas().len() > MECHANICAL_FILE_THRESHOLD {
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

fn format_git_time(seconds: i64) -> Result<String> {
    let timestamp = OffsetDateTime::from_unix_timestamp(seconds)
        .with_context(|| format!("invalid git timestamp {seconds}"))?;
    let iso = timestamp
        .format(&Iso8601::DATE)
        .context("failed to format commit date")?;
    Ok(iso)
}

pub fn infer_risk_level(commits: &[CommitEvidence]) -> RiskLevel {
    if commits
        .iter()
        .any(|commit| has_high_signal_marker(&commit.summary))
    {
        RiskLevel::HIGH
    } else if commits.len() <= 1 {
        RiskLevel::LOW
    } else {
        RiskLevel::MEDIUM
    }
}

fn has_high_signal_marker(summary: &str) -> bool {
    let summary = summary.to_ascii_lowercase();
    ["hotfix", "security", "incident", "vulnerability"]
        .iter()
        .any(|marker| summary.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{
        CommitEvidence, RiskLevel, blame_commit_evidence, discover_repository, extract_issue_refs,
        has_mechanical_summary, infer_risk_level, is_whitespace_only_diff_line, relative_repo_path,
        truncate_chars,
    };
    use std::path::Path;

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

    #[test]
    fn blames_a_single_line() {
        let repo = discover_repository(Path::new(".")).expect("repo should be discoverable");
        let relative =
            relative_repo_path(&repo, Path::new("README.md")).expect("path should be relative");
        let commits = blame_commit_evidence(&repo, &relative, 1, 1).expect("blame should succeed");

        assert!(!commits.is_empty());
        assert!(!commits[0].short_oid.is_empty());
        assert!(!commits[0].summary.is_empty());
    }

    #[test]
    fn high_signal_commits_produce_high_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            date: "2026-03-10".into(),
            summary: "hotfix: close security vulnerability".into(),
            message: "hotfix: close security vulnerability".into(),
            diff_excerpt: String::new(),
            issue_refs: Vec::new(),
            is_mechanical: false,
        }];

        assert_eq!(infer_risk_level(&commits), RiskLevel::HIGH);
    }

    #[test]
    fn sparse_history_produces_low_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            email: "alice@example.com".into(),
            date: "2026-03-10".into(),
            summary: "feat: add helper".into(),
            message: "feat: add helper".into(),
            diff_excerpt: String::new(),
            issue_refs: Vec::new(),
            is_mechanical: false,
        }];

        assert_eq!(infer_risk_level(&commits), RiskLevel::LOW);
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
}
