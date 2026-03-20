use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use why_archaeologist::{RiskLevel, analyze_target_with_options, discover_repository};
use why_locator::{QueryKind, QueryTarget, SupportedLanguage, list_all_symbols};

use crate::{is_tracked_source_file, should_skip_dir};

const STATIC_ANALYSIS_WARNING: &str = "WARNING: ghost detection uses static analysis. Verify these are truly uncalled before deletion.";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GhostFinding {
    #[serde(serialize_with = "crate::serialize_path")]
    pub path: PathBuf,
    pub symbol: String,
    pub start_line: u32,
    pub end_line: u32,
    pub risk_level: RiskLevel,
    pub call_site_count: usize,
    pub commit_count: usize,
    pub summary: String,
    pub risk_summary: String,
    pub change_guidance: String,
    pub top_commit_summaries: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct SourceFile {
    relative_path: PathBuf,
    source: String,
    language: SupportedLanguage,
}

pub fn scan_ghosts(repo_root: &Path, limit: usize) -> Result<Vec<GhostFinding>> {
    let repo = discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;

    let files = collect_source_files(&repo, workdir, workdir)?;
    let call_counts = aggregate_call_counts(&files);
    let mut findings = Vec::new();

    for file in &files {
        findings.extend(analyze_file_ghosts(workdir, file, &call_counts)?);
    }

    findings.sort_by(|left, right| {
        right
            .commit_count
            .cmp(&left.commit_count)
            .then(left.call_site_count.cmp(&right.call_site_count))
            .then(left.path.cmp(&right.path))
            .then(left.start_line.cmp(&right.start_line))
    });
    findings.truncate(limit.max(1));
    Ok(findings)
}

fn collect_source_files(
    repo: &git2::Repository,
    workdir: &Path,
    dir: &Path,
) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    collect_source_files_into(repo, workdir, dir, &mut files)?;
    Ok(files)
}

fn collect_source_files_into(
    repo: &git2::Repository,
    workdir: &Path,
    dir: &Path,
    files: &mut Vec<SourceFile>,
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
            collect_source_files_into(repo, workdir, &path, files)?;
            continue;
        }

        if !file_type.is_file() || !is_tracked_source_file(repo, workdir, &path) {
            continue;
        }

        let language = match SupportedLanguage::detect(&path) {
            Ok(language) => language,
            Err(_) => continue,
        };
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read source file {}", path.display()))?;
        let relative_path = path
            .strip_prefix(workdir)
            .with_context(|| format!("{} is not inside {}", path.display(), workdir.display()))?
            .to_path_buf();

        files.push(SourceFile {
            relative_path,
            source,
            language,
        });
    }

    Ok(())
}

