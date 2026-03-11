//! Repo-wide scanners.

pub mod time_bombs;

pub use time_bombs::{Severity, TimeBombFinding, TimeBombKind, scan_time_bombs};
