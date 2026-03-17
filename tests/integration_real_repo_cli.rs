mod common;

use anyhow::{Context, Result, anyhow, bail};
use common::ensure_success;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use tempfile::TempDir;

static WHY_BINARY: Mutex<Option<PathBuf>> = Mutex::new(None);
const REAL_REPO_ENV: &str = "WHY_REAL_REPO_PATH";

struct RealRepoFixture {
    _repo_dir: TempDir,
    path: PathBuf,
}

fn why_binary_path() -> Result<PathBuf> {
    let mut cached = WHY_BINARY
        .lock()
        .map_err(|_| anyhow!("why binary cache lock poisoned"))?;
    if let Some(path) = cached.as_ref() {
        return Ok(path.clone());
    }

    let path = if let Ok(why_binary) = std::env::var("CARGO_BIN_EXE_why") {
        PathBuf::from(why_binary)
    } else {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest_path = workspace_root.join("Cargo.toml");
        let target_dir = workspace_root.join("target").join("integration-bin");
        let status = Command::new("cargo")
            .env("CARGO_TARGET_DIR", &target_dir)
            .args(["build", "-q", "--manifest-path"])
            .arg(&manifest_path)
            .args(["-p", "why-core", "--bin", "why"])
            .status()
            .context("failed to build why binary for integration tests")?;
        if !status.success() {
            bail!(
                "building why binary for integration tests failed with status {:?}",
                status.code()
            );
        }

        let exe_name = if cfg!(windows) { "why.exe" } else { "why" };
        target_dir.join("debug").join(exe_name)
    };

    *cached = Some(path.clone());
    Ok(path)
}

fn setup_real_repo_from_env() -> Result<Option<RealRepoFixture>> {
    match std::env::var(REAL_REPO_ENV) {
        Ok(path) if !path.trim().is_empty() => setup_real_repo(Path::new(&path)).map(Some),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("environment variable {REAL_REPO_ENV} is not valid UTF-8")
        }
    }
}

fn setup_real_repo(path: &Path) -> Result<RealRepoFixture> {
    if !path.exists() {
        bail!("real repo path does not exist: {}", path.display());
    }
    if !path.is_dir() {
        bail!("real repo path is not a directory: {}", path.display());
    }
    if !path.join(".git").exists() {
        bail!("real repo path is not a git repository: {}", path.display());
    }

    let repo_dir = TempDir::new().context("failed to create tempdir for real repo clone")?;
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(path)
        .arg(repo_dir.path())
        .output()
        .with_context(|| format!("failed to clone real repo {}", path.display()))?;
    ensure_success(&output)?;

    let path = repo_dir.path().to_path_buf();
    Ok(RealRepoFixture {
        _repo_dir: repo_dir,
        path,
    })
}

fn run_why(repo: &RealRepoFixture, args: &[&str]) -> Result<Output> {
    let output = Command::new(why_binary_path()?)
        .args(args)
        .current_dir(&repo.path)
        .env("ANTHROPIC_API_KEY", "")
        .env("OPENAI_API_KEY", "")
        .env("ZAI_API_KEY", "")
        .env("CUSTOM_API_KEY", "")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME")
        .output()
        .context("failed to run why command against real repo")?;
    Ok(output)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn repo_relative_path<'a>(repo_root: &Path, target: &'a Path) -> Result<&'a Path> {
    target.strip_prefix(repo_root).map_err(|_| {
        anyhow!(
            "target path {} is not inside repo {}",
            target.display(),
            repo_root.display()
        )
    })
}

fn first_existing_target(repo_root: &Path) -> Result<PathBuf> {
    let candidates = [
        repo_root.join("crates/core/src/cli.rs"),
        repo_root.join("crates/core/src/main.rs"),
        repo_root.join("README.md"),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| anyhow!("real repo test requires a known target file"))
}

#[test]
fn query_json_shape_is_stable_when_enabled() -> Result<()> {
    let Some(repo) = setup_real_repo_from_env()? else {
        return Ok(());
    };

    let target_path = first_existing_target(&repo.path)?;
    let relative_path = repo_relative_path(&repo.path, &target_path)?;
    let relative_path_display = relative_path.display().to_string();
    let target = format!("{}:Cli::parse_mode", relative_path_display);

    let output = run_why(&repo, &[&target, "--json", "--no-llm", "--no-cache"])?;
    ensure_success(&output)?;

    let parsed: Value =
        serde_json::from_str(&stdout(&output)).context("real repo query should emit JSON")?;
    assert_eq!(parsed["mode"], "heuristic");
    assert!(parsed["summary"].as_str().is_some_and(|summary| {
        summary.contains(&relative_path_display) && summary.contains("Cli::parse_mode")
    }));
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(parsed["notes"].as_array().is_some_and(|items| {
        items.iter().any(|note| {
            note.as_str()
                .is_some_and(|text| text.contains("No LLM synthesis"))
        })
    }));

    Ok(())
}

#[test]
fn health_and_hotspots_work_when_enabled() -> Result<()> {
    let Some(repo) = setup_real_repo_from_env()? else {
        return Ok(());
    };

    let health_output = run_why(&repo, &["health", "--json"])?;
    ensure_success(&health_output)?;
    let health: Value = serde_json::from_str(&stdout(&health_output))
        .context("real repo health should emit JSON")?;
    assert!(health["debt_score"].is_number());
    assert!(health["signals"].is_object());
    assert!(
        health["notes"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let cache_path = repo.path.join(".why").join("cache.json");
    assert!(cache_path.exists());

    let hotspots_output = run_why(&repo, &["hotspots", "--limit", "5", "--json"])?;
    ensure_success(&hotspots_output)?;
    let hotspots: Value = serde_json::from_str(&stdout(&hotspots_output))
        .context("real repo hotspots should emit JSON")?;
    let findings = hotspots
        .as_array()
        .ok_or_else(|| anyhow!("hotspots output should be an array for a real repo"))?;
    assert!(!findings.is_empty());
    assert!(
        findings[0]["path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    assert!(
        findings[0]["risk_level"]
            .as_str()
            .is_some_and(|risk| !risk.is_empty())
    );
    assert!(findings[0]["owners"].is_array());

    Ok(())
}
