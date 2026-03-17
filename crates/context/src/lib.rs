//! Local code context extraction and heuristics.

use anyhow::{Context, Result};
use git2::Repository;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_RISK_LEVEL: &str = "LOW";
const DEFAULT_MAX_COMMITS: usize = 8;
const DEFAULT_RECENCY_WINDOW_DAYS: i64 = 90;
const DEFAULT_MECHANICAL_THRESHOLD_FILES: usize = 50;
const DEFAULT_COUPLING_SCAN_COMMITS: usize = 500;
const DEFAULT_COUPLING_RATIO_THRESHOLD: f64 = 0.30;
const DEFAULT_CACHE_MAX_ENTRIES: usize = 500;
const DEFAULT_LLM_TIMEOUT: u64 = 30;
const DEFAULT_LLM_RETRIES: u32 = 3;
const DEFAULT_LLM_MAX_TOKENS: u32 = 500;
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4";
const DEFAULT_ZAI_MODEL: &str = "glm-5";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_ZAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4/chat/completions";
const LOCAL_CONFIG_FILE_NAME: &str = "why.local.toml";
const GLOBAL_CONFIG_DIR_NAME: &str = "why";
const GLOBAL_CONFIG_FILE_NAME: &str = "why.toml";

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct WhyConfig {
    #[serde(default)]
    pub risk: RiskConfig,
    #[serde(default)]
    pub git: GitConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub github: GitHubConfig,
    #[serde(default)]
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct WhyConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<RiskConfigLayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitConfigLayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheConfigLayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GitHubConfigLayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmConfigLayer>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RiskConfig {
    #[serde(default = "default_risk_level")]
    pub default_level: String,
    #[serde(default)]
    pub keywords: RiskKeywords,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RiskConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<RiskKeywordsLayer>,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            default_level: default_risk_level(),
            keywords: RiskKeywords::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RiskKeywords {
    #[serde(default)]
    pub high: Vec<String>,
    #[serde(default)]
    pub medium: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RiskKeywordsLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct GitConfig {
    #[serde(default = "default_max_commits")]
    pub max_commits: usize,
    #[serde(default = "default_recency_window_days")]
    pub recency_window_days: i64,
    #[serde(default = "default_mechanical_threshold_files")]
    pub mechanical_threshold_files: usize,
    #[serde(default = "default_coupling_scan_commits")]
    pub coupling_scan_commits: usize,
    #[serde(default = "default_coupling_ratio_threshold")]
    pub coupling_ratio_threshold: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct GitConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_commits: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency_window_days: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mechanical_threshold_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupling_scan_commits: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupling_ratio_threshold: Option<f64>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            max_commits: default_max_commits(),
            recency_window_days: default_recency_window_days(),
            mechanical_threshold_files: default_mechanical_threshold_files(),
            coupling_scan_commits: default_coupling_scan_commits(),
            coupling_ratio_threshold: default_coupling_ratio_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CacheConfig {
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: usize,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CacheConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<usize>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: default_cache_max_entries(),
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubConfig {
    #[serde(default = "default_github_remote")]
    pub remote: String,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct GitHubConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl core::fmt::Debug for GitHubConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GitHubConfig")
            .field("remote", &self.remote)
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            remote: default_github_remote(),
            token: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Openai,
    #[default]
    Anthropic,
    Zai,
    Custom,
}

impl LlmProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Zai => "zai",
            Self::Custom => "custom",
        }
    }

    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some(DEFAULT_OPENAI_MODEL),
            Self::Anthropic => Some(DEFAULT_ANTHROPIC_MODEL),
            Self::Zai => Some(DEFAULT_ZAI_MODEL),
            Self::Custom => None,
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some(DEFAULT_OPENAI_BASE_URL),
            Self::Anthropic => Some(DEFAULT_ANTHROPIC_BASE_URL),
            Self::Zai => Some(DEFAULT_ZAI_BASE_URL),
            Self::Custom => None,
        }
    }

    pub fn api_key_env(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Zai => Some("ZAI_API_KEY"),
            Self::Custom => Some("CUSTOM_API_KEY"),
        }
    }
}

impl core::fmt::Display for LlmProvider {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    #[serde(default)]
    pub provider: LlmProvider,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default = "default_llm_retries")]
    pub retries: u32,
    #[serde(default = "default_llm_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_llm_timeout")]
    pub timeout: u64,
}

