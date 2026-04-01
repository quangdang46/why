use anyhow::{Context, Result, anyhow};
use git2::Repository;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use why_archaeologist::{ArchaeologyResult, CommitEvidence, analyze_target_with_options};
use why_cache::runtime_dir;
use why_context::{LlmProvider, ResolvedLlmConfig, WhyConfig};
use why_evidence::{
    EvidenceCommit, EvidenceContext, EvidencePack, EvidenceTarget, GitHubClient, GitHubEnrichment,
    enrich_github_refs,
};
use why_locator::{QueryKind, QueryTarget};
use why_synthesizer::{
    PolicyNote, WhyReport, build_query_prompt, build_system_prompt, client_from_config,
    heuristic_report, prompt_contract, synthesize_report,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRequest {
    pub target: QueryTarget,
    pub since_days: Option<u64>,
    pub no_llm: bool,
    pub include_github: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionAnswer {
    pub archaeology: ArchaeologyResult,
    pub evidence_pack: EvidencePack,
    pub report: WhyReport,
    pub github: GitHubEnrichment,
    pub target_label: String,
    pub policy: QuestionPolicyOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPolicyOutcome {
    pub evidence_payload_chars: usize,
    pub evidence_commit_limit: usize,
    pub llm_input_char_limit: usize,
    pub llm_allowed: bool,
    pub reasons: Vec<PolicyNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderBudget {
    max_commits: usize,
    max_input_chars: usize,
    min_output_tokens: u32,
}

pub fn answer_question(
    request: &QuestionRequest,
    cwd: &Path,
    repo: &Repository,
    config: &WhyConfig,
) -> Result<QuestionAnswer> {
    let archaeology = analyze_target_with_options(&request.target, cwd, request.since_days)?;
    let resolved_llm = config.resolved_llm_config();
    let provider_budget = provider_budget(&resolved_llm);
    let github = if request.include_github {
        build_github_enrichment(repo, config, &archaeology.commits)
    } else {
        GitHubEnrichment::default()
    };
    let evidence_pack = build_evidence_pack(
        &request.target,
        &archaeology,
        &github,
        provider_budget.max_commits,
    );
    let policy = evaluate_policy(
        request,
        &archaeology,
        &evidence_pack,
        &resolved_llm,
        &provider_budget,
    )?;
    let report = synthesize_question_report(
        request,
        &archaeology,
        &evidence_pack,
        &github,
        repo,
        config,
        &policy,
    )?;

    Ok(QuestionAnswer {
        archaeology,
        evidence_pack,
        report,
        github,
        target_label: format_target_label(&request.target),
        policy,
    })
}

pub fn build_github_enrichment(
    repo: &Repository,
    config: &WhyConfig,
    commits: &[CommitEvidence],
) -> GitHubEnrichment {
    let Some(remote_name) =
        (!config.github.remote.trim().is_empty()).then(|| config.github.remote.trim())
    else {
        return GitHubEnrichment::default();
    };

    let remote_url = match repo.find_remote(remote_name) {
        Ok(remote) => match remote.url() {
            Some(url) if !url.trim().is_empty() => url.to_string(),
            _ => {
                return GitHubEnrichment {
                    items: Vec::new(),
                    notes: vec![format!(
                        "GitHub enrichment skipped because remote '{remote_name}' has no URL"
                    )],
                };
            }
        },
        Err(error) => {
            return GitHubEnrichment {
                items: Vec::new(),
                notes: vec![format!(
                    "GitHub enrichment skipped because remote '{remote_name}' could not be read: {error}"
                )],
            };
        }
    };

    let client = match GitHubClient::from_config(config, &remote_url) {
        Ok(client) => client,
        Err(error) => {
            return GitHubEnrichment {
                items: Vec::new(),
                notes: vec![format!("GitHub enrichment unavailable: {error}")],
            };
        }
    };

    let issue_refs = commits
        .iter()
        .flat_map(|commit| commit.issue_refs.iter().cloned())
        .collect::<Vec<_>>();
    enrich_github_refs(&client, &issue_refs)
}

pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn format_target_label(target: &QueryTarget) -> String {
    match target.query_kind {
        QueryKind::Line => format!(
            "{}:{}",
            normalize_path(&target.path),
            target.start_line.unwrap_or_default()
        ),
        QueryKind::Range => format!(
            "{}:{}-{}",
            normalize_path(&target.path),
            target.start_line.unwrap_or_default(),
            target.end_line.unwrap_or_default()
        ),
        QueryKind::Symbol | QueryKind::QualifiedSymbol => format!(
            "{}:{}",
            normalize_path(&target.path),
            target.symbol.as_deref().unwrap_or("symbol")
        ),
    }
}

fn build_evidence_pack(
    target: &QueryTarget,
    archaeology: &ArchaeologyResult,
    github: &GitHubEnrichment,
    max_commits: usize,
) -> EvidencePack {
    let commit_slice_len = archaeology.commits.len().min(max_commits.max(1));
    why_evidence::build(
        &EvidenceTarget {
            file: normalize_path(&archaeology.target.path),
            symbol: target.symbol.clone(),
            lines: (
                archaeology.target.start_line as usize,
                archaeology.target.end_line as usize,
            ),
            language: infer_language(&archaeology.target.path),
        },
        &archaeology
            .commits
            .iter()
            .take(commit_slice_len)
            .map(|commit| EvidenceCommit {
                oid: commit.oid.clone(),
                date: commit.date.clone(),
                author: commit.author.clone(),
                summary: commit.summary.clone(),
                diff_excerpt: commit.diff_excerpt.clone(),
                coverage_score: commit.coverage_score,
                issue_refs: commit.issue_refs.clone(),
            })
            .collect::<Vec<_>>(),
        &EvidenceContext {
            comments: archaeology.local_context.comments.clone(),
            markers: archaeology.local_context.markers.clone(),
            risk_flags: archaeology.local_context.risk_flags.clone(),
            heuristic_risk: archaeology.risk_level.as_str().to_string(),
        },
        github,
    )
}

fn synthesize_question_report(
    request: &QuestionRequest,
    archaeology: &ArchaeologyResult,
    evidence_pack: &EvidencePack,
    github: &GitHubEnrichment,
    repo: &Repository,
    config: &WhyConfig,
    policy: &QuestionPolicyOutcome,
) -> Result<WhyReport> {
    let fallback = |extra_note: Option<String>| {
        let mut notes = archaeology.notes.clone();
        notes.extend(github.notes.iter().cloned());
        if let Some(extra_note) = extra_note {
            notes.push(extra_note);
        }
        heuristic_report(
            format!(
                "Heuristic analysis of {} based on {} relevant commit(s).",
                format_target_label(&request.target),
                archaeology.commits.len()
            ),
            parse_synth_risk(archaeology.risk_level.as_str()),
            archaeology
                .commits
                .iter()
                .map(|commit| format!("{} ({})", commit.summary, commit.date))
                .collect(),
            notes,
        )
    };

    if request.no_llm {
        let mut report = fallback(None);
        report.policy = policy.reasons.clone();
        return Ok(report);
    }

    let resolved_llm = config.resolved_llm_config();
    if !policy.llm_allowed {
        let mut report = fallback(None);
        report.policy = policy.reasons.clone();
        return Ok(report);
    }

    let fallback_note = "LLM synthesis failed; fell back to heuristic mode. See .why/runtime.log.";
    let client = match client_from_config(&resolved_llm) {
        Ok(client) => client,
        Err(error) => {
            log_llm_fallback(
                repo,
                "query",
                resolved_llm.provider,
                resolved_llm.model.clone(),
                Some(format_target_label(&request.target)),
                &error,
            );
            let mut report = fallback(Some(fallback_note.to_string()));
            report.policy = policy.reasons.clone();
            return Ok(report);
        }
    };

    let system_prompt = build_system_prompt(&prompt_contract());
    let user_prompt = build_query_prompt(evidence_pack);

    match synthesize_report(&*client, &system_prompt, &user_prompt) {
        Ok(mut report) => {
            report.policy = policy.reasons.clone();
            Ok(report)
        }
        Err(error) => {
            log_llm_fallback(
                repo,
                "query",
                resolved_llm.provider,
                resolved_llm.model.clone(),
                Some(format_target_label(&request.target)),
                &error,
            );
            let mut report = fallback(Some(fallback_note.to_string()));
            report.policy = policy.reasons.clone();
            Ok(report)
        }
    }
}

fn evaluate_policy(
    request: &QuestionRequest,
    archaeology: &ArchaeologyResult,
    evidence_pack: &EvidencePack,
    resolved_llm: &ResolvedLlmConfig,
    provider_budget: &ProviderBudget,
) -> Result<QuestionPolicyOutcome> {
    let evidence_payload_chars = serde_json::to_string(evidence_pack)?.len();
    let mut reasons = Vec::new();

    if archaeology.commits.len() > evidence_pack.history.commits_shown {
        reasons.push(PolicyNote {
            code: "evidence-commit-budget".into(),
            kind: "truncation".into(),
            message: format!(
                "Evidence budget kept {} of {} commits for {}.",
                evidence_pack.history.commits_shown,
                archaeology.commits.len(),
                format_target_label(&request.target)
            ),
        });
    }

    let diff_excerpts_elided = archaeology
        .commits
        .iter()
        .take(evidence_pack.history.commits_shown)
        .any(|commit| !commit.diff_excerpt.trim().is_empty())
        && evidence_pack
            .history
            .top_commits
            .iter()
            .all(|commit| commit.diff_excerpt.trim().is_empty());
    if diff_excerpts_elided {
        reasons.push(PolicyNote {
            code: "evidence-diff-elision".into(),
            kind: "truncation".into(),
            message: format!(
                "Diff excerpts were elided for {} to stay within the evidence payload budget.",
                format_target_label(&request.target)
            ),
        });
    }

    let mut llm_allowed = !request.no_llm;
    if !request.no_llm && resolved_llm.max_tokens < provider_budget.min_output_tokens {
        llm_allowed = false;
        reasons.push(PolicyNote {
            code: "llm-output-budget".into(),
            kind: "gate".into(),
            message: format!(
                "LLM synthesis was skipped because provider {} only had {} output tokens configured; policy requires at least {}.",
                resolved_llm.provider,
                resolved_llm.max_tokens,
                provider_budget.min_output_tokens
            ),
        });
    }

    if !request.no_llm && resolved_llm.auth_token.is_none() {
        reasons.push(PolicyNote {
            code: "llm-auth-missing".into(),
            kind: "gate".into(),
            message: format!(
                "LLM synthesis was skipped because provider {} has no resolved auth token.",
                resolved_llm.provider
            ),
        });
    }

    if !request.no_llm && evidence_payload_chars > provider_budget.max_input_chars {
        llm_allowed = false;
        reasons.push(PolicyNote {
            code: "llm-input-budget".into(),
            kind: "gate".into(),
            message: format!(
                "LLM synthesis was skipped because the evidence payload for {} was {} chars, above the {} char budget for provider {}.",
                format_target_label(&request.target),
                evidence_payload_chars,
                provider_budget.max_input_chars,
                resolved_llm.provider
            ),
        });
    }

    Ok(QuestionPolicyOutcome {
        evidence_payload_chars,
        evidence_commit_limit: provider_budget.max_commits,
        llm_input_char_limit: provider_budget.max_input_chars,
        llm_allowed,
        reasons,
    })
}

fn provider_budget(resolved_llm: &ResolvedLlmConfig) -> ProviderBudget {
    match resolved_llm.provider {
        LlmProvider::Anthropic => ProviderBudget {
            max_commits: ((resolved_llm.max_tokens / 100).clamp(4, 16)) as usize,
            max_input_chars: ((resolved_llm.max_tokens as usize) * 18).clamp(1_024, 12_000),
            min_output_tokens: 192,
        },
        LlmProvider::Openai => ProviderBudget {
            max_commits: ((resolved_llm.max_tokens / 125).clamp(4, 12)) as usize,
            max_input_chars: ((resolved_llm.max_tokens as usize) * 14).clamp(1_024, 10_000),
            min_output_tokens: 128,
        },
        LlmProvider::Zai | LlmProvider::Custom => ProviderBudget {
            max_commits: ((resolved_llm.max_tokens / 125).clamp(4, 12)) as usize,
            max_input_chars: ((resolved_llm.max_tokens as usize) * 14).clamp(1_024, 10_000),
            min_output_tokens: 128,
        },
    }
}

fn infer_language(path: &Path) -> String {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "rust",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("py") => "python",
        _ => "unknown",
    }
    .to_string()
}

fn parse_synth_risk(value: &str) -> why_synthesizer::RiskLevel {
    match value {
        "HIGH" => why_synthesizer::RiskLevel::HIGH,
        "MEDIUM" => why_synthesizer::RiskLevel::MEDIUM,
        _ => why_synthesizer::RiskLevel::LOW,
    }
}

fn runtime_log_path(repo: &Repository) -> Result<PathBuf> {
    let repo_root = repo
        .workdir()
        .ok_or_else(|| anyhow!("repository does not have a working directory"))?;
    Ok(runtime_dir(repo_root)?.join("runtime.log"))
}

fn append_runtime_log(repo: &Repository, entry: &RuntimeLogEntry) -> Result<()> {
    let path = runtime_log_path(repo)?;
    let payload = format!("{}\n", serde_json::to_string(entry)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("failed to open runtime log {}", path.display()))?;
        file.write_all(payload.as_bytes())
            .with_context(|| format!("failed to write runtime log {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open runtime log {}", path.display()))?;
        file.write_all(payload.as_bytes())
            .with_context(|| format!("failed to write runtime log {}", path.display()))?;
    }
    Ok(())
}

fn log_llm_fallback(
    repo: &Repository,
    mode: &'static str,
    provider: LlmProvider,
    model: Option<String>,
    target: Option<String>,
    error: &anyhow::Error,
) {
    let entry = RuntimeLogEntry {
        timestamp: current_unix_timestamp(),
        event: "llm_fallback",
        mode,
        provider: provider.to_string(),
        model,
        target,
        error: error.to_string(),
    };

    let _ = append_runtime_log(repo, &entry);
}

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug, Clone, serde::Serialize)]
struct RuntimeLogEntry {
    timestamp: i64,
    event: &'static str,
    mode: &'static str,
    provider: String,
    model: Option<String>,
    target: Option<String>,
    error: String,
}

