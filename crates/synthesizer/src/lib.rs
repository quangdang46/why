//! WhyReport synthesis and fallback behavior.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::env;
use std::thread;
use std::time::Duration;

const HAIKU_MODEL: &str = "claude-haiku-4-5";
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_TOKENS: u32 = 500;
const MAX_RETRIES: usize = 3;
const HAIKU_INPUT_COST_PER_MILLION: f64 = 0.25;
const HAIKU_OUTPUT_COST_PER_MILLION: f64 = 1.25;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum RiskLevel {
    HIGH,
    MEDIUM,
    LOW,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HIGH => "HIGH",
            Self::MEDIUM => "MEDIUM",
            Self::LOW => "LOW",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::HIGH => {
                "The history suggests security sensitivity, incident context, or non-routine compatibility risk."
            }
            Self::MEDIUM => {
                "The history suggests migration, retry, legacy, or transitional behavior that needs context before changes."
            }
            Self::LOW => {
                "The available history does not show unusual operational or compatibility pressure."
            }
        }
    }

    pub fn change_guidance(self) -> &'static str {
        match self {
            Self::HIGH => "Stop and investigate before deleting or heavily refactoring this target.",
            Self::MEDIUM => {
                "Change only after reviewing surrounding code and validating the behavior you might disturb."
            }
            Self::LOW => "Treat this as ordinary code unless stronger evidence appears elsewhere.",
        }
    }
}

impl core::str::FromStr for RiskLevel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "high" => Ok(Self::HIGH),
            "medium" => Ok(Self::MEDIUM),
            "low" => Ok(Self::LOW),
            other => bail!("unsupported risk level: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfidenceLevel {
    Low,
    Medium,
    MediumHigh,
    High,
}

impl ConfidenceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::MediumHigh => "medium-high",
            Self::High => "high",
        }
    }
}

impl core::str::FromStr for ConfidenceLevel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "medium-high" | "medium_high" | "medium high" => Ok(Self::MediumHigh),
            "high" => Ok(Self::High),
            other => bail!("unsupported confidence level: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReportMode {
    Heuristic,
    Synthesized,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WhyReport {
    pub summary: String,
    pub evidence: Vec<String>,
    pub inference: Vec<String>,
    pub unknowns: Vec<String>,
    pub risk_level: RiskLevel,
    pub risk_summary: String,
    pub change_guidance: String,
    pub confidence: ConfidenceLevel,
    pub mode: ReportMode,
    pub notes: Vec<String>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContract {
    pub response_format: &'static str,
    pub required_fields: &'static [&'static str],
    pub grounding_rules: &'static [&'static str],
}

#[derive(Debug, Deserialize)]
struct RawWhyReport {
    summary: String,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    inference: Vec<String>,
    #[serde(default)]
    unknowns: Vec<String>,
    risk_level: String,
    confidence: String,
    #[serde(default)]
    notes: Vec<String>,
}

