use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::{stdin, stdout};
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, MarkupContent, MarkupKind, MessageType, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};
use tower_lsp::{Client, LanguageServer, LspService, Server, async_trait};
use why_archaeologist::{
    ArchaeologyResult, analyze_target, discover_repository, relative_repo_path,
};
use why_locator::{QueryKind, QueryTarget, detect_language, list_all_symbols};

/// Run the why LSP server over stdio.
pub async fn run_stdio() -> Result<()> {
    let (service, socket) = LspService::new(|client| Backend { client });
    Server::new(stdin(), stdout(), socket).serve(service).await;
    Ok(())
}

struct Backend {
    client: Client,
}

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> LspResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::NONE,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            server_info: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "why LSP hover server ready")
            .await;
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        hover_at(params).map_err(|error| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InternalError,
            message: error.to_string().into(),
            data: None,
        })
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }
}

fn hover_at(params: HoverParams) -> Result<Option<Hover>> {
    let position = params.text_document_position_params.position;
    let uri = params.text_document_position_params.text_document.uri;
    let file_path = match uri.to_file_path() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };

    let repo = match discover_repository(file_path.parent().unwrap_or(file_path.as_path())) {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };
    let repo_root = repo
        .workdir()
        .map(PathBuf::from)
        .unwrap_or_else(|| file_path.parent().unwrap_or(Path::new(".")).to_path_buf());
    let line_number = position.line + 1;
    let relative_path = relative_repo_path(&repo, &file_path)?;
    let symbol_name = symbol_name_for_hover(&file_path, line_number)?;
    let target = QueryTarget {
        path: relative_path.clone(),
        start_line: Some(line_number),
        end_line: Some(line_number),
        symbol: None,
        query_kind: QueryKind::Line,
    };
    let report = analyze_target(&target, &repo_root)?;
    let cli_target = format!("{}:{}", relative_path.display(), line_number);
    let display_target = symbol_name
        .as_deref()
        .map(|symbol| format!("{symbol}()"))
        .unwrap_or_else(|| cli_target.clone());

    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format_hover_markdown(&display_target, &cli_target, &report),
        }),
        range: None,
    }))
}

fn symbol_name_for_hover(file_path: &Path, line_number: u32) -> Result<Option<String>> {
    let language = match detect_language(file_path) {
        Ok(language) => language,
        Err(_) => return Ok(None),
    };
    let source = fs::read_to_string(file_path)?;
    let symbol = list_all_symbols(language, &source)?
        .into_iter()
        .filter(|(_, start_line, end_line)| *start_line <= line_number && line_number <= *end_line)
        .min_by_key(|(_, start_line, end_line)| {
            (end_line.saturating_sub(*start_line), *start_line, *end_line)
        })
        .map(|(name, _, _)| name);
    Ok(symbol)
}

fn format_hover_markdown(
    display_target: &str,
    cli_target: &str,
    report: &ArchaeologyResult,
) -> String {
    let mut sections = vec![format!(
        "**{display_target}** — Risk: **{}**",
        report.risk_level.as_str()
    )];

    let history = concise_history(report);
    if !history.is_empty() {
        sections.push(history);
    }

    sections.push(report.risk_summary.clone());
    sections.push(format!("Run `why {cli_target}` for full report."));
    sections.join("\n\n")
}

fn concise_history(report: &ArchaeologyResult) -> String {
    let summaries = report
        .commits
        .iter()
        .map(|commit| commit.summary.trim())
        .filter(|summary| !summary.is_empty())
        .fold(Vec::<String>::new(), |mut acc, summary| {
            if !acc.iter().any(|existing| existing == summary) {
                acc.push(summary.to_string());
            }
            acc
        });

    if summaries.is_empty() {
        if report.local_context.risk_flags.is_empty() {
            return String::new();
        }
        return format!(
            "Local risk signals: {}.",
            report.local_context.risk_flags.join(", ")
        );
    }

    let mut text = summaries
        .into_iter()
        .take(2)
        .collect::<Vec<_>>()
        .join(" Then ");
    if !text.ends_with('.') {
        text.push('.');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{concise_history, format_hover_markdown, symbol_name_for_hover};
    use anyhow::Result;
    use std::fs;
    use tempfile::TempDir;
    use why_archaeologist::{
        ArchaeologyResult, CommitEvidence, LocalContext, OutputTarget, RiskLevel,
    };
    use why_locator::QueryKind;

    fn sample_result() -> ArchaeologyResult {
        ArchaeologyResult {
            target: OutputTarget {
                path: "src/auth.rs".into(),
                start_line: 4,
                end_line: 4,
                query_kind: QueryKind::Line,
            },
            commits: vec![
                CommitEvidence {
                    oid: "abcdef1234567890".into(),
                    short_oid: "abcdef12".into(),
                    author: "Fixture Bot".into(),
                    email: "fixture@example.com".into(),
                    time: 0,
                    date: "2026-03-13".into(),
                    summary: "hotfix: harden authenticate after auth bypass incident".into(),
                    message: String::new(),
                    diff_excerpt: String::new(),
                    coverage_score: 1.0,
                    relevance_score: 1.0,
                    issue_refs: vec![],
                    is_mechanical: false,
                },
                CommitEvidence {
                    oid: "fedcba0987654321".into(),
                    short_oid: "fedcba09".into(),
                    author: "Fixture Bot".into(),
                    email: "fixture@example.com".into(),
                    time: 0,
                    date: "2026-03-14".into(),
                    summary: "feat: add legacy mobile token path".into(),
                    message: String::new(),
                    diff_excerpt: String::new(),
                    coverage_score: 1.0,
                    relevance_score: 1.0,
                    issue_refs: vec![],
                    is_mechanical: false,
                },
            ],
            risk_level: RiskLevel::HIGH,
            risk_summary: RiskLevel::HIGH.summary().into(),
            change_guidance: RiskLevel::HIGH.change_guidance().into(),
            local_context: LocalContext {
                comments: vec![],
                markers: vec![],
                risk_flags: vec!["incident marker nearby".into()],
            },
            mode: "heuristic".into(),
            notes: vec![],
        }
    }

    #[test]
    fn symbol_name_for_hover_returns_smallest_enclosing_symbol() -> Result<()> {
        let temp = TempDir::new()?;
        let file_path = temp.path().join("sample.rs");
        fs::write(
            &file_path,
            r#"
fn outer() {
    fn inner() {
        println!("hello");
    }
    inner();
}
"#,
        )?;

        assert_eq!(symbol_name_for_hover(&file_path, 2)?, Some("outer".into()));
        assert_eq!(symbol_name_for_hover(&file_path, 3)?, Some("inner".into()));
        Ok(())
    }

    #[test]
    fn concise_history_falls_back_to_local_risk_flags() {
        let mut result = sample_result();
        result.commits.clear();
        assert_eq!(
            concise_history(&result),
            "Local risk signals: incident marker nearby."
        );
    }

    #[test]
    fn format_hover_markdown_includes_risk_history_and_cli_hint() {
        let markdown = format_hover_markdown("authenticate()", "src/auth.rs:4", &sample_result());

        assert!(markdown.contains("**authenticate()** — Risk: **HIGH**"));
        assert!(markdown.contains("hotfix: harden authenticate after auth bypass incident"));
        assert!(markdown.contains("feat: add legacy mobile token path"));
        assert!(markdown.contains(RiskLevel::HIGH.summary()));
        assert!(markdown.contains("Run `why src/auth.rs:4` for full report."));
    }
}
