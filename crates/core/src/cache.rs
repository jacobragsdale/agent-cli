//! A small JSON key-value store for slow-changing lookups: IDs, inventories.
//!
//! One file, `$XDG_CACHE_HOME/agent-cli/cache.json` (or `~/.cache/…`), 0600,
//! replaced atomically. Every entry carries its own expiry. A cache is never
//! worth failing a command over, so every problem here reads as a miss. It
//! never holds a secret: [`crate::Secret`] cannot be serialized.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

pub struct Cache {
    /// `None` turns the cache off: nothing is read or written.
    dir: Option<PathBuf>,
    /// Off under `--no-cache`, which still writes what it fetched, so the
    /// next plain call sees the fresh answer rather than the stale one.
    pub(crate) read: bool,
}

pub(crate) fn default_dir() -> Option<PathBuf> {
    crate::config::xdg_dir("XDG_CACHE_HOME", ".cache").map(|dir| dir.join("agent-cli"))
}

impl Cache {
    #[must_use]
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, read: true }
    }

    /// The value under `key`, if it is there and has not expired.
    #[must_use]
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        if !self.read {
            return None;
        }
        let entry = self.entries().remove(key)?;
        if entry["expires"].as_u64()? <= now() {
            return None;
        }
        serde_json::from_value(entry["value"].clone()).ok()
    }

    /// Stores `value` under `key` for `ttl`, dropping expired entries on the way.
    // ponytail: read-modify-write of one file; two agents writing at once can
    // lose one entry, which is a cache miss later. A file per key if it shows.
    pub fn put<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) {
        let (Some(dir), Ok(value)) = (&self.dir, serde_json::to_value(value)) else {
            return;
        };
        let mut entries = self.entries();
        let now = now();
        entries.retain(|_, entry| entry["expires"].as_u64().is_some_and(|at| at > now));
        entries.insert(
            key.to_owned(),
            json!({"expires": now + ttl.as_secs(), "value": value}),
        );
        let _ = std::fs::create_dir_all(dir).and_then(|()| {
            // A temp file in the same directory is created 0600 and renamed
            // over the old one, so a reader never sees half a file.
            let mut file = tempfile::NamedTempFile::new_in(dir)?;
            file.write_all(Value::Object(entries).to_string().as_bytes())?;
            file.persist(dir.join("cache.json"))
                .map(drop)
                .map_err(|error| error.error)
        });
    }

    fn entries(&self) -> Map<String, Value> {
        self.dir
            .as_ref()
            .and_then(|dir| std::fs::read(dir.join("cache.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_lives_for_its_ttl_and_no_longer() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(Some(dir.path().join("agent-cli")));
        cache.put("ado:project", &"1f2e", Duration::from_secs(60));
        cache.put("gone", &1, Duration::ZERO);
        assert_eq!(cache.get::<String>("ado:project").as_deref(), Some("1f2e"));
        assert_eq!(cache.get::<i32>("gone"), None);
        assert_eq!(cache.get::<i32>("never"), None);
        cache.put("next", &2, Duration::from_secs(60));
        let stored = std::fs::read_to_string(dir.path().join("agent-cli/cache.json")).unwrap();
        assert!(
            !stored.contains("gone"),
            "expired entries are dropped on write: {stored}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("agent-cli/cache.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_off_cache_neither_reads_nor_writes_and_a_corrupt_file_is_a_miss() {
        let off = Cache::new(None);
        off.put("k", &1, Duration::from_secs(60));
        assert_eq!(off.get::<i32>("k"), None);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cache.json"), "{not json").unwrap();
        let cache = Cache::new(Some(dir.path().to_owned()));
        assert_eq!(cache.get::<i32>("k"), None);
        cache.put("k", &1, Duration::from_secs(60));
        assert_eq!(cache.get::<i32>("k"), Some(1));
    }

    #[test]
    fn no_cache_skips_reading_but_writes_what_it_fetched() {
        let dir = tempfile::tempdir().unwrap();
        Cache::new(Some(dir.path().to_owned())).put("k", &1, Duration::from_secs(60));
        let mut fresh = Cache::new(Some(dir.path().to_owned()));
        fresh.read = false;
        assert_eq!(fresh.get::<i32>("k"), None);
        fresh.put("k", &2, Duration::from_secs(60));
        assert_eq!(
            Cache::new(Some(dir.path().to_owned())).get::<i32>("k"),
            Some(2)
        );
    }
}
