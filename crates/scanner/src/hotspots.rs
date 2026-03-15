use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository, Sort};
use serde::Serialize;
use why_archaeologist::{
    RiskLevel, TeamOwner, blame_commit_evidence, discover_repository, extract_local_context,
    infer_risk_level, summarize_ownership,
};
use why_context::load_config;

use crate::{is_tracked_source_file, should_skip_dir};
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HotspotFinding {
    pub path: PathBuf,
    pub churn_commits: usize,
    pub risk_level: RiskLevel,
    pub hotspot_score: f32,
    pub top_commit_summaries: Vec<String>,
    pub owners: Vec<TeamOwner>,
    pub bus_factor: usize,
    pub primary_owner: Option<String>,
}

pub fn scan_hotspots(
    repo_root: &Path,
    limit: usize,
    owner_filter: Option<&str>,
) -> Result<Vec<HotspotFinding>> {
    let repo = discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;
    let config = load_config(workdir)?;

    let mut findings = Vec::new();
    collect_hotspots(&repo, workdir, workdir, &config, owner_filter, &mut findings)?;
    findings.sort_by(|left, right| {
        right
            .hotspot_score
            .partial_cmp(&left.hotspot_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(right.churn_commits.cmp(&left.churn_commits))
            .then(left.path.cmp(&right.path))
    });
    findings.truncate(limit.max(1));
    Ok(findings)
}

fn collect_hotspots(
    repo: &Repository,
    workdir: &Path,
    dir: &Path,
    config: &why_context::WhyConfig,
    owner_filter: Option<&str>,
    findings: &mut Vec<HotspotFinding>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;

        if file_type.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_hotspots(repo, workdir, &path, config, owner_filter, findings)?;
            continue;
        }

        if !file_type.is_file() || !is_tracked_source_file(repo, workdir, &path) {
            continue;
        }

        if let Some(finding) = analyze_file_hotspot(repo, workdir, &path, config, owner_filter)? {
            findings.push(finding);
        }
    }

    Ok(())
}

fn analyze_file_hotspot(
    repo: &Repository,
    workdir: &Path,
    path: &Path,
    config: &why_context::WhyConfig,
    owner_filter: Option<&str>,
) -> Result<Option<HotspotFinding>> {
    let relative_path = path
        .strip_prefix(workdir)
        .with_context(|| format!("{} is not inside {}", path.display(), workdir.display()))?;
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read source file {}", path.display()))?;
    let line_count = source.lines().count() as u32;
    if line_count == 0 {
        return Ok(None);
    }

    let churn_commits = count_touching_commits(repo, relative_path)?;
    if churn_commits == 0 {
        return Ok(None);
    }

    let commits = blame_commit_evidence(repo, relative_path, 1, line_count, config, None)?;
    let local_context = extract_local_context(path, 1, line_count, config)?;
    let risk_level = infer_risk_level(&commits, &local_context, config);
    let hotspot_score = churn_commits as f32 * risk_weight(risk_level);
    let top_commit_summaries = commits
        .iter()
        .take(3)
        .map(|commit| commit.summary.clone())
        .collect();
    let ownership = summarize_ownership(&commits);
    let primary_owner = ownership.owners.first().map(|owner| owner.author.clone());

    if let Some(owner_filter) = owner_filter {
        let normalized = owner_filter.trim().to_ascii_lowercase();
        if !ownership
            .owners
            .iter()
            .any(|owner| owner.author.to_ascii_lowercase() == normalized)
        {
            return Ok(None);
        }
    }

    Ok(Some(HotspotFinding {
        path: relative_path.to_path_buf(),
        churn_commits,
        risk_level,
        hotspot_score,
        top_commit_summaries,
        owners: ownership.owners,
        bus_factor: ownership.bus_factor,
        primary_owner,
    }))
}

fn count_touching_commits(repo: &Repository, relative_path: &Path) -> Result<usize> {
    let mut revwalk = repo.revwalk().context("failed to create revwalk")?;
    revwalk.push_head().context("failed to walk HEAD")?;
    revwalk
        .set_sorting(Sort::TIME)
        .context("failed to set revwalk ordering")?;

    let mut count = 0;
    for oid in revwalk {
        let oid = oid.context("failed to read commit from revwalk")?;
        let commit = repo
            .find_commit(oid)
            .with_context(|| format!("failed to load commit {oid}"))?;
        if commit_touches_path(repo, &commit, relative_path)? {
            count += 1;
        }
    }

    Ok(count)
}

fn commit_touches_path(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    relative_path: &Path,
) -> Result<bool> {
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

    Ok(diff
        .deltas()
        .any(|delta| delta_matches_path(&delta, relative_path)))
}

fn delta_matches_path(delta: &git2::DiffDelta<'_>, relative_path: &Path) -> bool {
    [delta.new_file().path(), delta.old_file().path()]
        .into_iter()
        .flatten()
        .any(|path| path == relative_path)
}

