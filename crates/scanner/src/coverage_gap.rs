use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::Serialize;
use why_archaeologist::{RiskLevel, analyze_target_with_options, discover_repository};
use why_locator::{QueryKind, SupportedLanguage, list_all_symbols};

use crate::{is_tracked_source_file, should_skip_dir};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CoverageGapFinding {
    pub path: PathBuf,
    pub symbol: String,
    pub start_line: u32,
    pub end_line: u32,
    pub risk_level: RiskLevel,
    pub coverage_pct: f32,
    pub instrumented_lines: usize,
    pub covered_lines: usize,
    pub commit_count: usize,
    pub risk_flags: Vec<String>,
    pub summary: String,
    pub risk_summary: String,
    pub change_guidance: String,
    pub top_commit_summaries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CoverageGapReport {
    pub coverage_path: PathBuf,
    pub max_coverage: f32,
    pub findings: Vec<CoverageGapFinding>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct CoverageData {
    files: BTreeMap<PathBuf, FileCoverage>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct FileCoverage {
    lines: BTreeMap<u32, u64>,
}

impl CoverageData {
    fn from_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read coverage report {}", path.display()))?;
        let trimmed = text.trim_start();
        if trimmed.starts_with("TN:")
            || trimmed.starts_with("SF:")
            || trimmed.contains("\nSF:")
            || trimmed.contains("\nDA:")
        {
            Self::parse_lcov(&text)
        } else {
            Self::parse_llvm_cov_json(&text)
        }
    }

    fn parse_lcov(text: &str) -> Result<Self> {
        let mut data = CoverageData::default();
        let mut current_file: Option<PathBuf> = None;

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if let Some(path) = line.strip_prefix("SF:") {
                current_file = Some(normalize_coverage_path(path));
                continue;
            }
            if line == "end_of_record" {
                current_file = None;
                continue;
            }
            let Some(payload) = line.strip_prefix("DA:") else {
                continue;
            };
            let Some(file_path) = current_file.as_ref() else {
                continue;
            };
            let mut parts = payload.split(',');
            let line_number = parts
                .next()
                .context("LCOV record missing line number")?
                .parse::<u32>()
                .context("LCOV line number was not a valid integer")?;
            let hit_count = parts
                .next()
                .context("LCOV record missing hit count")?
                .parse::<u64>()
                .context("LCOV hit count was not a valid integer")?;
            data.files
                .entry(file_path.clone())
                .or_default()
                .lines
                .insert(line_number, hit_count);
        }

        Ok(data)
    }

    fn parse_llvm_cov_json(text: &str) -> Result<Self> {
        let parsed: LlvmCovExport =
            serde_json::from_str(text).context("failed to parse llvm-cov JSON coverage report")?;
        let mut data = CoverageData::default();

        for export in parsed.data {
            for file in export.files {
                let path = normalize_coverage_path(&file.filename);
                let entry = data.files.entry(path).or_default();
                for segment in file.segments {
                    if segment.len() < 3 {
                        continue;
                    }
                    let Some(line) = segment[0].as_u64() else {
                        continue;
                    };
                    if line == 0 {
                        continue;
                    }
                    let count = segment[2].as_i64().unwrap_or_default();
                    let line =
                        u32::try_from(line).context("llvm-cov line number overflowed u32")?;
                    let count = if count < 0 { 0 } else { count as u64 };
                    entry
                        .lines
                        .entry(line)
                        .and_modify(|existing| *existing = (*existing).max(count))
                        .or_insert(count);
                }
            }
        }

        Ok(data)
    }

    fn coverage_for_range(&self, path: &Path, start_line: u32, end_line: u32) -> CoverageSummary {
        let Some(file) = self.files.get(path).or_else(|| self.find_by_suffix(path)) else {
            return CoverageSummary::default();
        };

        let mut instrumented_lines = 0usize;
        let mut covered_lines = 0usize;
        for line in start_line..=end_line {
            let Some(hit_count) = file.lines.get(&line) else {
                continue;
            };
            instrumented_lines += 1;
            if *hit_count > 0 {
                covered_lines += 1;
            }
        }

        CoverageSummary {
            instrumented_lines,
            covered_lines,
        }
    }

    fn find_by_suffix(&self, path: &Path) -> Option<&FileCoverage> {
        self.files.iter().find_map(|(candidate, coverage)| {
            if candidate == path || candidate.ends_with(path) {
                Some(coverage)
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CoverageSummary {
    instrumented_lines: usize,
    covered_lines: usize,
}

impl CoverageSummary {
    fn pct(self) -> f32 {
        if self.instrumented_lines == 0 {
            0.0
        } else {
            (self.covered_lines as f32 / self.instrumented_lines as f32) * 100.0
        }
    }
}

#[derive(Debug, Deserialize)]
struct LlvmCovExport {
    data: Vec<LlvmCovData>,
}

#[derive(Debug, Deserialize)]
struct LlvmCovData {
    files: Vec<LlvmCovFile>,
}

#[derive(Debug, Deserialize)]
struct LlvmCovFile {
    filename: String,
    segments: Vec<Vec<serde_json::Value>>,
}

pub fn scan_coverage_gap(
    repo_root: &Path,
    coverage_path: &Path,
    limit: usize,
    max_coverage: f32,
) -> Result<CoverageGapReport> {
    if limit == 0 {
        bail!("limit must be greater than zero");
    }
    if !(0.0..=100.0).contains(&max_coverage) {
        bail!("max coverage must be between 0 and 100");
    }

    let repo = discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;
    let coverage_path = if coverage_path.is_absolute() {
        coverage_path.to_path_buf()
    } else {
        repo_root.join(coverage_path)
    };
    let coverage = CoverageData::from_file(&coverage_path)?;

    let mut findings = Vec::new();
    collect_findings(
        &repo,
        workdir,
        workdir,
        &coverage,
        max_coverage,
        &mut findings,
    )?;
    findings.sort_by(|left, right| {
        left.coverage_pct
            .partial_cmp(&right.coverage_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(right.commit_count.cmp(&left.commit_count))
            .then(left.path.cmp(&right.path))
            .then(left.start_line.cmp(&right.start_line))
    });
    findings.truncate(limit);

    let mut notes = vec![
        "Coverage percentage counts only instrumented lines inside each symbol range; lines without coverage entries are ignored.".into(),
        "Only HIGH-risk symbols at or below the requested coverage threshold are reported.".into(),
    ];
    if findings.is_empty() {
        notes.push("No HIGH-risk symbols met the current coverage-gap threshold.".into());
    }

    Ok(CoverageGapReport {
        coverage_path,
        max_coverage,
        findings,
        notes,
    })
}

fn collect_findings(
    repo: &git2::Repository,
    workdir: &Path,
    dir: &Path,
    coverage: &CoverageData,
    max_coverage: f32,
    findings: &mut Vec<CoverageGapFinding>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;

        if file_type.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_findings(repo, workdir, &path, coverage, max_coverage, findings)?;
            continue;
        }

        if !file_type.is_file() || !is_tracked_source_file(repo, workdir, &path) {
            continue;
        }

        findings.extend(analyze_file_symbols(
            workdir,
            &path,
            coverage,
            max_coverage,
        )?);
    }

    Ok(())
}

fn analyze_file_symbols(
    workdir: &Path,
    absolute_path: &Path,
    coverage: &CoverageData,
    max_coverage: f32,
) -> Result<Vec<CoverageGapFinding>> {
    let language = match SupportedLanguage::detect(absolute_path) {
        Ok(language) => language,
        Err(_) => return Ok(Vec::new()),
    };
    let source = fs::read_to_string(absolute_path)
        .with_context(|| format!("failed to read source file {}", absolute_path.display()))?;
    let relative_path = absolute_path
        .strip_prefix(workdir)
        .with_context(|| {
            format!(
                "{} is not inside {}",
                absolute_path.display(),
                workdir.display()
            )
        })?
        .to_path_buf();

    let mut findings = Vec::new();
    for (symbol, start_line, end_line) in list_all_symbols(language, &source)? {
        let coverage_summary = coverage.coverage_for_range(&relative_path, start_line, end_line);
        if coverage_summary.instrumented_lines == 0 {
            continue;
        }
        let coverage_pct = coverage_summary.pct();
        if coverage_pct > max_coverage {
            continue;
        }

        let result = analyze_target_with_options(
            &why_locator::QueryTarget {
                path: relative_path.clone(),
                start_line: None,
                end_line: None,
                symbol: Some(symbol.clone()),
                query_kind: QueryKind::Symbol,
            },
            workdir,
            None,
        )?;
        if result.risk_level != RiskLevel::HIGH
            || result.commits.is_empty()
            || result.local_context.risk_flags.is_empty()
        {
            continue;
        }

        let summary = result
            .commits
            .first()
            .map(|commit| commit.summary.clone())
            .unwrap_or_else(|| result.risk_summary.clone());
        let top_commit_summaries = result
            .commits
            .iter()
            .take(3)
            .map(|commit| commit.summary.clone())
            .collect();

        findings.push(CoverageGapFinding {
            path: relative_path.clone(),
            symbol,
            start_line,
            end_line,
            risk_level: result.risk_level,
            coverage_pct,
            instrumented_lines: coverage_summary.instrumented_lines,
            covered_lines: coverage_summary.covered_lines,
            commit_count: result.commits.len(),
            risk_flags: result.local_context.risk_flags.clone(),
            summary,
            risk_summary: result.risk_summary,
            change_guidance: result.change_guidance,
            top_commit_summaries,
        });
    }

    Ok(findings)
}

fn normalize_coverage_path(raw: &str) -> PathBuf {
    let normalized = raw.replace('\\', "/");
    let trimmed = normalized.strip_prefix("./").unwrap_or(&normalized);
    let trimmed = if trimmed.len() >= 2
        && trimmed.as_bytes()[1] == b':'
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
    {
        trimmed[2..].trim_start_matches('/')
    } else {
        trimmed
    };
    let path = Path::new(trimmed);
    let mut parts = Vec::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = parts.pop();
            }
            Component::RootDir | Component::Prefix(_) => {
                parts.clear();
            }
        }
    }

    let mut normalized_path = PathBuf::new();
    for part in parts {
        normalized_path.push(part);
    }
    normalized_path
}

#[cfg(test)]
mod tests {
    use super::{CoverageData, normalize_coverage_path, scan_coverage_gap};
    use anyhow::{Context, Result};
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_coverage_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for coverage fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email test@example.com
git config user.name 'Fixture Bot'
mkdir -p src
cat > src/auth.rs <<'EOF'
pub fn authenticate(token: &str) -> bool {
    // security: preserve legacy token validation while rollout completes
    // hotfix: block stale token replay from incident #1234
    token.starts_with("secure-") && token.len() > 4
}

pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
git add src/auth.rs
git commit -m 'feat: add auth helpers' >/dev/null
cat > src/auth.rs <<'EOF'
pub fn authenticate(token: &str) -> bool {
    // security: preserve legacy token validation while rollout completes
    // hotfix: block stale token replay from incident #1234
    if token.is_empty() {
        return false;
    }
    token.starts_with("secure-") && token.len() > 4
}

pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
git add src/auth.rs
git commit -m 'hotfix: harden auth token validation' >/dev/null
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create coverage fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "coverage fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    fn parse_lcov_basic() -> Result<()> {
        let data =
            CoverageData::parse_lcov("SF:src/auth.rs\nDA:1,1\nDA:2,0\nDA:3,3\nend_of_record\n")?;
        let summary = data.coverage_for_range(std::path::Path::new("src/auth.rs"), 1, 3);
        assert_eq!(summary.instrumented_lines, 3);
        assert_eq!(summary.covered_lines, 2);
        assert!((summary.pct() - 66.666664).abs() < 0.01);
        Ok(())
    }

    #[test]
    fn parse_llvm_cov_json_basic() -> Result<()> {
        let data = CoverageData::parse_llvm_cov_json(
            r#"{
              "data": [
                {
                  "files": [
                    {
                      "filename": "src/auth.rs",
                      "segments": [
                        [1, 0, 2, true, true, false],
                        [2, 0, 0, true, true, false],
                        [3, 0, 1, true, true, false]
                      ]
                    }
                  ]
                }
              ]
            }"#,
        )?;
        let summary = data.coverage_for_range(std::path::Path::new("src/auth.rs"), 1, 3);
        assert_eq!(summary.instrumented_lines, 3);
        assert_eq!(summary.covered_lines, 2);
        Ok(())
    }

    #[test]
    fn normalize_coverage_path_drops_absolute_prefixes() {
        assert_eq!(
            normalize_coverage_path("/tmp/work/src/auth.rs"),
            std::path::PathBuf::from("tmp/work/src/auth.rs")
        );
        assert_eq!(
            normalize_coverage_path("C:\\repo\\src\\auth.rs"),
            std::path::PathBuf::from("repo/src/auth.rs")
        );
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_coverage_gap_reports_high_risk_uncovered_symbols() -> Result<()> {
        let fixture = setup_coverage_repo()?;
        let coverage_path = fixture.path().join("lcov.info");
        fs::write(
            &coverage_path,
            "TN:\nSF:src/auth.rs\nDA:1,0\nDA:2,0\nDA:3,0\nDA:4,0\nDA:5,0\nDA:6,0\nDA:7,0\nDA:8,0\nDA:10,1\nDA:11,1\nDA:12,1\nend_of_record\n",
        )?;

        let report = scan_coverage_gap(fixture.path(), &coverage_path, 10, 20.0)?;
        assert_eq!(report.findings.len(), 1);
        let finding = &report.findings[0];
        assert_eq!(finding.path, std::path::Path::new("src/auth.rs"));
        assert_eq!(finding.symbol, "authenticate");
        assert_eq!(finding.risk_level, why_archaeologist::RiskLevel::HIGH);
        assert_eq!(finding.instrumented_lines, 8);
        assert_eq!(finding.covered_lines, 0);
        assert_eq!(finding.coverage_pct, 0.0);
        assert!(!finding.risk_flags.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_coverage_gap_respects_threshold() -> Result<()> {
        let fixture = setup_coverage_repo()?;
        let coverage_path = fixture.path().join("llvm-cov.json");
        fs::write(
            &coverage_path,
            r#"{
              "data": [
                {
                  "files": [
                    {
                      "filename": "src/auth.rs",
                      "segments": [
                        [1, 0, 1, true, true, false],
                        [2, 0, 1, true, true, false],
                        [3, 0, 1, true, true, false],
                        [4, 0, 0, true, true, false],
                        [5, 0, 0, true, true, false],
                        [6, 0, 0, true, true, false]
                      ]
                    }
                  ]
                }
              ]
            }"#,
        )?;

        let report = scan_coverage_gap(fixture.path(), &coverage_path, 10, 40.0)?;
        assert_eq!(report.findings.len(), 0);
        Ok(())
    }
}
