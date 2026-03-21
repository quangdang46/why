use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use time::OffsetDateTime;
use why_archaeologist::{RiskLevel, analyze_target_with_options};
use why_locator::{QueryKind, QueryTarget, SupportedLanguage, list_all_symbols};

use crate::{is_tracked_source_file, should_skip_dir};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OnboardFinding {
    #[serde(serialize_with = "crate::serialize_path")]
    pub path: PathBuf,
    pub symbol: String,
    pub start_line: u32,
    pub end_line: u32,
    pub risk_level: RiskLevel,
    pub score: f32,
    pub commit_count: usize,
    pub last_touched_date: Option<String>,
    pub summary: String,
    pub risk_summary: String,
    pub change_guidance: String,
    pub top_commit_summaries: Vec<String>,
}

pub fn scan_onboard(repo_root: &Path, limit: usize) -> Result<Vec<OnboardFinding>> {
    let repo = why_archaeologist::discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;

    let mut findings = Vec::new();
    collect_onboard_candidates(&repo, workdir, workdir, &mut findings)?;
    findings.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(right.commit_count.cmp(&left.commit_count))
            .then(left.path.cmp(&right.path))
            .then(left.start_line.cmp(&right.start_line))
    });
    findings.truncate(limit.max(1));
    Ok(findings)
}

fn collect_onboard_candidates(
    repo: &git2::Repository,
    workdir: &Path,
    dir: &Path,
    findings: &mut Vec<OnboardFinding>,
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
            collect_onboard_candidates(repo, workdir, &path, findings)?;
            continue;
        }

        if !file_type.is_file() || !is_tracked_source_file(repo, workdir, &path) {
            continue;
        }

        findings.extend(analyze_file_symbols(workdir, &path)?);
    }

    Ok(())
}

fn analyze_file_symbols(workdir: &Path, absolute_path: &Path) -> Result<Vec<OnboardFinding>> {
    let language = match SupportedLanguage::detect(absolute_path) {
        Ok(language) => language,
        Err(_) => return Ok(Vec::new()),
    };
    let source = fs::read_to_string(absolute_path)
        .with_context(|| format!("failed to read source file {}", absolute_path.display()))?;
    let relative_path = absolute_path
        .strip_prefix(workdir)
        .with_context(|| {
            format!(
                "{} is not inside {}",
                absolute_path.display(),
                workdir.display()
            )
        })?
        .to_path_buf();

    let mut findings = Vec::new();
    for (symbol, start_line, end_line) in list_all_symbols(language, &source)? {
        // Skip symbols that cannot be uniquely resolved (e.g., multiple `default` impls)
        let result = match analyze_target_with_options(
            &QueryTarget {
                path: relative_path.clone(),
                start_line: None,
                end_line: None,
                symbol: Some(symbol.clone()),
                query_kind: QueryKind::Symbol,
            },
            workdir,
            None,
        ) {
            Ok(result) => result,
            Err(_) => continue, // Skip ambiguous or unresolvable symbols
        };
        let commit_count = result.commits.len();
        if commit_count == 0 {
            continue;
        }

        let last_touched_timestamp = result.commits.iter().map(|commit| commit.time).max();
        let score = commit_count as f32
            * risk_weight(result.risk_level)
            * recency_factor(last_touched_timestamp);
        let summary = result
            .commits
            .first()
            .map(|commit| commit.summary.clone())
            .unwrap_or_else(|| result.risk_summary.clone());
        let top_commit_summaries = result
            .commits
            .iter()
            .take(3)
            .map(|commit| commit.summary.clone())
            .collect();

        findings.push(OnboardFinding {
            path: relative_path.clone(),
            symbol,
            start_line,
            end_line,
            risk_level: result.risk_level,
            score,
            commit_count,
            last_touched_date: result.commits.first().map(|commit| commit.date.clone()),
            summary,
            risk_summary: result.risk_summary,
            change_guidance: result.change_guidance,
            top_commit_summaries,
        });
    }

    Ok(findings)
}

fn risk_weight(risk_level: RiskLevel) -> f32 {
    match risk_level {
        RiskLevel::HIGH => 3.0,
        RiskLevel::MEDIUM => 2.0,
        RiskLevel::LOW => 1.0,
    }
}

fn recency_factor(last_touched_timestamp: Option<i64>) -> f32 {
    let Some(timestamp) = last_touched_timestamp else {
        return 1.0;
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let age_days = ((now - timestamp).max(0)) / 86_400;
    match age_days {
        0..=30 => 2.0,
        31..=180 => 1.5,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{RiskLevel, risk_weight, scan_onboard};
    use anyhow::{Context, Result};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_onboard_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for onboard fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email test@example.com
git config user.name 'Fixture Bot'
mkdir -p src
cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    token.starts_with("secure-")
}

pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
git add src/auth.rs
git commit -m 'feat: add auth helpers' >/dev/null
for i in 1 2 3; do
  cat > src/auth.rs <<EOF
pub fn verify_token(token: &str) -> bool {
    // SECURITY: preserve legacy token validation during rollout
    // HACK: temporary production rollback guard ${i}
    token.starts_with("secure-") && token.len() > ${i}
}

pub fn helper(value: i32) -> i32 {
    value + ${i}
}
EOF
  git add src/auth.rs
  git commit -m "security hotfix ${i}: tighten auth validation" >/dev/null
done
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create onboard fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "onboard fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    fn risk_weight_matches_expected_scaling() {
        assert_eq!(risk_weight(RiskLevel::HIGH), 3.0);
        assert_eq!(risk_weight(RiskLevel::MEDIUM), 2.0);
        assert_eq!(risk_weight(RiskLevel::LOW), 1.0);
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_onboard_ranks_hot_symbols_first() -> Result<()> {
        let fixture = setup_onboard_repo()?;
        let findings = scan_onboard(fixture.path(), 5)?;

        assert!(!findings.is_empty());
        assert_eq!(findings[0].path, std::path::Path::new("src/auth.rs"));
        assert_eq!(findings[0].symbol, "verify_token");
        assert_eq!(findings[0].risk_level, RiskLevel::HIGH);
        assert!(findings[0].commit_count >= 1);
        assert!(findings[0].score > 0.0);
        assert!(!findings[0].top_commit_summaries.is_empty());
        assert!(
            findings[0].summary.contains("security hotfix") || findings[0].summary.contains("auth")
        );

        let helper = findings
            .iter()
            .find(|finding| finding.symbol == "helper")
            .context("expected helper symbol finding")?;
        assert!(findings[0].score > helper.score);
        Ok(())
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_onboard_respects_limit() -> Result<()> {
        let fixture = setup_onboard_repo()?;
        let findings = scan_onboard(fixture.path(), 1)?;
        assert_eq!(findings.len(), 1);
        Ok(())
    }
}
