//! Repo-wide scanners.

use std::path::Path;

use git2::{Repository, Status};

pub mod coupling;
pub mod ghost;
pub mod health;
pub mod hotspots;
pub mod onboard;
pub mod outage;
pub mod pr_template;
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
pub use ghost::{GhostFinding, scan_ghosts};
pub use health::{HealthDelta, HealthReport, scan_health};
pub use hotspots::{HotspotFinding, scan_hotspots};
pub use onboard::{OnboardFinding, scan_onboard};
pub use outage::{OutageFinding, OutageReport, scan_outage, scan_outage_window};
pub use pr_template::{PrTemplateReport, StagedChange, StagedFile, scan_pr_template};
pub use time_bombs::{Severity, TimeBombFinding, TimeBombKind, scan_time_bombs};

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
