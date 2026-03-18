//! Common test utilities for integration tests.
//!
//! Provides fixture repo setup, CLI invocation helpers, and snapshot normalization.

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

static WHY_BINARY: Mutex<Option<PathBuf>> = Mutex::new(None);

pub struct FixtureRepo {
    _repo_dir: Option<TempDir>,
    pub path: PathBuf,
}

#[allow(dead_code)]
impl FixtureRepo {
    pub fn temp_home(&self) -> PathBuf {
        self.path.join(".home")
    }

    pub fn temp_xdg_config_home(&self) -> PathBuf {
        self.path.join(".config-home")
    }

    pub fn global_config_path(&self) -> PathBuf {
        self.temp_xdg_config_home().join("why").join("why.toml")
    }

    pub fn local_config_path(&self) -> PathBuf {
        self.path.join("why.local.toml")
    }

    pub fn run_command(&self, program: &str, args: &[&str]) -> Result<Output> {
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.path)
            .output()
            .with_context(|| format!("failed to run {program}"))?;

        Ok(output)
    }

    pub fn run_why(&self, args: &[&str]) -> Result<Output> {
        self.run_why_with_env(args, &[])
    }

    pub fn run_why_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Result<Output> {
        let mut command = Command::new(why_binary_path()?);
        command.args(args);
        command.current_dir(&self.path);
        apply_why_test_env(&mut command, envs);

        let output = command.output().context("failed to run why command")?;

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
        self.run_why_with_stdin_and_env(args, stdin, &[])
    }

    pub fn run_why_with_stdin_and_env(
        &self,
        args: &[&str],
        stdin: &str,
        envs: &[(&str, &str)],
    ) -> Result<Output> {
        let mut child = self.spawn_why_with_env(args, envs)?;

        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(stdin.as_bytes())
                .context("failed to write stdin to why command")?;
        }

        child
            .wait_with_output()
            .context("failed to wait for why command")
    }

    pub fn spawn_why(&self, args: &[&str]) -> Result<Child> {
        self.spawn_why_with_env(args, &[])
    }

    pub fn spawn_why_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Result<Child> {
        let mut command = Command::new(why_binary_path()?);
        command.args(args);
        command.current_dir(&self.path);
        apply_why_test_env(&mut command, envs);

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|mut child| {
                if let Some(stdout) = child.stdout.as_mut() {
                    let _ = stdout;
                }
                child
            })
            .context("failed to spawn why command")
    }

    pub fn stdout(&self, output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub fn stderr(&self, output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }
}

