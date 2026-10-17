use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::{Clients, Db, Memory, Persistence, Replication, Stats, info};
use crate::config::Redis;
use crate::session::on;

#[derive(clap::Args)]
pub struct InstanceGetArgs {
    /// Instance name from instance list (or its host, URL or connection string); defaults to the only one
    name: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstanceRow {
    /// What instance get and every --instance take.
    id: String,
    host: String,
    version: Option<String>,
    /// standalone, cluster or sentinel.
    mode: Option<String>,
    /// master (a primary) or slave (a replica).
    role: Option<String>,
    /// When it started, and the stats with it.
    started_at: Option<String>,
    memory: Memory,
    keyspace: Vec<Db>,
    stats: Stats,
    clients: Clients,
    persistence: Persistence,
    replication: Replication,
}

fn instance_get(ctx: &Ctx, args: InstanceGetArgs) -> Result<InstanceRow> {
    let redis = Redis::load(ctx.config())?;
    let spec = redis.instance(args.name.as_deref())?;
    let info = on(ctx, spec, info)?;
    if info.needs_room() {
        ctx.note(format!(
            "[next: agent-cli redis key list --instance {} --sort size]",
            spec.name
        ));
    }
    Ok(InstanceRow {
        id: spec.name.clone(),
        host: format!("{}:{}", spec.host, spec.port),
        version: info.text("redis_version"),
        mode: info.text("redis_mode"),
        role: info.text("role"),
        started_at: info.started_at(),
        memory: info.memory(),
        stats: info.stats(),
        clients: info.clients(),
        persistence: info.persistence(),
        replication: info.replication(),
        keyspace: info.dbs,
    })
}

command! {
    pub INSTANCE_GET = ["redis", "instance", "get"], Read,
    "Show a Redis instance's health: memory used and free, evictions, hit ratio",
    keywords: ["info", "full", "space", "maxmemory", "evicted", "uptime", "replica", "persistence", "status"],
    example: "redis instance get orders-prod --fields memory,stats.evicted_keys",
    run: instance_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::next_command;
    use serde_json::json;

    use crate::instance::INFO;
    use crate::testing::{bulk, config, redis, serve};

    #[test]
    fn health_reads_info_and_a_full_instance_names_the_keys_that_fill_it() {
        let fake = serve(vec![("INFO", bulk(INFO))]);
        let outcome = redis(
            &["redis", "instance", "get", "orders-prod"],
            &config(&[("orders-qa", 9), ("orders-prod", fake.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["id"], "orders-prod");
        assert_eq!(row["version"], "7.2.4");
        assert_eq!(
            row["memory"],
            json!({"used_bytes": 943_718_400, "free_bytes": 130_023_424, "used_pct": 87.9,
                "bounded_by": "maxmemory", "maxmemory_bytes": 1_073_741_824, "policy": "allkeys-lru",
                "peak_bytes": 1_000_000_000, "rss_bytes": 1_073_741_824, "fragmentation": 1.14})
        );
        assert_eq!(row["stats"]["hit_ratio"], 0.9);
        assert_eq!(
            row["keyspace"][0],
            json!({"db": 0, "keys": 200_000, "expiring": 150_000, "avg_ttl_s": 3600})
        );
        assert_eq!(
            next_command(&outcome.stderr).unwrap(),
            [
                "redis",
                "key",
                "list",
                "--instance",
                "orders-prod",
                "--sort",
                "size"
            ]
        );
        assert_eq!(fake.sent(), ["CLIENT SETNAME agent-cli", "INFO"]);
    }

    #[test]
    fn a_refused_info_is_exit_three_naming_the_reads_its_user_needs() {
        let fake = serve(vec![(
            "INFO",
            crate::session::Reply::Error(
                "NOPERM User agent has no permissions to run the 'info' command".into(),
            ),
        )]);
        let outcome = redis(&["redis", "instance", "get"], &config(&[("qa", fake.port)]));
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains(crate::session::ACL),
            "{}",
            outcome.stderr
        );
    }
}