pub fn prompt_contract() -> PromptContract {
    PromptContract {
        response_format: "Return a single JSON object with no prose before or after it.",
        required_fields: &[
            "summary",
            "evidence",
            "inference",
            "unknowns",
            "risk_level",
            "confidence",
        ],
        grounding_rules: &[
            "Use evidence for direct historical facts only.",
            "Use inference for conclusions drawn from the evidence.",
            "List unknowns explicitly when history is sparse or ambiguous.",
            "Do not invent incidents, PR details, or dependencies not present in the evidence pack.",
        ],
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub timeout_secs: u64,
}

impl core::fmt::Debug for AnthropicConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AnthropicConfig")
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl AnthropicConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("ANTHROPIC_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("ANTHROPIC_API_KEY is not set; rerun with --no-llm or configure credentials"))?;

        Ok(Self {
            api_key,
            model: HAIKU_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicRequest {
    pub system_prompt: String,
    pub user_prompt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicResponse {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MessageRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<MessageContent<'a>>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct MessageContent<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct MessageResponse {
    content: Vec<ResponseBlock>,
    #[serde(default)]
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct ResponseBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    error: ApiError,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone)]
pub struct AnthropicClient {
    config: AnthropicConfig,
    http: Client,
    endpoint: String,
}

#[derive(Debug)]
struct HttpStatusError {
    status: StatusCode,
    message: String,
}

impl core::fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{} (HTTP {})", self.message, self.status.as_u16())
    }
}

impl std::error::Error for HttpStatusError {}

impl AnthropicClient {
    pub fn from_env() -> Result<Self> {
        Self::new(AnthropicConfig::from_env()?)
    }

    pub fn new(config: AnthropicConfig) -> Result<Self> {
        let http = build_http_client(config.timeout_secs)?;
        Ok(Self {
            config,
            http,
            endpoint: ANTHROPIC_API_URL.to_string(),
        })
    }

    fn request_body<'a>(&'a self, request: &'a AnthropicRequest) -> MessageRequest<'a> {
        MessageRequest {
            model: &self.config.model,
            max_tokens: self.config.max_tokens,
            system: &request.system_prompt,
            messages: vec![MessageContent {
                role: "user",
                content: &request.user_prompt,
            }],
        }
    }

    pub fn request_builder(&self, request: &AnthropicRequest) -> RequestBuilder {
        self.http
            .post(&self.endpoint)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(CONTENT_TYPE, "application/json")
            .json(&self.request_body(request))
    }

    pub fn send(&self, request: &AnthropicRequest) -> Result<AnthropicResponse> {
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            match self.try_send(request) {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if attempt + 1 == MAX_RETRIES || !is_retryable_error(&error) {
                        return Err(error);
                    }
                    last_error = Some(error);
                    thread::sleep(retry_delay(attempt + 1));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Anthropic request failed without an error")))
    }

    fn try_send(&self, request: &AnthropicRequest) -> Result<AnthropicResponse> {
        let response = self
            .request_builder(request)
            .send()
            .context("failed to send Anthropic request")?;
        let status = response.status();
        let body = response
            .text()
            .context("failed to read Anthropic response body")?;

        if !status.is_success() {
            let message = parse_api_error_message(&body)
                .unwrap_or_else(|| format!("Anthropic API returned HTTP {}", status.as_u16()));
            return Err(HttpStatusError { status, message }.into());
        }

        parse_message_response(&body)
    }
}

pub fn parse_response(raw: &str) -> Result<WhyReport> {
    let cleaned = strip_markdown_fences(raw);
    let parsed: RawWhyReport = serde_json::from_str(&cleaned)
        .map_err(|error| anyhow!("failed to parse WhyReport JSON: {error}"))?;
    let risk_level: RiskLevel = parsed.risk_level.parse()?;
    let confidence: ConfidenceLevel = parsed.confidence.parse()?;

    Ok(WhyReport {
        summary: parsed.summary,
        evidence: parsed.evidence,
        inference: parsed.inference,
        unknowns: parsed.unknowns,
        risk_level,
        risk_summary: risk_level.summary().to_string(),
        change_guidance: risk_level.change_guidance().to_string(),
        confidence,
        mode: ReportMode::Synthesized,
        notes: parsed.notes,
        cost_usd: None,
    })
}

pub fn heuristic_report(
    summary: impl Into<String>,
    risk_level: RiskLevel,
    evidence: Vec<String>,
    notes: Vec<String>,
) -> WhyReport {
    WhyReport {
        summary: summary.into(),
        evidence,
        inference: Vec::new(),
        unknowns: vec!["No model synthesis was available for this query.".to_string()],
        risk_level,
        risk_summary: risk_level.summary().to_string(),
        change_guidance: risk_level.change_guidance().to_string(),
        confidence: ConfidenceLevel::Low,
        mode: ReportMode::Heuristic,
        notes,
        cost_usd: None,
    }
}

pub fn estimate_cost_usd(input_tokens: u64, output_tokens: u64) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * HAIKU_INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * HAIKU_OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

fn build_http_client(timeout_secs: u64) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static(ANTHROPIC_VERSION));
    Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .context("failed to build Anthropic client")
}

fn parse_message_response(body: &str) -> Result<AnthropicResponse> {
    let response: MessageResponse = serde_json::from_str(body)
        .map_err(|error| anyhow!("failed to parse Anthropic response JSON: {error}"))?;

    let text = response
        .content
        .into_iter()
        .find(|block| block.kind == "text")
        .map(|block| block.text)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("Anthropic response did not contain a text content block"))?;

    let input_tokens = response.usage.input_tokens;
    let output_tokens = response.usage.output_tokens;
    Ok(AnthropicResponse {
        text,
        input_tokens,
        output_tokens,
        cost_usd: estimate_cost_usd(input_tokens, output_tokens),
    })
}