impl core::fmt::Debug for LlmConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LlmConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[redacted]"),
            )
            .field("retries", &self.retries)
            .field("max_tokens", &self.max_tokens)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LlmConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<LlmProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::default(),
            model: None,
            base_url: None,
            auth_token: None,
            retries: default_llm_retries(),
            max_tokens: default_llm_max_tokens(),
            timeout: default_llm_timeout(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedLlmConfig {
    pub provider: LlmProvider,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub auth_token: Option<String>,
    pub retries: u32,
    pub max_tokens: u32,
    pub timeout: u64,
}

impl core::fmt::Debug for ResolvedLlmConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ResolvedLlmConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[redacted]"),
            )
            .field("retries", &self.retries)
            .field("max_tokens", &self.max_tokens)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl WhyConfig {
    pub fn github_token(&self) -> Option<String> {
        resolve_secret(Some("GITHUB_TOKEN"), self.github.token.as_deref(), None)
    }

    pub fn resolved_llm_config(&self) -> ResolvedLlmConfig {
        let provider = self.llm.provider;
        let model = self
            .llm
            .model
            .clone()
            .and_then(|value| normalize_text(&value))
            .or_else(|| provider.default_model().map(str::to_string));
        let base_url = self
            .llm
            .base_url
            .clone()
            .and_then(|value| normalize_text(&value))
            .or_else(|| provider.default_base_url().map(str::to_string));

        ResolvedLlmConfig {
            provider,
            model,
            base_url,
            auth_token: resolve_secret(
                provider.api_key_env(),
                self.llm.auth_token.as_deref(),
                None,
            ),
            retries: self.llm.retries,
            max_tokens: self.llm.max_tokens,
            timeout: self.llm.timeout,
        }
    }
}

pub fn load_config(start_dir: &Path) -> Result<WhyConfig> {
    let mut config = WhyConfig::default();

    if let Some(global_path) = global_config_path().filter(|path| path.is_file()) {
        config.apply_layer(load_config_layer_from_path(&global_path)?);
    }

    if let Some(config_path) = local_config_path(start_dir) {
        config.apply_layer(load_config_layer_from_path(&config_path)?);
    }

    Ok(config)
}

pub fn load_config_from_path(config_path: &Path) -> Result<WhyConfig> {
    let mut config = WhyConfig::default();
    config.apply_layer(load_config_layer_from_path(config_path)?);
    Ok(config)
}

pub fn load_config_layer_from_path(config_path: &Path) -> Result<WhyConfigLayer> {
    let contents = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config file {}", config_path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file {}", config_path.display()))
}

pub fn write_config_layer_to_path(config_path: &Path, layer: &WhyConfigLayer) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(layer)
        .with_context(|| format!("failed to serialize config for {}", config_path.display()))?;
    fs::write(config_path, contents)
        .with_context(|| format!("failed to write config file {}", config_path.display()))
}

pub fn global_config_path() -> Option<PathBuf> {
    if let Some(xdg_config_home) = env::var_os("XDG_CONFIG_HOME") {
        let xdg_path = PathBuf::from(xdg_config_home);
        if !xdg_path.as_os_str().is_empty() {
            return Some(
                xdg_path
                    .join(GLOBAL_CONFIG_DIR_NAME)
                    .join(GLOBAL_CONFIG_FILE_NAME),
            );
        }
    }

    env::var_os("HOME").and_then(|home| {
        let home_path = PathBuf::from(home);
        (!home_path.as_os_str().is_empty()).then_some(
            home_path
                .join(".config")
                .join(GLOBAL_CONFIG_DIR_NAME)
                .join(GLOBAL_CONFIG_FILE_NAME),
        )
    })
}

pub fn local_config_path(start_dir: &Path) -> Option<PathBuf> {
    let search_root = config_search_root(start_dir);
    find_config_path(start_dir, search_root.as_deref())
}

pub fn local_config_target_path(start_dir: &Path) -> Option<PathBuf> {
    local_config_path(start_dir).or_else(|| {
        let root = config_search_root(start_dir)
            .or_else(|| config_anchor(start_dir).map(Path::to_path_buf))?;
        Some(root.join(LOCAL_CONFIG_FILE_NAME))
    })
}

fn find_config_path(start_dir: &Path, search_root: Option<&Path>) -> Option<PathBuf> {
    let mut current = config_anchor(start_dir)?.to_path_buf();
    let search_root = search_root.map(Path::to_path_buf);

    loop {
        let candidate = current.join(LOCAL_CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }

        if search_root.as_ref().is_some_and(|root| *root == current) {
            return None;
        }

        if !current.pop() {
            return None;
        }
    }
}

fn config_search_root(start_dir: &Path) -> Option<PathBuf> {
    let anchor = config_anchor(start_dir)?;

    Some(
        Repository::discover(anchor)
            .ok()
            .and_then(|repo| repo.workdir().map(Path::to_path_buf))
            .unwrap_or_else(|| anchor.to_path_buf()),
    )
}

