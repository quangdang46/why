use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const CACHE_DIR_NAME: &str = ".why";
const CACHE_FILE_NAME: &str = "cache.json";
const HEALTH_SNAPSHOT_LIMIT: usize = 52;

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
    pub details: HashMap<String, u32>,
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
    path: PathBuf,
    max_entries: usize,
    data: CacheFile,
}

impl Cache {
    pub fn open(repo_root: &Path, max_entries: usize) -> Result<Self> {
        let dir = repo_root.join(CACHE_DIR_NAME);
        ensure_cache_dir(&dir)?;

        let path = dir.join(CACHE_FILE_NAME);
        let data = if path.exists() {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read cache file {}", path.display()))?;
            if bytes.is_empty() {
                CacheFile::default()
            } else {
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("failed to parse cache file {}", path.display()))?
            }
        } else {
            CacheFile::default()
        };

        Ok(Self {
            path,
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
        let parent = self
            .path
            .parent()
            .context("cache path has no parent directory")?;
        ensure_cache_dir(parent)?;

        let tmp_path = self.path.with_extension("json.tmp");
        let payload =
            serde_json::to_vec_pretty(&self.data).context("failed to encode cache file")?;
        write_cache_file(&tmp_path, &payload)?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to replace cache file {} with {}",
                self.path.display(),
                tmp_path.display()
            )
        })?;
        Ok(())
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn ensure_cache_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create cache directory {}", path.display()))?;
    set_owner_only_permissions(path, 0o700)
}

fn write_cache_file(path: &Path, payload: &[u8]) -> Result<()> {
    fs::write(path, payload)
        .with_context(|| format!("failed to write cache temp file {}", path.display()))?;
    set_owner_only_permissions(path, 0o600)
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
    use super::{Cache, HealthSnapshot};
    use std::collections::HashMap;
    use std::fs;

    use anyhow::Result;
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

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
            let mut details = HashMap::new();
            details.insert("time_bombs".into(), week as u32);
            cache.insert_health_snapshot(HealthSnapshot {
                timestamp: week,
                debt_score: week as u32,
                details,
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
        let cache_file = cache_dir.join("cache.json");
        assert_eq!(fs::metadata(cache_dir)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(cache_file)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }
}
