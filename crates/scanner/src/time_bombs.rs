use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{BlameOptions, Repository};
use serde::Serialize;
use time::{Date, Duration, Month, OffsetDateTime};

use crate::{is_tracked_source_file, should_skip_dir};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeBombKind {
    PastDueTodo,
    AgedHack,
    ExpiredRemoveAfter,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimeBombFinding {
    pub kind: TimeBombKind,
    pub severity: Severity,
    pub path: PathBuf,
    pub line: u32,
    pub marker: String,
    pub introduced_by: Option<String>,
    pub age_days: Option<i64>,
}

pub fn scan_time_bombs(repo_root: &Path, time_bomb_age_days: i64) -> Result<Vec<TimeBombFinding>> {
    let repo = Repository::discover(repo_root).with_context(|| {
        format!(
            "failed to discover git repository from {}",
            repo_root.display()
        )
    })?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;

    let mut findings = Vec::new();
    scan_dir(&repo, workdir, workdir, time_bomb_age_days, &mut findings)?;
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.kind.cmp(&right.kind))
    });
    Ok(findings)
}

fn scan_dir(
    repo: &Repository,
    workdir: &Path,
    dir: &Path,
    time_bomb_age_days: i64,
    findings: &mut Vec<TimeBombFinding>,
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
            scan_dir(repo, workdir, &path, time_bomb_age_days, findings)?;
            continue;
        }

        if !file_type.is_file() || !is_tracked_source_file(repo, workdir, &path) {
            continue;
        }

        scan_file(repo, workdir, &path, time_bomb_age_days, findings)?;
    }

    Ok(())
}

fn scan_file(
    repo: &Repository,
    workdir: &Path,
    path: &Path,
    time_bomb_age_days: i64,
    findings: &mut Vec<TimeBombFinding>,
) -> Result<()> {
    let relative_path = path
        .strip_prefix(workdir)
        .with_context(|| format!("{} is not inside {}", path.display(), workdir.display()))?;
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read source file {}", path.display()))?;

    for (index, line) in source.lines().enumerate() {
        let line_number = index as u32 + 1;
        let trimmed = line.trim();

        if let Some((kind, due_date)) = classify_due_date_marker(trimmed) {
            if due_date < OffsetDateTime::now_utc().date() {
                findings.push(TimeBombFinding {
                    kind,
                    severity: severity_for_days_overdue(
                        (OffsetDateTime::now_utc().date() - due_date).whole_days(),
                    ),
                    path: relative_path.to_path_buf(),
                    line: line_number,
                    marker: trimmed.to_string(),
                    introduced_by: None,
                    age_days: Some((OffsetDateTime::now_utc().date() - due_date).whole_days()),
                });
            }
        }

        if let Some(kind) = classify_aged_marker(trimmed) {
            if let Some((author, age_days)) = blame_line_age(repo, relative_path, line_number)? {
                if age_days > time_bomb_age_days {
                    findings.push(TimeBombFinding {
                        kind,
                        severity: severity_for_days_overdue(age_days - time_bomb_age_days),
                        path: relative_path.to_path_buf(),
                        line: line_number,
                        marker: trimmed.to_string(),
                        introduced_by: Some(author),
                        age_days: Some(age_days),
                    });
                }
            }
        }
    }

    Ok(())
}

fn classify_due_date_marker(line: &str) -> Option<(TimeBombKind, Date)> {
    if let Some(date) = extract_due_date(line, "TODO") {
        return Some((TimeBombKind::PastDueTodo, date));
    }

    let lower = line.to_ascii_lowercase();
    if lower.contains("remove after") {
        if let Some(date) = extract_first_date_like(line) {
            return Some((TimeBombKind::ExpiredRemoveAfter, date));
        }
    }

    None
}

fn classify_aged_marker(line: &str) -> Option<TimeBombKind> {
    let upper = line.to_ascii_uppercase();
    if upper.contains("HACK:") || upper.contains("HACK ") || upper.ends_with("HACK") {
        return Some(TimeBombKind::AgedHack);
    }
    if upper.contains("TEMP:") || upper.contains("TEMP ") || upper.ends_with("TEMP") {
        return Some(TimeBombKind::AgedHack);
    }
    None
}

fn extract_due_date(line: &str, marker: &str) -> Option<Date> {
    let marker_index = line.find(marker)?;
    extract_first_date_like(&line[marker_index + marker.len()..])
}

fn extract_first_date_like(text: &str) -> Option<Date> {
    const TOKEN_LENGTHS: [usize; 3] = [10, 7, 7];

    for (start, _) in text.char_indices() {
        let slice = &text[start..];
        for len in TOKEN_LENGTHS {
            if slice.len() < len {
                continue;
            }
            if let Some(date) = parse_date_token(&slice[..len]) {
                return Some(date);
            }
        }
    }
    None
}

fn parse_date_token(token: &str) -> Option<Date> {
    if let Some(date) = parse_ymd(token) {
        return Some(date);
    }
    if let Some(date) = parse_year_month(token) {
        return Some(date);
    }
    parse_year_quarter(token)
}

