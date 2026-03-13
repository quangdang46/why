//! Repo-wide scanners.

pub mod coupling;
pub mod ghost;
pub mod health;
pub mod hotspots;
pub mod onboard;
pub mod time_bombs;

pub use coupling::{CouplingFinding, CouplingReport, scan_coupling};
pub use ghost::{GhostFinding, scan_ghosts};
pub use health::{HealthDelta, HealthReport, scan_health};
pub use hotspots::{HotspotFinding, scan_hotspots};
pub use onboard::{OnboardFinding, scan_onboard};
pub use time_bombs::{Severity, TimeBombFinding, TimeBombKind, scan_time_bombs};
