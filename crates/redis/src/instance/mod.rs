//! What instance list and get share: `INFO` read into the fields that answer
//! "is it healthy, and how full is it".

pub mod get;
pub mod list;

use std::collections::HashMap;

use agent_cli_core::{now, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::session::{Session, cmd};

/// `INFO`'s `key:value` lines, and its keyspace.
pub(crate) struct Info {
    fields: HashMap<String, String>,
    pub dbs: Vec<Db>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Db {
    pub db: u32,
    pub keys: i64,
    /// Keys with a TTL.
    pub expiring: i64,
    /// The average TTL of a sample of them, in seconds.
    pub avg_ttl_s: Option<i64>,
}

/// `INFO` with its default sections: one call that works on Redis 6 too,
/// which takes one section name at most.
// VERIFY(work): that Azure's access policies (Data Reader) allow INFO, and
// which fields Azure Managed Redis leaves out; every one is optional.
pub(crate) fn info(session: &mut Session) -> Result<Info> {
    let text = session.call(cmd!("INFO"))?.text().unwrap_or_default();
    Ok(Info::parse(&text))
}

impl Info {
    pub fn parse(text: &str) -> Self {
        let mut fields = HashMap::new();
        let mut dbs = Vec::new();
        for line in text.lines().map(str::trim) {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if let Some(db) = key.strip_prefix("db").and_then(|n| n.parse().ok()) {
                let part = |name: &str| {
                    value.split(',').find_map(|pair| {
                        pair.strip_prefix(name)?
                            .strip_prefix('=')?
                            .parse::<i64>()
                            .ok()
                    })
                };
                dbs.push(Db {
                    db,
                    keys: part("keys").unwrap_or_default(),
                    expiring: part("expires").unwrap_or_default(),
                    avg_ttl_s: part("avg_ttl").filter(|ms| *ms > 0).map(|ms| ms / 1000),
                });
                continue;
            }
            if !line.starts_with('#') {
                fields.insert(key.to_owned(), value.to_owned());
            }
        }
        Self { fields, dbs }
    }

    pub fn text(&self, key: &str) -> Option<String> {
        self.fields
            .get(key)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.fields.get(key)?.parse().ok()
    }

    fn float(&self, key: &str) -> Option<f64> {
        self.fields.get(key)?.parse().ok()
    }

    /// `uptime_in_seconds` back from now: when the counters started.
    pub fn started_at(&self) -> Option<String> {
        let up = self.int("uptime_in_seconds")?;
        Some(utc_time(now() - time::Duration::seconds(up)))
    }

    pub fn keys(&self) -> i64 {
        self.dbs.iter().map(|db| db.keys).sum()
    }

    pub fn memory(&self) -> Memory {
        let used = self.int("used_memory");
        let max = self.int("maxmemory").filter(|max| *max > 0);
        let host = self.int("total_system_memory").filter(|total| *total > 0);
        let (bound, by) = match (max, host) {
            (Some(max), _) => (Some(max), Some("maxmemory")),
            (None, Some(host)) => (Some(host), Some("host memory")),
            (None, None) => (None, None),
        };
        let free = used.zip(bound).map(|(used, bound)| (bound - used).max(0));
        let used_pct = used
            .zip(bound)
            .map(|(used, bound)| (used as f64 * 1000.0 / bound as f64).round() / 10.0);
        Memory {
            used_bytes: used,
            free_bytes: free,
            used_pct,
            bounded_by: by.map(str::to_owned),
            maxmemory_bytes: max,
            policy: self.text("maxmemory_policy"),
            peak_bytes: self.int("used_memory_peak"),
            rss_bytes: self.int("used_memory_rss"),
            fragmentation: self.float("mem_fragmentation_ratio"),
        }
    }

    pub fn hit_ratio(&self) -> Option<f64> {
        let (hits, misses) = (self.int("keyspace_hits")?, self.int("keyspace_misses")?);
        (hits + misses > 0)
            .then(|| (hits as f64 * 1000.0 / (hits + misses) as f64).round() / 1000.0)
    }

    pub fn stats(&self) -> Stats {
        Stats {
            hit_ratio: self.hit_ratio(),
            hits: self.int("keyspace_hits"),
            misses: self.int("keyspace_misses"),
            evicted_keys: self.int("evicted_keys"),
            expired_keys: self.int("expired_keys"),
            ops_per_sec: self.int("instantaneous_ops_per_sec"),
            rejected_connections: self.int("rejected_connections"),
        }
    }

    pub fn clients(&self) -> Clients {
        Clients {
            connected: self.int("connected_clients"),
            blocked: self.int("blocked_clients"),
            max: self.int("maxclients"),
        }
    }

    pub fn persistence(&self) -> Persistence {
        Persistence {
            last_save_at: self
                .int("rdb_last_save_time")
                .filter(|unix| *unix > 0)
                .and_then(|unix| time::OffsetDateTime::from_unix_timestamp(unix).ok())
                .map(utc_time),
            last_bgsave: self.text("rdb_last_bgsave_status"),
            changes_since_save: self.int("rdb_changes_since_last_save"),
            aof: self.int("aof_enabled").map(|on| on == 1),
        }
    }

    pub fn replication(&self) -> Replication {
        let master = self
            .text("master_host")
            .map(|host| match self.text("master_port") {
                Some(port) => format!("{host}:{port}"),
                None => host,
            });
        Replication {
            replicas: self.int("connected_slaves"),
            master,
            link: self.text("master_link_status"),
            last_io_s: self.int("master_last_io_seconds_ago"),
        }
    }

    /// True when it is full or has dropped keys to stay under maxmemory.
    pub fn needs_room(&self) -> bool {
        self.memory().used_pct.is_some_and(|pct| pct >= 90.0)
            || self.int("evicted_keys").is_some_and(|evicted| evicted > 0)
    }
}

/// Bytes. `free_bytes` and `used_pct` are of `maxmemory`, or of the host's
/// memory when maxmemory is 0 (unbounded): `bounded_by` says which.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Memory {
    pub used_bytes: Option<i64>,
    pub free_bytes: Option<i64>,
    pub used_pct: Option<f64>,
    pub bounded_by: Option<String>,
    pub maxmemory_bytes: Option<i64>,
    /// What happens at maxmemory: noeviction refuses writes, allkeys-lru …
    /// evicts.
    pub policy: Option<String>,
    pub peak_bytes: Option<i64>,
    pub rss_bytes: Option<i64>,
    /// rss over used: above 1.5 the allocator holds much it does not use.
    pub fragmentation: Option<f64>,
}

/// Counted since `started_at`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Stats {
    /// Hits over reads that looked for a key.
    pub hit_ratio: Option<f64>,
    pub hits: Option<i64>,
    pub misses: Option<i64>,
    /// Keys dropped to stay under maxmemory.
    pub evicted_keys: Option<i64>,
    pub expired_keys: Option<i64>,
    pub ops_per_sec: Option<i64>,
    /// Connections refused at maxclients.
    pub rejected_connections: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Clients {
    pub connected: Option<i64>,
    pub blocked: Option<i64>,
    pub max: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Persistence {
    pub last_save_at: Option<String>,
    /// ok or err.
    pub last_bgsave: Option<String>,
    pub changes_since_save: Option<i64>,
    pub aof: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Replication {
    /// Replicas connected to this primary.
    pub replicas: Option<i64>,
    /// A replica's primary, host:port.
    pub master: Option<String>,
    /// A replica's link to it: up or down.
    pub link: Option<String>,
    pub last_io_s: Option<i64>,
}

#[cfg(test)]
pub(crate) const INFO: &str = "# Server\r\nredis_version:7.2.4\r\nredis_mode:standalone\r\nuptime_in_seconds:86400\r\n\
# Clients\r\nconnected_clients:12\r\nblocked_clients:0\r\nmaxclients:10000\r\n\
# Memory\r\nused_memory:943718400\r\nused_memory_rss:1073741824\r\nused_memory_peak:1000000000\r\n\
total_system_memory:8589934592\r\nmaxmemory:1073741824\r\nmaxmemory_policy:allkeys-lru\r\nmem_fragmentation_ratio:1.14\r\n\
# Persistence\r\nrdb_changes_since_last_save:10\r\nrdb_last_save_time:1791158400\r\nrdb_last_bgsave_status:ok\r\naof_enabled:0\r\n\
# Stats\r\ninstantaneous_ops_per_sec:340\r\nrejected_connections:0\r\nexpired_keys:5000\r\nevicted_keys:120\r\n\
keyspace_hits:900\r\nkeyspace_misses:100\r\n\
# Replication\r\nrole:master\r\nconnected_slaves:1\r\n\
# Keyspace\r\ndb0:keys=200000,expires=150000,avg_ttl=3600000,subexpiry=0\r\ndb2:keys=5,expires=0,avg_ttl=0\r\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_becomes_memory_against_maxmemory_or_the_host_and_a_keyspace() {
        let info = Info::parse(INFO);
        let memory = info.memory();
        assert_eq!(memory.free_bytes, Some(1_073_741_824 - 943_718_400));
        assert_eq!(
            (memory.used_pct, memory.bounded_by.as_deref()),
            (Some(87.9), Some("maxmemory"))
        );
        assert_eq!(info.hit_ratio(), Some(0.9));
        assert_eq!(info.keys(), 200_005);
        assert_eq!(
            (info.dbs[0].expiring, info.dbs[0].avg_ttl_s),
            (150_000, Some(3600))
        );
        assert_eq!(info.dbs[1].avg_ttl_s, None);
        assert!(info.needs_room(), "it evicted 120 keys");
        assert_eq!(
            info.persistence().last_save_at.as_deref(),
            Some("2026-10-05T00:00:00Z")
        );
        let unbounded = Info::parse(
            &INFO
                .replace("maxmemory:1073741824", "maxmemory:0")
                .replace("evicted_keys:120", "evicted_keys:0"),
        );
        let memory = unbounded.memory();
        assert_eq!(
            (
                memory.maxmemory_bytes,
                memory.bounded_by.as_deref(),
                memory.used_pct
            ),
            (None, Some("host memory"), Some(11.0))
        );
        assert!(!unbounded.needs_room());
        let bare = Info::parse("redis_version:6.0.14\r\n");
        assert_eq!(
            (
                bare.memory().free_bytes,
                bare.hit_ratio(),
                bare.started_at()
            ),
            (None, None, None)
        );
    }
}
