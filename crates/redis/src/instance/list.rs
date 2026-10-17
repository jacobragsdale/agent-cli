use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::info;
use crate::config::Redis;
use crate::session::OnInstance;

#[derive(clap::Args)]
pub struct InstanceListArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstanceSummary {
    /// What instance get and every --instance take.
    id: String,
    host: String,
    version: Option<String>,
    role: Option<String>,
    used_bytes: Option<i64>,
    /// Of maxmemory, or of the host's memory when maxmemory is 0.
    used_pct: Option<f64>,
    maxmemory_bytes: Option<i64>,
    evicted_keys: Option<i64>,
    clients: Option<i64>,
    keys: Option<i64>,
    /// Why it did not answer; the rest is then absent.
    error: Option<String>,
}

/// How long one instance gets, so one that is down costs the rest little.
const PROBE: Duration = Duration::from_secs(5);

fn instance_list(ctx: &Ctx, _: InstanceListArgs) -> Result<Vec<InstanceSummary>> {
    let redis = Redis::load(ctx.config())?;
    let rows = redis
        .instances
        .iter()
        .map(|spec| {
            let mut row = InstanceSummary {
                id: spec.name.clone(),
                host: format!("{}:{}", spec.host, spec.port),
                version: None,
                role: None,
                used_bytes: None,
                used_pct: None,
                maxmemory_bytes: None,
                evicted_keys: None,
                clients: None,
                keys: None,
                error: None,
            };
            let op = OnInstance {
                spec,
                deadline: ctx.deadline().min(Instant::now() + PROBE),
                plan: serde_json::json!({"instance": spec.name}),
                work: info,
            };
            match ctx.read(op) {
                Ok(info) => {
                    let memory = info.memory();
                    row.version = info.text("redis_version");
                    row.role = info.text("role");
                    row.used_bytes = memory.used_bytes;
                    row.used_pct = memory.used_pct;
                    row.maxmemory_bytes = memory.maxmemory_bytes;
                    row.evicted_keys = info.int("evicted_keys");
                    row.clients = info.int("connected_clients");
                    row.keys = Some(info.keys());
                }
                Err(error) => row.error = Some(format!("{error:#}")),
            }
            row
        })
        .collect();
    Ok(rows)
}

command! {
    pub INSTANCE_LIST = ["redis", "instance", "list"], Read,
    "List the configured Redis instances, each with its memory, evictions and clients",
    keywords: ["caches", "environments", "health", "configured", "full", "up", "down"],
    example: "redis instance list --fields id,used_pct,evicted_keys,error",
    run: instance_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::instance::INFO;
    use crate::testing::{bulk, config, redis, serve};

    #[test]
    fn every_instance_answers_or_says_why_not() {
        let fake = serve(vec![("INFO", bulk(INFO))]);
        let outcome = redis(
            &["redis", "instance", "list"],
            &config(&[("orders-qa", fake.port), ("orders-prod", 9)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"id": "orders-qa", "host": format!("127.0.0.1:{}", fake.port), "version": "7.2.4",
                "role": "master", "used_bytes": 943_718_400, "used_pct": 87.9, "maxmemory_bytes": 1_073_741_824,
                "evicted_keys": 120, "clients": 12, "keys": 200_005})
        );
        assert_eq!(rows[1]["id"], "orders-prod");
        assert!(
            rows[1]["error"]
                .as_str()
                .unwrap()
                .contains("cannot connect to orders-prod at 127.0.0.1:9"),
            "{rows}"
        );
    }
}