#[cfg(test)]
mod tests {
    use super::{QuestionRequest, answer_question, format_target_label};
    use anyhow::{Context, Result};
    use git2::Repository;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;
    use why_context::{LlmConfig, LlmProvider, WhyConfig};
    use why_locator::{QueryKind, QueryTarget};
    use why_synthesizer::ReportMode;

    #[test]
    fn answers_queries_with_heuristic_report_and_evidence_pack() -> Result<()> {
        let fixture = setup_repo()?;
        let repo = Repository::discover(fixture.path())?;
        let answer = answer_question(
            &QuestionRequest {
                target: QueryTarget {
                    path: "src/auth.rs".into(),
                    start_line: Some(2),
                    end_line: Some(2),
                    symbol: None,
                    query_kind: QueryKind::Line,
                },
                since_days: None,
                no_llm: true,
                include_github: false,
            },
            fixture.path(),
            &repo,
            &WhyConfig::default(),
        )?;

        assert_eq!(answer.target_label, "src/auth.rs:2");
        assert_eq!(answer.report.mode, ReportMode::Heuristic);
        assert_eq!(answer.evidence_pack.target.file, "src/auth.rs");
        assert!(!answer.report.evidence.is_empty());
        assert!(
            answer
                .report
                .evidence
                .iter()
                .any(|item| item.contains("hotfix: keep legacy auth guard"))
        );
        assert!(answer.github.notes.is_empty());
        assert!(answer.report.policy.is_empty());
        Ok(())
    }

