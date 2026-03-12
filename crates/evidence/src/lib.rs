//! Evidence pack construction.

use anyhow::{Context, Result, anyhow, bail};
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

#[derive(Debug, Clone)]
pub struct GitHubClient {
    repo: GitHubRepo,
    auth_value: Option<String>,
    client: Client,
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

        match self.auth_value.as_deref() {
            Some(auth_value) => builder.header(AUTHORIZATION, format!("Bearer {auth_value}")),
            None => builder,
        }
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
        .default_headers(headers)
        .build()
        .context("failed to build GitHub client")
}

pub fn build(
    target: &EvidenceTarget,
    commits: &[EvidenceCommit],
    context: &EvidenceContext,
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
        let pack = build(&sample_target(), &commits, &sample_context());
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
        let pack = build(&sample_target(), &commits, &sample_context());

        assert_eq!(pack.signals.issue_refs, vec!["#42", "#7", "#99"]);
    }

    #[test]
    fn test_total_commit_count_preserved_when_payload_is_reduced() {
        let commits: Vec<_> = (0..20)
            .map(|index| sample_commit(index, 2_000, vec!["#1", "#2", "#3", "#4", "#5", "#6"]))
            .collect();
        let pack = build(&sample_target(), &commits, &sample_context());

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
        let pack = build(&sample_target(), &commits, &context);

        assert!(pack.signals.issue_refs.len() <= MAX_SIGNAL_ISSUE_REFS);
        assert!(pack.signals.risk_keywords.len() <= MAX_SIGNAL_RISK_KEYWORDS);
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
    fn test_github_client_uses_config_and_builds_issue_endpoint() {
        let config = WhyConfig {
            github: GitHubConfig {
                remote: "origin".into(),
                token: Some("test-placeholder".into()),
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
}
