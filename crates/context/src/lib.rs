//! Local code context extraction and heuristics.

use anyhow::{Context, Result};
use git2::Repository;
use serde::Deserialize;
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
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RiskConfig {
    #[serde(default = "default_risk_level")]
    pub default_level: String,
    #[serde(default)]
    pub keywords: RiskKeywords,
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

impl WhyConfig {
    pub fn github_token(&self) -> Option<String> {
        env::var("GITHUB_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
            .or_else(|| {
                self.github
                    .token
                    .clone()
                    .filter(|token| !token.trim().is_empty())
            })
    }
}

pub fn load_config(start_dir: &Path) -> Result<WhyConfig> {
    let search_root = config_search_root(start_dir);
    let Some(config_path) = find_config_path(start_dir, search_root.as_deref()) else {
        return Ok(WhyConfig::default());
    };

    load_config_from_path(&config_path)
}

pub fn load_config_from_path(config_path: &Path) -> Result<WhyConfig> {
    let contents = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config file {}", config_path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file {}", config_path.display()))
}

fn find_config_path(start_dir: &Path, search_root: Option<&Path>) -> Option<PathBuf> {
    let mut current = if start_dir.is_dir() {
        start_dir.to_path_buf()
    } else {
        start_dir.parent()?.to_path_buf()
    };
    let search_root = search_root.map(Path::to_path_buf);

    loop {
        let candidate = current.join(".why.toml");
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
    let anchor = if start_dir.is_dir() {
        start_dir
    } else {
        start_dir.parent()?
    };

    Some(
        Repository::discover(anchor)
            .ok()
            .and_then(|repo| repo.workdir().map(Path::to_path_buf))
            .unwrap_or_else(|| anchor.to_path_buf()),
    )
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

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CACHE_MAX_ENTRIES, DEFAULT_COUPLING_RATIO_THRESHOLD, DEFAULT_COUPLING_SCAN_COMMITS,
        DEFAULT_MAX_COMMITS, DEFAULT_MECHANICAL_THRESHOLD_FILES, DEFAULT_RECENCY_WINDOW_DAYS,
        DEFAULT_RISK_LEVEL, WhyConfig, config_search_root, find_config_path, load_config,
        load_config_from_path,
    };
    use anyhow::Result;
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn returns_defaults_when_config_is_missing() -> Result<()> {
        let tempdir = TempDir::new()?;
        let config = load_config(tempdir.path())?;

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
        assert!(config.risk.keywords.high.is_empty());
        assert!(config.risk.keywords.medium.is_empty());

        Ok(())
    }

    #[test]
    fn loads_risk_keywords_and_git_overrides_from_toml() -> Result<()> {
        let tempdir = TempDir::new()?;
        let config_path = tempdir.path().join(".why.toml");
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
"#,
            ),
        )?;

        let config = load_config_from_path(&config_path)?;

        assert_eq!(
            config,
            WhyConfig {
                risk: super::RiskConfig {
                    default_level: "medium".into(),
                    keywords: super::RiskKeywords {
                        high: vec!["pci".into(), "reconciliation".into()],
                        medium: vec!["terraform".into()],
                    },
                },
                git: super::GitConfig {
                    max_commits: 5,
                    recency_window_days: 30,
                    mechanical_threshold_files: 12,
                    coupling_scan_commits: 120,
                    coupling_ratio_threshold: 0.45,
                },
                cache: super::CacheConfig { max_entries: 42 },
                github: super::GitHubConfig {
                    remote: "upstream".into(),
                    token: Some("test-placeholder".into()),
                },
            }
        );

        Ok(())
    }

    #[test]
    fn finds_config_in_parent_directory() -> Result<()> {
        let tempdir = TempDir::new()?;
        let nested_dir = tempdir.path().join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        let config_path = tempdir.path().join(".why.toml");
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
        let tempdir = TempDir::new()?;
        let repo_root = tempdir.path().join("repo");
        let nested_dir = repo_root.join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(&repo_root)?;

        let outside_config = tempdir.path().join(".why.toml");
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
        let tempdir = TempDir::new()?;
        let outer_dir = tempdir.path().join("outer");
        let nested_dir = outer_dir.join("work/src");
        fs::create_dir_all(&nested_dir)?;

        fs::write(
            tempdir.path().join(".why.toml"),
            "[risk]\ndefault_level = \"HIGH\"\n",
        )?;
        fs::write(
            outer_dir.join(".why.toml"),
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
        let tempdir = TempDir::new()?;
        let repo_root = tempdir.path().join("repo");
        let nested_dir = repo_root.join("src/lib");
        fs::create_dir_all(&nested_dir)?;
        git_init(&repo_root)?;

        let repo_config = repo_root.join(".why.toml");
        fs::write(&repo_config, "[risk]\ndefault_level = \"HIGH\"\n")?;
        fs::write(
            tempdir.path().join(".why.toml"),
            "[risk]\ndefault_level = \"LOW\"\n",
        )?;

        assert_eq!(load_config(&nested_dir)?.risk.default_level, "HIGH");

        Ok(())
    }

    #[test]
    fn example_config_stays_in_sync_with_supported_surface() -> Result<()> {
        let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(".why.toml.example");
        let example_contents = fs::read_to_string(&config_path)?;
        let config = load_config_from_path(&config_path)?;

        assert!(example_contents.contains("[cache]"));
        assert!(example_contents.contains("max_entries"));
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

        Ok(())
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

    #[test]
    fn github_token_prefers_environment_over_config() {
        let _guard = EnvGuard::set("GITHUB_TOKEN", Some("github_env_token"));
        let config = WhyConfig {
            github: super::GitHubConfig {
                remote: "origin".into(),
                token: Some("github_config_token".into()),
            },
            ..WhyConfig::default()
        };

        assert_eq!(config.github_token().as_deref(), Some("github_env_token"));
    }

    #[test]
    fn github_token_falls_back_to_config_when_env_missing() {
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
    }

    #[test]
    fn github_token_ignores_empty_values() {
        let _guard = EnvGuard::set("GITHUB_TOKEN", Some("   "));
        let config = WhyConfig {
            github: super::GitHubConfig {
                remote: "origin".into(),
                token: Some("   ".into()),
            },
            ..WhyConfig::default()
        };

        assert_eq!(config.github_token(), None);
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