    #[test]
    fn formats_symbol_target_labels() {
        let target = QueryTarget {
            path: "src/auth.rs".into(),
            start_line: Some(1),
            end_line: Some(4),
            symbol: Some("AuthService::login".into()),
            query_kind: QueryKind::QualifiedSymbol,
        };

        assert_eq!(
            format_target_label(&target),
            "src/auth.rs:AuthService::login"
        );
    }

    #[test]
    fn policy_marks_commit_budget_truncation() -> Result<()> {
        let fixture = setup_repo_with_rewrites(6, 32)?;
        let repo = Repository::discover(fixture.path())?;
        let config = WhyConfig {
            llm: LlmConfig {
                provider: LlmProvider::Openai,
                model: Some("gpt-4.1-mini".into()),
                base_url: None,
                auth_token: None,
                retries: 1,
                max_tokens: 500,
                timeout: 30,
            },
            ..WhyConfig::default()
        };

        let answer = answer_question(
            &QuestionRequest {
                target: QueryTarget {
                    path: "src/auth.rs".into(),
                    start_line: Some(1),
                    end_line: Some(8),
                    symbol: None,
                    query_kind: QueryKind::Range,
                },
                since_days: None,
                no_llm: true,
                include_github: false,
            },
            fixture.path(),
            &repo,
            &config,
        )?;

        assert!(
            answer
                .policy
                .reasons
                .iter()
                .any(|note| { note.code == "evidence-commit-budget" && note.kind == "truncation" })
        );
        assert!(
            answer.evidence_pack.history.commits_shown < answer.archaeology.commits.len(),
            "evidence budget should trim commits before packaging"
        );
        assert!(
            answer
                .report
                .policy
                .iter()
                .any(|note| note.code == "evidence-commit-budget"),
            "policy notes should flow into structured report output"
        );
        Ok(())
    }