fn config_anchor(start_dir: &Path) -> Option<&Path> {
    if start_dir.is_dir() {
        Some(start_dir)
    } else {
        start_dir.parent()
    }
}

fn normalize_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn resolve_secret(
    primary_env: Option<&str>,
    inline: Option<&str>,
    env_reference: Option<&str>,
) -> Option<String> {
    primary_env
        .and_then(env_var_trimmed)
        .or_else(|| inline.and_then(normalize_text))
        .or_else(|| {
            env_reference
                .and_then(normalize_text)
                .as_deref()
                .and_then(env_var_trimmed)
        })
}

fn env_var_trimmed(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|value| normalize_text(&value))
}

impl WhyConfig {
    fn apply_layer(&mut self, layer: WhyConfigLayer) {
        if let Some(risk) = layer.risk {
            self.risk.apply_layer(risk);
        }
        if let Some(git) = layer.git {
            self.git.apply_layer(git);
        }
        if let Some(cache) = layer.cache {
            self.cache.apply_layer(cache);
        }
        if let Some(github) = layer.github {
            self.github.apply_layer(github);
        }
        if let Some(llm) = layer.llm {
            self.llm.apply_layer(llm);
        }
    }
}

impl RiskConfig {
    fn apply_layer(&mut self, layer: RiskConfigLayer) {
        if let Some(default_level) = layer.default_level.and_then(|value| normalize_text(&value)) {
            self.default_level = default_level;
        }
        if let Some(keywords) = layer.keywords {
            self.keywords.apply_layer(keywords);
        }
    }
}

impl RiskKeywords {
    fn apply_layer(&mut self, layer: RiskKeywordsLayer) {
        if let Some(high) = layer.high {
            self.high = high;
        }
        if let Some(medium) = layer.medium {
            self.medium = medium;
        }
    }
}

impl GitConfig {
    fn apply_layer(&mut self, layer: GitConfigLayer) {
        if let Some(max_commits) = layer.max_commits {
            self.max_commits = max_commits;
        }
        if let Some(recency_window_days) = layer.recency_window_days {
            self.recency_window_days = recency_window_days;
        }
        if let Some(mechanical_threshold_files) = layer.mechanical_threshold_files {
            self.mechanical_threshold_files = mechanical_threshold_files;
        }
        if let Some(coupling_scan_commits) = layer.coupling_scan_commits {
            self.coupling_scan_commits = coupling_scan_commits;
        }
        if let Some(coupling_ratio_threshold) = layer.coupling_ratio_threshold {
            self.coupling_ratio_threshold = coupling_ratio_threshold;
        }
    }
}

impl CacheConfig {
    fn apply_layer(&mut self, layer: CacheConfigLayer) {
        if let Some(max_entries) = layer.max_entries {
            self.max_entries = max_entries;
        }
    }
}

impl GitHubConfig {
    fn apply_layer(&mut self, layer: GitHubConfigLayer) {
        if let Some(remote) = layer.remote.and_then(|value| normalize_text(&value)) {
            self.remote = remote;
        }
        if let Some(token) = layer.token.and_then(|value| normalize_text(&value)) {
            self.token = Some(token);
        }
    }
}

impl LlmConfig {
    fn apply_layer(&mut self, layer: LlmConfigLayer) {
        if let Some(provider) = layer.provider {
            if provider != self.provider {
                self.model = None;
                self.base_url = None;
                self.auth_token = None;
            }
            self.provider = provider;
        }
        if let Some(model) = layer.model.and_then(|value| normalize_text(&value)) {
            self.model = Some(model);
        }
        if let Some(base_url) = layer.base_url.and_then(|value| normalize_text(&value)) {
            self.base_url = Some(base_url);
        }
        if let Some(auth_token) = layer.auth_token.and_then(|value| normalize_text(&value)) {
            self.auth_token = Some(auth_token);
        }
        if let Some(retries) = layer.retries {
            self.retries = retries;
        }
        if let Some(max_tokens) = layer.max_tokens {
            self.max_tokens = max_tokens;
        }
        if let Some(timeout) = layer.timeout {
            self.timeout = timeout;
        }
    }
}

fn default_risk_level() -> String {
    DEFAULT_RISK_LEVEL.to_string()
}

fn default_max_commits() -> usize {
    DEFAULT_MAX_COMMITS
}

fn default_recency_window_days() -> i64 {
    DEFAULT_RECENCY_WINDOW_DAYS
}

fn default_mechanical_threshold_files() -> usize {
    DEFAULT_MECHANICAL_THRESHOLD_FILES
}

fn default_coupling_scan_commits() -> usize {
    DEFAULT_COUPLING_SCAN_COMMITS
}