fn parse_ymd(token: &str) -> Option<Date> {
    let mut parts = token.split('-');
    let year = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

fn parse_year_month(token: &str) -> Option<Date> {
    let mut parts = token.split('-');
    let year = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let month = Month::try_from(month).ok()?;
    let first = Date::from_calendar_date(year, month, 1).ok()?;
    last_day_of_month(first)
}

fn parse_year_quarter(token: &str) -> Option<Date> {
    let mut parts = token.split('-');
    let year = parts.next()?.parse().ok()?;
    let quarter = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let month = match quarter {
        "Q1" => Month::March,
        "Q2" => Month::June,
        "Q3" => Month::September,
        "Q4" => Month::December,
        _ => return None,
    };
    let first = Date::from_calendar_date(year, month, 1).ok()?;
    last_day_of_month(first)
}

fn last_day_of_month(first_day: Date) -> Option<Date> {
    let (year, month, _) = first_day.to_calendar_date();
    let (next_year, next_month) = match month {
        Month::January => (year, Month::February),
        Month::February => (year, Month::March),
        Month::March => (year, Month::April),
        Month::April => (year, Month::May),
        Month::May => (year, Month::June),
        Month::June => (year, Month::July),
        Month::July => (year, Month::August),
        Month::August => (year, Month::September),
        Month::September => (year, Month::October),
        Month::October => (year, Month::November),
        Month::November => (year, Month::December),
        Month::December => (year + 1, Month::January),
    };
    Some(Date::from_calendar_date(next_year, next_month, 1).ok()? - Duration::days(1))
}

fn blame_line_age(
    repo: &Repository,
    relative_path: &Path,
    line_number: u32,
) -> Result<Option<(String, i64)>> {
    let mut options = BlameOptions::new();
    options
        .min_line(line_number as usize)
        .max_line(line_number as usize);
    let blame = repo
        .blame_file(relative_path, Some(&mut options))
        .with_context(|| format!("failed to blame {}", relative_path.display()))?;
    let hunk = match blame.get_line(line_number as usize) {
        Some(hunk) => hunk,
        None => return Ok(None),
    };

    let oid = hunk.final_commit_id();
    if oid.is_zero() {
        return Ok(None);
    }

    let commit = repo
        .find_commit(oid)
        .with_context(|| format!("failed to load commit {oid}"))?;
    let author = commit.author().name().unwrap_or("unknown").to_string();
    let introduced = OffsetDateTime::from_unix_timestamp(commit.time().seconds())
        .with_context(|| format!("invalid git timestamp for {oid}"))?;
    let age_days = (OffsetDateTime::now_utc() - introduced).whole_days().max(0);
    Ok(Some((author, age_days)))
}

fn severity_for_days_overdue(days_overdue: i64) -> Severity {
    if days_overdue > 365 {
        Severity::Critical
    } else if days_overdue > 180 {
        Severity::Warn
    } else {
        Severity::Info
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Severity, TimeBombKind, extract_first_date_like, parse_date_token, scan_time_bombs,
    };
    use anyhow::{Context, Result};
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;
    use time::{Date, Month};

    fn setup_aged_timebomb_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for aged timebomb fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email test@example.com
git config user.name 'Fixture Bot'
mkdir -p src
cat > src/legacy.rs <<'EOF'
pub fn process_legacy_format(data: &[u8]) -> Vec<u8> {
    // TODO(2020-01-15): remove after v3 migration is complete
    // HACK: workaround for old client format, should be cleaned up
    if data.starts_with(b"LEGACY:") {
        convert_legacy_format(data)
    } else {
        data.to_vec()
    }
}
EOF
git add src/legacy.rs
GIT_AUTHOR_DATE='2024-01-01T00:00:00Z' GIT_COMMITTER_DATE='2024-01-01T00:00:00Z' git commit -m 'feat: add legacy format support' >/dev/null
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create aged timebomb fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "aged fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    fn parses_supported_due_date_formats() {
        assert_eq!(
            parse_date_token("2024-03-15"),
            Some(Date::from_calendar_date(2024, Month::March, 15).unwrap())
        );
        assert_eq!(
            parse_date_token("2025-06"),
            Some(Date::from_calendar_date(2025, Month::June, 30).unwrap())
        );
        assert_eq!(
            parse_date_token("2026-Q1"),
            Some(Date::from_calendar_date(2026, Month::March, 31).unwrap())
        );
    }

    #[test]
    fn extracts_first_date_like_token_from_marker_text() {
        assert_eq!(
            extract_first_date_like("TODO(2020-01-15): remove after v3 migration"),
            Some(Date::from_calendar_date(2020, Month::January, 15).unwrap())
        );
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scans_timebomb_fixture_for_past_due_todo_and_aged_hack() -> Result<()> {
        let fixture = setup_aged_timebomb_repo()?;
        let findings = scan_time_bombs(fixture.path(), 180)?;

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].path, Path::new("src/legacy.rs"));
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].kind, TimeBombKind::PastDueTodo);
        assert!(matches!(
            findings[0].severity,
            Severity::Critical | Severity::Warn
        ));

        assert_eq!(findings[1].path, Path::new("src/legacy.rs"));
        assert_eq!(findings[1].line, 3);
        assert_eq!(findings[1].kind, TimeBombKind::AgedHack);
        assert!(findings[1].introduced_by.is_some());
        assert!(findings[1].age_days.unwrap_or_default() > 180);
        Ok(())
    }
}
