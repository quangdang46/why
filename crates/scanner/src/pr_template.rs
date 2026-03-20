use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Delta, DiffOptions, Patch, Repository};
use serde::Serialize;
use why_locator::{QueryKind, QueryTarget, SupportedLanguage, list_all_symbols};

use crate::{HotspotFinding, TimeBombFinding, is_source_file, scan_hotspots, scan_time_bombs};

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
    #[serde(serialize_with = "crate::serialize_path")]
    pub path: PathBuf,
    pub change: StagedChange,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StagedLineRange {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StagedDiffFile {
    #[serde(serialize_with = "crate::serialize_path")]
    pub path: PathBuf,
    pub change: StagedChange,
    pub changed_ranges: Vec<StagedLineRange>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiffReviewTarget {
    pub target: QueryTarget,
    pub symbol: Option<String>,
    pub change: StagedChange,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiffReviewPlan {
    pub staged_files: Vec<StagedDiffFile>,
    pub targets: Vec<DiffReviewTarget>,
    pub skipped: Vec<String>,
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

    let hotspots = scan_hotspots(workdir, usize::MAX, None)?;
    let time_bombs = scan_time_bombs(workdir, TIME_BOMB_AGE_DAYS)?;

    Ok(PrTemplateReport {
        title_suggestion: build_title(&staged_files),
        summary: build_summary(&staged_files),
        risk_notes: build_risk_notes(&staged_files, &hotspots, &time_bombs),
        test_plan: build_test_plan(&staged_files),
        staged_files,
    })
}

pub fn scan_diff_review(repo_root: &Path) -> Result<DiffReviewPlan> {
    let repo = Repository::discover(repo_root).with_context(|| {
        format!(
            "failed to discover git repository from {}",
            repo_root.display()
        )
    })?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;
    let staged_files = staged_diff_files(&repo)?;

    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for file in &staged_files {
        targets.extend(build_diff_review_targets(workdir, file, &mut skipped)?);
    }

    Ok(DiffReviewPlan {
        staged_files,
        targets,
        skipped,
    })
}

fn staged_files(repo: &Repository) -> Result<Vec<StagedFile>> {
    Ok(staged_diff_files(repo)?
        .into_iter()
        .map(|file| StagedFile {
            path: file.path,
            change: file.change,
        })
        .collect())
}

fn staged_diff_files(repo: &Repository) -> Result<Vec<StagedDiffFile>> {
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    let index = repo.index().context("failed to read git index")?;
    let mut options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut options))
        .context("failed to inspect staged diff")?;

    let mut files = diff
        .deltas()
        .enumerate()
        .filter_map(|(index, delta)| staged_diff_file_from_delta(&diff, index, &delta))
        .collect::<Result<Vec<_>>>()?;
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.change.cmp(&right.change))
    });
    files.dedup_by(|left, right| left.path == right.path && left.change == right.change);
    Ok(files)
}

fn staged_diff_file_from_delta(
    diff: &git2::Diff<'_>,
    index: usize,
    delta: &git2::DiffDelta<'_>,
) -> Option<Result<StagedDiffFile>> {
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

    Some(
        staged_ranges_for_delta(diff, index).map(|changed_ranges| StagedDiffFile {
            path,
            change,
            changed_ranges,
        }),
    )
}

fn staged_ranges_for_delta(diff: &git2::Diff<'_>, index: usize) -> Result<Vec<StagedLineRange>> {
    let Some(patch) = Patch::from_diff(diff, index).context("failed to inspect staged patch")?
    else {
        return Ok(Vec::new());
    };

    let mut ranges = Vec::new();
    for hunk_index in 0..patch.num_hunks() {
        let (hunk, _) = patch
            .hunk(hunk_index)
            .with_context(|| format!("failed to inspect staged patch hunk {}", hunk_index + 1))?;
        ranges.push(staged_line_range_from_hunk(&hunk));
    }
    ranges.sort_by(|left, right| {
        left.start_line
            .cmp(&right.start_line)
            .then(left.end_line.cmp(&right.end_line))
    });
    ranges.dedup();
    Ok(ranges)
}

fn staged_line_range_from_hunk(hunk: &git2::DiffHunk<'_>) -> StagedLineRange {
    let start_line = hunk.new_start().max(1);
    let end_line = if hunk.new_lines() == 0 {
        start_line
    } else {
        start_line + hunk.new_lines() - 1
    };
    StagedLineRange {
        start_line,
        end_line,
    }
}

