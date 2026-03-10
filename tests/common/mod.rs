//! Common test utilities for integration tests.
//!
//! Provides fixture repo setup, CLI invocation helpers, and snapshot normalization.

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

pub struct FixtureRepo {
    _dir: TempDir,
    pub path: PathBuf,
}

impl FixtureRepo {
    pub fn run_command(&self, program: &str, args: &[&str]) -> Result<Output> {
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.path)
            .output()
            .with_context(|| format!("failed to run {program}"))?;

        Ok(output)
    }

    pub fn run_why(&self, args: &[&str]) -> Result<Output> {
        let why_binary = std::env::var("WHY_BINARY")
            .unwrap_or_else(|_| "target/debug/why".to_string());

        let output = Command::new(&why_binary)
            .args(args)
            .current_dir(&self.path)
            .env("ANTHROPIC_API_KEY", "")
            .output()
            .with_context(|| format!("failed to run why binary at {why_binary}"))?;

        Ok(output)
    }

    pub fn run_why_json<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let mut all_args = args.to_vec();
        if !all_args.contains(&"--json") {
            all_args.push("--json");
        }

        let output = self.run_why(&all_args)?;
        ensure_success(&output)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        serde_json::from_str(&stdout)
            .map_err(|err| anyhow!("failed to parse JSON output: {err}\nstdout:\n{stdout}"))
    }

    pub fn stdout(&self, output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub fn stderr(&self, output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }
}

pub fn setup_fixture(name: &str) -> Result<FixtureRepo> {
    let dir = TempDir::new().context("failed to create tempdir for fixture repo")?;
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
        .join("setup.sh");

    if !fixture_root.exists() {
        bail!("fixture script not found: {}", fixture_root.display());
    }

    let output = Command::new("bash")
        .arg(&fixture_root)
        .arg(dir.path())
        .output()
        .with_context(|| format!("failed to run fixture setup script {}", fixture_root.display()))?;

    if !output.status.success() {
        bail!(
            "fixture setup failed for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(FixtureRepo {
        path: dir.path().to_path_buf(),
        _dir: dir,
    })
}

pub fn setup_hotfix_repo() -> Result<FixtureRepo> {
    setup_fixture("hotfix_repo")
}

pub fn setup_compat_shim_repo() -> Result<FixtureRepo> {
    setup_fixture("compat_shim_repo")
}

pub fn setup_sparse_repo() -> Result<FixtureRepo> {
    setup_fixture("sparse_repo")
}

pub fn setup_coupling_repo() -> Result<FixtureRepo> {
    setup_fixture("coupling_repo")
}

pub fn setup_timebomb_repo() -> Result<FixtureRepo> {
    setup_fixture("timebomb_repo")
}

pub fn setup_ghost_repo() -> Result<FixtureRepo> {
    setup_fixture("ghost_repo")
}

pub fn setup_split_repo() -> Result<FixtureRepo> {
    setup_fixture("split_repo")
}

pub fn ensure_success(output: &Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }

    bail!(
        "command failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn normalize_terminal_snapshot(text: &str) -> String {
    text.lines()
        .map(|line| line.replace('\\', "/"))
        .map(|line| normalize_paths(&line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn normalize_json_snapshot(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut normalized = serde_json::Map::new();
            for (key, val) in map {
                match key.as_str() {
                    "elapsed_ms" | "duration_ms" | "timestamp" | "generated_at" | "cache_key" => {}
                    _ => {
                        normalized.insert(key.clone(), normalize_json_snapshot(val));
                    }
                }
            }
            Value::Object(normalized)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize_json_snapshot).collect()),
        Value::String(text) => Value::String(normalize_paths(text)),
        other => other.clone(),
    }
}

fn normalize_paths(text: &str) -> String {
    text.replace("\\r\\n", "\n")
        .replace(env!("CARGO_MANIFEST_DIR"), "<repo>")
        .replace("/tmp/", "<tmp>/")
}
