//! Repo-wide scanners.

pub mod hotspots;
pub mod time_bombs;

pub use hotspots::{HotspotFinding, scan_hotspots};
pub use time_bombs::{Severity, TimeBombFinding, TimeBombKind, scan_time_bombs};
