use anyhow::{Context, Result, bail};
use git2::{BlameOptions, Repository};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BlameCommit {
    pub oid: String,
    pub short_oid: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BlameReport {
    pub repo_root: PathBuf,
    pub relative_path: PathBuf,
    pub commits: Vec<BlameCommit>,
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

pub fn blame_range(
    repo: &Repository,
    relative_path: &Path,
    start_line: u32,
    end_line: u32,
) -> Result<BlameReport> {
    if start_line == 0 || end_line == 0 {
        bail!("blame lines must be 1-based");
    }

    if end_line < start_line {
        bail!("blame range end must be greater than or equal to start");
    }

    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?
        .to_path_buf();

    let mut options = BlameOptions::new();
    options
        .min_line(start_line as usize)
        .max_line(end_line as usize);

    let blame = repo
        .blame_file(relative_path, Some(&mut options))
        .with_context(|| format!("failed to blame {}", relative_path.display()))?;

    let mut seen = BTreeSet::new();
    for hunk in blame.iter() {
        let oid = hunk.final_commit_id().to_string();
        if oid.is_empty() || oid == "0000000000000000000000000000000000000000" {
            continue;
        }
        seen.insert(oid);
    }

    let commits = seen
        .into_iter()
        .map(|oid| BlameCommit {
            short_oid: oid.chars().take(8).collect(),
            oid,
        })
        .collect();

    Ok(BlameReport {
        repo_root: workdir,
        relative_path: relative_path.to_path_buf(),
        commits,
    })
}

#[cfg(test)]
mod tests {
    use super::{blame_range, discover_repository, relative_repo_path};
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
        let report = blame_range(&repo, &relative, 1, 1).expect("blame should succeed");

        assert_eq!(report.relative_path, Path::new("README.md"));
        assert!(!report.commits.is_empty());
    }
}
