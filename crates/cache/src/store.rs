use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::io::Write;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const CACHE_DIR_NAME: &str = ".why";
const CACHE_ENTRIES_FILE_NAME: &str = "cache.jsonl";
const HEALTH_SNAPSHOTS_FILE_NAME: &str = "health.jsonl";
const HEALTH_SNAPSHOT_LIMIT: usize = 52;
const MAX_CACHE_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheEntry {
    pub key: String,
    pub report: Value,
    pub created_at: i64,
    pub head_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthSnapshot {
    pub timestamp: i64,
    pub debt_score: u32,
    #[serde(alias = "details")]
    pub signals: HashMap<String, u32>,
    #[serde(default)]
    pub head_hash: Option<String>,
    #[serde(default)]
    pub ref_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheFile {
    #[serde(default)]
    entries: Vec<CacheEntry>,
    #[serde(default)]
    health_snapshots: Vec<HealthSnapshot>,
}

#[derive(Debug, Clone)]
pub struct Cache {
    entries_path: PathBuf,
    health_path: PathBuf,
    max_entries: usize,
    data: CacheFile,
}

pub fn runtime_dir(repo_root: &Path) -> Result<PathBuf> {
    let dir = repo_root.join(CACHE_DIR_NAME);
    ensure_cache_dir(&dir)?;
    ensure_gitignore_contains_cache_dir(repo_root)?;
    Ok(dir)
}

impl Cache {
    pub fn open(repo_root: &Path, max_entries: usize) -> Result<Self> {
        let dir = runtime_dir(repo_root)?;

        let entries_path = dir.join(CACHE_ENTRIES_FILE_NAME);
        let health_path = dir.join(HEALTH_SNAPSHOTS_FILE_NAME);
        let data = read_cache_state(&entries_path, &health_path)?;

        Ok(Self {
            entries_path,
            health_path,
            max_entries,
            data,
        })
    }

    pub fn make_key(relative_file_path: &str, symbol_or_line: &str, head_hash: &str) -> String {
        let head_prefix: String = head_hash.chars().take(12).collect();
        format!("{}:{}:{}", relative_file_path, symbol_or_line, head_prefix)
    }

    pub fn get<T>(&self, key: &str) -> Option<T>
    where
        T: DeserializeOwned,
    {
        self.data
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| serde_json::from_value(entry.report.clone()).ok())
    }

    pub fn get_entry(&self, key: &str) -> Option<&CacheEntry> {
        self.data.entries.iter().find(|entry| entry.key == key)
    }

    pub fn set<T>(&mut self, key: String, report: T, head_hash: &str) -> Result<()>
    where
        T: Serialize,
    {
        let report = serde_json::to_value(report).context("failed to serialize cache report")?;
        let entry = CacheEntry {
            key: key.clone(),
            report,
            created_at: now_ts(),
            head_hash: head_hash.to_string(),
        };

        self.data.entries.retain(|existing| existing.key != key);
        self.data.entries.push(entry);
        self.enforce_entry_limit();
        self.persist()
    }

    pub fn insert_health_snapshot(&mut self, snapshot: HealthSnapshot) -> Result<()> {
        self.data.health_snapshots.push(snapshot);
        self.data
            .health_snapshots
            .sort_by_key(|snapshot| snapshot.timestamp);

        if self.data.health_snapshots.len() > HEALTH_SNAPSHOT_LIMIT {
            let overflow = self.data.health_snapshots.len() - HEALTH_SNAPSHOT_LIMIT;
            self.data.health_snapshots.drain(0..overflow);
        }

        self.persist()
    }

    pub fn health_snapshots(&self) -> &[HealthSnapshot] {
        &self.data.health_snapshots
    }

    fn enforce_entry_limit(&mut self) {
        if self.max_entries == 0 {
            self.data.entries.clear();
            return;
        }

        self.data.entries.sort_by_key(|entry| entry.created_at);
        if self.data.entries.len() > self.max_entries {
            let overflow = self.data.entries.len() - self.max_entries;
            self.data.entries.drain(0..overflow);
        }
    }

    fn persist(&self) -> Result<()> {
        let entries_parent = self
            .entries_path
            .parent()
            .context("entries path has no parent directory")?;
        ensure_cache_dir(entries_parent)?;
        let health_parent = self
            .health_path
            .parent()
            .context("health path has no parent directory")?;
        ensure_cache_dir(health_parent)?;

        persist_cache_entries(&self.entries_path, &self.data.entries)?;
        persist_health_snapshots(&self.health_path, &self.data.health_snapshots)?;
        Ok(())
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn read_cache_state(entries_path: &Path, health_path: &Path) -> Result<CacheFile> {
    let entries_metadata = safe_cache_file_metadata(entries_path)?;
    let health_metadata = safe_cache_file_metadata(health_path)?;
    let entries = match entries_metadata.as_ref() {
        Some(metadata) => read_cache_entries(entries_path, metadata.len())?,
        None => Vec::new(),
    };
    let health_snapshots = match health_metadata.as_ref() {
        Some(metadata) => read_health_snapshots(health_path, metadata.len())?,
        None => Vec::new(),
    };

    Ok(CacheFile {
        entries,
        health_snapshots,
    })
}

fn read_cache_entries(path: &Path, file_len: u64) -> Result<Vec<CacheEntry>> {
    if file_len == 0 {
        return Ok(Vec::new());
    }
    if file_len > MAX_CACHE_FILE_BYTES {
        bail!(
            "cache file {} exceeds the {} byte safety limit",
            path.display(),
            MAX_CACHE_FILE_BYTES
        );
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read cache file {}", path.display()))?;
    let mut entries = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<CacheEntry>(trimmed).with_context(|| {
            format!(
                "failed to parse cache entry {} in {}",
                index + 1,
                path.display()
            )
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

fn read_health_snapshots(path: &Path, file_len: u64) -> Result<Vec<HealthSnapshot>> {
    if file_len == 0 {
        return Ok(Vec::new());
    }
    if file_len > MAX_CACHE_FILE_BYTES {
        bail!(
            "cache file {} exceeds the {} byte safety limit",
            path.display(),
            MAX_CACHE_FILE_BYTES
        );
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read cache file {}", path.display()))?;
    let mut snapshots = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let snapshot = serde_json::from_str::<HealthSnapshot>(trimmed).with_context(|| {
            format!(
                "failed to parse health snapshot {} in {}",
                index + 1,
                path.display()
            )
        })?;
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}

fn ensure_cache_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                bail!("cache directory {} must not be a symlink", path.display());
            }
            if !metadata.is_dir() {
                bail!("cache directory {} is not a directory", path.display());
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("failed to create cache directory {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect cache directory {}", path.display()));
        }
    }

    set_owner_only_permissions(path, 0o700)
}

fn ensure_gitignore_contains_cache_dir(repo_root: &Path) -> Result<()> {
    let gitignore_path = repo_root.join(".gitignore");
    let contents = match fs::read_to_string(&gitignore_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read gitignore {}", gitignore_path.display()));
        }
    };

    if contents.lines().any(|line| line.trim() == CACHE_DIR_NAME) {
        return Ok(());
    }

    let mut updated = contents;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(CACHE_DIR_NAME);
    updated.push('\n');
    fs::write(&gitignore_path, updated)
        .with_context(|| format!("failed to update gitignore {}", gitignore_path.display()))
}

fn safe_cache_file_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                bail!("cache file {} must not be a symlink", path.display());
            }
            if !metadata.is_file() {
                bail!("cache file {} is not a regular file", path.display());
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to inspect cache file {}", path.display()))
        }
    }
}

