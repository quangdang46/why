//! Repo-wide scanners.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use git2::Repository;
use serde::{Serialize, Serializer};

pub mod coupling;
pub mod coverage_gap;
pub mod ghost;
pub mod health;
pub mod hotspots;
pub mod onboard;
pub mod outage;
pub mod pr_template;
pub mod rename_safe;
pub mod time_bombs;

const SOURCE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "cs", "go", "h", "hpp", "java", "js", "jsx", "py", "rb", "rs", "swift", "ts",
    "tsx",
];
const SKIPPED_DIR_NAMES: &[&str] = &[
    ".git",
    ".why",
    "target",
    "node_modules",
    "vendor",
    "vendors",
    "dist",
    "build",
    "coverage",
];

pub use coupling::{CouplingFinding, CouplingReport, scan_coupling};
pub use coverage_gap::{CoverageGapFinding, CoverageGapReport, scan_coverage_gap};
pub use ghost::{GhostFinding, scan_ghosts};
pub use health::{
    HealthBaselineReference, HealthComparison, HealthDelta, HealthGateSummary, HealthReport,
    HealthSignalDelta, scan_health,
};
pub use hotspots::{HotspotFinding, scan_hotspots};
pub use onboard::{OnboardFinding, scan_onboard};
pub use outage::{OutageFinding, OutageReport, scan_outage, scan_outage_window};
pub use pr_template::{
    DiffReviewPlan, DiffReviewTarget, PrTemplateReport, StagedChange, StagedDiffFile, StagedFile,
    StagedLineRange, scan_diff_review, scan_pr_template,
};
pub use rename_safe::{
    RenameSafeCallerFinding, RenameSafeReport, RenameSafeTarget, scan_rename_safe,
};
pub use time_bombs::{Severity, TimeBombFinding, TimeBombKind, scan_time_bombs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackedSourceFile {
    pub absolute_path: PathBuf,
    pub relative_path: PathBuf,
}

pub(crate) fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn serialize_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&normalized_path(path))
}

pub(crate) fn serialize_paths<S>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    paths
        .iter()
        .map(|path| normalized_path(path))
        .collect::<Vec<_>>()
        .serialize(serializer)
}

pub(crate) fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SOURCE_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| SKIPPED_DIR_NAMES.contains(&name))
        .unwrap_or(false)
}

pub(crate) fn tracked_source_files(
    repo: &Repository,
    workdir: &Path,
) -> Result<Vec<TrackedSourceFile>> {
    let index = repo.index().context("failed to open git index")?;
    let mut files = Vec::new();

    for entry in index.iter() {
        let relative_path = PathBuf::from(String::from_utf8_lossy(&entry.path).into_owned());
        if path_has_skipped_dir(&relative_path) || !is_source_file(&relative_path) {
            continue;
        }

        let absolute_path = workdir.join(&relative_path);
        if !absolute_path.is_file() {
            continue;
        }

        files.push(TrackedSourceFile {
            absolute_path,
            relative_path,
        });
    }

    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    files.dedup_by(|left, right| left.relative_path == right.relative_path);
    Ok(files)
}

fn path_has_skipped_dir(path: &Path) -> bool {
    path.components().any(|component| match component {
        std::path::Component::Normal(name) => name
            .to_str()
            .map(|name| SKIPPED_DIR_NAMES.contains(&name))
            .unwrap_or(false),
        _ => false,
    })
}
