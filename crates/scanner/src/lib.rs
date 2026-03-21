//! Repo-wide scanners.

use std::path::{Path, PathBuf};

use git2::{Repository, Status};
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

pub(crate) fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| SKIPPED_DIR_NAMES.contains(&name))
        .unwrap_or(false)
}

pub(crate) fn is_tracked_source_file(repo: &Repository, workdir: &Path, path: &Path) -> bool {
    if !is_source_file(path) {
        return false;
    }

    let Ok(relative_path) = path.strip_prefix(workdir) else {
        return false;
    };

    match repo.status_file(relative_path) {
        Ok(status) => !status.intersects(Status::WT_NEW | Status::IGNORED),
        Err(_) => false,
    }
}