fn parse_api_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<ApiErrorEnvelope>(body)
        .ok()
        .map(|envelope| envelope.error.message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn retry_delay(attempt_index: usize) -> Duration {
    match attempt_index {
        1 => Duration::from_secs(2),
        _ => Duration::from_secs(4),
    }
}

fn is_retryable_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<HttpStatusError>()
        .map(|status_error| {
            status_error.status == StatusCode::TOO_MANY_REQUESTS
                || status_error.status.is_server_error()
        })
        .unwrap_or(false)
}

fn strip_markdown_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    let mut lines = trimmed.lines();
    let Some(first_line) = lines.next() else {
        return String::new();
    };
    if !first_line.starts_with("```") {
        return trimmed.to_string();
    }

    let mut body = Vec::new();
    for line in lines {
        if line.trim_start().starts_with("```") {
            break;
        }
        body.push(line);
    }
    body.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        ANTHROPIC_VERSION, AnthropicClient, AnthropicConfig, AnthropicRequest,
        ConfidenceLevel, HttpStatusError, ReportMode, RiskLevel, StatusCode,
        estimate_cost_usd, heuristic_report, is_retryable_error, parse_message_response,
        parse_response, prompt_contract, retry_delay,
    };

    #[test]
    fn parses_valid_response_into_why_report() {
        let report = parse_response(
            r#"{
                "summary":"Token expiry logic comes from a logout hotfix.",
                "evidence":["fix: tokens not expiring on logout"],
                "inference":["Removal could reopen a session invalidation bug."],
                "unknowns":["No incident ticket was linked in the commits."],
                "risk_level":"HIGH",
                "confidence":"medium-high",
                "notes":["Keep evidence and inference separate."]
            }"#,
        )
        .expect("response should parse");

        assert_eq!(report.summary, "Token expiry logic comes from a logout hotfix.");
        assert_eq!(report.risk_level, RiskLevel::HIGH);
        assert_eq!(report.confidence, ConfidenceLevel::MediumHigh);
        assert_eq!(report.mode, ReportMode::Synthesized);
        assert_eq!(report.risk_summary, RiskLevel::HIGH.summary());
        assert_eq!(report.change_guidance, RiskLevel::HIGH.change_guidance());
    }

    #[test]
    fn strips_markdown_fences_before_parsing() {
        let report = parse_response(
            "```json\n{\n  \"summary\": \"Historical auth behavior\",\n  \"evidence\": [\"fix: auth regression\"],\n  \"inference\": [],\n  \"unknowns\": [],\n  \"risk_level\": \"medium\",\n  \"confidence\": \"low\",\n  \"notes\": []\n}\n```",
        )
        .expect("fenced JSON should parse");

        assert_eq!(report.summary, "Historical auth behavior");
        assert_eq!(report.risk_level, RiskLevel::MEDIUM);
    }

    #[test]
    fn normalizes_risk_level_variants() {
        for value in ["HIGH", "high", "High"] {
            let report = parse_response(&format!(
                r#"{{
                    "summary":"x",
                    "evidence":[],
                    "inference":[],
                    "unknowns":[],
                    "risk_level":"{value}",
                    "confidence":"medium",
                    "notes":[]
                }}"#
            ))
            .expect("risk level should normalize");
            assert_eq!(report.risk_level, RiskLevel::HIGH);
        }
    }

    #[test]
    fn heuristic_report_marks_low_confidence_without_key() {
        let report = heuristic_report(
            "No API key available; using heuristic-only output.",
            RiskLevel::LOW,
            vec!["single low-signal commit".to_string()],
            vec!["No LLM synthesis in phase 1".to_string()],
        );

        assert_eq!(report.mode, ReportMode::Heuristic);
        assert_eq!(report.confidence, ConfidenceLevel::Low);
        assert_eq!(report.cost_usd, None);
        assert_eq!(report.unknowns, vec!["No model synthesis was available for this query."]);
    }

    #[test]
    fn calculates_cost_from_known_token_counts() {
        let cost = estimate_cost_usd(2_000, 500);
        assert!((cost - 0.001125).abs() < 1e-12);
    }

    #[test]
    fn exposes_prompt_contract_requirements() {
        let contract = prompt_contract();
        assert!(contract.response_format.contains("JSON object"));
        assert_eq!(
            contract.required_fields,
            [
                "summary",
                "evidence",
                "inference",
                "unknowns",
                "risk_level",
                "confidence",
            ]
        );
        assert!(contract
            .grounding_rules
            .iter()
            .any(|rule| rule.contains("Do not invent")));
    }

    #[test]
    fn heuristic_report_uses_current_contract_shape() {
        let report = heuristic_report(
            "Heuristic analysis of src/auth.rs:authenticate based on 2 relevant commit(s).",
            RiskLevel::MEDIUM,
            vec!["compat shim added for legacy clients (2024-02-10)".into()],
            vec!["No LLM synthesis in phase 4".into()],
        );

        assert_eq!(report.mode, ReportMode::Heuristic);
        assert_eq!(report.confidence, ConfidenceLevel::Low);
        assert_eq!(report.inference, Vec::<String>::new());
        assert_eq!(report.risk_summary, RiskLevel::MEDIUM.summary());
        assert_eq!(report.change_guidance, RiskLevel::MEDIUM.change_guidance());
        assert_eq!(report.notes, vec!["No LLM synthesis in phase 4"]);
        assert_eq!(report.cost_usd, None);
    }

    #[test]
    fn anthropic_config_defaults_match_plan() {
        let config = AnthropicConfig {
            api_key: "anthropic_test_token".into(),
            model: "claude-haiku-4-5".into(),
            max_tokens: 500,
            timeout_secs: 30,
        };
        assert_eq!(config.api_key, "anthropic_test_token");
        assert_eq!(config.model, "claude-haiku-4-5");
        assert_eq!(config.max_tokens, 500);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn anthropic_request_builder_sets_required_headers() {
        let client = AnthropicClient::new(AnthropicConfig {
            api_key: "anthropic_test_token".into(),
            model: "claude-haiku-4-5".into(),
            max_tokens: 500,
            timeout_secs: 30,
        })
        .expect("client should build");
        let request = AnthropicRequest {
            system_prompt: "system rules".into(),
            user_prompt: "user prompt".into(),
        };

        let built = client
            .request_builder(&request)
            .build()
            .expect("request should build");

        assert_eq!(built.method().as_str(), "POST");
        assert_eq!(built.url().as_str(), "https://api.anthropic.com/v1/messages");
        assert_eq!(
            built.headers().get("x-api-key").and_then(|value| value.to_str().ok()),
            Some("anthropic_test_token")
        );
        assert_eq!(
            built.headers()
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some(ANTHROPIC_VERSION)
        );
    }

    #[test]
    fn parses_anthropic_response_and_costs_usage() {
        let response = parse_message_response(
            r#"{
                "content": [
                    {"type": "text", "text": "{\"summary\":\"ok\"}"}
                ],
                "usage": {
                    "input_tokens": 1200,
                    "output_tokens": 400
                }
            }"#,
        )
        .expect("response should parse");

        assert_eq!(response.text, r#"{"summary":"ok"}"#);
        assert_eq!(response.input_tokens, 1200);
        assert_eq!(response.output_tokens, 400);
        assert!((response.cost_usd - 0.0008).abs() < 1e-12);
    }

    #[test]
    fn retries_only_on_rate_limit_and_server_errors() {
        let rate_limited = anyhow::Error::new(HttpStatusError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "rate limited".into(),
        });
        let server_error = anyhow::Error::new(HttpStatusError {
            status: StatusCode::BAD_GATEWAY,
            message: "upstream failed".into(),
        });
        let client_error = anyhow::Error::new(HttpStatusError {
            status: StatusCode::BAD_REQUEST,
            message: "bad request".into(),
        });

        assert!(is_retryable_error(&rate_limited));
        assert!(is_retryable_error(&server_error));
        assert!(!is_retryable_error(&client_error));
    }

    #[test]
    fn retry_backoff_matches_plan() {
        assert_eq!(retry_delay(1).as_secs(), 2);
        assert_eq!(retry_delay(2).as_secs(), 4);
        assert_eq!(retry_delay(3).as_secs(), 4);
    }

    #[test]
    fn anthropic_config_debug_redacts_api_key() {
        let config = AnthropicConfig {
            api_key: "anthropic_debug_token".into(),
            model: "claude-haiku-4-5".into(),
            max_tokens: 500,
            timeout_secs: 30,
        };

        let debug = format!("{:?}", config);
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("anthropic_debug_token"));
    }

}
