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
use why_archaeologist::{discover_repository, relative_repo_path};
use why_context::load_config;
use why_locator::{QueryKind, QueryTarget, detect_language, list_all_symbols};
use why_question_engine::{QuestionAnswer, QuestionRequest, answer_question, format_target_label};

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
    let config = load_config(&repo_root)?;
    let answer = answer_question(
        &QuestionRequest {
            target: target.clone(),
            since_days: None,
            no_llm: true,
            include_github: false,
        },
        &repo_root,
        &repo,
        &config,
    )?;
    let cli_target = format_target_label(&target);
    let display_target = symbol_name
        .as_deref()
        .map(|symbol| format!("{symbol}()"))
        .unwrap_or_else(|| cli_target.clone());

    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format_hover_markdown(&display_target, &cli_target, &answer),
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
    answer: &QuestionAnswer,
) -> String {
    let mut sections = vec![format!(
        "**{display_target}** — Risk: **{}**",
        answer.report.risk_level.as_str()
    )];

    sections.push(answer.report.summary.clone());

    let history = concise_history(answer);
    if !history.is_empty() {
        sections.push(history);
    }

    sections.push(answer.report.risk_summary.clone());
    sections.push(format!("Run `why {cli_target}` for full report."));
    sections.join("\n\n")
}

fn concise_history(answer: &QuestionAnswer) -> String {
    if !answer.report.evidence.is_empty() {
        let mut text = answer
            .report
            .evidence
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(" Then ");
        if !text.ends_with('.') {
            text.push('.');
        }
        return text;
    }

    if answer.archaeology.local_context.risk_flags.is_empty() {
        return String::new();
    }

    format!(
        "Local risk signals: {}.",
        answer.archaeology.local_context.risk_flags.join(", ")
    )
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
    use why_question_engine::QuestionAnswer;
    use why_synthesizer::{ConfidenceLevel, ReportMode, WhyReport};

    fn sample_answer() -> QuestionAnswer {
        let archaeology = ArchaeologyResult {
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
        };
        let report = WhyReport {
            summary: "Heuristic analysis of src/auth.rs:4 based on 2 relevant commit(s).".into(),
            evidence: vec![
                "hotfix: harden authenticate after auth bypass incident (2026-03-13)".into(),
                "feat: add legacy mobile token path (2026-03-14)".into(),
            ],
            inference: Vec::new(),
            unknowns: vec!["No model synthesis was available for this query.".into()],
            risk_level: why_synthesizer::RiskLevel::HIGH,
            risk_summary: why_synthesizer::RiskLevel::HIGH.summary().into(),
            change_guidance: why_synthesizer::RiskLevel::HIGH.change_guidance().into(),
            confidence: ConfidenceLevel::Low,
            mode: ReportMode::Heuristic,
            notes: Vec::new(),
            policy: Vec::new(),
            cost_usd: None,
        };

        QuestionAnswer {
            archaeology,
            evidence_pack: why_evidence::EvidencePack {
                target: why_evidence::TargetInfo {
                    file: "src/auth.rs".into(),
                    symbol: Some("authenticate".into()),
                    lines: (4, 4),
                    language: "rust".into(),
                },
                local_context: why_evidence::LocalContextInfo {
                    comments: Vec::new(),
                    markers: Vec::new(),
                    risk_flags: vec!["incident marker nearby".into()],
                },
                history: why_evidence::HistoryInfo {
                    total_commit_count: 2,
                    commits_shown: 2,
                    top_commits: Vec::new(),
                },
                signals: why_evidence::SignalInfo {
                    issue_refs: Vec::new(),
                    risk_keywords: Vec::new(),
                    heuristic_risk: "HIGH".into(),
                    github_items: Vec::new(),
                    github_notes: Vec::new(),
                },
            },
            report,
            github: why_evidence::GitHubEnrichment::default(),
            target_label: "src/auth.rs:4".into(),
            policy: why_question_engine::QuestionPolicyOutcome {
                evidence_payload_chars: 512,
                evidence_commit_limit: 4,
                llm_input_char_limit: 2048,
                llm_allowed: false,
                reasons: Vec::new(),
            },
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
        let mut answer = sample_answer();
        answer.report.evidence.clear();
        assert_eq!(
            concise_history(&answer),
            "Local risk signals: incident marker nearby."
        );
    }

    #[test]
    fn format_hover_markdown_includes_risk_history_and_cli_hint() {
        let markdown = format_hover_markdown("authenticate()", "src/auth.rs:4", &sample_answer());

        assert!(markdown.contains("**authenticate()** — Risk: **HIGH**"));
        assert!(markdown.contains("hotfix: harden authenticate after auth bypass incident"));
        assert!(markdown.contains("feat: add legacy mobile token path"));
        assert!(
            markdown.contains("Heuristic analysis of src/auth.rs:4 based on 2 relevant commit(s).")
        );
        assert!(markdown.contains(why_synthesizer::RiskLevel::HIGH.summary()));
        assert!(markdown.contains("Run `why src/auth.rs:4` for full report."));
    }
}
