use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, command, now, utc_time};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;

use super::escape;
use crate::config::Redis;
use crate::session::{Reply, Session, cmd, on};

#[derive(clap::Args)]
pub struct KeyListArgs {
    /// Keys matching this glob: session:*, orders:44?? (default: every key)
    pattern: Option<String>,
    /// Instance name from instance list (or its host, URL or connection string); defaults to the only one
    #[arg(long)]
    instance: Option<String>,
    /// Only keys of this type
    #[arg(long = "type", value_parser = PossibleValuesParser::new(["string", "hash", "list", "set", "zset", "stream"]))]
    kind: Option<String>,
    /// key: the first matches, by name; size or expires: the largest or the
    /// soonest to expire among --sample matches
    #[arg(long, default_value = "key", value_parser = PossibleValuesParser::new(["key", "size", "expires"]))]
    sort: String,
    /// Matching keys --sort size or expires ranks
    #[arg(long, default_value_t = 50_000)]
    sample: usize,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct KeyRow {
    /// What key get takes with --instance.
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Memory it takes, in bytes.
    size_bytes: Option<i64>,
    /// Absent: it never expires.
    expires_at: Option<String>,
    /// When it was last read or written (not under an LFU policy); a key
    /// get reads it, the metadata agent-cli asks for does not.
    last_access: Option<String>,
}

/// Keys `SCAN` looks at per call: Redis does that much work, then answers.
const PAGE: usize = 1000;

/// Keys described per round trip.
const BATCH: usize = 500;

/// What `SCAN` found before it stopped.
struct Scanned {
    keys: Vec<Vec<u8>>,
    /// Calls made, each over about [`PAGE`] keys.
    pages: usize,
    /// It walked the whole keyspace.
    done: bool,
}

fn key_list(ctx: &Ctx, args: KeyListArgs) -> Result<Vec<KeyRow>> {
    let redis = Redis::load(ctx.config())?;
    let spec = redis.instance(args.instance.as_deref())?;
    let pattern = args.pattern.as_deref().unwrap_or("*");
    let want = if args.sort == "key" {
        args.limit + 1
    } else {
        args.sample.max(args.limit)
    };
    // Leave a quarter of the time (at most 5 s) to describe what was found.
    let left = ctx.remaining()?;
    let scan_until = Instant::now() + left - (left / 4).min(Duration::from_secs(5));
    let (total, scanned, mut rows) = on(ctx, spec, |session| {
        let total = session.call(cmd!("DBSIZE"))?.int().unwrap_or_default();
        let scanned = scan(session, pattern, args.kind.as_deref(), want, scan_until)?;
        let rows = describe(session, &scanned.keys)?;
        Ok((total, scanned, rows))
    })?;
    rows.retain(|row| row.kind.is_some());
    match args.sort.as_str() {
        "size" => rows.sort_by_key(|row| std::cmp::Reverse(row.size_bytes)),
        "expires" => {
            rows.retain(|row| row.expires_at.is_some());
            rows.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));
        }
        _ => {}
    }
    let found = rows.len();
    rows.truncate(args.limit);
    let looked = if scanned.done {
        format!("all {total} keys")
    } else {
        format!(
            "~{} of {total} keys",
            (scanned.pages * PAGE).min(total as usize)
        )
    };
    match (args.sort.as_str(), scanned.done) {
        ("key", _) if found > args.limit => ctx.note(format!(
            "[the first {} matching {pattern}; more match; --limit N]",
            args.limit
        )),
        ("key", false) => ctx.note(format!(
            "[scan stopped at the deadline after {looked}; narrow the pattern or raise --timeout]"
        )),
        (sort, _) if sort != "key" => ctx.note(format!(
            "[{} {} of {found} matching {pattern} in {looked}{}]",
            if sort == "size" {
                "largest"
            } else {
                "soonest to expire"
            },
            rows.len(),
            if scanned.done {
                ""
            } else {
                "; --sample N looks at more"
            }
        )),
        _ => {}
    }
    Ok(rows)
}

/// `SCAN` (never `KEYS`, which stops the server while it walks every key)
/// until `want` matches, the end of the keyspace, or `until`.
fn scan(
    session: &mut Session,
    pattern: &str,
    kind: Option<&str>,
    want: usize,
    until: Instant,
) -> Result<Scanned> {
    let mut found = Scanned {
        keys: Vec::new(),
        pages: 0,
        done: false,
    };
    let mut cursor = "0".to_owned();
    while found.keys.len() < want && Instant::now() < until {
        let mut command = cmd!("SCAN", cursor, "MATCH", pattern, "COUNT", PAGE.to_string());
        if let Some(kind) = kind {
            command.extend(cmd!("TYPE", kind));
        }
        let page = session.call(command)?;
        let items = page.items();
        found.pages += 1;
        cursor = items
            .first()
            .and_then(Reply::text)
            .unwrap_or_else(|| "0".to_owned());
        found.keys.extend(
            items
                .get(1)
                .map(Reply::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|key| key.bytes().map(<[u8]>::to_vec)),
        );
        if cursor == "0" {
            found.done = true;
            break;
        }
    }
    // SCAN may return a key twice; sorted, the first matches are by name.
    found.keys.sort();
    found.keys.dedup();
    Ok(found)
}