    #[test]
    fn policy_gates_llm_when_output_budget_is_too_small() -> Result<()> {
        let fixture = setup_repo_with_rewrites(2, 16)?;
        let repo = Repository::discover(fixture.path())?;
        let config = WhyConfig {
            llm: LlmConfig {
                provider: LlmProvider::Openai,
                model: Some("gpt-4.1-mini".into()),
                base_url: None,
                auth_token: Some("test-token".into()),
                retries: 1,
                max_tokens: 64,
                timeout: 30,
            },
            ..WhyConfig::default()
        };

        let answer = answer_question(
            &QuestionRequest {
                target: QueryTarget {
                    path: "src/auth.rs".into(),
                    start_line: Some(2),
                    end_line: Some(2),
                    symbol: None,
                    query_kind: QueryKind::Line,
                },
                since_days: None,
                no_llm: false,
                include_github: false,
            },
            fixture.path(),
            &repo,
            &config,
        )?;

        assert_eq!(answer.report.mode, ReportMode::Heuristic);
        assert!(!answer.policy.llm_allowed);
        assert!(
            answer
                .policy
                .reasons
                .iter()
                .any(|note| { note.code == "llm-output-budget" && note.kind == "gate" })
        );
        assert!(
            answer
                .report
                .policy
                .iter()
                .any(|note| note.code == "llm-output-budget")
        );
        Ok(())
    }

