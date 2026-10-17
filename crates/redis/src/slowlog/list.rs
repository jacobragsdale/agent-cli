use agent_cli_core::{Ctx, When, command, utc_time};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;

use crate::config::Redis;
use crate::key::escape;
use crate::session::{Reply, cmd, on};

#[derive(clap::Args)]
pub struct SlowlogListArgs {
    /// Instance name from instance list (or its host, URL or connection string); defaults to the only one
    #[arg(long)]
    instance: Option<String>,
    /// Only commands logged since (15m, 2h, 7d, a date)
    #[arg(long)]
    since: Option<When>,
    /// time: newest first; duration: slowest first
    #[arg(long, default_value = "time", value_parser = PossibleValuesParser::new(["time", "duration"]))]
    sort: String,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SlowRow {
    at: String,
    duration_ms: f64,
    /// Its name, as the client sent it; the arguments are values and never
    /// print.
    command: String,
    /// The key it worked on, for a command whose first argument is one.
    key: Option<String>,
    /// The client's address, as client list prints it.
    client: Option<String>,
    client_name: Option<String>,
}

/// Commands whose first argument names what they do (`CLIENT SETNAME`,
/// `CONFIG SET`), which prints with the name; never a value.
const CONTAINERS: &[&str] = &[
    "acl", "client", "cluster", "command", "config", "function", "latency", "memory", "module",
    "object", "pubsub", "script", "slowlog", "xgroup", "xinfo",
];

/// Entries read: Redis keeps 128 by default (`slowlog-max-len`).
const KEEP: usize = 1024;

/// Commands whose first argument is a key: only for these does `key`
/// print, since any other argument can be a value (a password, a token).
const KEY_FIRST: &[&str] = &[
    "append",
    "bitcount",
    "bitpos",
    "decr",
    "decrby",
    "del",
    "dump",
    "exists",
    "expire",
    "expireat",
    "get",
    "getdel",
    "getex",
    "getrange",
    "getset",
    "hdel",
    "hexists",
    "hget",
    "hgetall",
    "hincrby",
    "hincrbyfloat",
    "hkeys",
    "hlen",
    "hmget",
    "hmset",
    "hrandfield",
    "hscan",
    "hset",
    "hsetnx",
    "hstrlen",
    "hvals",
    "incr",
    "incrby",
    "incrbyfloat",
    "lindex",
    "linsert",
    "llen",
    "lpop",
    "lpos",
    "lpush",
    "lrange",
    "lrem",
    "lset",
    "ltrim",
    "mget",
    "persist",
    "pexpire",
    "pexpireat",
    "psetex",
    "pttl",
    "rename",
    "restore",
    "rpop",
    "rpush",
    "sadd",
    "scard",
    "set",
    "setex",
    "setnx",
    "setrange",
    "sismember",
    "smembers",
    "smismember",
    "sort",
    "sort_ro",
    "spop",
    "srandmember",
    "srem",
    "sscan",
    "strlen",
    "ttl",
    "type",
    "unlink",
    "xadd",
    "xdel",
    "xlen",
    "xrange",
    "xrevrange",
    "xtrim",
    "zadd",
    "zcard",
    "zcount",
    "zincrby",
    "zrange",
    "zrangebyscore",
    "zrank",
    "zrem",
    "zremrangebyrank",
    "zremrangebyscore",
    "zrevrange",
    "zrevrangebyscore",
    "zscan",
    "zscore",
];

// VERIFY(work): that Azure's access policies allow SLOWLOG GET.
fn slowlog_list(ctx: &Ctx, args: SlowlogListArgs) -> Result<Vec<SlowRow>> {
    let redis = Redis::load(ctx.config())?;
    let spec = redis.instance(args.instance.as_deref())?;
    let log = on(ctx, spec, |session| {
        session.call(cmd!("SLOWLOG", "GET", KEEP.to_string()))
    })?;
    let since = args.since.map(|when| when.0.unix_timestamp());
    let mut rows: Vec<SlowRow> = log
        .items()
        .iter()
        .filter_map(|entry| {
            let fields = entry.items();
            let at = fields.get(1)?.int()?;
            if since.is_some_and(|since| at < since) {
                return None;
            }
            let words = fields.get(3).map(Reply::items).unwrap_or_default();
            let name = words.first()?.text()?;
            let lower = name.to_ascii_lowercase();
            let key = KEY_FIRST
                .contains(&lower.as_str())
                .then(|| words.get(1)?.bytes().map(escape))
                .flatten();
            let command = match words.get(1).and_then(Reply::text) {
                Some(sub) if CONTAINERS.contains(&lower.as_str()) => {
                    format!("{name} {}", sub.to_ascii_uppercase())
                }
                _ => name,
            };
            Some(SlowRow {
                at: utc_time(time::OffsetDateTime::from_unix_timestamp(at).ok()?),
                duration_ms: fields.get(2)?.int()? as f64 / 1000.0,
                command,
                key,
                client: fields
                    .get(4)
                    .and_then(Reply::text)
                    .filter(|text| !text.is_empty()),
                client_name: fields
                    .get(5)
                    .and_then(Reply::text)
                    .filter(|text| !text.is_empty()),
            })
        })
        .collect();
    let order = if args.sort == "duration" {
        rows.sort_by(|a, b| b.duration_ms.total_cmp(&a.duration_ms));
        "slowest"
    } else {
        "newest"
    };
    if rows.len() > args.limit {
        ctx.note(format!(
            "[{} of {}, {order} first; --limit N]",
            args.limit,
            rows.len()
        ));
        rows.truncate(args.limit);
    }
    Ok(rows)
}

command! {
    pub SLOWLOG_LIST = ["redis", "slowlog", "list"], Read,
    "List the slow commands a Redis instance logged, newest first, with their keys",
    keywords: ["slow", "latency", "blocking", "duration", "commands", "expensive"],
    example: "redis slowlog list --instance orders-prod --since 1h --sort duration --fields at,duration_ms,command,key",
    run: slowlog_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::session::Reply;
    use crate::testing::{array, bulk, config, redis, serve};

    fn entry(id: i64, at: i64, micros: i64, words: &[&str]) -> Reply {
        array([
            Reply::Int(id),
            Reply::Int(at),
            Reply::Int(micros),
            array(words.iter().map(bulk)),
            bulk("10.244.1.17:52114"),
            bulk("orders-api"),
        ])
    }

    #[test]
    fn slow_commands_print_their_key_but_never_a_value() {
        let fake = serve(vec![(
            "SLOWLOG GET 1024",
            array([
                entry(3, 1_791_158_460, 25_500, &["HGETALL", "cart:9"]),
                entry(
                    2,
                    1_791_158_400,
                    12_000,
                    &["SET", "token:7", "s3cret-value"],
                ),
                entry(
                    1,
                    1_791_158_400,
                    11_000,
                    &["EVAL", "return redis.call('get', KEYS[1])", "1", "k"],
                ),
                entry(
                    1,
                    1_791_158_400,
                    11_000,
                    &["config", "set", "requirepass", "s3cret-value"],
                ),
                entry(0, 1_700_000_000, 50_000, &["KEYS", "*"]),
            ]),
        )]);
        let outcome = redis(
            &["redis", "slowlog", "list", "--since", "2026-10-01"],
            &config(&[("prod", fake.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"at": "2026-10-05T00:01:00Z", "duration_ms": 25.5, "command": "HGETALL", "key": "cart:9",
                "client": "10.244.1.17:52114", "client_name": "orders-api"})
        );
        assert_eq!(rows[1]["key"], "token:7");
        assert!(
            rows[2].get("key").is_none(),
            "an EVAL's first argument is a script"
        );
        assert_eq!(
            (rows[3]["command"].as_str(), rows[3].get("key")),
            (Some("config SET"), None)
        );
        assert_eq!(
            rows.as_array().unwrap().len(),
            4,
            "--since leaves out 2023's"
        );
        assert!(!outcome.stdout.contains("s3cret"), "{}", outcome.stdout);
        let outcome = redis(
            &[
                "redis", "slowlog", "list", "--sort", "duration", "--limit", "2",
            ],
            &config(&[("prod", fake.port)]),
        );
        let slowest: Vec<f64> = outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["duration_ms"].as_f64().unwrap())
            .collect();
        assert_eq!(slowest, [50.0, 25.5]);
        assert!(
            outcome
                .stderr
                .contains("[2 of 5, slowest first; --limit N]"),
            "{}",
            outcome.stderr
        );
    }
}
