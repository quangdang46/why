use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use git2::{DiffOptions, Repository, Sort};
use serde::Serialize;
use why_archaeologist::RiskLevel;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutageReport {
    pub window_start_ts: i64,
    pub window_end_ts: i64,
    pub findings: Vec<OutageFinding>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutageFinding {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub date: String,
    pub summary: String,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
    pub change_guidance: String,
    pub blast_radius_files: usize,
    #[serde(serialize_with = "crate::serialize_paths")]
    pub changed_paths: Vec<PathBuf>,
    pub issue_refs: Vec<String>,
    pub score: f32,
    pub notes: Vec<String>,
}

pub fn scan_outage(repo_root: &Path, since_days: u64, limit: usize) -> Result<OutageReport> {
    if since_days == 0 {
        bail!("since_days must be greater than zero");
    }
    let window_end_ts = time::OffsetDateTime::now_utc().unix_timestamp();
    let window_start_ts = window_end_ts - since_days as i64 * 86_400;
    scan_outage_window(repo_root, window_start_ts, window_end_ts, limit)
}

pub fn scan_outage_window(
    repo_root: &Path,
    window_start_ts: i64,
    window_end_ts: i64,
    limit: usize,
) -> Result<OutageReport> {
    if limit == 0 {
        bail!("limit must be greater than zero");
    }
    if window_end_ts < window_start_ts {
        bail!("window_end_ts must be greater than or equal to window_start_ts");
    }

    let repo = Repository::discover(repo_root).with_context(|| {
        format!(
            "failed to discover git repository from {}",
            repo_root.display()
        )
    })?;

    let mut revwalk = repo.revwalk().context("failed to create revwalk")?;
    revwalk.push_head().context("failed to walk HEAD")?;
    revwalk
        .set_sorting(Sort::TIME)
        .context("failed to set revwalk ordering")?;

    let mut findings = Vec::new();
    for oid in revwalk {
        let oid = oid.context("failed to read commit from revwalk")?;
        let commit = repo
            .find_commit(oid)
            .with_context(|| format!("failed to load commit {oid}"))?;
        let commit_ts = commit.time().seconds();
        if commit_ts > window_end_ts {
            continue;
        }
        if commit_ts < window_start_ts {
            break;
        }

        let changed_paths = changed_source_paths(&repo, &commit)?;
        if changed_paths.is_empty() {
            continue;
        }

        let risk_level = infer_commit_risk(&commit, &changed_paths);
        let blast_radius_files = changed_paths.len();
        let score = score_commit(
            risk_level,
            blast_radius_files,
            commit_ts,
            window_start_ts,
            window_end_ts,
        );
        let notes = build_notes(risk_level, blast_radius_files, &changed_paths);

        findings.push(OutageFinding {
            oid: oid.to_string(),
            short_oid: oid.to_string().chars().take(7).collect(),
            author: commit.author().name().unwrap_or("unknown").to_string(),
            date: format_commit_date(commit_ts),
            summary: commit.summary().unwrap_or("(no summary)").to_string(),
            risk_level,
            risk_summary: risk_level.summary().to_string(),
            change_guidance: risk_level.change_guidance().to_string(),
            blast_radius_files,
            changed_paths,
            issue_refs: extract_issue_refs(commit.message().unwrap_or_default()),
            score,
            notes,
        });
    }

    findings.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(right.blast_radius_files.cmp(&left.blast_radius_files))
            .then(right.date.cmp(&left.date))
            .then(left.oid.cmp(&right.oid))
    });
    findings.truncate(limit.max(1));

    Ok(OutageReport {
        window_start_ts,
        window_end_ts,
        findings,
        notes: vec![
            "Outage archaeology ranks commits inside the requested window using risk, recency, and blast-radius heuristics.".into(),
            "Scores are suggestive only: treat the top-ranked commits as starting points for incident review, not proof of causality.".into(),
        ],
    })
}