fn build_diff_review_targets(
    workdir: &Path,
    file: &StagedDiffFile,
    skipped: &mut Vec<String>,
) -> Result<Vec<DiffReviewTarget>> {
    if matches!(file.change, StagedChange::Deleted) {
        skipped.push(format!(
            "Skipped {} because deleted files have no working-tree source to analyze.",
            file.path.display()
        ));
        return Ok(Vec::new());
    }

    if !is_source_file(&file.path) {
        skipped.push(format!(
            "Skipped {} because it is not a supported source file.",
            file.path.display()
        ));
        return Ok(Vec::new());
    }

    let absolute_path = workdir.join(&file.path);
    if !absolute_path.is_file() {
        skipped.push(format!(
            "Skipped {} because the working-tree file is unavailable.",
            file.path.display()
        ));
        return Ok(Vec::new());
    }

    let language = match SupportedLanguage::detect(&absolute_path) {
        Ok(language) => language,
        Err(_) => {
            skipped.push(format!(
                "Skipped {} because its language is not supported for symbol analysis.",
                file.path.display()
            ));
            return Ok(Vec::new());
        }
    };

    let source = fs::read_to_string(&absolute_path)
        .with_context(|| format!("failed to read source file {}", absolute_path.display()))?;
    build_diff_review_targets_for_source(file, language, &source, skipped)
}

fn build_diff_review_targets_for_source(
    file: &StagedDiffFile,
    language: SupportedLanguage,
    source: &str,
    skipped: &mut Vec<String>,
) -> Result<Vec<DiffReviewTarget>> {
    let symbols = list_all_symbols(language, source)?;
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();

    for (symbol, start_line, end_line) in symbols {
        if !overlaps_changed_ranges(start_line, end_line, &file.changed_ranges) {
            continue;
        }

        if !seen.insert((start_line, end_line, Some(symbol.clone()))) {
            continue;
        }

        targets.push(DiffReviewTarget {
            target: QueryTarget {
                path: file.path.clone(),
                start_line: Some(start_line),
                end_line: Some(end_line),
                symbol: None,
                query_kind: QueryKind::Range,
            },
            symbol: Some(symbol),
            change: file.change,
        });
    }

    if !targets.is_empty() {
        return Ok(targets);
    }

    if file.changed_ranges.is_empty() {
        skipped.push(format!(
            "Skipped {} because the staged diff did not expose analyzable changed line ranges.",
            file.path.display()
        ));
        return Ok(Vec::new());
    }

    for range in &file.changed_ranges {
        if !seen.insert((range.start_line, range.end_line, None)) {
            continue;
        }

        targets.push(DiffReviewTarget {
            target: QueryTarget {
                path: file.path.clone(),
                start_line: Some(range.start_line),
                end_line: Some(range.end_line),
                symbol: None,
                query_kind: QueryKind::Range,
            },
            symbol: None,
            change: file.change,
        });
    }
    skipped.push(format!(
        "Fell back to line-range analysis for {} because no changed symbols could be resolved.",
        file.path.display()
    ));

    Ok(targets)
}

fn overlaps_changed_ranges(start_line: u32, end_line: u32, ranges: &[StagedLineRange]) -> bool {
    ranges.iter().any(|range| {
        let overlap_start = start_line.max(range.start_line);
        let overlap_end = end_line.min(range.end_line);
        overlap_start <= overlap_end
    })
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
    use super::{
        DiffReviewTarget, PrTemplateReport, StagedChange, StagedDiffFile, StagedFile,
        StagedLineRange, build_diff_review_targets_for_source, build_title,
    };
    use std::path::PathBuf;
    use why_locator::{QueryKind, SupportedLanguage};

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

    #[test]
    fn diff_review_targets_select_overlapping_symbols() {
        let file = StagedDiffFile {
            path: PathBuf::from("src/lib.rs"),
            change: StagedChange::Modified,
            changed_ranges: vec![StagedLineRange {
                start_line: 5,
                end_line: 5,
            }],
        };
        let source = r#"fn alpha() {
    let left = 1;
}

fn beta() {
    let right = 2;
}
"#;
        let mut skipped = Vec::new();

        let targets = build_diff_review_targets_for_source(
            &file,
            SupportedLanguage::Rust,
            source,
            &mut skipped,
        )
        .expect("diff review targets should build");

        assert!(skipped.is_empty());
        assert_eq!(
            targets,
            vec![DiffReviewTarget {
                target: why_locator::QueryTarget {
                    path: PathBuf::from("src/lib.rs"),
                    start_line: Some(5),
                    end_line: Some(7),
                    symbol: None,
                    query_kind: QueryKind::Range,
                },
                symbol: Some("beta".into()),
                change: StagedChange::Modified,
            }]
        );
    }

    #[test]
    fn diff_review_targets_fall_back_to_changed_ranges_when_no_symbol_matches() {
        let file = StagedDiffFile {
            path: PathBuf::from("src/lib.rs"),
            change: StagedChange::Modified,
            changed_ranges: vec![StagedLineRange {
                start_line: 1,
                end_line: 1,
            }],
        };
        let source = "// crate docs only\n\nconst VALUE: usize = 1;\n";
        let mut skipped = Vec::new();

        let targets = build_diff_review_targets_for_source(
            &file,
            SupportedLanguage::Rust,
            source,
            &mut skipped,
        )
        .expect("diff review targets should build");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].symbol, None);
        assert_eq!(targets[0].target.start_line, Some(1));
        assert_eq!(targets[0].target.end_line, Some(1));
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("Fell back to line-range analysis"));
    }
}
