//! Repo-wide scanners.

pub mod coupling;
pub mod hotspots;
pub mod time_bombs;

pub use coupling::{CouplingFinding, CouplingReport, scan_coupling};
pub use hotspots::{HotspotFinding, scan_hotspots};
pub use time_bombs::{Severity, TimeBombFinding, TimeBombKind, scan_time_bombs};
