//! Common test utilities for integration tests.
//!
//! Provides fixture repo setup, CLI invocation helpers, and snapshot normalization.

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

pub struct FixtureRepo {
    _dir: TempDir,
    pub path: PathBuf,
}

#[allow(dead_code)]
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
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest_path = workspace_root.join("Cargo.toml");
        let mut command = if let Ok(why_binary) = std::env::var("CARGO_BIN_EXE_why") {
            let mut cmd = Command::new(why_binary);
            cmd.args(args);
            cmd.current_dir(&self.path);
            cmd
        } else {
            let mut cmd = Command::new("cargo");
            cmd.args(["run", "-q", "--manifest-path"]);
            cmd.arg(&manifest_path);
            cmd.args(["-p", "why-core", "--bin", "why", "--"]);
            cmd.args(args);
            cmd.current_dir(&self.path);
            cmd
        };

        let output = command
            .env("ANTHROPIC_API_KEY", "")
            .output()
            .context("failed to run why command")?;

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

    pub fn run_why_with_stdin(&self, args: &[&str], stdin: &str) -> Result<Output> {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest_path = workspace_root.join("Cargo.toml");
        let mut command = if let Ok(why_binary) = std::env::var("CARGO_BIN_EXE_why") {
            let mut cmd = Command::new(why_binary);
            cmd.args(args);
            cmd.current_dir(&self.path);
            cmd
        } else {
            let mut cmd = Command::new("cargo");
            cmd.args(["run", "-q", "--manifest-path"]);
            cmd.arg(&manifest_path);
            cmd.args(["-p", "why-core", "--bin", "why", "--"]);
            cmd.args(args);
            cmd.current_dir(&self.path);
            cmd
        };

        let mut child = command
            .env("ANTHROPIC_API_KEY", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn why command")?;

        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(stdin.as_bytes())
                .context("failed to write stdin to why command")?;
        }

        child.wait_with_output().context("failed to wait for why command")
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
        .with_context(|| {
            format!(
                "failed to run fixture setup script {}",
                fixture_root.display()
            )
        })?;

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

pub fn setup_typescript_repo() -> Result<FixtureRepo> {
    setup_fixture("typescript_repo")
}

pub fn setup_javascript_repo() -> Result<FixtureRepo> {
    setup_fixture("javascript_repo")
}

#[allow(dead_code)]
pub fn setup_coupling_repo() -> Result<FixtureRepo> {
    setup_fixture("coupling_repo")
}

#[allow(dead_code)]
pub fn setup_timebomb_repo() -> Result<FixtureRepo> {
    setup_fixture("timebomb_repo")
}

#[allow(dead_code)]
pub fn setup_ghost_repo() -> Result<FixtureRepo> {
    setup_fixture("ghost_repo")
}

#[allow(dead_code)]
pub fn setup_split_repo() -> Result<FixtureRepo> {
    setup_fixture("split_repo")
}

#[allow(dead_code)]
pub fn setup_python_repo() -> Result<FixtureRepo> {
    setup_fixture("python_repo")
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

#[allow(dead_code)]
pub fn normalize_terminal_snapshot(text: &str) -> String {
    text.lines()
        .map(|line| line.replace('\\', "/"))
        .map(|line| normalize_paths(&line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn normalize_paths(text: &str) -> String {
    text.replace("\\r\\n", "\n")
        .replace(env!("CARGO_MANIFEST_DIR"), "<repo>")
        .replace("/tmp/", "<tmp>/")
}