fn risk_weight(risk_level: RiskLevel) -> f32 {
    match risk_level {
        RiskLevel::HIGH => 3.0,
        RiskLevel::MEDIUM => 2.0,
        RiskLevel::LOW => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{HotspotFinding, risk_weight, scan_hotspots};
    use crate::should_skip_dir;
    use anyhow::{Context, Result};
    use std::process::Command;
    use tempfile::TempDir;
    use why_archaeologist::RiskLevel;

    fn setup_hotspot_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for hotspot fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email test@example.com
git config user.name 'Fixture Bot'
mkdir -p src
cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    // SECURITY: keep legacy validation during staged rollout
    token.starts_with("secure-")
}
EOF
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
git add src/auth.rs src/util.rs
git commit -m 'feat: add auth and util helpers' >/dev/null
for i in 1 2 3; do
  cat > src/auth.rs <<EOF
pub fn verify_token(token: &str) -> bool {
    // SECURITY: keep legacy validation during staged rollout
    // HACK: temporary rollback guard ${i}
    token.starts_with("secure-") && token.len() > ${i}
}
EOF
  git add src/auth.rs
  git commit -m "security hotfix ${i}: tighten token validation" >/dev/null
done
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create hotspot fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "hotspot fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    fn setup_multi_owner_hotspot_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for multi-owner hotspot fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email alice@example.com
git config user.name 'Alice Analyst'
mkdir -p src
cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    // SECURITY: keep legacy validation during staged rollout
    token.starts_with("secure-")
}
EOF
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
git add src/auth.rs src/util.rs
git commit -m 'feat: add auth and util helpers' >/dev/null
git config user.email bob@example.com
git config user.name 'Bob Builder'
cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    // SECURITY: keep legacy validation during staged rollout
    // HACK: temporary rollback guard
    token.starts_with("secure-") && token.len() > 1
}
EOF
git add src/auth.rs
git commit -m 'security hotfix: tighten token validation' >/dev/null
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create multi-owner hotspot fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "multi-owner hotspot fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    fn weights_risk_levels_for_hotspot_scoring() {
        assert_eq!(risk_weight(RiskLevel::HIGH), 3.0);
        assert_eq!(risk_weight(RiskLevel::MEDIUM), 2.0);
        assert_eq!(risk_weight(RiskLevel::LOW), 1.0);
    }

    #[test]
    fn scan_hotspots_ranks_high_churn_high_risk_files_first() -> Result<()> {
        let fixture = setup_hotspot_repo()?;
        let findings = scan_hotspots(fixture.path(), 5, None)?;

        assert!(!findings.is_empty());
        assert_eq!(findings[0].path, std::path::Path::new("src/auth.rs"));
        assert!(findings[0].churn_commits >= 4);
        assert_eq!(findings[0].risk_level, RiskLevel::HIGH);
        assert!(findings[0].hotspot_score >= 12.0);
        assert!(
            findings[0]
                .top_commit_summaries
                .iter()
                .any(|summary| summary.contains("security hotfix"))
        );

        let util = findings
            .iter()
            .find(|finding: &&HotspotFinding| finding.path == std::path::Path::new("src/util.rs"))
            .context("expected util hotspot finding")?;
        assert!(findings[0].hotspot_score > util.hotspot_score);
        Ok(())
    }

    #[test]
    fn scan_hotspots_respects_limit() -> Result<()> {
        let fixture = setup_hotspot_repo()?;
        let findings = scan_hotspots(fixture.path(), 1, None)?;
        assert_eq!(findings.len(), 1);
        Ok(())
    }

    #[test]
    fn scan_hotspots_includes_ownership_metadata() -> Result<()> {
        let fixture = setup_hotspot_repo()?;
        let findings = scan_hotspots(fixture.path(), 5, None)?;

        let auth = findings
            .iter()
            .find(|finding: &&HotspotFinding| finding.path == std::path::Path::new("src/auth.rs"))
            .context("expected auth hotspot finding")?;
        assert_eq!(auth.primary_owner.as_deref(), Some("Fixture Bot"));
        assert_eq!(auth.bus_factor, 1);
        assert_eq!(auth.owners.len(), 1);
        assert_eq!(auth.owners[0].author, "Fixture Bot");
        assert_eq!(auth.owners[0].commit_count, 2);
        assert_eq!(auth.owners[0].ownership_percent, 100);
        Ok(())
    }

    #[test]
    fn scan_hotspots_filters_by_owner_case_insensitively() -> Result<()> {
        let fixture = setup_multi_owner_hotspot_repo()?;
        let findings = scan_hotspots(fixture.path(), 5, Some(" bob builder "))?;

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, std::path::Path::new("src/auth.rs"));
        assert_eq!(findings[0].primary_owner.as_deref(), Some("Alice Analyst"));
        assert_eq!(findings[0].bus_factor, 1);
        assert_eq!(findings[0].owners.len(), 2);
        assert_eq!(findings[0].owners[0].author, "Alice Analyst");
        assert_eq!(findings[0].owners[0].commit_count, 1);
        assert_eq!(findings[0].owners[0].ownership_percent, 50);
        assert_eq!(findings[0].owners[1].author, "Bob Builder");
        assert_eq!(findings[0].owners[1].commit_count, 1);
        assert_eq!(findings[0].owners[1].ownership_percent, 50);
        Ok(())
    }

    #[test]
    fn scan_hotspots_returns_empty_when_owner_is_absent() -> Result<()> {
        let fixture = setup_multi_owner_hotspot_repo()?;
        let findings = scan_hotspots(fixture.path(), 5, Some("carol reviewer"))?;
        assert!(findings.is_empty());
        Ok(())
    }

    #[test]
    fn skips_vendored_directories() {
        assert!(should_skip_dir(std::path::Path::new("node_modules")));
        assert!(should_skip_dir(std::path::Path::new("vendor")));
        assert!(should_skip_dir(std::path::Path::new("dist")));
        assert!(!should_skip_dir(std::path::Path::new("src")));
    }
}