    fn setup_repo() -> Result<TempDir> {
        let dir = TempDir::new()?;
        git(dir.path(), &["init", "-b", "main"])?;
        git(dir.path(), &["config", "user.name", "Fixture Bot"])?;
        git(dir.path(), &["config", "user.email", "fixture@example.com"])?;

        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir)?;
        let file_path = src_dir.join("auth.rs");
        fs::write(
            &file_path,
            "pub fn verify_token() {\n    legacy_auth_guard();\n}\n",
        )?;
        git(dir.path(), &["add", "src/auth.rs"])?;
        git(
            dir.path(),
            &["commit", "-m", "hotfix: keep legacy auth guard"],
        )?;
        Ok(dir)
    }

    fn setup_repo_with_rewrites(commit_count: usize, line_width: usize) -> Result<TempDir> {
        let dir = setup_repo()?;
        let file_path = dir.path().join("src").join("auth.rs");

        for iteration in 0..commit_count {
            let body = format!(
                "pub fn verify_token() {{\n    legacy_auth_guard();\n{}\n}}\n",
                (0..=iteration)
                    .map(|line| format!(
                        "    audit_guard_{}(\"{}\");",
                        line,
                        "x".repeat(line_width + line)
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            fs::write(&file_path, body)?;
            git(dir.path(), &["add", "src/auth.rs"])?;
            let message = format!("chore: reshape auth guard {iteration}");
            git(dir.path(), &["commit", "-m", &message])?;
        }

        Ok(dir)
    }

    fn git(cwd: &Path, args: &[&str]) -> Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
        if output.status.success() {
            return Ok(());
        }

        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}
