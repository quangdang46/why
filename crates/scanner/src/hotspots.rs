use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository, Sort};
use serde::Serialize;
use why_archaeologist::{
    RiskLevel, blame_commit_evidence, discover_repository, extract_local_context, infer_risk_level,
};
use why_context::load_config;

pub(crate) const SOURCE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "cs", "go", "h", "hpp", "java", "js", "jsx", "py", "rb", "rs", "swift", "ts",
    "tsx",
];
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HotspotFinding {
    pub path: PathBuf,
    pub churn_commits: usize,
    pub risk_level: RiskLevel,
    pub hotspot_score: f32,
    pub top_commit_summaries: Vec<String>,
}

pub fn scan_hotspots(repo_root: &Path, limit: usize) -> Result<Vec<HotspotFinding>> {
    let repo = discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;
    let config = load_config(workdir)?;

    let mut findings = Vec::new();
    collect_hotspots(&repo, workdir, workdir, &config, &mut findings)?;
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
    findings: &mut Vec<HotspotFinding>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;

        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "target" || name == ".why" {
                continue;
            }
            collect_hotspots(repo, workdir, &path, config, findings)?;
            continue;
        }

        if !file_type.is_file() || !is_source_file(&path) {
            continue;
        }

        if let Some(finding) = analyze_file_hotspot(repo, workdir, &path, config)? {
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

    Ok(Some(HotspotFinding {
        path: relative_path.to_path_buf(),
        churn_commits,
        risk_level,
        hotspot_score,
        top_commit_summaries,
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

fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SOURCE_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
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

    #[test]
    fn weights_risk_levels_for_hotspot_scoring() {
        assert_eq!(risk_weight(RiskLevel::HIGH), 3.0);
        assert_eq!(risk_weight(RiskLevel::MEDIUM), 2.0);
        assert_eq!(risk_weight(RiskLevel::LOW), 1.0);
    }

    #[test]
    fn scan_hotspots_ranks_high_churn_high_risk_files_first() -> Result<()> {
        let fixture = setup_hotspot_repo()?;
        let findings = scan_hotspots(fixture.path(), 5)?;

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
        let findings = scan_hotspots(fixture.path(), 1)?;
        assert_eq!(findings.len(), 1);
        Ok(())
    }
}