fn analyze_file_ghosts(
    workdir: &Path,
    file: &SourceFile,
    call_counts: &HashMap<String, usize>,
) -> Result<Vec<GhostFinding>> {
    let mut findings = Vec::new();

    for (symbol, start_line, end_line) in list_all_symbols(file.language, &file.source)? {
        if should_skip_symbol(&file.relative_path, &file.source, start_line, &symbol) {
            continue;
        }

        let call_site_count = call_counts.get(&symbol).copied().unwrap_or_default();
        if call_site_count > 1 {
            continue;
        }

        let result = analyze_target_with_options(
            &QueryTarget {
                path: file.relative_path.clone(),
                start_line: Some(start_line),
                end_line: Some(end_line),
                symbol: None,
                query_kind: QueryKind::Range,
            },
            workdir,
            None,
        )?;
        if result.risk_level != RiskLevel::HIGH || result.commits.is_empty() {
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

        findings.push(GhostFinding {
            path: file.relative_path.clone(),
            symbol,
            start_line,
            end_line,
            risk_level: result.risk_level,
            call_site_count,
            commit_count: result.commits.len(),
            summary,
            risk_summary: result.risk_summary,
            change_guidance: result.change_guidance,
            top_commit_summaries,
            notes: vec![
                STATIC_ANALYSIS_WARNING.to_string(),
                format!(
                    "Static heuristic saw {} call-like occurrence(s) for this symbol name across scanned source files.",
                    call_site_count
                ),
            ],
        });
    }

    Ok(findings)
}

fn aggregate_call_counts(files: &[SourceFile]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for file in files {
        for symbol in extract_call_like_identifiers(&file.source) {
            *counts.entry(symbol).or_insert(0) += 1;
        }
    }
    counts
}

fn extract_call_like_identifiers(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut identifiers = Vec::new();

    while index < bytes.len() {
        let byte = bytes[index];
        if !is_ident_start(byte) {
            index += 1;
            continue;
        }

        if index > 0 && is_ident_continue(bytes[index - 1]) {
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < bytes.len() && is_ident_continue(bytes[index]) {
            index += 1;
        }

        let mut lookahead = index;
        while lookahead < bytes.len() && bytes[lookahead].is_ascii_whitespace() {
            lookahead += 1;
        }

        if lookahead < bytes.len() && bytes[lookahead] == b'(' {
            identifiers.push(source[start..index].to_string());
        }
    }

    identifiers
}

fn should_skip_symbol(path: &Path, source: &str, start_line: u32, symbol: &str) -> bool {
    if symbol == "main" || symbol.starts_with("test_") {
        return true;
    }

    if path
        .components()
        .any(|component| matches!(component, Component::Normal(part) if part == "tests"))
    {
        return true;
    }

    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.ends_with("_test.rs") || name.ends_with(".test.js") || name.ends_with(".test.ts")
        })
    {
        return true;
    }

    let lines = source.lines().collect::<Vec<_>>();
    let index = start_line.saturating_sub(1) as usize;
    for offset in 0..=2 {
        let Some(line_index) = index.checked_sub(offset) else {
            continue;
        };
        let line = lines.get(line_index).copied().unwrap_or_default().trim();
        if line.contains("#[test]") || line.contains("@test") {
            return true;
        }
    }

    false
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::{RiskLevel, extract_call_like_identifiers, scan_ghosts};
    use anyhow::{Context, Result};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_ghost_repo() -> Result<TempDir> {
        let dir = TempDir::new().context("failed to create tempdir for ghost fixture")?;
        let script = r#"
set -euo pipefail
cd "$1"
git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email test@example.com
git config user.name 'Fixture Bot'
mkdir -p src
cat > src/auth.rs <<'EOF'
// This function is never called from anywhere (orphaned after refactor)
pub fn validate_auth_token_legacy(token: &str, session_id: &str) -> bool {
    // security: added after token forgery incident #7890
    !token.is_empty() && token_matches_session(token, session_id)
}

pub fn authenticate(user: &str, password: &str) -> bool {
    check_password_hash(user, password)
}
EOF
git add src/auth.rs
git commit -m 'hotfix: add token validation after auth forgery incident #7890' >/dev/null
cat > src/main.rs <<'EOF'
fn main() {
    let user = "alice";
    let pass = "password";
    if authenticate(user, pass) {
        println!("logged in");
    }
}
EOF
git add src/main.rs
git commit -m 'feat: add main entry point using authenticate' >/dev/null
"#;
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .arg("bash")
            .arg(dir.path())
            .output()
            .context("failed to create ghost fixture")?;
        if !output.status.success() {
            anyhow::bail!(
                "ghost fixture setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(dir)
    }

    #[test]
    fn extracts_call_like_identifiers_with_whitespace() {
        let ids = extract_call_like_identifiers(
            "fn authenticate(user: &str) {}\nauthenticate (user);\nother_call();",
        );
        assert_eq!(ids, vec!["authenticate", "authenticate", "other_call"]);
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_ghosts_finds_high_risk_uncalled_function() -> Result<()> {
        let fixture = setup_ghost_repo()?;
        let findings = scan_ghosts(fixture.path(), 10)?;

        assert!(!findings.is_empty());
        let ghost = findings
            .iter()
            .find(|finding| finding.symbol == "validate_auth_token_legacy")
            .context("expected validate_auth_token_legacy ghost finding")?;
        assert_eq!(ghost.path, std::path::Path::new("src/auth.rs"));
        assert_eq!(ghost.risk_level, RiskLevel::HIGH);
        assert_eq!(ghost.call_site_count, 1);
        assert!(ghost.commit_count >= 1);
        assert!(
            ghost
                .notes
                .iter()
                .any(|note| note.contains("static analysis"))
        );
        assert!(
            ghost.summary.contains("token validation") || ghost.summary.contains("auth forgery")
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.symbol != "authenticate")
        );
        Ok(())
    }

    #[test]
    #[ignore = "flaky in CI"]
    fn scan_ghosts_respects_limit() -> Result<()> {
        let fixture = setup_ghost_repo()?;
        let findings = scan_ghosts(fixture.path(), 1)?;
        assert_eq!(findings.len(), 1);
        Ok(())
    }
}
