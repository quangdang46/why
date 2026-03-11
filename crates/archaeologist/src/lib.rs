use anyhow::{Context, Result, bail};
use git2::{BlameOptions, Oid, Repository};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;
use why_locator::{QueryKind, QueryTarget, resolve_target};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommitEvidence {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub date: String,
    pub summary: String,
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
    let summary = commit.summary().unwrap_or("(no summary)").to_string();
    let date = format_git_time(commit.time().seconds())?;
    let oid_text = oid.to_string();

    Ok(CommitEvidence {
        short_oid: oid_text.chars().take(8).collect(),
        oid: oid_text,
        author: author_name,
        date,
        summary,
    })
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
        CommitEvidence, RiskLevel, blame_commit_evidence, discover_repository, infer_risk_level,
        relative_repo_path,
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
            date: "2026-03-10".into(),
            summary: "hotfix: close security vulnerability".into(),
        }];

        assert_eq!(infer_risk_level(&commits), RiskLevel::HIGH);
    }

    #[test]
    fn sparse_history_produces_low_risk() {
        let commits = vec![CommitEvidence {
            oid: "abc".into(),
            short_oid: "abc".into(),
            author: "alice".into(),
            date: "2026-03-10".into(),
            summary: "feat: add helper".into(),
        }];

        assert_eq!(infer_risk_level(&commits), RiskLevel::LOW);
    }
}
