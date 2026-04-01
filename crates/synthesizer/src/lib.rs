//! WhyReport synthesis and fallback behavior with provider-neutral LLM support.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::thread;
use std::time::Duration;
use why_context::{LlmProvider, ResolvedLlmConfig};
use why_evidence::EvidencePack;

#[cfg(test)]
const DEFAULT_TIMEOUT_SECS: u64 = 30;
#[cfg(test)]
const DEFAULT_MAX_TOKENS: u32 = 500;
// Anthropic-specific constants
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_INPUT_COST_PER_MILLION: f64 = 0.25;
const ANTHROPIC_OUTPUT_COST_PER_MILLION: f64 = 1.25;

// OpenAI-compatible defaults
const OPENAI_INPUT_COST_PER_MILLION: f64 = 0.15;
const OPENAI_OUTPUT_COST_PER_MILLION: f64 = 0.60;

// ZAI defaults (also OpenAI-compatible)

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
            Self::HIGH => {
                "Stop and investigate before deleting or heavily refactoring this target."
            }
            Self::MEDIUM => {
                "Change only after reviewing surrounding code and validating behavior you might disturb."
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyNote {
    pub code: String,
    pub kind: String,
    pub message: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy: Vec<PolicyNote>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffReviewFinding {
    pub target: String,
    pub path: String,
    pub symbol: Option<String>,
    pub risk_level: RiskLevel,
    pub confidence: ConfidenceLevel,
    pub why_it_matters: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffReviewReport {
    pub summary: String,
    pub findings: Vec<DiffReviewFinding>,
    pub reviewer_focus: Vec<String>,
    pub unknowns: Vec<String>,
    pub notes: Vec<String>,
    pub mode: ReportMode,
    pub cost_usd: Option<f64>,
    pub github_comment_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDiffReviewFinding {
    target: String,
    path: String,
    #[serde(default)]
    symbol: Option<String>,
    risk_level: String,
    confidence: String,
    why_it_matters: String,
}

#[derive(Debug, Deserialize)]
struct RawDiffReviewReport {
    summary: String,
    #[serde(default)]
    findings: Vec<RawDiffReviewFinding>,
    #[serde(default)]
    reviewer_focus: Vec<String>,
    #[serde(default)]
    unknowns: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
}

// ============================================================================
// Provider-neutral types
// ============================================================================

/// Generic LLM request that can be sent to any provider
#[derive(Debug, Clone, PartialEq)]
pub struct LlmRequest {
    pub system_prompt: String,
    pub user_prompt: String,
}

/// Generic LLM response with optional usage and cost information
#[derive(Debug, Clone, PartialEq)]
pub struct LlmResponse {
    pub text: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// Trait for LLM clients with provider-specific implementations
pub trait LlmClient: Send + Sync + core::fmt::Debug {
    /// Send a request to the LLM provider
    fn send(&self, request: &LlmRequest) -> Result<LlmResponse>;

    /// Get the provider type for debugging
    fn provider(&self) -> LlmProvider;
}

// ============================================================================
// Anthropic adapter
// ============================================================================

#[derive(Clone)]
struct AnthropicAdapter {
    api_key: String,
    model: String,
    max_tokens: u32,
    retries: usize,
    http: Client,
    endpoint: String,
    provider: LlmProvider,
}

impl core::fmt::Debug for AnthropicAdapter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AnthropicAdapter")
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("retries", &self.retries)
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

impl AnthropicAdapter {
    fn new(
        api_key: String,
        model: String,
        max_tokens: u32,
        retries: u32,
        timeout: u64,
        endpoint: String,
    ) -> Result<Self> {
        let http = build_http_client(timeout)?;
        Ok(Self {
            api_key,
            model,
            max_tokens,
            retries: retries.max(1) as usize,
            http,
            endpoint: normalize_anthropic_endpoint(&endpoint),
            provider: LlmProvider::Anthropic,
        })
    }

    fn from_config(config: &ResolvedLlmConfig) -> Result<Self> {
        let api_key = config.auth_token.clone().ok_or_else(|| {
            anyhow!(
                "{} is not set",
                config.provider.api_key_env().unwrap_or("LLM auth token")
            )
        })?;
        let model = config
            .model
            .clone()
            .ok_or_else(|| anyhow!("anthropic provider requires 'model' to be configured"))?;
        let endpoint = config
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("anthropic provider requires 'base_url' to be configured"))?;
        let mut adapter = Self::new(
            api_key,
            model,
            config.max_tokens,
            config.retries,
            config.timeout,
            endpoint,
        )?;
        adapter.provider = config.provider;
        Ok(adapter)
    }

    fn request_body<'a>(&'a self, request: &'a LlmRequest) -> MessageRequest<'a> {
        MessageRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: &request.system_prompt,
            messages: vec![MessageContent {
                role: "user",
                content: &request.user_prompt,
            }],
        }
    }

    fn request_builder(&self, request: &LlmRequest) -> RequestBuilder {
        self.http
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(CONTENT_TYPE, "application/json")
            .json(&self.request_body(request))
    }

    fn send_internal(&self, request: &LlmRequest) -> Result<LlmResponse> {
        let mut last_error = None;

        for attempt in 0..self.retries {
            match self.try_send(request) {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if attempt + 1 == self.retries || !is_retryable_error(&error) {
                        return Err(error);
                    }
                    last_error = Some(error);
                    thread::sleep(retry_delay(attempt + 1));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Anthropic request failed without an error")))
    }

    fn try_send(&self, request: &LlmRequest) -> Result<LlmResponse> {
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

        parse_anthropic_response(&body)
    }
}

impl LlmClient for AnthropicAdapter {
    fn send(&self, request: &LlmRequest) -> Result<LlmResponse> {
        self.send_internal(request)
    }

    fn provider(&self) -> LlmProvider {
        self.provider
    }
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
struct AnthropicMessageResponse {
    content: Vec<ResponseBlock>,
    #[serde(default)]
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct ResponseBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

fn normalize_anthropic_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/v1/messages") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/messages")
    }
}

fn parse_anthropic_response(body: &str) -> Result<LlmResponse> {
    let response: AnthropicMessageResponse = serde_json::from_str(body)
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
    let cost_usd = Some(estimate_anthropic_cost(input_tokens, output_tokens));

    Ok(LlmResponse {
        text,
        input_tokens: Some(input_tokens),
        output_tokens: Some(output_tokens),
        cost_usd,
    })
}

fn estimate_anthropic_cost(input_tokens: u64, output_tokens: u64) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * ANTHROPIC_INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * ANTHROPIC_OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

// ============================================================================
// OpenAI-compatible adapter (for OpenAI, ZAI, custom)
// ============================================================================

#[derive(Clone)]
struct OpenAICompatibleAdapter {
    api_key: String,
    model: String,
    max_tokens: u32,
    retries: usize,
    http: Client,
    endpoint: String,
    provider: LlmProvider,
}

impl core::fmt::Debug for OpenAICompatibleAdapter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OpenAICompatibleAdapter")
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("retries", &self.retries)
            .field("endpoint", &self.endpoint)
            .field("provider", &self.provider)
            .finish()
    }
}

impl OpenAICompatibleAdapter {
    fn new(
        api_key: String,
        model: String,
        max_tokens: u32,
        retries: u32,
        timeout: u64,
        endpoint: String,
        provider: LlmProvider,
    ) -> Result<Self> {
        let http = build_http_client(timeout)?;
        Ok(Self {
            api_key,
            model,
            max_tokens,
            retries: retries.max(1) as usize,
            http,
            endpoint,
            provider,
        })
    }

    fn from_config(config: &ResolvedLlmConfig) -> Result<Self> {
        let api_key = config.auth_token.clone().ok_or_else(|| {
            anyhow!(
                "{} is not set",
                config.provider.api_key_env().unwrap_or("LLM auth token")
            )
        })?;
        let endpoint = config.base_url.clone().ok_or_else(|| {
            anyhow!(
                "{} provider requires 'base_url' to be configured",
                config.provider
            )
        })?;
        let model = config.model.clone().ok_or_else(|| {
            anyhow!(
                "{} provider requires 'model' to be configured",
                config.provider
            )
        })?;
        Self::new(
            api_key,
            model,
            config.max_tokens,
            config.retries,
            config.timeout,
            endpoint,
            config.provider,
        )
    }

    fn request_body<'a>(&'a self, request: &'a LlmRequest) -> OpenAIMessageRequest<'a> {
        OpenAIMessageRequest {
            model: &self.model,
            max_tokens: Some(self.max_tokens),
            messages: vec![
                OpenAIMessage {
                    role: "system",
                    content: &request.system_prompt,
                },
                OpenAIMessage {
                    role: "user",
                    content: &request.user_prompt,
                },
            ],
        }
    }

    fn request_builder(&self, request: &LlmRequest) -> RequestBuilder {
        self.http
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("x-api-key", &self.api_key)
            .header(CONTENT_TYPE, "application/json")
            .json(&self.request_body(request))
    }

    fn send_internal(&self, request: &LlmRequest) -> Result<LlmResponse> {
        let mut last_error = None;

        for attempt in 0..self.retries {
            match self.try_send(request) {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if attempt + 1 == self.retries || !is_retryable_error(&error) {
                        return Err(error);
                    }
                    last_error = Some(error);
                    thread::sleep(retry_delay(attempt + 1));
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow!("OpenAI-compatible request failed without an error")))
    }

    fn try_send(&self, request: &LlmRequest) -> Result<LlmResponse> {
        let response = self
            .request_builder(request)
            .send()
            .context("failed to send OpenAI-compatible request")?;
        let status = response.status();
        let body = response
            .text()
            .context("failed to read OpenAI-compatible response body")?;

        if !status.is_success() {
            let message = parse_api_error_message(&body)
                .unwrap_or_else(|| format!("API returned HTTP {}", status.as_u16()));
            return Err(HttpStatusError { status, message }.into());
        }

        parse_openai_response(&body)
    }
}

impl LlmClient for OpenAICompatibleAdapter {
    fn send(&self, request: &LlmRequest) -> Result<LlmResponse> {
        self.send_internal(request)
    }

    fn provider(&self) -> LlmProvider {
        self.provider
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct OpenAIMessageRequest<'a> {
    model: &'a str,
    max_tokens: Option<u32>,
    messages: Vec<OpenAIMessage<'a>>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct OpenAIMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    #[serde(default)]
    usage: OpenAIUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessageContent,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessageContent {
    content: String,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAIUsage {
    #[serde(rename = "prompt_tokens", default)]
    prompt_tokens: u64,
    #[serde(rename = "completion_tokens", default)]
    completion_tokens: u64,
}

fn parse_openai_response(body: &str) -> Result<LlmResponse> {
    let response: OpenAIResponse = serde_json::from_str(body)
        .map_err(|error| anyhow!("failed to parse OpenAI-compatible response JSON: {error}"))?;

    let text = response
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("OpenAI-compatible response did not contain message content"))?;

    let input_tokens = response.usage.prompt_tokens;
    let output_tokens = response.usage.completion_tokens;
    let cost_usd = Some(estimate_openai_cost(input_tokens, output_tokens));

    Ok(LlmResponse {
        text,
        input_tokens: Some(input_tokens),
        output_tokens: Some(output_tokens),
        cost_usd,
    })
}

fn estimate_openai_cost(input_tokens: u64, output_tokens: u64) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * OPENAI_INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * OPENAI_OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

// ============================================================================
// Client factory
// ============================================================================

/// Create an LLM client from the merged configuration
pub fn client_from_config(config: &ResolvedLlmConfig) -> Result<Box<dyn LlmClient>> {
    match config.provider {
        LlmProvider::Anthropic | LlmProvider::Zai | LlmProvider::Custom => {
            let adapter = AnthropicAdapter::from_config(config)?;
            Ok(Box::new(adapter))
        }
        LlmProvider::Openai => {
            let adapter = OpenAICompatibleAdapter::from_config(config)?;
            Ok(Box::new(adapter))
        }
    }
}

// ============================================================================
// Provider-neutral synthesis functions
// ============================================================================

/// Synthesize a WhyReport using the configured LLM provider
pub fn synthesize_report(
    client: &dyn LlmClient,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<WhyReport> {
    let request = LlmRequest {
        system_prompt: system_prompt.to_string(),
        user_prompt: user_prompt.to_string(),
    };

    let response = client
        .send(&request)
        .with_context(|| format!("LLM request failed for provider: {:?}", client.provider()))?;

    let mut report = parse_response(&response.text)?;
    report.cost_usd = response.cost_usd;
    Ok(report)
}

/// Synthesize a DiffReviewReport using the configured LLM provider
pub fn synthesize_diff_review(
    client: &dyn LlmClient,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<DiffReviewReport> {
    let request = LlmRequest {
        system_prompt: system_prompt.to_string(),
        user_prompt: user_prompt.to_string(),
    };

    let response = client
        .send(&request)
        .with_context(|| format!("LLM request failed for provider: {:?}", client.provider()))?;

    let mut report = parse_diff_review_response(&response.text)?;
    report.cost_usd = response.cost_usd;
    Ok(report)
}

// ============================================================================
// Existing provider-neutral helpers (preserved)
// ============================================================================

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
            "Use inference for conclusions drawn from evidence.",
            "List unknowns explicitly when history is sparse or ambiguous.",
            "Do not invent incidents, PR details, or dependencies not present in evidence pack.",
        ],
    }
}

pub fn build_system_prompt(contract: &PromptContract) -> String {
    format!(
        "You are a careful code archaeology assistant. {} Required fields: {}. Grounding rules: {}",
        contract.response_format,
        contract.required_fields.join(", "),
        contract.grounding_rules.join(" ")
    )
}

pub fn build_query_prompt(pack: &EvidencePack) -> String {
    let evidence_json = serde_json::to_string_pretty(pack)
        .unwrap_or_else(|_| "{\"error\":\"failed to serialize evidence pack\"}".to_string());
    format!(
        "Use this evidence pack to explain why the target exists and risk of changing it.\n\nEvidence pack:\n{}",
        evidence_json
    )
}

pub fn build_diff_review_prompt(target: &str, packs: &[EvidencePack]) -> String {
    let packs_json = serde_json::to_string_pretty(packs).unwrap_or_else(|_| "[]".to_string());
    format!(
        "Risk analysis for changes in: {}\nReturn a single JSON object with fields: summary, findings, reviewer_focus, unknowns, notes.\nEach finding must include: target, path, symbol, risk_level, confidence, why_it_matters.\nUse only the supplied evidence packs.\n\nEvidence packs:\n{}",
        target, packs_json
    )
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
        policy: Vec::new(),
        cost_usd: None,
    })
}

pub fn parse_diff_review_response(raw: &str) -> Result<DiffReviewReport> {
    let cleaned = strip_markdown_fences(raw);
    let parsed: RawDiffReviewReport = serde_json::from_str(&cleaned)
        .map_err(|error| anyhow!("failed to parse DiffReviewReport JSON: {error}"))?;

    let findings = parsed
        .findings
        .into_iter()
        .map(|finding| {
            Ok(DiffReviewFinding {
                target: finding.target,
                path: finding.path,
                symbol: finding.symbol,
                risk_level: finding.risk_level.parse()?,
                confidence: finding.confidence.parse()?,
                why_it_matters: finding.why_it_matters,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(DiffReviewReport {
        summary: parsed.summary,
        findings,
        reviewer_focus: parsed.reviewer_focus,
        unknowns: parsed.unknowns,
        notes: parsed.notes,
        mode: ReportMode::Synthesized,
        cost_usd: None,
        github_comment_url: None,
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
        policy: Vec::new(),
        cost_usd: None,
    }
}

pub fn heuristic_diff_review_report(
    summary: impl Into<String>,
    findings: Vec<DiffReviewFinding>,
    reviewer_focus: Vec<String>,
    notes: Vec<String>,
) -> DiffReviewReport {
    DiffReviewReport {
        summary: summary.into(),
        findings,
        reviewer_focus,
        unknowns: vec!["No model synthesis was available for this diff review.".to_string()],
        notes,
        mode: ReportMode::Heuristic,
        cost_usd: None,
        github_comment_url: None,
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

#[derive(Debug)]
struct HttpStatusError {
    status: StatusCode,
    message: String,
}

impl core::fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "{} (HTTP {})",
            self.message,
            self.status.as_u16()
        )
    }
}

impl std::error::Error for HttpStatusError {}

#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    error: ApiError,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

fn build_http_client(timeout_secs: u64) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "anthropic-version",
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(timeout_secs))
        .https_only(true)
        .build()
        .context("failed to build HTTP client")
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use why_context::{LlmProvider, ResolvedLlmConfig};
    use why_evidence::{
        CommitSummary, EvidencePack, HistoryInfo, LocalContextInfo, SignalInfo, TargetInfo,
    };

    fn sample_pack(symbol: &str, issue_ref: &str) -> EvidencePack {
        EvidencePack {
            target: TargetInfo {
                file: "src/auth.rs".into(),
                symbol: Some(symbol.into()),
                lines: (10, 18),
                language: "rust".into(),
            },
            local_context: LocalContextInfo {
                comments: vec!["hotfix preserved for legacy clients".into()],
                markers: vec!["TODO: remove after rollout".into()],
                risk_flags: vec!["auth".into(), "token".into()],
            },
            history: HistoryInfo {
                total_commit_count: 1,
                commits_shown: 1,
                top_commits: vec![CommitSummary {
                    oid: "abc12345".into(),
                    date: "2026-03-14".into(),
                    author: "alice".into(),
                    summary: "fix: preserve auth compatibility path".into(),
                    diff_excerpt: "+ preserve legacy auth fallback".into(),
                    coverage_pct: 75,
                    issue_refs: vec![issue_ref.into()],
                }],
            },
            signals: SignalInfo {
                issue_refs: vec![issue_ref.into()],
                risk_keywords: vec!["auth".into(), "incident".into()],
                heuristic_risk: "HIGH".into(),
                github_items: Vec::new(),
                github_notes: vec!["GitHub enrichment unavailable".into()],
            },
        }
    }

    #[test]
    fn prompt_contract_exposes_requirements() {
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
        assert!(
            contract
                .grounding_rules
                .iter()
                .any(|rule| rule.contains("Do not invent"))
        );
    }

    #[test]
    fn build_system_prompt_embeds_contract_details() {
        let contract = prompt_contract();
        let prompt = build_system_prompt(&contract);

        assert!(prompt.contains("careful code archaeology assistant"));
        assert!(prompt.contains(contract.response_format));
        assert!(prompt.contains("summary, evidence, inference, unknowns, risk_level, confidence"));
        assert!(prompt.contains("Do not invent incidents, PR details, or dependencies"));
    }

    #[test]
    fn build_query_prompt_serializes_evidence_pack() {
        let prompt = build_query_prompt(&sample_pack("authenticate", "#42"));

        assert!(prompt.contains("Use this evidence pack to explain why the target exists"));
        assert!(prompt.contains("\"symbol\": \"authenticate\""));
        assert!(prompt.contains("\"issue_refs\": ["));
        assert!(prompt.contains("\"#42\""));
    }

    #[test]
    fn build_diff_review_prompt_serializes_multiple_packs() {
        let prompt = build_diff_review_prompt(
            "main..feature",
            &[
                sample_pack("authenticate", "#42"),
                sample_pack("refresh_session", "#77"),
            ],
        );

        assert!(prompt.contains("Risk analysis for changes in: main..feature"));
        assert!(prompt.contains("Return a single JSON object with fields: summary, findings, reviewer_focus, unknowns, notes."));
        assert!(prompt.contains("\"symbol\": \"authenticate\""));
        assert!(prompt.contains("\"symbol\": \"refresh_session\""));
        assert!(prompt.contains("\"#77\""));
    }

    #[test]
    fn parses_valid_response_into_why_report() {
        let report = parse_response(
            r#"{
                "summary":"Token expiry logic comes from a logout hotfix.",
                "evidence":["fix: tokens not expiring on logout"],
                "inference":["Removal could reopen a session invalidation bug."],
                "unknowns":["No incident ticket was linked in commits."],
                "risk_level":"HIGH",
                "confidence":"medium-high",
                "notes":["Keep evidence and inference separate."]
            }"#,
        )
        .expect("response should parse");

        assert_eq!(
            report.summary,
            "Token expiry logic comes from a logout hotfix."
        );
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
    fn parses_valid_response_into_diff_review_report() {
        let report = parse_diff_review_response(
            r#"{
                "summary":"The staged diff touches one historically risky auth path.",
                "findings":[{
                    "target":"src/auth.rs:authenticate",
                    "path":"src/auth.rs",
                    "symbol":"authenticate",
                    "risk_level":"HIGH",
                    "confidence":"medium-high",
                    "why_it_matters":"The function was repeatedly patched for logout/session regressions."
                }],
                "reviewer_focus":["Verify logout invalidation coverage."],
                "unknowns":["No linked incident doc was present in the sampled commits."],
                "notes":["Keep evidence and inference separate."]
            }"#,
        )
        .expect("response should parse");

        assert_eq!(
            report.summary,
            "The staged diff touches one historically risky auth path."
        );
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].risk_level, RiskLevel::HIGH);
        assert_eq!(report.findings[0].confidence, ConfidenceLevel::MediumHigh);
        assert_eq!(report.mode, ReportMode::Synthesized);
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
        assert_eq!(
            report.unknowns,
            vec!["No model synthesis was available for this query."]
        );
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
    fn heuristic_diff_review_report_marks_low_confidence_without_key() {
        let report = heuristic_diff_review_report(
            "Heuristic diff review of staged changes.",
            vec![DiffReviewFinding {
                target: "src/auth.rs:authenticate".into(),
                path: "src/auth.rs".into(),
                symbol: Some("authenticate".into()),
                risk_level: RiskLevel::HIGH,
                confidence: ConfidenceLevel::Low,
                why_it_matters: "Repeated auth fixes show a risky change surface.".into(),
            }],
            vec!["Review auth regression coverage.".into()],
            vec!["Heuristic-only diff review.".into()],
        );

        assert_eq!(report.mode, ReportMode::Heuristic);
        assert_eq!(report.cost_usd, None);
        assert_eq!(
            report.unknowns,
            vec!["No model synthesis was available for this diff review."]
        );
    }

    fn sample_config(provider: LlmProvider) -> ResolvedLlmConfig {
        let (model, base_url) = match provider {
            LlmProvider::Anthropic => (
                "claude-haiku-4-5-20251001",
                "https://api.anthropic.com/v1/messages",
            ),
            LlmProvider::Openai => ("gpt-5.4", "https://api.openai.com/v1/chat/completions"),
            LlmProvider::Zai => ("glm-5", "https://api.z.ai/api/anthropic/v1/messages"),
            LlmProvider::Custom => (
                "custom-model",
                "https://api.example.com/v1/chat/completions",
            ),
        };

        ResolvedLlmConfig {
            provider,
            model: Some(model.to_string()),
            base_url: Some(base_url.to_string()),
            auth_token: Some("test_key".to_string()),
            retries: 3,
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout: DEFAULT_TIMEOUT_SECS,
        }
    }

    #[test]
    fn client_from_config_creates_anthropic_adapter() {
        let client = client_from_config(&sample_config(LlmProvider::Anthropic));
        assert!(client.is_ok());
        assert_eq!(client.unwrap().provider(), LlmProvider::Anthropic);
    }

    #[test]
    fn anthropic_adapter_requires_api_key() {
        let mut config = sample_config(LlmProvider::Anthropic);
        config.auth_token = None;

        let client = client_from_config(&config);
        assert!(client.is_err());
        assert!(
            client
                .unwrap_err()
                .to_string()
                .contains("ANTHROPIC_API_KEY is not set")
        );
    }

    #[test]
    fn openai_adapter_requires_api_key() {
        let mut config = sample_config(LlmProvider::Openai);
        config.auth_token = None;

        let client = client_from_config(&config);
        assert!(client.is_err());
        assert!(
            client
                .unwrap_err()
                .to_string()
                .contains("OPENAI_API_KEY is not set")
        );
    }

    #[test]
    fn custom_adapter_requires_base_url_and_model() {
        let mut missing_base_url = sample_config(LlmProvider::Custom);
        missing_base_url.base_url = None;
        let client = client_from_config(&missing_base_url);
        assert!(client.is_err());
        assert!(client.unwrap_err().to_string().contains("base_url"));

        let mut missing_model = sample_config(LlmProvider::Custom);
        missing_model.model = None;
        let client = client_from_config(&missing_model);
        assert!(client.is_err());
        assert!(client.unwrap_err().to_string().contains("model"));
    }

    #[test]
    fn openai_adapter_works_with_credentials() {
        let client = client_from_config(&sample_config(LlmProvider::Openai));
        assert!(client.is_ok());
        assert_eq!(client.unwrap().provider(), LlmProvider::Openai);
    }

    #[test]
    fn zai_adapter_works_with_credentials() {
        let client = client_from_config(&sample_config(LlmProvider::Zai));
        assert!(client.is_ok());
        assert_eq!(client.unwrap().provider(), LlmProvider::Zai);
    }

    #[test]
    fn client_debug_reflects_configured_retries() {
        let mut config = sample_config(LlmProvider::Openai);
        config.retries = 7;

        let client = client_from_config(&config).expect("client should build");
        let debug = format!("{client:?}");
        assert!(debug.contains("retries: 7"));
    }

    #[test]
    fn openai_adapter_enforces_https_only_transport() {
        let client = build_http_client(30).expect("client should build");
        let error = client
            .post("http://api.openai.com/v1/chat/completions")
            .send()
            .expect_err("http requests should be rejected");

        assert!(error.to_string().contains("HTTPS") || error.to_string().contains("http://"));
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
    fn estimates_anthropic_cost() {
        let cost = estimate_anthropic_cost(2_000, 500);
        assert!((cost - 0.001125).abs() < 1e-12);
    }

    #[test]
    fn estimates_openai_cost() {
        let cost = estimate_openai_cost(2_000, 500);
        assert!((cost - 0.0006).abs() < 1e-12);
    }
}
