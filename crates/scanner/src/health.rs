use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::hotspots::HotspotFinding;
use crate::scan_hotspots;
use crate::time_bombs::{TimeBombKind, scan_time_bombs};
use why_archaeologist::RiskLevel;

const TIME_BOMB_AGE_DAYS: i64 = 180;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthDelta {
    pub direction: &'static str,
    pub amount: i64,
    pub previous_score: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthSignalDelta {
    pub current: u32,
    pub baseline: u32,
    pub delta: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthBaselineReference {
    pub source: String,
    pub timestamp: i64,
    pub head_hash: Option<String>,
    pub ref_name: Option<String>,
    pub debt_score: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthComparison {
    pub baseline: HealthBaselineReference,
    pub score_delta: i64,
    pub signal_deltas: BTreeMap<String, HealthSignalDelta>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthGateSummary {
    pub passed: bool,
    pub absolute_threshold: Option<u32>,
    pub max_regression: Option<u32>,
    pub signal_budgets: BTreeMap<String, u32>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthReport {
    pub debt_score: u32,
    pub signals: HashMap<String, u32>,
    pub delta: Option<HealthDelta>,
    pub comparison: Option<HealthComparison>,
    pub gate: Option<HealthGateSummary>,
    pub notes: Vec<String>,
}

pub fn scan_health(repo_root: &Path) -> Result<HealthReport> {
    let time_bombs = scan_time_bombs(repo_root, TIME_BOMB_AGE_DAYS)?;
    let hotspots = scan_hotspots(repo_root, usize::MAX, None)?;

    let total_time_bombs = time_bombs.len() as u32;
    let stale_hacks = time_bombs
        .iter()
        .filter(|finding| matches!(finding.kind, TimeBombKind::AgedHack))
        .count() as u32;
    let high_risk_files = hotspots
        .iter()
        .filter(|finding| finding.risk_level == RiskLevel::HIGH)
        .count() as u32;
    let hotspot_files = top_quartile_count(&hotspots);

    let debt_score =
        ((total_time_bombs * 3) + high_risk_files + (hotspot_files * 2) + stale_hacks).min(100);

    let mut signals = HashMap::new();
    signals.insert("time_bombs".into(), total_time_bombs);
    signals.insert("high_risk_files".into(), high_risk_files);
    signals.insert("hotspot_files".into(), hotspot_files);
    signals.insert("stale_hacks".into(), stale_hacks);

    Ok(HealthReport {
        debt_score,
        signals,
        delta: None,
        comparison: None,
        gate: None,
        notes: vec![
            "Current health aggregation uses implemented scanner signals only: time bombs, high-risk files, hotspot quartile, and stale hacks.".into(),
            "Plan-only metrics such as ghost functions, coverage gaps, and bus-factor hotspots are not included until those scanners exist.".into(),
        ],
    })
}

fn top_quartile_count(hotspots: &[HotspotFinding]) -> u32 {
    if hotspots.is_empty() {
        return 0;
    }

    hotspots.len().div_ceil(4) as u32
}

#[cfg(test)]
mod tests {
    use super::{scan_health, top_quartile_count};
    use anyhow::{Context, Result};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_health_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for health fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email test@example.com
git config user.name 'Fixture Bot'
mkdir -p src
cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    // TODO(2020-01-15): remove rollback once incident review is complete
    // HACK: temporary token guard from production incident
    token.starts_with("secure-")
}
EOF
git add src/auth.rs
git commit -m 'feat: add auth guard' >/dev/null
for i in 1 2 3; do
  python - <<'PY' "$i"
from pathlib import Path
import re
import sys
value = sys.argv[1]
path = Path('src/auth.rs')
text = path.read_text()
if '// security: duplicate charge incident follow-up' not in text:
    text = text.replace(
        '    // HACK: temporary token guard from production incident\n',
        f'    // HACK: temporary token guard from production incident\n    // security: duplicate charge incident follow-up {value}\n',
    )
else:
    text = re.sub(
        r'// security: duplicate charge incident follow-up \d+',
        f'// security: duplicate charge incident follow-up {value}',
        text,
    )
text = re.sub(
    r'token\.starts_with\("secure-"\)(?: && token\.len\(\) > \d+)?',
    f'token.starts_with("secure-") && token.len() > {value}',
    text,
)
path.write_text(text)
PY
  git add src/auth.rs
  git commit -m "hotfix ${i}: tighten auth guard" >/dev/null
done
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
git add src/util.rs
git commit -m 'feat: add util helper' >/dev/null
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create health fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "health fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    fn quartile_count_rounds_up() {
        assert_eq!(top_quartile_count(&[]), 0);
        assert_eq!(
            top_quartile_count(&[fake_hotspot(), fake_hotspot(), fake_hotspot()]),
            1
        );
        assert_eq!(
            top_quartile_count(&[
                fake_hotspot(),
                fake_hotspot(),
                fake_hotspot(),
                fake_hotspot(),
                fake_hotspot()
            ]),
            2
        );
    }

    #[test]
    fn health_scan_aggregates_current_signals() -> Result<()> {
        let fixture = setup_health_repo()?;
        let report = scan_health(fixture.path())?;

        assert!(report.debt_score > 0);
        assert_eq!(report.signals.get("time_bombs").copied(), Some(1));
        assert_eq!(report.signals.get("stale_hacks").copied(), Some(0));
        assert_eq!(report.signals.get("high_risk_files").copied(), Some(1));
        assert_eq!(report.signals.get("hotspot_files").copied(), Some(1));
        assert!(report.delta.is_none());
        assert!(report.comparison.is_none());
        assert!(report.gate.is_none());
        assert_eq!(report.notes.len(), 2);
        Ok(())
    }

    fn fake_hotspot() -> crate::hotspots::HotspotFinding {
        crate::hotspots::HotspotFinding {
            path: std::path::PathBuf::from("src/example.rs"),
            churn_commits: 1,
            risk_level: why_archaeologist::RiskLevel::LOW,
            hotspot_score: 1.0,
            top_commit_summaries: Vec::new(),
            owners: Vec::new(),
            bus_factor: 0,
            primary_owner: None,
        }
    }
}
