//! Common test utilities for integration tests.
//!
//! Provides fixture repo setup, CLI invocation helpers, and snapshot normalization.

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
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
        let mut command = Command::new(why_binary_path()?);
        command.args(args);
        command.current_dir(&self.path);

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
        let mut child = self.spawn_why(args)?;

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
        let mut command = Command::new(why_binary_path()?);
        command.args(args);
        command.current_dir(&self.path);

        command
            .env("ANTHROPIC_API_KEY", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|mut child| {
                if let Some(stdout) = child.stdout.as_mut() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::FileTypeExt;
                        let _ = stdout;
                    }
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
