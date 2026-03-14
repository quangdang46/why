use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Delta, DiffOptions, Repository};
use serde::Serialize;

use crate::{scan_hotspots, scan_time_bombs, HotspotFinding, TimeBombFinding};

const TIME_BOMB_AGE_DAYS: i64 = 180;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PrTemplateReport {
    pub title_suggestion: String,
    pub summary: Vec<String>,
    pub risk_notes: Vec<String>,
    pub test_plan: Vec<String>,
    pub staged_files: Vec<StagedFile>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StagedFile {
    pub path: PathBuf,
    pub change: StagedChange,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum StagedChange {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl StagedChange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type-changed",
        }
    }

    fn title_verb(self) -> &'static str {
        match self {
            Self::Added => "add",
            Self::Modified => "update",
            Self::Deleted => "remove",
            Self::Renamed => "rename",
            Self::Copied => "copy",
            Self::TypeChanged => "update",
        }
    }
}

pub fn scan_pr_template(repo_root: &Path) -> Result<PrTemplateReport> {
    let repo = Repository::discover(repo_root).with_context(|| {
        format!(
            "failed to discover git repository from {}",
            repo_root.display()
        )
    })?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;
    let staged_files = staged_files(&repo)?;

    if staged_files.is_empty() {
        return Ok(PrTemplateReport {
            title_suggestion: "summarize staged changes".into(),
            summary: vec!["No staged changes were found. Stage the intended diff before generating a PR template.".into()],
            risk_notes: vec!["PR template generation uses the staged diff only; unstaged edits are intentionally ignored.".into()],
            test_plan: vec!["[ ] Stage the intended changes and regenerate the template.".into()],
            staged_files,
        });
    }

    let hotspots = scan_hotspots(workdir, usize::MAX)?;
    let time_bombs = scan_time_bombs(workdir, TIME_BOMB_AGE_DAYS)?;

    Ok(PrTemplateReport {
        title_suggestion: build_title(&staged_files),
        summary: build_summary(&staged_files),
        risk_notes: build_risk_notes(&staged_files, &hotspots, &time_bombs),
        test_plan: build_test_plan(&staged_files),
        staged_files,
    })
}

fn staged_files(repo: &Repository) -> Result<Vec<StagedFile>> {
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    let index = repo.index().context("failed to read git index")?;
    let mut options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut options))
        .context("failed to inspect staged diff")?;

    let mut files = diff
        .deltas()
        .filter_map(|delta| staged_file_from_delta(&delta))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.change.cmp(&right.change))
    });
    files.dedup_by(|left, right| left.path == right.path && left.change == right.change);
    Ok(files)
}

fn staged_file_from_delta(delta: &git2::DiffDelta<'_>) -> Option<StagedFile> {
    let change = match delta.status() {
        Delta::Added => StagedChange::Added,
        Delta::Modified => StagedChange::Modified,
        Delta::Deleted => StagedChange::Deleted,
        Delta::Renamed => StagedChange::Renamed,
        Delta::Copied => StagedChange::Copied,
        Delta::Typechange => StagedChange::TypeChanged,
        _ => return None,
    };

    let path = delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())?
        .to_path_buf();

    Some(StagedFile { path, change })
}

fn build_title(staged_files: &[StagedFile]) -> String {
    if staged_files.len() == 1 {
        let file = &staged_files[0];
        return format!("{} {}", file.change.title_verb(), file.path.display());
    }

    let mut counts = BTreeMap::new();
    for file in staged_files {
        *counts.entry(file.change).or_insert(0usize) += 1;
    }
    let dominant_change = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(change, _)| change)
        .unwrap_or(StagedChange::Modified);

    format!("{} staged changes", dominant_change.title_verb())
}

fn build_summary(staged_files: &[StagedFile]) -> Vec<String> {
    let total = staged_files.len();
    let mut counts = BTreeMap::new();
    for file in staged_files {
        *counts.entry(file.change.as_str()).or_insert(0usize) += 1;
    }
    let change_breakdown = counts
        .into_iter()
        .map(|(change, count)| format!("{count} {change}"))
        .collect::<Vec<_>>()
        .join(", ");

    let preview = staged_files
        .iter()
        .take(3)
        .map(|file| file.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut summary = vec![format!(
        "Stage review covers {total} file(s): {change_breakdown}."
    )];
    summary.push(format!("Primary files: {preview}."));

    let areas = staged_files
        .iter()
        .filter_map(|file| file.path.components().next())
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<BTreeSet<_>>();
    if !areas.is_empty() {
        summary.push(format!(
            "Touched areas: {}.",
            areas.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    summary
}

fn build_risk_notes(
    staged_files: &[StagedFile],
    hotspots: &[HotspotFinding],
    time_bombs: &[TimeBombFinding],
) -> Vec<String> {
    let staged_paths = staged_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let mut notes = Vec::new();

    for hotspot in hotspots {
        if staged_paths.contains(&hotspot.path) {
            notes.push(format!(
                "{} is already a {} hotspot (score {:.2}, churn {}).",
                hotspot.path.display(),
                hotspot.risk_level.as_str(),
                hotspot.hotspot_score,
                hotspot.churn_commits
            ));
        }
    }

    let mut time_bomb_paths = time_bombs
        .iter()
        .filter(|finding| staged_paths.contains(&finding.path))
        .map(|finding| finding.path.display().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    time_bomb_paths.sort();
    for path in time_bomb_paths {
        notes.push(format!(
            "{path} already contains time-bomb markers; review deadline and cleanup context before merge."
        ));
    }

    if notes.is_empty() {
        notes.push(
            "No existing hotspot or time-bomb scanner warnings matched the staged files.".into(),
        );
    }

    notes
}

fn build_test_plan(staged_files: &[StagedFile]) -> Vec<String> {
    let preview = staged_files
        .iter()
        .take(3)
        .map(|file| file.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    vec![
        format!("[ ] Run targeted tests covering: {preview}."),
        "[ ] Review the staged diff to confirm the summary and risk notes still match the final PR scope.".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{build_title, PrTemplateReport, StagedChange, StagedFile};
    use std::path::PathBuf;

    #[test]
    fn title_uses_single_file_change_when_available() {
        let report = vec![StagedFile {
            path: PathBuf::from("src/lib.rs"),
            change: StagedChange::Modified,
        }];
        assert_eq!(build_title(&report), "update src/lib.rs");
    }

    #[test]
    fn empty_report_serializes_cleanly() {
        let report = PrTemplateReport {
            title_suggestion: "summarize staged changes".into(),
            summary: vec!["No staged changes were found.".into()],
            risk_notes: Vec::new(),
            test_plan: Vec::new(),
            staged_files: Vec::new(),
        };
        let json = serde_json::to_string(&report).expect("report should serialize");
        assert!(json.contains("title_suggestion"));
    }
}