fn apply_why_test_env(command: &mut Command, envs: &[(&str, &str)]) {
    command
        .env("ANTHROPIC_API_KEY", "")
        .env("OPENAI_API_KEY", "")
        .env("ZAI_API_KEY", "")
        .env("CUSTOM_API_KEY", "")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME");

    for (key, value) in envs {
        command.env(key, value);
    }
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

#[cfg(windows)]
fn windows_bash_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(program_files) = env::var_os("ProgramFiles") {
        let base = PathBuf::from(program_files).join("Git");
        candidates.push(base.join("bin").join("bash.exe"));
        candidates.push(base.join("usr").join("bin").join("bash.exe"));
    }

    if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
        let base = PathBuf::from(program_files_x86).join("Git");
        candidates.push(base.join("bin").join("bash.exe"));
        candidates.push(base.join("usr").join("bin").join("bash.exe"));
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn run_fixture_setup_script(fixture_root: &Path, repo_root: &Path) -> Result<Output> {
    #[cfg(windows)]
    let mut command = Command::new(windows_bash_path().unwrap_or_else(|| PathBuf::from("bash")));

    #[cfg(not(windows))]
    let mut command = Command::new("bash");

    #[cfg(windows)]
    {
        command
            .arg("-lc")
            .arg(
                "script=\"$(cygpath -u \"$1\")\"; repo_root=\"$(cygpath -u \"$2\")\"; exec \"$script\" \"$repo_root\"",
            )
            .arg("--")
            .arg(fixture_root)
            .arg(repo_root);
    }

    #[cfg(not(windows))]
    {
        command.arg(fixture_root).arg(repo_root);
    }

    command
        .env_remove("WHY_BENCH_HISTORY_COMMITS")
        .env_remove("WHY_BENCH_EXTRA_FILES")
        .output()
        .with_context(|| {
            format!(
                "failed to run fixture setup script {}",
                fixture_root.display()
            )
        })
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

    let output = run_fixture_setup_script(&fixture_root, dir.path())?;

    if !output.status.success() {
        bail!(
            "fixture setup failed for {name} with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(FixtureRepo {
        path: dir.path().to_path_buf(),
        _repo_dir: Some(dir),
    })
}

#[allow(dead_code)]
pub fn setup_real_repo(path: impl AsRef<Path>) -> Result<FixtureRepo> {
    let path = path.as_ref();
    if !path.exists() {
        bail!("real repo path does not exist: {}", path.display());
    }
    if !path.is_dir() {
        bail!("real repo path is not a directory: {}", path.display());
    }
    if !path.join(".git").exists() {
        bail!("real repo path is not a git repository: {}", path.display());
    }

    let dir = TempDir::new().context("failed to create tempdir for real repo clone")?;
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(path)
        .arg(dir.path())
        .output()
        .with_context(|| format!("failed to clone real repo {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "failed to clone real repo {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(FixtureRepo {
        path: dir.path().to_path_buf(),
        _repo_dir: Some(dir),
    })
}

#[allow(dead_code)]
pub fn setup_real_repo_from_env(var_name: &str) -> Result<Option<FixtureRepo>> {
    match std::env::var(var_name) {
        Ok(path) if !path.trim().is_empty() => setup_real_repo(path).map(Some),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("environment variable {var_name} is not valid UTF-8")
        }
    }
}

#[allow(dead_code)]
pub fn setup_hotfix_repo() -> Result<FixtureRepo> {
    setup_fixture("hotfix_repo")
}

#[allow(dead_code)]
pub fn setup_compat_shim_repo() -> Result<FixtureRepo> {
    setup_fixture("compat_shim_repo")
}

#[allow(dead_code)]
pub fn setup_sparse_repo() -> Result<FixtureRepo> {
    setup_fixture("sparse_repo")
}

#[allow(dead_code)]
pub fn setup_typescript_repo() -> Result<FixtureRepo> {
    setup_fixture("typescript_repo")
}

#[allow(dead_code)]
pub fn setup_javascript_repo() -> Result<FixtureRepo> {
    setup_fixture("javascript_repo")
}

#[allow(dead_code)]
pub fn setup_coupling_repo() -> Result<FixtureRepo> {
    setup_fixture("coupling_repo")
}

#[allow(dead_code)]
pub fn setup_coupling_rich_repo() -> Result<FixtureRepo> {
    setup_fixture("coupling_rich_repo")
}

#[allow(dead_code)]
pub fn setup_timebomb_repo() -> Result<FixtureRepo> {
    setup_fixture("timebomb_repo")
}

#[allow(dead_code)]
pub fn setup_timebomb_rich_repo() -> Result<FixtureRepo> {
    setup_fixture("timebomb_rich_repo")
}

#[allow(dead_code)]
pub fn setup_ghost_repo() -> Result<FixtureRepo> {
    setup_fixture("ghost_repo")
}

#[allow(dead_code)]
pub fn setup_rename_safe_repo() -> Result<FixtureRepo> {
    setup_fixture("rename_safe_repo")
}

#[allow(dead_code)]
pub fn setup_outage_repo() -> Result<FixtureRepo> {
    setup_fixture("outage_repo")
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
pub fn wait_for_child_stdout(child: &mut Child, needle: &str, timeout: Duration) -> Result<String> {
    let start = Instant::now();
    let stdout = child
        .stdout
        .as_mut()
        .ok_or_else(|| anyhow!("spawned child should expose stdout"))?;
    let mut buffer = String::new();
    let mut chunk = [0u8; 1024];

    while start.elapsed() < timeout {
        match stdout.read(&mut chunk) {
            Ok(0) => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(read) => {
                buffer.push_str(&String::from_utf8_lossy(&chunk[..read]));
                if buffer.contains(needle) {
                    return Ok(buffer);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error).context("failed to read child stdout"),
        }
    }

    bail!(
        "timed out waiting for child stdout to contain {:?}. output so far:\n{}",
        needle,
        buffer
    )
}

#[allow(dead_code)]
pub fn assert_terminal_golden(name: &str, text: &str) -> Result<()> {
    let actual = normalize_terminal_snapshot(text);
    let golden_path = golden_path(name, "txt");
    let expected = fs::read_to_string(&golden_path)
        .with_context(|| format!("failed to read terminal golden {}", golden_path.display()))?;
    let actual = actual.trim_end().to_string();
    let expected = normalize_terminal_snapshot(&expected)
        .trim_end()
        .to_string();

    if actual == expected {
        return Ok(());
    }

    bail!(
        "terminal golden mismatch for {}\nexpected ({}):\n{}\n\nactual:\n{}",
        name,
        golden_path.display(),
        expected,
        actual
    )
}

#[allow(dead_code)]
pub fn assert_json_golden(name: &str, value: &Value) -> Result<()> {
    let actual = normalize_json_snapshot(value);
    let golden_path = golden_path(name, "json");
    let expected_text = fs::read_to_string(&golden_path)
        .with_context(|| format!("failed to read JSON golden {}", golden_path.display()))?;
    let expected: Value = serde_json::from_str(&expected_text).with_context(|| {
        format!(
            "failed to parse JSON golden {} as JSON",
            golden_path.display()
        )
    })?;

    if actual == expected {
        return Ok(());
    }

    bail!(
        "JSON golden mismatch for {}\nexpected ({}):\n{}\n\nactual:\n{}",
        name,
        golden_path.display(),
        serde_json::to_string_pretty(&expected)?,
        serde_json::to_string_pretty(&actual)?
    )
}

#[allow(dead_code)]
pub fn normalize_terminal_snapshot(text: &str) -> String {
    strip_osc8_hyperlinks(text)
        .lines()
        .map(|line| line.replace('\\', "/"))
        .map(|line| normalize_paths(&line))
        .map(|line| normalize_dynamic_text(&line))
        .map(|line| normalize_terminal_line(&line))
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
                    "elapsed_ms"
                    | "duration_ms"
                    | "timestamp"
                    | "generated_at"
                    | "cache_key"
                    | "oid"
                    | "short_oid"
                    | "dominant_commit_oid"
                    | "dominant_commit_short_oid"
                    | "time"
                    | "date"
                    | "relevance_score" => {}
                    _ => {
                        normalized.insert(key.clone(), normalize_json_snapshot(val));
                    }
                }
            }
            Value::Object(normalized)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize_json_snapshot).collect()),
        Value::String(text) => Value::String(normalize_dynamic_text(&normalize_paths(text))),
        other => other.clone(),
    }
}

