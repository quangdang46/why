use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use why_archaeologist::{
    ArchaeologyResult, RiskLevel, analyze_target_with_options, discover_repository,
};
use why_locator::{
    QueryKind, QueryTarget, SupportedLanguage, SymbolDefinition, list_symbol_definitions,
    resolve_target,
};

use crate::{is_tracked_source_file, should_skip_dir};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenameSafeTarget {
    #[serde(serialize_with = "crate::serialize_path")]
    pub path: PathBuf,
    pub symbol: String,
    pub qualified_name: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
    pub change_guidance: String,
    pub commit_count: usize,
    pub summary: String,
    pub top_commit_summaries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenameSafeCallerFinding {
    #[serde(serialize_with = "crate::serialize_path")]
    pub path: PathBuf,
    pub symbol: String,
    pub qualified_name: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub call_site_count: usize,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
    pub change_guidance: String,
    pub commit_count: usize,
    pub summary: String,
    pub top_commit_summaries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenameSafeReport {
    pub mode: String,
    pub target: RenameSafeTarget,
    pub callers: Vec<RenameSafeCallerFinding>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallerTarget {
    path: PathBuf,
    symbol: String,
    qualified_name: Option<String>,
    start_line: u32,
    end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CallerKey {
    path: PathBuf,
    symbol: String,
    qualified_name: Option<String>,
    start_line: u32,
    end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallerAccumulator {
    key: CallerKey,
    call_site_count: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SourceScanSummary {
    skipped_top_level_occurrences: usize,
    same_name_alternatives: usize,
}

pub fn scan_rename_safe(
    repo_root: &Path,
    target: &QueryTarget,
    since_days: Option<u64>,
) -> Result<RenameSafeReport> {
    if !matches!(
        target.query_kind,
        QueryKind::Symbol | QueryKind::QualifiedSymbol
    ) {
        bail!("rename-safe requires a symbol target like <file>:<symbol> or <file>:<Type::method>");
    }

    let resolved = resolve_target(target, repo_root)?;
    let absolute_target_path = repo_root.join(&resolved.path);
    let language = SupportedLanguage::detect(&absolute_target_path)?;
    if language != SupportedLanguage::Rust {
        bail!("rename-safe currently supports Rust symbol targets only");
    }

    let target_source = fs::read_to_string(&absolute_target_path).with_context(|| {
        format!(
            "failed to read source file {}",
            absolute_target_path.display()
        )
    })?;
    let target_definitions = list_symbol_definitions(language, &target_source)?;
    let target_definition = target_definitions
        .into_iter()
        .find(|definition| {
            definition.start_line == resolved.start_line && definition.end_line == resolved.end_line
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "rename-safe could not match the resolved target to a Rust symbol definition"
            )
        })?;

    let caller_target = CallerTarget {
        path: resolved.path.clone(),
        symbol: target_definition.name.clone(),
        qualified_name: target_definition.qualified_name.clone(),
        start_line: target_definition.start_line,
        end_line: target_definition.end_line,
    };

    let repo = discover_repository(repo_root)?;
    let workdir = repo
        .workdir()
        .context("repository does not have a working directory")?;

    let mut caller_map = BTreeMap::new();
    let scan_summary =
        collect_callers_in_dir(&repo, workdir, workdir, &caller_target, &mut caller_map)?;

    let target_result = analyze_target_with_options(target, repo_root, since_days)?;
    let mut callers = caller_map
        .into_values()
        .map(|caller| build_caller_finding(repo_root, caller, since_days))
        .collect::<Result<Vec<_>>>()?;
    callers.sort_by(|left, right| {
        risk_rank(right.risk_level)
            .cmp(&risk_rank(left.risk_level))
            .then(right.commit_count.cmp(&left.commit_count))
            .then(right.call_site_count.cmp(&left.call_site_count))
            .then(left.path.cmp(&right.path))
            .then(left.start_line.cmp(&right.start_line))
    });

    let mut notes = vec![format!(
        "Rename-safe caller discovery is heuristic: scanned Rust call-like occurrences of `{}` and attributed them to the smallest enclosing symbol.",
        caller_target.symbol
    )];
    if let Some(qualified_name) = &caller_target.qualified_name {
        notes.push(format!(
            "Resolved target to `{}`. Prefer `{}:{}` for future rename-safe runs.",
            qualified_name,
            caller_target.path.display(),
            qualified_name
        ));
    }
    if scan_summary.same_name_alternatives > 0 {
        notes.push(format!(
            "Found {} other Rust symbol(s) named `{}`. Caller matching still keys on the short method name, so validate these results before renaming.",
            scan_summary.same_name_alternatives,
            caller_target.symbol
        ));
    }
    if scan_summary.skipped_top_level_occurrences > 0 {
        notes.push(format!(
            "Skipped {} call-like occurrence(s) outside an enclosing Rust symbol.",
            scan_summary.skipped_top_level_occurrences
        ));
    }
    if callers.is_empty() {
        notes.push(format!(
            "No caller symbols were found for `{}`.",
            caller_target
                .qualified_name
                .as_deref()
                .unwrap_or(&caller_target.symbol)
        ));
    }

    Ok(RenameSafeReport {
        mode: "rename-safe".to_string(),
        target: RenameSafeTarget {
            path: target_result.target.path.clone(),
            symbol: caller_target.symbol,
            qualified_name: caller_target.qualified_name,
            start_line: target_definition.start_line,
            end_line: target_definition.end_line,
            risk_level: target_result.risk_level,
            risk_summary: target_result.risk_summary.clone(),
            change_guidance: target_result.change_guidance.clone(),
            commit_count: target_result.commits.len(),
            summary: primary_summary(&target_result),
            top_commit_summaries: top_commit_summaries(&target_result),
        },
        callers,
        notes,
    })
}

fn build_caller_finding(
    repo_root: &Path,
    caller: CallerAccumulator,
    since_days: Option<u64>,
) -> Result<RenameSafeCallerFinding> {
    let result = analyze_target_with_options(
        &QueryTarget {
            path: caller.key.path.clone(),
            start_line: Some(caller.key.start_line),
            end_line: Some(caller.key.end_line),
            symbol: None,
            query_kind: QueryKind::Range,
        },
        repo_root,
        since_days,
    )?;

    Ok(RenameSafeCallerFinding {
        path: result.target.path.clone(),
        symbol: caller.key.symbol,
        qualified_name: caller.key.qualified_name,
        start_line: caller.key.start_line,
        end_line: caller.key.end_line,
        call_site_count: caller.call_site_count,
        risk_level: result.risk_level,
        risk_summary: result.risk_summary.clone(),
        change_guidance: result.change_guidance.clone(),
        commit_count: result.commits.len(),
        summary: primary_summary(&result),
        top_commit_summaries: top_commit_summaries(&result),
    })
}

fn collect_callers_in_dir(
    repo: &git2::Repository,
    workdir: &Path,
    dir: &Path,
    target: &CallerTarget,
    callers: &mut BTreeMap<CallerKey, CallerAccumulator>,
) -> Result<SourceScanSummary> {
    let mut summary = SourceScanSummary::default();

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
            let child = collect_callers_in_dir(repo, workdir, &path, target, callers)?;
            summary.skipped_top_level_occurrences += child.skipped_top_level_occurrences;
            summary.same_name_alternatives += child.same_name_alternatives;
            continue;
        }

        if !file_type.is_file() || !is_tracked_source_file(repo, workdir, &path) {
            continue;
        }

        if SupportedLanguage::detect(&path).ok() != Some(SupportedLanguage::Rust) {
            continue;
        }

        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read source file {}", path.display()))?;
        let relative_path = path
            .strip_prefix(workdir)
            .with_context(|| format!("{} is not inside {}", path.display(), workdir.display()))?;
        let file_summary = collect_callers_from_source(relative_path, &source, target, callers)?;
        summary.skipped_top_level_occurrences += file_summary.skipped_top_level_occurrences;
        summary.same_name_alternatives += file_summary.same_name_alternatives;
    }

    Ok(summary)
}

fn collect_callers_from_source(
    relative_path: &Path,
    source: &str,
    target: &CallerTarget,
    callers: &mut BTreeMap<CallerKey, CallerAccumulator>,
) -> Result<SourceScanSummary> {
    let definitions = list_symbol_definitions(SupportedLanguage::Rust, source)?;
    let mut summary = SourceScanSummary {
        skipped_top_level_occurrences: 0,
        same_name_alternatives: definitions
            .iter()
            .filter(|definition| {
                definition.name == target.symbol
                    && !is_target_definition(relative_path, definition, target)
            })
            .count(),
    };

    for line in extract_call_like_occurrence_lines(source, &target.symbol) {
        let Some(definition) = smallest_enclosing_symbol(&definitions, line) else {
            summary.skipped_top_level_occurrences += 1;
            continue;
        };

        if is_definition_signature_occurrence(definition, line, &target.symbol)
            || is_target_definition(relative_path, definition, target)
        {
            continue;
        }

        let key = CallerKey {
            path: relative_path.to_path_buf(),
            symbol: definition.name.clone(),
            qualified_name: definition.qualified_name.clone(),
            start_line: definition.start_line,
            end_line: definition.end_line,
        };

        callers
            .entry(key.clone())
            .and_modify(|entry| entry.call_site_count += 1)
            .or_insert(CallerAccumulator {
                key,
                call_site_count: 1,
            });
    }

    Ok(summary)
}

fn smallest_enclosing_symbol(
    definitions: &[SymbolDefinition],
    line: u32,
) -> Option<&SymbolDefinition> {
    definitions
        .iter()
        .filter(|definition| definition.start_line <= line && line <= definition.end_line)
        .min_by_key(|definition| {
            (
                definition.end_line.saturating_sub(definition.start_line),
                definition.start_line,
            )
        })
}

fn is_definition_signature_occurrence(
    definition: &SymbolDefinition,
    line: u32,
    target_symbol: &str,
) -> bool {
    definition.start_line == line && definition.name == target_symbol
}

fn is_target_definition(
    relative_path: &Path,
    definition: &SymbolDefinition,
    target: &CallerTarget,
) -> bool {
    relative_path == target.path
        && definition.name == target.symbol
        && definition.qualified_name == target.qualified_name
        && definition.start_line == target.start_line
        && definition.end_line == target.end_line
}

fn extract_call_like_occurrence_lines(source: &str, target_symbol: &str) -> Vec<u32> {
    let bytes = source.as_bytes();
    let line_starts = line_starts(bytes);
    let mut index = 0;
    let mut lines = Vec::new();

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

        if lookahead < bytes.len()
            && bytes[lookahead] == b'('
            && &source[start..index] == target_symbol
        {
            let line = line_starts.partition_point(|offset| *offset <= start) as u32;
            lines.push(line.max(1));
        }
    }

    lines
}

fn line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' && index + 1 < bytes.len() {
            starts.push(index + 1);
        }
    }
    starts
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn risk_rank(level: RiskLevel) -> u8 {
    match level {
        RiskLevel::HIGH => 3,
        RiskLevel::MEDIUM => 2,
        RiskLevel::LOW => 1,
    }
}

