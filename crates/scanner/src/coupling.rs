use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository, Sort};
use serde::Serialize;
use why_archaeologist::{commit_is_mechanical, discover_repository, relative_repo_path};
use why_context::load_config;
use why_locator::{QueryTarget, resolve_target};

use crate::is_source_file;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CouplingFinding {
    pub path: PathBuf,
    pub shared_commits: usize,
    pub target_commit_count: usize,
    pub coupling_ratio: f64,
    pub top_commit_summaries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CouplingReport {
    pub target_path: PathBuf,
    pub scan_commits: usize,
    pub target_commit_count: usize,
    pub results: Vec<CouplingFinding>,
}

pub fn scan_coupling(
    repo_root: &Path,
    target: &QueryTarget,
    limit: usize,
) -> Result<CouplingReport> {
    let repo = discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;
    let config = load_config(workdir)?;
    let resolved = resolve_target(target, repo_root)?;
    let target_path = relative_repo_path(&repo, &resolved.path)?;

    let mut revwalk = repo.revwalk().context("failed to create revwalk")?;
    revwalk.push_head().context("failed to walk HEAD")?;
    revwalk
        .set_sorting(Sort::TIME)
        .context("failed to set revwalk ordering")?;

    let mut target_commit_count = 0;
    let mut scan_commits = 0;
    let mut candidates: HashMap<PathBuf, CandidateStats> = HashMap::new();

    for oid in revwalk.take(config.git.coupling_scan_commits) {
        let oid = oid.context("failed to read commit from revwalk")?;
        let commit = repo
            .find_commit(oid)
            .with_context(|| format!("failed to load commit {oid}"))?;
        scan_commits += 1;

        if commit_is_mechanical(&repo, &commit, &config)? {
            continue;
        }

        let touched_paths = commit_touched_source_paths(&repo, &commit)?;
        if !touched_paths.iter().any(|path| path == &target_path) {
            continue;
        }

        target_commit_count += 1;
        let summary = commit.summary().unwrap_or("(no summary)").to_string();
        for path in touched_paths {
            if path == target_path {
                continue;
            }
            let entry = candidates.entry(path).or_default();
            entry.shared_commits += 1;
            entry.commit_summaries.push(summary.clone());
        }
    }

    let mut results = candidates
        .into_iter()
        .filter_map(|(path, stats)| {
            if target_commit_count == 0 || stats.shared_commits == 0 {
                return None;
            }
            let coupling_ratio = stats.shared_commits as f64 / target_commit_count as f64;
            if coupling_ratio < config.git.coupling_ratio_threshold {
                return None;
            }
            let mut top_commit_summaries = stats.commit_summaries;
            top_commit_summaries.sort();
            top_commit_summaries.dedup();
            top_commit_summaries.reverse();
            top_commit_summaries.truncate(3);
            Some(CouplingFinding {
                path,
                shared_commits: stats.shared_commits,
                target_commit_count,
                coupling_ratio,
                top_commit_summaries,
            })
        })
        .collect::<Vec<_>>();

    results.sort_by(|left, right| {
        right
            .coupling_ratio
            .partial_cmp(&left.coupling_ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(right.shared_commits.cmp(&left.shared_commits))
            .then(left.path.cmp(&right.path))
    });
    results.truncate(limit.max(1));

    Ok(CouplingReport {
        target_path,
        scan_commits,
        target_commit_count,
        results,
    })
}

#[derive(Debug, Default)]
struct CandidateStats {
    shared_commits: usize,
    commit_summaries: Vec<String>,
}

fn commit_touched_source_paths(
    repo: &Repository,
    commit: &git2::Commit<'_>,
) -> Result<Vec<PathBuf>> {
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

    let mut options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))
        .context("failed to inspect commit diff")?;

    let mut paths = Vec::new();
    for delta in diff.deltas() {
        for path in [delta.new_file().path(), delta.old_file().path()]
            .into_iter()
            .flatten()
        {
            if is_source_file(path) && !paths.iter().any(|existing| existing == path) {
                paths.push(path.to_path_buf());
            }
        }
    }

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::scan_coupling;
    use anyhow::Result;
    use why_locator::parse_target;

    fn setup_coupling_repo() -> Result<tempfile::TempDir> {
        let dir = tempfile::TempDir::new()?;
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("coupling_repo")
            .join("setup.sh");
        let output = std::process::Command::new("bash")
            .arg(fixture)
            .arg(dir.path())
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "coupling fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_coupling_reports_expected_positive_pair() -> Result<()> {
        let repo = setup_coupling_repo()?;
        let target = parse_target("src/schema.rs:1", None)?;
        let report = scan_coupling(repo.path(), &target, 5)?;

        assert_eq!(report.target_path, std::path::Path::new("src/schema.rs"));
        assert_eq!(report.target_commit_count, 5);
        assert!(!report.results.is_empty());
        assert_eq!(report.results[0].path, std::path::Path::new("src/data.rs"));
        assert_eq!(report.results[0].shared_commits, 5);
        assert_eq!(report.results[0].target_commit_count, 5);
        assert!((report.results[0].coupling_ratio - 1.0).abs() < f64::EPSILON);

        Ok(())
    }
}
