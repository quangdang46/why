//! Evidence pack construction.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use why_context::WhyConfig;

const MAX_PAYLOAD_CHARS: usize = 8_000;
const MAX_DIFF_CHARS: usize = 500;
const MAX_COMMENT_CHARS: usize = 200;
const MAX_MARKER_CHARS: usize = 150;
const MAX_SUBJECT_CHARS: usize = 120;
const MAX_SIGNAL_ISSUE_REFS: usize = 20;
const MAX_COMMIT_ISSUE_REFS: usize = 5;
const MAX_SIGNAL_RISK_KEYWORDS: usize = 10;
const MAX_SIGNAL_GITHUB_ITEMS: usize = 5;
const MAX_SIGNAL_GITHUB_NOTES: usize = 10;
const MAX_GITHUB_TITLE_CHARS: usize = 120;
const MAX_GITHUB_BODY_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidencePack {
    pub target: TargetInfo,
    pub local_context: LocalContextInfo,
    pub history: HistoryInfo,
    pub signals: SignalInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInfo {
    pub file: String,
    pub symbol: Option<String>,
    pub lines: (usize, usize),
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalContextInfo {
    pub comments: Vec<String>,
    pub markers: Vec<String>,
    pub risk_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitSummary {
    pub oid: String,
    pub date: String,
    pub author: String,
    pub summary: String,
    pub diff_excerpt: String,
    pub coverage_pct: u32,
    pub issue_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryInfo {
    pub total_commit_count: usize,
    pub commits_shown: usize,
    pub top_commits: Vec<CommitSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalInfo {
    pub issue_refs: Vec<String>,
    pub risk_keywords: Vec<String>,
    pub heuristic_risk: String,
    #[serde(default)]
    pub github_items: Vec<GitHubItem>,
    #[serde(default)]
    pub github_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceTarget {
    pub file: String,
    pub symbol: Option<String>,
    pub lines: (usize, usize),
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceContext {
    pub comments: Vec<String>,
    pub markers: Vec<String>,
    pub risk_flags: Vec<String>,
    pub heuristic_risk: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceCommit {
    pub oid: String,
    pub date: String,
    pub author: String,
    pub summary: String,
    pub diff_excerpt: String,
    pub coverage_score: f32,
    pub issue_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepo {
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRef {
    pub number: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubItem {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub html_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubComment {
    pub id: u64,
    pub html_url: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubFetchOutcome {
    Item(GitHubItem),
    Degraded { note: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitHubEnrichment {
    pub items: Vec<GitHubItem>,
    pub notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubApiErrorEnvelope {
    message: String,
}

#[derive(Debug, Serialize)]
struct GitHubCommentRequest<'a> {
    body: &'a str,
}

#[derive(Clone)]
pub struct GitHubClient {
    repo: GitHubRepo,
    auth_value: Option<String>,
    client: Client,
}

impl core::fmt::Debug for GitHubClient {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GitHubClient")
            .field("repo", &self.repo)
            .field(
                "auth_value",
                &self.auth_value.as_ref().map(|_| "[redacted]"),
            )
            .finish_non_exhaustive()
    }
}

impl GitHubClient {
    pub fn from_config(config: &WhyConfig, remote_url: &str) -> Result<Self> {
        let repo = parse_github_remote(remote_url)?;
        let auth_value = config.github_token();
        let client = build_http_client()?;
        Ok(Self {
            repo,
            auth_value,
            client,
        })
    }

    pub fn repo(&self) -> &GitHubRepo {
        &self.repo
    }

    fn authorize_request(&self, builder: RequestBuilder) -> RequestBuilder {
        match self.auth_value.as_deref() {
            Some(auth_value) => builder.header(AUTHORIZATION, format!("Bearer {auth_value}")),
            None => builder,
        }
    }

    pub fn issue_endpoint(&self, issue: &GitHubRef) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/issues/{}",
            self.repo.owner, self.repo.name, issue.number
        )
    }

    pub fn request_issue(&self, issue: &GitHubRef) -> RequestBuilder {
        let builder = self
            .client
            .get(self.issue_endpoint(issue))
            .header(ACCEPT, "application/vnd.github+json");

        self.authorize_request(builder)
    }

    pub fn issue_comment_endpoint(&self, issue: &GitHubRef) -> String {
        format!("{}/comments", self.issue_endpoint(issue))
    }

    pub fn request_issue_comment(&self, issue: &GitHubRef, body: &str) -> RequestBuilder {
        let builder = self
            .client
            .post(self.issue_comment_endpoint(issue))
            .header(ACCEPT, "application/vnd.github+json")
            .json(&GitHubCommentRequest { body });

        self.authorize_request(builder)
    }

    pub fn fetch_issue(&self, issue: &GitHubRef) -> Result<GitHubFetchOutcome> {
        self.fetch_issue_from_response(self.request_issue(issue).send(), issue)
    }

    pub fn post_issue_comment(&self, issue: &GitHubRef, body: &str) -> Result<GitHubComment> {
        self.post_issue_comment_from_response(self.request_issue_comment(issue, body).send(), issue)
    }

    fn fetch_issue_from_response(
        &self,
        response: reqwest::Result<reqwest::blocking::Response>,
        issue: &GitHubRef,
    ) -> Result<GitHubFetchOutcome> {
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return Ok(GitHubFetchOutcome::Degraded {
                    note: format!(
                        "GitHub issue #{} enrichment unavailable: {}",
                        issue.number, error
                    ),
                });
            }
        };

        let status = response.status();
        let body = response
            .text()
            .with_context(|| format!("failed to read GitHub issue #{} response", issue.number))?;

        parse_github_issue_response(issue.number, status, &body)
    }

    fn post_issue_comment_from_response(
        &self,
        response: reqwest::Result<reqwest::blocking::Response>,
        issue: &GitHubRef,
    ) -> Result<GitHubComment> {
        let response = response
            .with_context(|| format!("failed to post GitHub comment for issue #{}", issue.number))?;

        let status = response.status();
        let body = response.text().with_context(|| {
            format!(
                "failed to read GitHub comment response for issue #{}",
                issue.number
            )
        })?;

        parse_github_comment_response(issue.number, status, &body)
    }
}

pub fn parse_github_remote(remote_url: &str) -> Result<GitHubRepo> {
    let trimmed = remote_url.trim();
    let rest = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.strip_prefix("https://github.com/"))
        .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))
        .ok_or_else(|| anyhow!("unsupported GitHub remote: {trimmed}"))?;

    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let owner = parts.next().unwrap_or_default().trim();
    let name = parts.next().unwrap_or_default().trim();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        bail!("unsupported GitHub remote: {trimmed}");
    }

    Ok(GitHubRepo {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

fn build_http_client() -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("why-cli/0.1"));
    Client::builder()
        .https_only(true)
        .default_headers(headers)
        .build()
        .context("failed to build GitHub client")
}

fn parse_github_issue_response(
    issue_number: u64,
    status: StatusCode,
    body: &str,
) -> Result<GitHubFetchOutcome> {
    if status.is_success() {
        let item: GitHubItem = serde_json::from_str(body)
            .with_context(|| format!("failed to parse GitHub issue #{} response", issue_number))?;
        return Ok(GitHubFetchOutcome::Item(item));
    }

    Ok(GitHubFetchOutcome::Degraded {
        note: format_github_degradation_note(issue_number, status, body),
    })
}

fn parse_github_comment_response(
    issue_number: u64,
    status: StatusCode,
    body: &str,
) -> Result<GitHubComment> {
    if status.is_success() {
        return serde_json::from_str(body).with_context(|| {
            format!(
                "failed to parse GitHub comment response for issue #{}",
                issue_number
            )
        });
    }

    bail!(
        "failed to post GitHub comment for issue #{}: {}",
        issue_number,
        format_github_degradation_note(issue_number, status, body)
    )
}

fn format_github_degradation_note(issue_number: u64, status: StatusCode, body: &str) -> String {
    let message = parse_github_api_error_message(body)
        .unwrap_or_else(|| format!("GitHub returned HTTP {}", status.as_u16()));

    if status == StatusCode::TOO_MANY_REQUESTS {
        return format!(
            "GitHub issue #{} enrichment skipped due to rate limiting (HTTP 429): {}",
            issue_number, message
        );
    }

    if status == StatusCode::FORBIDDEN {
        let lower = message.to_ascii_lowercase();
        if lower.contains("rate limit") {
            return format!(
                "GitHub issue #{} enrichment skipped due to rate limiting (HTTP 403): {}",
                issue_number, message
            );
        }
        return format!(
            "GitHub issue #{} enrichment skipped because access was denied (HTTP 403): {}",
            issue_number, message
        );
    }

    if status == StatusCode::UNAUTHORIZED {
        return format!(
            "GitHub issue #{} enrichment skipped because authentication failed (HTTP 401): {}",
            issue_number, message
        );
    }

    if status.is_server_error() {
        return format!(
            "GitHub issue #{} enrichment temporarily unavailable (HTTP {}): {}",
            issue_number,
            status.as_u16(),
            message
        );
    }

    format!(
        "GitHub issue #{} enrichment skipped (HTTP {}): {}",
        issue_number,
        status.as_u16(),
        message
    )
}

fn parse_github_api_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<GitHubApiErrorEnvelope>(body)
        .ok()
        .map(|envelope| envelope.message.trim().to_string())
        .filter(|message| !message.is_empty())
}

pub fn build(
    target: &EvidenceTarget,
    commits: &[EvidenceCommit],
    context: &EvidenceContext,
    github: &GitHubEnrichment,
) -> EvidencePack {
    let total_commit_count = commits.len();
    let all_issue_refs = dedupe_issue_refs(commits);
    let full = build_internal(
        target,
        commits,
        context,
        &all_issue_refs,
        total_commit_count,
        true,
        github,
    );

    if serialized_len(&full) <= MAX_PAYLOAD_CHARS {
        return full;
    }

    let mut reduced = build_internal(
        target,
        commits,
        context,
        &all_issue_refs,
        total_commit_count,
        false,
        github,
    );
    if serialized_len(&reduced) <= MAX_PAYLOAD_CHARS {
        return reduced;
    }

    let mut slice_len = commits.len().max(1);
    while slice_len > 1 {
        slice_len = (slice_len / 2).max(1);
        reduced = build_internal(
            target,
            &commits[..slice_len],
            context,
            &all_issue_refs,
            total_commit_count,
            false,
            github,
        );
        if serialized_len(&reduced) <= MAX_PAYLOAD_CHARS {
            return reduced;
        }
    }

    reduced
}

fn build_internal(
    target: &EvidenceTarget,
    commits: &[EvidenceCommit],
    context: &EvidenceContext,
    all_issue_refs: &[String],
    total_commit_count: usize,
    include_diffs: bool,
    github: &GitHubEnrichment,
) -> EvidencePack {
    let top_commits: Vec<CommitSummary> = commits
        .iter()
        .map(|commit| CommitSummary {
            oid: truncate(&commit.oid, 8),
            date: commit.date.clone(),
            author: commit.author.clone(),
            summary: truncate(&commit.summary, MAX_SUBJECT_CHARS),
            diff_excerpt: if include_diffs {
                truncate(&commit.diff_excerpt, MAX_DIFF_CHARS)
            } else {
                String::new()
            },
            coverage_pct: (commit.coverage_score * 100.0).round() as u32,
            issue_refs: commit
                .issue_refs
                .iter()
                .take(MAX_COMMIT_ISSUE_REFS)
                .cloned()
                .collect(),
        })
        .collect();

    EvidencePack {
        target: TargetInfo {
            file: target.file.clone(),
            symbol: target.symbol.clone(),
            lines: target.lines,
            language: target.language.clone(),
        },
        local_context: LocalContextInfo {
            comments: context
                .comments
                .iter()
                .take(5)
                .map(|comment| truncate(comment, MAX_COMMENT_CHARS))
                .collect(),
            markers: context
                .markers
                .iter()
                .take(5)
                .map(|marker| truncate(marker, MAX_MARKER_CHARS))
                .collect(),
            risk_flags: context.risk_flags.iter().take(10).cloned().collect(),
        },
        history: HistoryInfo {
            total_commit_count,
            commits_shown: top_commits.len(),
            top_commits,
        },
        signals: SignalInfo {
            issue_refs: all_issue_refs
                .iter()
                .take(MAX_SIGNAL_ISSUE_REFS)
                .cloned()
                .collect(),
            risk_keywords: context
                .risk_flags
                .iter()
                .take(MAX_SIGNAL_RISK_KEYWORDS)
                .cloned()
                .collect(),
            heuristic_risk: context.heuristic_risk.clone(),
            github_items: github
                .items
                .iter()
                .take(MAX_SIGNAL_GITHUB_ITEMS)
                .map(|item| GitHubItem {
                    number: item.number,
                    title: truncate(&item.title, MAX_GITHUB_TITLE_CHARS),
                    body: truncate(&item.body, MAX_GITHUB_BODY_CHARS),
                    html_url: item.html_url.clone(),
                })
                .collect(),
            github_notes: github
                .notes
                .iter()
                .take(MAX_SIGNAL_GITHUB_NOTES)
                .cloned()
                .collect(),
        },
    }
}

fn dedupe_issue_refs(commits: &[EvidenceCommit]) -> Vec<String> {
    let mut refs: Vec<String> = commits
        .iter()
        .flat_map(|commit| commit.issue_refs.iter().cloned())
        .collect();
    refs.sort();
    refs.dedup();
    refs
}

pub fn parse_github_ref(value: &str) -> Option<GitHubRef> {
    let number = value.trim().strip_prefix('#')?.parse().ok()?;
    Some(GitHubRef { number })
}

fn dedupe_github_refs(issue_refs: &[String]) -> Vec<GitHubRef> {
    let mut refs = issue_refs
        .iter()
        .filter_map(|issue_ref| parse_github_ref(issue_ref))
        .collect::<Vec<_>>();
    refs.sort_by_key(|issue| issue.number);
    refs.dedup_by_key(|issue| issue.number);
    refs
}

fn is_terminal_github_degradation(note: &str) -> bool {
    note.contains("rate limiting")
        || note.contains("authentication failed")
        || note.contains("access was denied")
}

pub fn enrich_github_refs(client: &GitHubClient, issue_refs: &[String]) -> GitHubEnrichment {
    let mut enrichment = GitHubEnrichment::default();

    for issue in dedupe_github_refs(issue_refs) {
        match client.fetch_issue(&issue) {
            Ok(GitHubFetchOutcome::Item(item)) => enrichment.items.push(item),
            Ok(GitHubFetchOutcome::Degraded { note }) => {
                let should_stop = is_terminal_github_degradation(&note);
                enrichment.notes.push(note);
                if should_stop {
                    break;
                }
            }
            Err(error) => enrichment.notes.push(format!(
                "GitHub issue #{} enrichment unavailable: {}",
                issue.number, error
            )),
        }
    }

    enrichment
}

fn serialized_len(pack: &EvidencePack) -> usize {
    serde_json::to_string(pack)
        .map(|json| json.len())
        .unwrap_or(usize::MAX)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }

    let truncated: String = text.chars().take(max).collect();
    if let Some(last_space) = truncated.rfind(' ') {
        format!("{}…", &truncated[..last_space])
    } else {
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use why_context::{GitHubConfig, WhyConfig};

    fn sample_target() -> EvidenceTarget {
        EvidenceTarget {
            file: "src/auth.rs".into(),
            symbol: Some("authenticate".into()),
            lines: (10, 22),
            language: "rust".into(),
        }
    }

    fn sample_context() -> EvidenceContext {
        EvidenceContext {
            comments: vec!["security-sensitive auth flow".into()],
            markers: vec!["TODO: remove after mobile rollout".into()],
            risk_flags: vec!["auth".into(), "token".into()],
            heuristic_risk: "HIGH".into(),
        }
    }

    fn sample_commit(index: usize, diff_len: usize, issue_refs: Vec<&str>) -> EvidenceCommit {
        EvidenceCommit {
            oid: format!("abcdef12{index}"),
            date: "2026-03-11".into(),
            author: "alice".into(),
            summary: format!("hotfix commit {index} for authentication path"),
            diff_excerpt: "x".repeat(diff_len),
            coverage_score: 0.75,
            issue_refs: issue_refs.into_iter().map(str::to_string).collect(),
        }
    }

    #[test]
    fn test_payload_within_budget() {
        let commits: Vec<_> = (0..20)
            .map(|index| sample_commit(index, 2_000, vec!["#1", "#2"]))
            .collect();
        let pack = build(
            &sample_target(),
            &commits,
            &sample_context(),
            &GitHubEnrichment::default(),
        );
        let json = serde_json::to_string(&pack).expect("pack should serialize");

        assert!(json.len() <= MAX_PAYLOAD_CHARS);
        assert!(pack.history.commits_shown <= commits.len());
    }

    #[test]
    fn test_diff_excerpt_truncated() {
        let pack = build(
            &sample_target(),
            &[sample_commit(1, 900, vec!["#42"])],
            &sample_context(),
            &GitHubEnrichment::default(),
        );
        let diff = &pack.history.top_commits[0].diff_excerpt;

        assert!(diff.chars().count() <= MAX_DIFF_CHARS + 1);
        assert!(diff.ends_with('…'));
    }

    #[test]
    fn test_issue_refs_deduplicated() {
        let commits = vec![
            sample_commit(1, 20, vec!["#42", "#7"]),
            sample_commit(2, 20, vec!["#7", "#99"]),
        ];
        let pack = build(
            &sample_target(),
            &commits,
            &sample_context(),
            &GitHubEnrichment::default(),
        );

        assert_eq!(pack.signals.issue_refs, vec!["#42", "#7", "#99"]);
    }

    #[test]
    fn test_total_commit_count_preserved_when_payload_is_reduced() {
        let commits: Vec<_> = (0..20)
            .map(|index| sample_commit(index, 2_000, vec!["#1", "#2", "#3", "#4", "#5", "#6"]))
            .collect();
        let pack = build(
            &sample_target(),
            &commits,
            &sample_context(),
            &GitHubEnrichment::default(),
        );

        assert_eq!(pack.history.total_commit_count, commits.len());
        assert!(pack.history.commits_shown <= pack.history.total_commit_count);
    }

    #[test]
    fn test_signal_lists_are_bounded() {
        let commits: Vec<_> = (0..30)
            .map(|index| {
                sample_commit(
                    index,
                    100,
                    vec![
                        "#1", "#2", "#3", "#4", "#5", "#6", "#7", "#8", "#9", "#10", "#11", "#12",
                        "#13", "#14", "#15", "#16", "#17", "#18", "#19", "#20", "#21", "#22",
                        "#23", "#24", "#25",
                    ],
                )
            })
            .collect();
        let context = EvidenceContext {
            comments: vec![],
            markers: vec![],
            risk_flags: (0..20).map(|index| format!("flag-{index}")).collect(),
            heuristic_risk: "MEDIUM".into(),
        };
        let github = GitHubEnrichment {
            items: (0..10)
                .map(|index| GitHubItem {
                    number: index,
                    title: format!("Title {index} {}", "x".repeat(200)),
                    body: "body ".repeat(200),
                    html_url: format!("https://github.com/acme/repo/issues/{index}"),
                })
                .collect(),
            notes: (0..20).map(|index| format!("note-{index}")).collect(),
        };
        let pack = build(&sample_target(), &commits, &context, &github);

        assert!(pack.signals.issue_refs.len() <= MAX_SIGNAL_ISSUE_REFS);
        assert!(pack.signals.risk_keywords.len() <= MAX_SIGNAL_RISK_KEYWORDS);
        assert!(pack.signals.github_items.len() <= MAX_SIGNAL_GITHUB_ITEMS);
        assert!(pack.signals.github_notes.len() <= MAX_SIGNAL_GITHUB_NOTES);
        assert!(
            pack.signals
                .github_items
                .iter()
                .all(|item| item.title.chars().count() <= MAX_GITHUB_TITLE_CHARS + 1)
        );
        assert!(
            pack.signals
                .github_items
                .iter()
                .all(|item| item.body.chars().count() <= MAX_GITHUB_BODY_CHARS + 1)
        );
        assert!(
            pack.history
                .top_commits
                .iter()
                .all(|commit| commit.issue_refs.len() <= MAX_COMMIT_ISSUE_REFS)
        );
    }

    #[test]
    fn test_truncate_no_op_when_short() {
        assert_eq!(truncate("short", 100), "short");
    }

    #[test]
    fn test_parse_github_remote_https() {
        let repo = parse_github_remote("https://github.com/anthropics/why.git")
            .expect("https remote should parse");

        assert_eq!(repo.owner, "anthropics");
        assert_eq!(repo.name, "why");
    }

    #[test]
    fn test_parse_github_remote_ssh() {
        let repo = parse_github_remote("git@github.com:anthropics/why.git")
            .expect("ssh remote should parse");

        assert_eq!(repo.owner, "anthropics");
        assert_eq!(repo.name, "why");
    }

    #[test]
    fn test_parse_github_remote_rejects_non_github_host() {
        let error = parse_github_remote("https://gitlab.com/anthropics/why.git")
            .expect_err("non-GitHub remote should fail");

        assert!(error.to_string().contains("unsupported GitHub remote"));
    }

    #[test]
    fn test_github_client_enforces_https_only_transport() {
        let client = build_http_client().expect("client should build");
        let error = client
            .get("http://api.github.com/repos/anthropics/why/issues/42")
            .send()
            .expect_err("http requests should be rejected");

        assert!(error.to_string().contains("HTTPS") || error.to_string().contains("http://"));
    }

    #[test]
    fn test_github_client_uses_config_and_builds_issue_endpoint() {
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: Some("github_test_token".into()),
            },
            ..WhyConfig::default()
        };

        let client = GitHubClient::from_config(&config, "https://github.com/anthropics/why.git")
            .expect("client should build from config");
        let endpoint = client.issue_endpoint(&GitHubRef { number: 42 });

        assert_eq!(client.repo().owner, "anthropics");
        assert_eq!(client.repo().name, "why");
        assert_eq!(
            endpoint,
            "https://api.github.com/repos/anthropics/why/issues/42"
        );
    }

    #[test]
    fn test_github_client_request_issue_sets_bearer_auth_when_token_present() {
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: Some("github_test_token".into()),
            },
            ..WhyConfig::default()
        };

        let client = GitHubClient::from_config(&config, "https://github.com/anthropics/why.git")
            .expect("client should build from config");
        let request = client
            .request_issue(&GitHubRef { number: 42 })
            .build()
            .expect("request should build");

        let auth = request
            .headers()
            .get(AUTHORIZATION)
            .expect("auth header should be present")
            .to_str()
            .expect("auth header should be valid utf-8");
        assert_eq!(auth, "Bearer github_test_token");
    }

    #[test]
    fn test_github_client_request_issue_omits_auth_when_token_absent() {
        let config = WhyConfig::default();

        let client = GitHubClient::from_config(&config, "https://github.com/anthropics/why.git")
            .expect("client should build from config");
        let request = client
            .request_issue(&GitHubRef { number: 42 })
            .build()
            .expect("request should build");

        assert!(request.headers().get(AUTHORIZATION).is_none());
    }

    #[test]
    fn test_github_client_builds_issue_comment_endpoint() {
        let config = WhyConfig::default();
        let client = GitHubClient::from_config(&config, "https://github.com/anthropics/why.git")
            .expect("client should build from config");

        assert_eq!(
            client.issue_comment_endpoint(&GitHubRef { number: 42 }),
            "https://api.github.com/repos/anthropics/why/issues/42/comments"
        );
    }

    #[test]
    fn test_github_client_request_issue_comment_sets_auth_and_json_body() {
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: Some("github_test_token".into()),
            },
            ..WhyConfig::default()
        };
        let client = GitHubClient::from_config(&config, "https://github.com/anthropics/why.git")
            .expect("client should build from config");
        let request = client
            .request_issue_comment(&GitHubRef { number: 42 }, "hello from why")
            .build()
            .expect("request should build");

        assert_eq!(request.method().as_str(), "POST");
        assert_eq!(
            request.url().as_str(),
            "https://api.github.com/repos/anthropics/why/issues/42/comments"
        );
        let auth = request
            .headers()
            .get(AUTHORIZATION)
            .expect("auth header should be present")
            .to_str()
            .expect("auth header should be valid utf-8");
        assert_eq!(auth, "Bearer github_test_token");
        let body = request.body().expect("request should have body");
        let bytes = body.as_bytes().expect("body should be buffered bytes");
        let payload: serde_json::Value =
            serde_json::from_slice(bytes).expect("comment payload should be valid JSON");
        assert_eq!(payload["body"], "hello from why");
    }

    #[test]
    fn test_github_client_debug_redacts_auth_token() {
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: Some("github_debug_token".into()),
            },
            ..WhyConfig::default()
        };

        let client = GitHubClient::from_config(&config, "https://github.com/anthropics/why.git")
            .expect("client should build from config");
        let debug = format!("{:?}", client);

        assert!(debug.contains("anthropics"));
        assert!(debug.contains("why"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("github_debug_token"));
    }

    #[test]
    fn test_parse_github_issue_response_parses_successful_responses() {
        let outcome = parse_github_issue_response(
            42,
            StatusCode::OK,
            r#"{"number":42,"title":"Fix auth","body":"Context","html_url":"https://github.com/anthropics/why/issues/42"}"#,
        )
        .expect("response should parse");

        assert_eq!(
            outcome,
            GitHubFetchOutcome::Item(GitHubItem {
                number: 42,
                title: "Fix auth".into(),
                body: "Context".into(),
                html_url: "https://github.com/anthropics/why/issues/42".into(),
            })
        );
    }

    #[test]
    fn test_format_github_degradation_note_mentions_rate_limit() {
        let note = format_github_degradation_note(
            42,
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"message":"API rate limit exceeded"}"#,
        );

        assert!(note.contains("issue #42"));
        assert!(note.contains("rate limiting"));
        assert!(note.contains("HTTP 429"));
    }

    #[test]
    fn test_format_github_degradation_note_distinguishes_auth_failures() {
        let note = format_github_degradation_note(
            7,
            StatusCode::UNAUTHORIZED,
            r#"{"message":"Bad credentials"}"#,
        );

        assert!(note.contains("issue #7"));
        assert!(note.contains("authentication failed"));
        assert!(note.contains("HTTP 401"));
    }

    #[test]
    fn test_parse_github_comment_response_parses_successful_responses() {
        let comment = parse_github_comment_response(
            42,
            StatusCode::CREATED,
            r#"{"id":99,"html_url":"https://github.com/anthropics/why/issues/42#issuecomment-99","body":"Looks good"}"#,
        )
        .expect("comment response should parse");

        assert_eq!(
            comment,
            GitHubComment {
                id: 99,
                html_url: "https://github.com/anthropics/why/issues/42#issuecomment-99".into(),
                body: "Looks good".into(),
            }
        );
    }

    #[test]
    fn test_parse_github_comment_response_surfaces_http_errors() {
        let error = parse_github_comment_response(
            42,
            StatusCode::FORBIDDEN,
            r#"{"message":"Resource not accessible by integration"}"#,
        )
        .expect_err("comment response should fail");

        assert!(
            error
                .to_string()
                .contains("failed to post GitHub comment for issue #42")
        );
        assert!(error.to_string().contains("access was denied"));
    }

    #[test]
    fn test_parse_github_api_error_message_reads_message_field() {
        let message = parse_github_api_error_message(r#"{"message":"secondary rate limit"}"#);
        assert_eq!(message.as_deref(), Some("secondary rate limit"));
    }

    #[test]
    fn test_terminal_github_degradation_detection_matches_rate_limit_and_auth() {
        assert!(is_terminal_github_degradation(
            "GitHub issue #42 enrichment skipped due to rate limiting (HTTP 429): API rate limit exceeded"
        ));
        assert!(is_terminal_github_degradation(
            "GitHub issue #7 enrichment skipped because authentication failed (HTTP 401): Bad credentials"
        ));
        assert!(is_terminal_github_degradation(
            "GitHub issue #9 enrichment skipped because access was denied (HTTP 403): Resource not accessible"
        ));
        assert!(!is_terminal_github_degradation(
            "GitHub issue #5 enrichment temporarily unavailable (HTTP 500): server error"
        ));
    }

    #[test]
    fn test_parse_github_ref_accepts_hash_prefixed_numbers() {
        assert_eq!(parse_github_ref("#42"), Some(GitHubRef { number: 42 }));
        assert_eq!(parse_github_ref("42"), None);
        assert_eq!(parse_github_ref("#abc"), None);
    }

    #[test]
    fn test_dedupe_github_refs_normalizes_repeated_issue_numbers() {
        let refs = dedupe_github_refs(&[
            "#42".to_string(),
            "#7".to_string(),
            "#42".to_string(),
            "garbage".to_string(),
            "#7".to_string(),
        ]);

        assert_eq!(
            refs,
            vec![GitHubRef { number: 7 }, GitHubRef { number: 42 }]
        );
    }
}