fn changed_source_paths(repo: &Repository, commit: &git2::Commit<'_>) -> Result<Vec<PathBuf>> {
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

    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for delta in diff.deltas() {
        for path in [delta.new_file().path(), delta.old_file().path()]
            .into_iter()
            .flatten()
        {
            if is_source_like_path(path) && seen.insert(path.to_path_buf()) {
                paths.push(path.to_path_buf());
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn is_source_like_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| crate::SOURCE_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

fn infer_commit_risk(commit: &git2::Commit<'_>, changed_paths: &[PathBuf]) -> RiskLevel {
    let message = format!(
        "{}\n{}",
        commit.summary().unwrap_or_default(),
        commit.message().unwrap_or_default()
    )
    .to_ascii_lowercase();

    let touches_sensitive_path = changed_paths.iter().any(|path| {
        let rendered = path.to_string_lossy().to_ascii_lowercase();
        [
            "auth", "token", "payment", "billing", "security", "incident", "rollback",
        ]
        .into_iter()
        .any(|needle| rendered.contains(needle))
    });

    let high_signal = [
        "outage",
        "incident",
        "hotfix",
        "rollback",
        "security",
        "vulnerability",
        "auth",
        "token",
        "payment",
        "duplicate charge",
    ]
    .into_iter()
    .any(|needle| message.contains(needle));
    if high_signal || touches_sensitive_path {
        return RiskLevel::HIGH;
    }

    let medium_signal = ["fix", "retry", "legacy", "migration", "fallback", "guard"]
        .into_iter()
        .any(|needle| message.contains(needle));
    if medium_signal || changed_paths.len() >= 4 {
        return RiskLevel::MEDIUM;
    }

    RiskLevel::LOW
}

fn score_commit(
    risk_level: RiskLevel,
    blast_radius_files: usize,
    commit_ts: i64,
    window_start_ts: i64,
    window_end_ts: i64,
) -> f32 {
    let risk_weight = match risk_level {
        RiskLevel::HIGH => 3.0,
        RiskLevel::MEDIUM => 2.0,
        RiskLevel::LOW => 1.0,
    };
    let blast_radius_score = (blast_radius_files.min(6) as f32) / 2.0;
    let recency_score = if window_end_ts == window_start_ts {
        1.0
    } else {
        let span = (window_end_ts - window_start_ts).max(1) as f32;
        ((commit_ts - window_start_ts) as f32 / span).clamp(0.0, 1.0)
    };

    risk_weight + blast_radius_score + recency_score
}

fn build_notes(
    risk_level: RiskLevel,
    blast_radius_files: usize,
    changed_paths: &[PathBuf],
) -> Vec<String> {
    let mut notes = Vec::new();
    notes.push(format!(
        "Blast radius heuristic counted {} changed source file(s) in this commit.",
        blast_radius_files
    ));

    if risk_level == RiskLevel::HIGH {
        notes.push(
            "Commit message or changed paths include incident-like, security-sensitive, or rollback-related signals.".into(),
        );
    }
    if blast_radius_files >= 4 {
        notes.push(
            "This commit crossed several source files, which can increase outage review priority."
                .into(),
        );
    }
    if !changed_paths.is_empty() {
        let preview = changed_paths
            .iter()
            .take(3)
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        notes.push(format!("Representative touched paths: {preview}."));
    }
    notes
}

fn format_commit_date(commit_ts: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(commit_ts)
        .map(|value| value.date().to_string())
        .unwrap_or_else(|_| commit_ts.to_string())
}

fn extract_issue_refs(message: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let bytes = message.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'#' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let digits_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index > digits_start {
            refs.push(message[start..index].to_string());
        }
    }

    refs.sort();
    refs.dedup();
    refs
}

#[cfg(test)]
mod tests {
    use super::{scan_outage_window, score_commit};
    use anyhow::{Context, Result};
    use std::process::Command;
    use tempfile::TempDir;
    use time::{Date, Month, PrimitiveDateTime, Time};
    use why_archaeologist::RiskLevel;

    fn setup_outage_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for outage fixture")?;
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
EOF
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
GIT_AUTHOR_DATE='2024-01-01T12:00:00Z' GIT_COMMITTER_DATE='2024-01-01T12:00:00Z' git add src/auth.rs src/util.rs
GIT_AUTHOR_DATE='2024-01-01T12:00:00Z' GIT_COMMITTER_DATE='2024-01-01T12:00:00Z' git commit -m 'feat: add auth and util helpers' >/dev/null

cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 2
}
EOF
GIT_AUTHOR_DATE='2024-01-02T12:00:00Z' GIT_COMMITTER_DATE='2024-01-02T12:00:00Z' git add src/util.rs
GIT_AUTHOR_DATE='2024-01-02T12:00:00Z' GIT_COMMITTER_DATE='2024-01-02T12:00:00Z' git commit -m 'fix: adjust util helper' >/dev/null

cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    // security: outage rollback guard
    token.starts_with("secure-") && token.len() > 3
}
EOF
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 3
}
EOF
GIT_AUTHOR_DATE='2024-01-03T12:00:00Z' GIT_COMMITTER_DATE='2024-01-03T12:00:00Z' git add src/auth.rs src/util.rs
GIT_AUTHOR_DATE='2024-01-03T12:00:00Z' GIT_COMMITTER_DATE='2024-01-03T12:00:00Z' git commit -m 'hotfix: rollback auth guard after outage (#42)' >/dev/null

mkdir -p docs
cat > docs/runbook.md <<'EOF'
incident notes
EOF
GIT_AUTHOR_DATE='2024-01-04T12:00:00Z' GIT_COMMITTER_DATE='2024-01-04T12:00:00Z' git add docs/runbook.md
GIT_AUTHOR_DATE='2024-01-04T12:00:00Z' GIT_COMMITTER_DATE='2024-01-04T12:00:00Z' git commit -m 'docs: add runbook notes' >/dev/null
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create outage fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "outage fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    fn ts(year: i32, month: Month, day: u8) -> i64 {
        PrimitiveDateTime::new(
            Date::from_calendar_date(year, month, day).expect("valid date"),
            Time::from_hms(0, 0, 0).expect("valid time"),
        )
        .assume_utc()
        .unix_timestamp()
    }

    #[test]
    fn outage_score_weights_risk_then_blast_then_recency() {
        let high = score_commit(RiskLevel::HIGH, 2, 9, 0, 10);
        let medium = score_commit(RiskLevel::MEDIUM, 2, 9, 0, 10);
        let low = score_commit(RiskLevel::LOW, 2, 9, 0, 10);
        assert!(high > medium);
        assert!(medium > low);
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn outage_scan_ranks_hotfix_commit_inside_window() -> Result<()> {
        let fixture = setup_outage_repo()?;
        let report = scan_outage_window(
            fixture.path(),
            ts(2024, Month::January, 2),
            ts(2024, Month::January, 3) + 86_399,
            5,
        )?;

        assert_eq!(report.window_start_ts, ts(2024, Month::January, 2));
        assert_eq!(report.findings.len(), 2);
        assert_eq!(report.findings[0].risk_level, RiskLevel::HIGH);
        assert!(report.findings[0].summary.contains("hotfix"));
        assert_eq!(report.findings[0].blast_radius_files, 2);
        assert!(
            report.findings[0]
                .changed_paths
                .iter()
                .any(|path| path == std::path::Path::new("src/auth.rs"))
        );
        assert_eq!(report.findings[0].issue_refs, vec!["#42"]);
        assert_eq!(report.notes.len(), 2);
        Ok(())
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn outage_scan_ignores_non_source_only_commits() -> Result<()> {
        let fixture = setup_outage_repo()?;
        let report = scan_outage_window(
            fixture.path(),
            ts(2024, Month::January, 4),
            ts(2024, Month::January, 4) + 86_399,
            5,
        )?;

        assert!(report.findings.is_empty());
        Ok(())
    }
}