fn persist_cache_entries(path: &Path, entries: &[CacheEntry]) -> Result<()> {
    let tmp_path = path.with_extension("jsonl.tmp");
    let mut payload = String::new();
    for entry in entries {
        payload.push_str(&serde_json::to_string(entry).context("failed to encode cache entry")?);
        payload.push('\n');
    }
    write_cache_file(&tmp_path, payload.as_bytes())?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to replace cache file {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

fn persist_health_snapshots(path: &Path, snapshots: &[HealthSnapshot]) -> Result<()> {
    let tmp_path = path.with_extension("jsonl.tmp");
    let mut payload = String::new();
    for snapshot in snapshots {
        payload.push_str(
            &serde_json::to_string(snapshot).context("failed to encode health snapshot")?,
        );
        payload.push('\n');
    }
    write_cache_file(&tmp_path, payload.as_bytes())?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to replace health snapshot file {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

fn write_cache_file(path: &Path, payload: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to create cache temp file {}", path.display()))?;
        file.write_all(payload)
            .with_context(|| format!("failed to write cache temp file {}", path.display()))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, payload)
            .with_context(|| format!("failed to write cache temp file {}", path.display()))?;
        set_owner_only_permissions(path, 0o600)
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path, mode: u32) -> Result<()> {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Cache, HealthSnapshot, MAX_CACHE_FILE_BYTES, runtime_dir};
    use std::collections::HashMap;
    use std::fs;

    use anyhow::Result;
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct FakeReport {
        summary: String,
        risk_level: String,
    }

    #[test]
    fn cache_key_includes_target_identity_and_head_hash_prefix() {
        let key = Cache::make_key("src/auth/session.rs", "authenticate", "a1b2c3d4e5f67890");
        assert_eq!(key, "src/auth/session.rs:authenticate:a1b2c3d4e5f6");
    }

    #[test]
    fn set_get_round_trip_returns_same_report() -> Result<()> {
        let dir = tempdir()?;
        let mut cache = Cache::open(dir.path(), 10)?;
        let report = FakeReport {
            summary: "auth hotfix".into(),
            risk_level: "HIGH".into(),
        };
        let key = Cache::make_key("src/auth.rs", "verify_token", "abcdef1234567890");

        cache.set(key.clone(), report.clone(), "abcdef1234567890")?;

        let loaded: FakeReport = cache.get(&key).expect("cache hit");
        assert_eq!(loaded, report);
        Ok(())
    }

    #[test]
    fn same_target_under_different_head_hash_misses() -> Result<()> {
        let dir = tempdir()?;
        let mut cache = Cache::open(dir.path(), 10)?;
        let report = FakeReport {
            summary: "legacy path".into(),
            risk_level: "MEDIUM".into(),
        };

        let old_key = Cache::make_key("src/payments.rs", "process", "111111111111aaaa");
        cache.set(old_key, report, "111111111111aaaa")?;

        let new_key = Cache::make_key("src/payments.rs", "process", "222222222222bbbb");
        let loaded: Option<FakeReport> = cache.get(&new_key);
        assert!(loaded.is_none());
        Ok(())
    }

    #[test]
    fn oldest_entry_is_evicted_when_max_entries_is_exceeded() -> Result<()> {
        let dir = tempdir()?;
        let mut cache = Cache::open(dir.path(), 2)?;

        cache.set(
            Cache::make_key("src/a.rs", "a", "aaaaaaaaaaaa1111"),
            FakeReport {
                summary: "first".into(),
                risk_level: "LOW".into(),
            },
            "aaaaaaaaaaaa1111",
        )?;
        std::thread::sleep(std::time::Duration::from_secs(1));
        cache.set(
            Cache::make_key("src/b.rs", "b", "bbbbbbbbbbbb2222"),
            FakeReport {
                summary: "second".into(),
                risk_level: "LOW".into(),
            },
            "bbbbbbbbbbbb2222",
        )?;
        std::thread::sleep(std::time::Duration::from_secs(1));
        cache.set(
            Cache::make_key("src/c.rs", "c", "cccccccccccc3333"),
            FakeReport {
                summary: "third".into(),
                risk_level: "LOW".into(),
            },
            "cccccccccccc3333",
        )?;

        let reloaded = Cache::open(dir.path(), 2)?;
        let first: Option<FakeReport> = reloaded.get("src/a.rs:a:aaaaaaaaaaaa");
        let second: Option<FakeReport> = reloaded.get("src/b.rs:b:bbbbbbbbbbbb");
        let third: Option<FakeReport> = reloaded.get("src/c.rs:c:cccccccccccc");

        assert!(first.is_none());
        assert!(second.is_some());
        assert!(third.is_some());
        Ok(())
    }

    #[test]
    fn keeping_fifty_three_health_snapshots_retain_only_fifty_two() -> Result<()> {
        let dir = tempdir()?;
        let mut cache = Cache::open(dir.path(), 10)?;

        for week in 0..53 {
            let mut signals = HashMap::new();
            signals.insert("time_bombs".into(), week as u32);
            cache.insert_health_snapshot(HealthSnapshot {
                timestamp: week,
                debt_score: week as u32,
                signals,
                head_hash: None,
                ref_name: None,
            })?;
        }

        let snapshots = cache.health_snapshots();
        assert_eq!(snapshots.len(), 52);
        assert_eq!(
            snapshots.first().map(|snapshot| snapshot.timestamp),
            Some(1)
        );
        assert_eq!(
            snapshots.last().map(|snapshot| snapshot.timestamp),
            Some(52)
        );
        Ok(())
    }

    #[test]
    fn health_snapshot_backwards_compatibly_reads_legacy_details_field() {
        let snapshot: HealthSnapshot =
            serde_json::from_str(r#"{"timestamp":1,"debt_score":7,"details":{"time_bombs":2}}"#)
                .expect("legacy health snapshot should parse");
        assert_eq!(snapshot.signals.get("time_bombs"), Some(&2));
        assert_eq!(snapshot.head_hash, None);
        assert_eq!(snapshot.ref_name, None);
    }

    #[cfg(unix)]
    #[test]
    fn cache_uses_owner_only_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir()?;
        let mut cache = Cache::open(dir.path(), 10)?;
        let key = Cache::make_key("src/auth.rs", "verify_token", "abcdef1234567890");
        cache.set(
            key,
            FakeReport {
                summary: "auth hotfix".into(),
                risk_level: "HIGH".into(),
            },
            "abcdef1234567890",
        )?;

        let cache_dir = dir.path().join(".why");
        let cache_file = cache_dir.join("cache.jsonl");
        let health_file = cache_dir.join("health.jsonl");
        assert_eq!(fs::metadata(cache_dir)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(cache_file)?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(health_file)?.permissions().mode() & 0o777,
            0o600
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_cache_directory() -> Result<()> {
        let dir = tempdir()?;
        let real_dir = dir.path().join("real-cache-dir");
        fs::create_dir_all(&real_dir)?;
        symlink(&real_dir, dir.path().join(".why"))?;

        let error = Cache::open(dir.path(), 10).expect_err("symlinked cache dir should fail");
        assert!(error.to_string().contains("must not be a symlink"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_cache_file() -> Result<()> {
        let dir = tempdir()?;
        let cache_dir = dir.path().join(".why");
        fs::create_dir_all(&cache_dir)?;
        let target = dir.path().join("elsewhere.json");
        fs::write(&target, b"{}")?;
        symlink(&target, cache_dir.join("cache.jsonl"))?;

        let error = Cache::open(dir.path(), 10).expect_err("symlinked cache file should fail");
        assert!(error.to_string().contains("must not be a symlink"));
        Ok(())
    }

    #[test]
    fn runtime_dir_appends_why_to_existing_gitignore() -> Result<()> {
        let dir = tempdir()?;
        fs::write(dir.path().join(".gitignore"), "target\n")?;

        runtime_dir(dir.path())?;

        let gitignore = fs::read_to_string(dir.path().join(".gitignore"))?;
        assert_eq!(gitignore, "target\n.why\n");
        Ok(())
    }

    #[test]
    fn runtime_dir_does_not_duplicate_existing_gitignore_entry() -> Result<()> {
        let dir = tempdir()?;
        fs::write(dir.path().join(".gitignore"), "target\n.why\n")?;

        runtime_dir(dir.path())?;

        let gitignore = fs::read_to_string(dir.path().join(".gitignore"))?;
        assert_eq!(gitignore, "target\n.why\n");
        Ok(())
    }

    #[test]
    fn runtime_dir_appends_why_after_line_without_trailing_newline() -> Result<()> {
        let dir = tempdir()?;
        fs::write(dir.path().join(".gitignore"), "target")?;

        runtime_dir(dir.path())?;

        let gitignore = fs::read_to_string(dir.path().join(".gitignore"))?;
        assert_eq!(gitignore, "target\n.why\n");
        Ok(())
    }

    #[test]
    fn runtime_dir_does_not_create_gitignore_when_missing() -> Result<()> {
        let dir = tempdir()?;

        runtime_dir(dir.path())?;

        assert!(!dir.path().join(".gitignore").exists());
        Ok(())
    }

    #[test]
    fn rejects_oversized_cache_file() -> Result<()> {
        let dir = tempdir()?;
        let cache_dir = dir.path().join(".why");
        fs::create_dir_all(&cache_dir)?;
        let cache_path = cache_dir.join("cache.jsonl");
        fs::write(&cache_path, vec![b'x'; (MAX_CACHE_FILE_BYTES as usize) + 1])?;

        let error = Cache::open(dir.path(), 10).expect_err("oversized cache file should fail");
        assert!(error.to_string().contains("safety limit"));
        Ok(())
    }
}