#[allow(dead_code)]
fn golden_path(name: &str, extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.{extension}"))
}

#[allow(dead_code)]
fn normalize_terminal_line(line: &str) -> String {
    let mut normalized = String::new();
    let mut segment = String::new();

    for ch in line.chars() {
        if ch.is_whitespace() {
            if !segment.is_empty() {
                if is_hex_segment(&segment) {
                    normalized.push_str("<oid>");
                } else {
                    normalized.push_str(&segment);
                }
                segment.clear();
            }
            normalized.push(ch);
        } else {
            segment.push(ch);
        }
    }

    if !segment.is_empty() {
        if is_hex_segment(&segment) {
            normalized.push_str("<oid>");
        } else {
            normalized.push_str(&segment);
        }
    }

    normalized
}

#[allow(dead_code)]
fn strip_osc8_hyperlinks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remainder = text;
    let open = "\u{1b}]8;;";
    let close = "\u{1b}]8;;\u{1b}\\";
    let terminator = "\u{1b}\\";

    while let Some(start) = remainder.find(open) {
        result.push_str(&remainder[..start]);
        remainder = &remainder[start + open.len()..];

        let Some(url_end) = remainder.find(terminator) else {
            result.push_str(open);
            result.push_str(remainder);
            return result;
        };
        remainder = &remainder[url_end + terminator.len()..];

        let Some(label_end) = remainder.find(close) else {
            result.push_str(remainder);
            return result;
        };
        result.push_str(&remainder[..label_end]);
        remainder = &remainder[label_end + close.len()..];
    }

    result.push_str(remainder);
    result
}

#[allow(dead_code)]
fn is_hex_segment(segment: &str) -> bool {
    let trimmed = segment.trim_matches(|c: char| c == '—' || c == ',' || c == '(' || c == ')');
    (7..=40).contains(&trimmed.len()) && trimmed.chars().all(|c| c.is_ascii_hexdigit())
}

#[allow(dead_code)]
fn normalize_paths(text: &str) -> String {
    text.replace("\\r\\n", "\n")
        .replace(env!("CARGO_MANIFEST_DIR"), "<repo>")
        .replace("/tmp/", "<tmp>/")
}

#[allow(dead_code)]
fn normalize_dynamic_text(text: &str) -> String {
    let text = text.replace("Ã", "×").replace("â", "—");
    let bytes = text.as_bytes();
    let mut normalized = String::with_capacity(text.len());
    let mut index = 0;

    while index < bytes.len() {
        if index + 10 <= bytes.len()
            && bytes[index].is_ascii_digit()
            && bytes[index + 1].is_ascii_digit()
            && bytes[index + 2].is_ascii_digit()
            && bytes[index + 3].is_ascii_digit()
            && bytes[index + 4] == b'-'
            && bytes[index + 5].is_ascii_digit()
            && bytes[index + 6].is_ascii_digit()
            && bytes[index + 7] == b'-'
            && bytes[index + 8].is_ascii_digit()
            && bytes[index + 9].is_ascii_digit()
        {
            normalized.push_str("<date>");
            index += 10;
            continue;
        }

        normalized.push(bytes[index] as char);
        index += 1;
    }

    normalized
}