fn default_coupling_ratio_threshold() -> f64 {
    DEFAULT_COUPLING_RATIO_THRESHOLD
}

fn default_cache_max_entries() -> usize {
    DEFAULT_CACHE_MAX_ENTRIES
}

fn default_github_remote() -> String {
    "origin".to_string()
}

fn default_llm_timeout() -> u64 {
    DEFAULT_LLM_TIMEOUT
}

fn default_llm_retries() -> u32 {
    DEFAULT_LLM_RETRIES
}

fn default_llm_max_tokens() -> u32 {
    DEFAULT_LLM_MAX_TOKENS
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ANTHROPIC_BASE_URL, DEFAULT_ANTHROPIC_MODEL, DEFAULT_CACHE_MAX_ENTRIES,
        DEFAULT_COUPLING_RATIO_THRESHOLD, DEFAULT_COUPLING_SCAN_COMMITS, DEFAULT_LLM_MAX_TOKENS,
        DEFAULT_LLM_RETRIES, DEFAULT_LLM_TIMEOUT, DEFAULT_MAX_COMMITS,
        DEFAULT_MECHANICAL_THRESHOLD_FILES, DEFAULT_RECENCY_WINDOW_DAYS, DEFAULT_RISK_LEVEL,
        LlmProvider, WhyConfig, config_search_root, find_config_path, global_config_path,
        load_config, load_config_from_path, local_config_target_path, write_config_layer_to_path,
    };
    use anyhow::Result;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn returns_defaults_when_config_is_missing() -> Result<()> {
        let _env = TestEnv::new()?;
        let tempdir = TempDir::new()?;
        let config = load_config(tempdir.path())?;
        let resolved = config.resolved_llm_config();

        assert_eq!(config.risk.default_level, DEFAULT_RISK_LEVEL);
        assert_eq!(config.git.max_commits, DEFAULT_MAX_COMMITS);
        assert_eq!(
            config.git.mechanical_threshold_files,
            DEFAULT_MECHANICAL_THRESHOLD_FILES
        );
        assert_eq!(config.git.recency_window_days, DEFAULT_RECENCY_WINDOW_DAYS);
        assert_eq!(
            config.git.coupling_scan_commits,
            DEFAULT_COUPLING_SCAN_COMMITS
        );
        assert_eq!(
            config.git.coupling_ratio_threshold,
            DEFAULT_COUPLING_RATIO_THRESHOLD
        );
        assert_eq!(config.cache.max_entries, DEFAULT_CACHE_MAX_ENTRIES);
        assert_eq!(config.github.remote, "origin");
        assert_eq!(config.github.token, None);
        assert_eq!(config.llm.provider, LlmProvider::Anthropic);
        assert_eq!(config.llm.model, None);
        assert_eq!(config.llm.retries, DEFAULT_LLM_RETRIES);
        assert_eq!(config.llm.timeout, DEFAULT_LLM_TIMEOUT);
        assert_eq!(config.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(resolved.model.as_deref(), Some(DEFAULT_ANTHROPIC_MODEL));
        assert_eq!(
            resolved.base_url.as_deref(),
            Some(DEFAULT_ANTHROPIC_BASE_URL)
        );
        assert!(config.risk.keywords.high.is_empty());
        assert!(config.risk.keywords.medium.is_empty());

        Ok(())
    }

    #[test]
    fn loads_risk_keywords_git_and_llm_overrides_from_toml() -> Result<()> {
        let _env = TestEnv::new()?;
        let tempdir = TempDir::new()?;
        let config_path = tempdir.path().join("why.local.toml");
        let github_key = "token";
        fs::write(
            &config_path,
            format!(
                r#"
[risk]
default_level = "medium"

[risk.keywords]
high = ["pci", "reconciliation"]
medium = ["terraform"]

[git]
max_commits = 5
recency_window_days = 30
mechanical_threshold_files = 12
coupling_scan_commits = 120
coupling_ratio_threshold = 0.45

[cache]
max_entries = 42

[github]
remote = "upstream"
{github_key} = "test-placeholder"

[llm]
provider = "openai"
model = "gpt-5.4-mini"
base_url = "https://api.openai.com/v1/chat/completions"
auth_token = "openai-inline-token"
retries = 5
timeout = 45
max_tokens = 700
"#,
            ),
        )?;

        let config = load_config_from_path(&config_path)?;
        let resolved = config.resolved_llm_config();

        assert_eq!(config.risk.default_level, "medium");
        assert_eq!(
            config.risk.keywords.high,
            vec!["pci".to_string(), "reconciliation".to_string()]
        );
        assert_eq!(config.risk.keywords.medium, vec!["terraform".to_string()]);
        assert_eq!(config.git.max_commits, 5);
        assert_eq!(config.git.recency_window_days, 30);
        assert_eq!(config.git.mechanical_threshold_files, 12);
        assert_eq!(config.git.coupling_scan_commits, 120);
        assert_eq!(config.git.coupling_ratio_threshold, 0.45);
        assert_eq!(config.cache.max_entries, 42);
        assert_eq!(config.github.remote, "upstream");
        assert_eq!(config.github.token.as_deref(), Some("test-placeholder"));
        assert_eq!(config.llm.provider, LlmProvider::Openai);
        assert_eq!(config.llm.model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(
            config.llm.base_url.as_deref(),
            Some("https://api.openai.com/v1/chat/completions")
        );
        assert_eq!(
            config.llm.auth_token.as_deref(),
            Some("openai-inline-token")
        );
        assert_eq!(config.llm.retries, 5);
        assert_eq!(config.llm.timeout, 45);
        assert_eq!(config.llm.max_tokens, 700);
        assert_eq!(resolved.auth_token.as_deref(), Some("openai-inline-token"));

        Ok(())
    }

    #[test]
    fn finds_config_in_parent_directory() -> Result<()> {
        let tempdir = TempDir::new()?;
        let nested_dir = tempdir.path().join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        let config_path = tempdir.path().join("why.local.toml");
        fs::write(&config_path, "[risk]\ndefault_level = \"LOW\"\n")?;

        assert_eq!(find_config_path(&nested_dir, None), Some(config_path));

        Ok(())
    }

    #[test]
    fn repo_search_root_uses_git_workdir_boundary() -> Result<()> {
        let tempdir = TempDir::new()?;
        let repo_root = tempdir.path().join("repo");
        let nested_dir = repo_root.join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(&repo_root)?;

        assert_eq!(config_search_root(&nested_dir), Some(repo_root));

        Ok(())
    }

    #[test]
    fn config_search_stops_at_repo_root() -> Result<()> {
        let _env = TestEnv::new()?;
        let tempdir = TempDir::new()?;
        let repo_root = tempdir.path().join("repo");
        let nested_dir = repo_root.join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(&repo_root)?;

        let outside_config = tempdir.path().join("why.local.toml");
        fs::write(&outside_config, "[risk]\ndefault_level = \"HIGH\"\n")?;

        assert_eq!(
            find_config_path(&nested_dir, Some(repo_root.as_path())),
            None
        );
        assert_eq!(
            load_config(&nested_dir)?.risk.default_level,
            DEFAULT_RISK_LEVEL
        );

        Ok(())
    }

    #[test]
    fn config_search_does_not_inherit_parent_config_outside_repo() -> Result<()> {
        let _env = TestEnv::new()?;
        let tempdir = TempDir::new()?;
        let outer_dir = tempdir.path().join("outer");
        let nested_dir = outer_dir.join("work/src");
        fs::create_dir_all(&nested_dir)?;

        fs::write(
            tempdir.path().join("why.local.toml"),
            "[risk]\ndefault_level = \"HIGH\"\n",
        )?;
        fs::write(
            outer_dir.join("why.local.toml"),
            "[risk]\ndefault_level = \"MEDIUM\"\n",
        )?;

        assert_eq!(
            load_config(&nested_dir)?.risk.default_level,
            DEFAULT_RISK_LEVEL
        );

        Ok(())
    }

    #[test]
    fn load_config_keeps_repo_local_config_visible() -> Result<()> {
        let _env = TestEnv::new()?;
        let tempdir = TempDir::new()?;
        let repo_root = tempdir.path().join("repo");
        let nested_dir = repo_root.join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(&repo_root)?;

        let repo_config = repo_root.join("why.local.toml");
        fs::write(&repo_config, "[risk]\ndefault_level = \"HIGH\"\n")?;
        fs::write(
            tempdir.path().join("why.local.toml"),
            "[risk]\ndefault_level = \"LOW\"\n",
        )?;

        assert_eq!(load_config(&nested_dir)?.risk.default_level, "HIGH");

        Ok(())
    }

    #[test]
    fn loads_global_config_with_xdg_first_semantics() -> Result<()> {
        let env = TestEnv::new()?;
        let global_path = env.xdg_config_home.join("why").join("why.toml");
        fs::create_dir_all(global_path.parent().expect("global parent"))?;
        fs::write(
            &global_path,
            r#"
[llm]
provider = "openai"
model = "gpt-5.4"
auth_token = "OPENAI_CONFIG_TOKEN"
"#,
        )?;

        let tempdir = TempDir::new()?;
        let config = load_config(tempdir.path())?;
        assert_eq!(config.llm.provider, LlmProvider::Openai);
        assert_eq!(config.llm.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(
            config.llm.auth_token.as_deref(),
            Some("OPENAI_CONFIG_TOKEN")
        );
        assert_eq!(global_config_path(), Some(global_path));

        Ok(())
    }

    #[test]
    fn global_config_falls_back_to_home_config_directory() -> Result<()> {
        let env = TestEnv::new_without_xdg()?;
        let global_path = env.home.join(".config/why/why.toml");
        fs::create_dir_all(global_path.parent().expect("home config parent"))?;
        fs::write(&global_path, "[risk]\ndefault_level = \"MEDIUM\"\n")?;

        let tempdir = TempDir::new()?;
        let config = load_config(tempdir.path())?;
        assert_eq!(config.risk.default_level, "MEDIUM");
        assert_eq!(global_config_path(), Some(global_path));

        Ok(())
    }

    #[test]
    fn repo_local_config_overrides_global_config_field_by_field() -> Result<()> {
        let env = TestEnv::new()?;
        let global_path = env.xdg_config_home.join("why").join("why.toml");
        fs::create_dir_all(global_path.parent().expect("global parent"))?;
        fs::write(
            &global_path,
            r#"
[risk]
default_level = "MEDIUM"

[llm]
provider = "openai"
model = "gpt-5.4"
auth_token = "openai-token"
retries = 4
timeout = 60
max_tokens = 900
"#,
        )?;

        let repo_root = env.tempdir.path().join("repo");
        let nested_dir = repo_root.join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(&repo_root)?;
        fs::write(
            repo_root.join("why.local.toml"),
            r#"
[llm]
provider = "anthropic"
"#,
        )?;

        let config = load_config(&nested_dir)?;
        let resolved = config.resolved_llm_config();
        assert_eq!(config.risk.default_level, "MEDIUM");
        assert_eq!(config.llm.provider, LlmProvider::Anthropic);
        assert_eq!(config.llm.model, None);
        assert_eq!(config.llm.auth_token, None);
        assert_eq!(config.llm.retries, 4);
        assert_eq!(config.llm.timeout, 60);
        assert_eq!(config.llm.max_tokens, 900);
        assert_eq!(resolved.model.as_deref(), Some(DEFAULT_ANTHROPIC_MODEL));
        assert_eq!(
            resolved.base_url.as_deref(),
            Some(DEFAULT_ANTHROPIC_BASE_URL)
        );
        assert_eq!(resolved.auth_token, None);

        Ok(())
    }

    #[test]
    fn local_config_target_path_prefers_existing_local_file() -> Result<()> {
        let _env = TestEnv::new()?;
        let repo_root = TempDir::new()?;
        let nested_dir = repo_root.path().join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(repo_root.path())?;
        let local_path = repo_root.path().join("why.local.toml");
        fs::write(&local_path, "[risk]\ndefault_level = \"HIGH\"\n")?;

        assert_eq!(local_config_target_path(&nested_dir), Some(local_path));

        Ok(())
    }

    #[test]
    fn local_config_target_path_defaults_to_repo_root() -> Result<()> {
        let _env = TestEnv::new()?;
        let repo_root = TempDir::new()?;
        let nested_dir = repo_root.path().join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(repo_root.path())?;

        assert_eq!(
            local_config_target_path(&nested_dir),
            Some(repo_root.path().join("why.local.toml"))
        );

        Ok(())
    }

    #[test]
    fn write_config_layer_round_trips() -> Result<()> {
        let _env = TestEnv::new()?;
        let tempdir = TempDir::new()?;
        let path = tempdir.path().join("nested/why.toml");
        let layer = super::WhyConfigLayer {
            llm: Some(super::LlmConfigLayer {
                provider: Some(LlmProvider::Custom),
                model: Some("custom-model".into()),
                base_url: Some("https://example.invalid/v1".into()),
                auth_token: Some("custom-token".into()),
                retries: Some(6),
                max_tokens: Some(777),
                timeout: Some(55),
            }),
            ..super::WhyConfigLayer::default()
        };

        write_config_layer_to_path(&path, &layer)?;
        let loaded = super::load_config_layer_from_path(&path)?;
        assert_eq!(loaded, layer);

        Ok(())
    }

    #[test]
    fn example_config_stays_in_sync_with_supported_surface() -> Result<()> {
        let _env = TestEnv::new()?;
        let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(".why.toml.example");
        let example_contents = fs::read_to_string(&config_path)?;
        let config = load_config_from_path(&config_path)?;

        assert!(example_contents.contains("[cache]"));
        assert!(example_contents.contains("[llm]"));
        assert!(!example_contents.contains("[llm.anthropic]"));
        assert!(!example_contents.contains("[llm.openai]"));
        assert!(!example_contents.contains("[llm.zai]"));
        assert!(!example_contents.contains("[llm.custom]"));
        assert_eq!(config.risk.default_level, DEFAULT_RISK_LEVEL);
        assert_eq!(
            config.risk.keywords.high,
            vec!["pci", "reconciliation", "ledger"]
        );
        assert_eq!(
            config.risk.keywords.medium,
            vec!["terraform", "webhook", "idempotency"]
        );
        assert_eq!(config.git.max_commits, DEFAULT_MAX_COMMITS);
        assert_eq!(config.git.recency_window_days, DEFAULT_RECENCY_WINDOW_DAYS);
        assert_eq!(
            config.git.mechanical_threshold_files,
            DEFAULT_MECHANICAL_THRESHOLD_FILES
        );
        assert_eq!(
            config.git.coupling_scan_commits,
            DEFAULT_COUPLING_SCAN_COMMITS
        );
        assert_eq!(
            config.git.coupling_ratio_threshold,
            DEFAULT_COUPLING_RATIO_THRESHOLD
        );
        assert_eq!(config.cache.max_entries, DEFAULT_CACHE_MAX_ENTRIES);
        assert_eq!(config.github.remote, "origin");
        assert_eq!(config.github.token, None);
        assert_eq!(config.llm.provider, LlmProvider::Anthropic);
        assert_eq!(config.llm.retries, DEFAULT_LLM_RETRIES);
        assert_eq!(config.llm.timeout, DEFAULT_LLM_TIMEOUT);
        assert_eq!(config.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(config.llm.model, None);
        assert_eq!(config.llm.base_url, None);
        assert_eq!(config.llm.auth_token, None);

        Ok(())
    }

    #[test]
    fn github_token_prefers_environment_over_config() -> Result<()> {
        let _env = TestEnv::new()?;
        let _guard = EnvGuard::set("GITHUB_TOKEN", Some("github_env_token"));
        let config = WhyConfig {
            github: super::GitHubConfig {
                remote: "origin".into(),
                token: Some("github_config_token".into()),
            },
            ..WhyConfig::default()
        };

        assert_eq!(config.github_token().as_deref(), Some("github_env_token"));
        Ok(())
    }

    #[test]
    fn github_token_falls_back_to_config_when_env_missing() -> Result<()> {
        let _env = TestEnv::new()?;
        let _guard = EnvGuard::set("GITHUB_TOKEN", None);
        let config = WhyConfig {
            github: super::GitHubConfig {
                remote: "origin".into(),
                token: Some("github_config_token".into()),
            },
            ..WhyConfig::default()
        };

        assert_eq!(
            config.github_token().as_deref(),
            Some("github_config_token")
        );
        Ok(())
    }

    #[test]
    fn github_token_ignores_empty_values() -> Result<()> {
        let _env = TestEnv::new()?;
        let _guard = EnvGuard::set("GITHUB_TOKEN", Some("   "));
        let config = WhyConfig {
            github: super::GitHubConfig {
                remote: "origin".into(),
                token: Some("   ".into()),
            },
            ..WhyConfig::default()
        };

        assert_eq!(config.github_token(), None);
        Ok(())
    }

    #[test]
    fn llm_credentials_prefer_provider_env_over_config_value() -> Result<()> {
        let _env = TestEnv::new()?;
        let _openai_env = EnvGuard::set("OPENAI_API_KEY", Some("openai_env_token"));
        let config = WhyConfig {
            llm: super::LlmConfig {
                provider: LlmProvider::Openai,
                model: Some("gpt-5.4".into()),
                auth_token: Some("openai_inline_token".into()),
                ..super::LlmConfig::default()
            },
            ..WhyConfig::default()
        };

        assert_eq!(
            config.resolved_llm_config().auth_token.as_deref(),
            Some("openai_env_token")
        );

        let _clear_openai = EnvGuard::set("OPENAI_API_KEY", None);
        assert_eq!(
            config.resolved_llm_config().auth_token.as_deref(),
            Some("openai_inline_token")
        );

        Ok(())
    }

    #[test]
    fn custom_llm_uses_config_value_and_ignores_blank_values() -> Result<()> {
        let _env = TestEnv::new()?;
        let config = WhyConfig {
            llm: super::LlmConfig {
                provider: LlmProvider::Custom,
                auth_token: Some("   ".into()),
                base_url: Some("https://example.invalid/v1".into()),
                ..super::LlmConfig::default()
            },
            ..WhyConfig::default()
        };

        let resolved = config.resolved_llm_config();
        assert_eq!(resolved.auth_token, None);
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://example.invalid/v1")
        );

        Ok(())
    }

    #[test]
    fn llm_config_debug_redacts_auth_token_values() {
        let config = WhyConfig {
            llm: super::LlmConfig {
                provider: LlmProvider::Anthropic,
                auth_token: Some("anthropic_debug_token".into()),
                base_url: Some("https://example.invalid/v1".into()),
                ..super::LlmConfig::default()
            },
            ..WhyConfig::default()
        };

        let debug = format!("{:?}", config.llm);
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("anthropic_debug_token"));
    }

    #[test]
    fn github_config_debug_redacts_token_values() {
        let config = WhyConfig {
            github: super::GitHubConfig {
                remote: "origin".into(),
                token: Some("github_debug_token".into()),
            },
            ..WhyConfig::default()
        };

        let debug = format!("{:?}", config.github);
        assert!(debug.contains("origin"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("github_debug_token"));
    }

    fn git_init(path: &Path) -> Result<()> {
        let output = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()?;
        if output.status.success() {
            return Ok(());
        }

        let fallback = Command::new("git").arg("init").current_dir(path).output()?;
        if fallback.status.success() {
            return Ok(());
        }

        anyhow::bail!(
            "git init failed: {}",
            String::from_utf8_lossy(&fallback.stderr)
        )
    }

    struct TestEnv {
        tempdir: TempDir,
        xdg_config_home: PathBuf,
        home: PathBuf,
        _xdg_guard: EnvGuard,
        _home_guard: EnvGuard,
        _anthropic_guard: EnvGuard,
        _openai_guard: EnvGuard,
        _zai_guard: EnvGuard,
        _github_guard: EnvGuard,
        _lock: MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn new() -> Result<Self> {
            let tempdir = TempDir::new()?;
            let xdg_config_home = tempdir.path().join("xdg");
            let home = tempdir.path().join("home");
            fs::create_dir_all(&xdg_config_home)?;
            fs::create_dir_all(&home)?;
            let lock = ENV_LOCK.lock().unwrap_or_else(|poison| {
                // Recover from poisoned lock by clearing the poison state
                poison.into_inner()
            });
            let xdg_value = xdg_config_home.to_string_lossy().into_owned();
            let home_value = home.to_string_lossy().into_owned();

            let xdg_guard = EnvGuard::set("XDG_CONFIG_HOME", Some(&xdg_value));
            let home_guard = EnvGuard::set("HOME", Some(&home_value));
            let anthropic_guard = EnvGuard::set("ANTHROPIC_API_KEY", None);
            let openai_guard = EnvGuard::set("OPENAI_API_KEY", None);
            let zai_guard = EnvGuard::set("ZAI_API_KEY", None);
            let github_guard = EnvGuard::set("GITHUB_TOKEN", None);

            Ok(Self {
                tempdir,
                xdg_config_home,
                home,
                _xdg_guard: xdg_guard,
                _home_guard: home_guard,
                _anthropic_guard: anthropic_guard,
                _openai_guard: openai_guard,
                _zai_guard: zai_guard,
                _github_guard: github_guard,
                _lock: lock,
            })
        }

        fn new_without_xdg() -> Result<Self> {
            let tempdir = TempDir::new()?;
            let xdg_config_home = tempdir.path().join("xdg");
            let home = tempdir.path().join("home");
            fs::create_dir_all(&xdg_config_home)?;
            fs::create_dir_all(&home)?;
            let lock = ENV_LOCK.lock().unwrap_or_else(|poison| {
                // Recover from poisoned lock by clearing the poison state
                poison.into_inner()
            });
            let home_value = home.to_string_lossy().into_owned();

            let xdg_guard = EnvGuard::set("XDG_CONFIG_HOME", None);
            let home_guard = EnvGuard::set("HOME", Some(&home_value));
            let anthropic_guard = EnvGuard::set("ANTHROPIC_API_KEY", None);
            let openai_guard = EnvGuard::set("OPENAI_API_KEY", None);
            let zai_guard = EnvGuard::set("ZAI_API_KEY", None);
            let github_guard = EnvGuard::set("GITHUB_TOKEN", None);

            Ok(Self {
                tempdir,
                xdg_config_home,
                home,
                _xdg_guard: xdg_guard,
                _home_guard: home_guard,
                _anthropic_guard: anthropic_guard,
                _openai_guard: openai_guard,
                _zai_guard: zai_guard,
                _github_guard: github_guard,
                _lock: lock,
            })
        }
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let previous = env::var(key).ok();
            match value {
                Some(value) => unsafe { env::set_var(key, value) },
                None => unsafe { env::remove_var(key) },
            }

            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.as_deref() {
                Some(value) => unsafe { env::set_var(self.key, value) },
                None => unsafe { env::remove_var(self.key) },
            }
        }
    }
}