/// Each key's type, size, expiry and last use, [`BATCH`] keys a round trip.
/// A key gone since the scan has no type.
fn describe(session: &mut Session, keys: &[Vec<u8>]) -> Result<Vec<KeyRow>> {
    let mut rows = Vec::with_capacity(keys.len());
    for batch in keys.chunks(BATCH) {
        let commands: Vec<Vec<Vec<u8>>> = batch
            .iter()
            .flat_map(|key| {
                [
                    cmd!("TYPE", key),
                    cmd!("PTTL", key),
                    cmd!("MEMORY", "USAGE", key),
                    cmd!("OBJECT", "IDLETIME", key),
                ]
            })
            .collect();
        let replies = session.pipeline(&commands)?;
        let moment = now();
        for (key, got) in batch.iter().zip(replies.chunks(4)) {
            rows.push(KeyRow {
                id: escape(key),
                kind: got[0].text().filter(|kind| kind != "none"),
                size_bytes: got[2].int(),
                expires_at: got[1]
                    .int()
                    .filter(|ms| *ms >= 0)
                    .map(|ms| utc_time(moment + time::Duration::milliseconds(ms))),
                last_access: got[3]
                    .int()
                    .map(|idle| utc_time(moment - time::Duration::seconds(idle))),
            });
        }
    }
    Ok(rows)
}

command! {
    pub KEY_LIST = ["redis", "key", "list"], Read,
    "List Redis keys matching a pattern, with type, size and expiry; or the largest",
    keywords: ["scan", "find", "search", "keys", "biggest", "memory", "ttl", "expiring", "prefix", "cache"],
    example: "redis key list 'session:*' --instance orders-qa --fields id,type,expires_at",
    run: key_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::session::Reply;
    use crate::testing::{array, bulk, config, redis, serve};

    fn described(key: &str, kind: &str, ttl: i64, size: i64) -> Vec<(String, Reply)> {
        vec![
            (format!("TYPE {key}"), Reply::Status(kind.into())),
            (format!("PTTL {key}"), Reply::Int(ttl)),
            (format!("MEMORY USAGE {key}"), Reply::Int(size)),
            (format!("OBJECT IDLETIME {key}"), Reply::Int(5)),
        ]
    }

    fn fake(script: Vec<(String, Reply)>) -> crate::testing::Fake {
        serve(
            script
                .iter()
                .map(|(command, reply)| (command.as_str(), reply.clone()))
                .collect(),
        )
    }

    #[test]
    fn keys_come_from_scan_never_keys_and_are_described_in_one_round_trip() {
        let mut script = vec![
            ("DBSIZE".to_owned(), Reply::Int(4)),
            (
                "SCAN 0 MATCH session:* COUNT 1000".to_owned(),
                array([bulk("7"), array([bulk("session:b")])]),
            ),
            (
                "SCAN 7 MATCH session:* COUNT 1000".to_owned(),
                array([bulk("0"), array([bulk("session:a"), bulk("session:c")])]),
            ),
        ];
        script.extend(described("session:a", "string", 60_000, 90));
        script.extend(described("session:b", "hash", -1, 900));
        script.extend(described("session:c", "string", 1_000, 50));
        let server = fake(script);
        let outcome = redis(
            &["redis", "key", "list", "session:*", "--limit", "2"],
            &config(&[("qa", server.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(
            (rows[0]["id"].as_str(), rows[0]["type"].as_str()),
            (Some("session:a"), Some("string"))
        );
        assert!(
            rows[0]["expires_at"].is_string() && rows[1].get("expires_at").is_none(),
            "{rows}"
        );
        assert!(
            outcome
                .stderr
                .contains("[the first 2 matching session:*; more match; --limit N]"),
            "{}",
            outcome.stderr
        );
        let sent = server.sent();
        assert!(
            sent.iter().all(|command| !command.starts_with("KEYS")),
            "{sent:?}"
        );

        let outcome = redis(
            &["redis", "key", "list", "session:*", "--sort", "size"],
            &config(&[("qa", server.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let ids: Vec<String> = outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["session:b", "session:a", "session:c"]);
        assert!(
            outcome
                .stderr
                .contains("[largest 3 of 3 matching session:* in all 4 keys]"),
            "{}",
            outcome.stderr
        );
        let outcome = redis(
            &["redis", "key", "list", "session:*", "--sort", "expires"],
            &config(&[("qa", server.port)]),
        );
        assert_eq!(
            outcome
                .json()
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].clone())
                .collect::<Vec<_>>(),
            [json!("session:c"), json!("session:a")]
        );
    }

    #[test]
    fn a_type_filter_goes_to_scan() {
        let server = fake(vec![
            ("DBSIZE".to_owned(), Reply::Int(4)),
            (
                "SCAN 0 MATCH * COUNT 1000 TYPE stream".to_owned(),
                array([bulk("0"), array([])]),
            ),
        ]);
        let outcome = redis(
            &["redis", "key", "list", "--type", "stream"],
            &config(&[("qa", server.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([]));
    }
}