fn primary_summary(result: &ArchaeologyResult) -> String {
    result
        .commits
        .first()
        .map(|commit| commit.summary.clone())
        .unwrap_or_else(|| result.risk_summary.clone())
}

fn top_commit_summaries(result: &ArchaeologyResult) -> Vec<String> {
    result
        .commits
        .iter()
        .take(3)
        .map(|commit| commit.summary.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CallerAccumulator, CallerTarget, QueryKind, QueryTarget, SupportedLanguage,
        collect_callers_from_source, extract_call_like_occurrence_lines, scan_rename_safe,
    };
    use anyhow::Result;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn extracts_call_like_occurrences_with_line_numbers() {
        let source =
            "fn alpha() {\n    process_payment();\n}\n\nfn beta() {\n    process_payment ( );\n}\n";
        assert_eq!(
            extract_call_like_occurrence_lines(source, "process_payment"),
            vec![2, 6]
        );
    }

    #[test]
    fn collects_callers_and_dedupes_repeated_calls() -> Result<()> {
        let source = r#"
struct PaymentService;
struct RefundService;
struct CheckoutOrchestrator;

impl PaymentService {
    fn process_payment(&self) {
        charge();
    }
}

impl RefundService {
    fn process_payment(&self) {
        refund();
    }
}

impl CheckoutOrchestrator {
    fn complete_checkout(&self, payment: &PaymentService) {
        payment.process_payment();
        payment.process_payment();
    }
}
"#;
        let target_definition = super::list_symbol_definitions(SupportedLanguage::Rust, source)?
            .into_iter()
            .find(|definition| {
                definition.qualified_name.as_deref() == Some("PaymentService::process_payment")
            })
            .expect("target definition should exist");
        let target = CallerTarget {
            path: PathBuf::from("src/payment.rs"),
            symbol: "process_payment".into(),
            qualified_name: Some("PaymentService::process_payment".into()),
            start_line: target_definition.start_line,
            end_line: target_definition.end_line,
        };
        let mut callers = BTreeMap::new();

        let summary = collect_callers_from_source(
            Path::new("src/payment.rs"),
            source,
            &target,
            &mut callers,
        )?;

        assert_eq!(summary.same_name_alternatives, 1);
        assert_eq!(summary.skipped_top_level_occurrences, 0);
        assert_eq!(callers.len(), 1);
        let collected = callers.values().next().expect("caller should exist");
        assert_eq!(collected.call_site_count, 2);
        assert_eq!(collected.key.symbol, "complete_checkout");
        assert_eq!(
            collected.key.qualified_name.as_deref(),
            Some("CheckoutOrchestrator::complete_checkout")
        );
        Ok(())
    }

    #[test]
    fn rename_safe_rejects_non_symbol_targets() {
        let dir = tempdir().expect("tempdir");
        let target = QueryTarget {
            path: PathBuf::from("src/lib.rs"),
            start_line: Some(10),
            end_line: Some(12),
            symbol: None,
            query_kind: QueryKind::Range,
        };
        let error = scan_rename_safe(dir.path(), &target, None)
            .expect_err("rename-safe should reject non-symbol targets");
        assert!(
            error
                .to_string()
                .contains("rename-safe requires a symbol target")
        );
    }

    #[test]
    fn rename_safe_rejects_non_rust_targets() -> Result<()> {
        let dir = tempdir()?;
        let file = dir.path().join("src").join("auth.ts");
        fs::create_dir_all(file.parent().expect("parent"))?;
        fs::write(
            &file,
            "export function authenticate(token: string): boolean { return token.length > 0; }\n",
        )?;
        let target = QueryTarget {
            path: PathBuf::from("src/auth.ts"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };
        let error = scan_rename_safe(dir.path(), &target, None)
            .expect_err("rename-safe should reject non-Rust targets");
        assert!(
            error
                .to_string()
                .contains("rename-safe currently supports Rust symbol targets only")
        );
        Ok(())
    }

    #[allow(dead_code)]
    fn _assert_caller_count(
        callers: &BTreeMap<super::CallerKey, CallerAccumulator>,
        expected: usize,
    ) {
        assert_eq!(callers.len(), expected);
    }
}
