use agent_cli_core::{Ctx, command};
use anyhow::Result;

use super::{Key, MEMBER, Reach, SHOW, read, unescape};
use crate::config::Redis;
use crate::session::{cmd, on};

#[derive(clap::Args)]
pub struct KeyGetArgs {
    /// The key, as key list prints it (\xNN for a byte that is not UTF-8)
    key: String,
    /// Instance name from instance list (or its host, URL or connection string); defaults to the only one
    #[arg(long)]
    instance: Option<String>,
    /// Most members of a hash, list, set, sorted set or stream to print
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn key_get(ctx: &Ctx, args: KeyGetArgs) -> Result<Key> {
    let redis = Redis::load(ctx.config())?;
    let spec = redis.instance(args.instance.as_deref())?;
    let key = unescape(&args.key);
    let reach = Reach {
        show: SHOW,
        member: MEMBER,
        members: args.limit,
    };
    let saving = ctx.globals().output.is_some();
    let (row, whole) = on(ctx, spec, |session| {
        let row = read(session, &key, reach)?;
        let whole = if saving && row.kind == "string" {
            session
                .call(cmd!("GETRANGE", key, "0", "-1"))?
                .bytes()
                .map(<[u8]>::to_vec)
        } else {
            None
        };
        Ok((row, whole))
    })?;
    if let Some(bytes) = whole {
        let saved = match row.compression.as_deref() {
            Some(kind @ ("gzip" | "zlib")) => super::inflate_all(&bytes, kind),
            _ => bytes,
        };
        ctx.save(&saved)?;
    } else if row.truncated && row.kind == "string" {
        ctx.note(format!(
            "[the first {} KB of {} bytes; --output FILE saves all of it]",
            SHOW >> 10,
            row.length.unwrap_or_default()
        ));
    } else if row.truncated {
        ctx.note(format!(
            "[{} of {} members; --limit N]",
            args.limit,
            row.length.unwrap_or_default()
        ));
    }
    if row.format.as_deref() == Some("pickle") {
        ctx.note(format!(
            "[a Python pickle, never loaded; to read it: agent-cli redis key get {} --instance {} --output FILE, then python -m pickletools FILE (it shows a pickle without running it)]",
            if row.id.contains(char::is_whitespace) { "KEY" } else { &row.id },
            spec.name
        ));
    }
    Ok(row)
}

command! {
    pub KEY_GET = ["redis", "key", "get"], Read,
    "Show a Redis key: value decoded, type, size, when it expires and was last used",
    keywords: ["value", "ttl", "expiry", "expire", "cached", "lifespan", "json", "gzip", "pickle", "hash"],
    example: "redis key get session:7f3a --instance orders-qa --fields type,expires_at,value",
    run: key_get,
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use agent_cli_core::testing::non_utc_times;
    use serde_json::json;

    use crate::session::Reply;
    use crate::testing::{array, bulk, config, redis, serve};

    fn meta(key: &str, kind: &str, ttl: i64) -> Vec<(String, Reply)> {
        vec![
            (format!("TYPE {key}"), Reply::Status(kind.into())),
            (format!("PTTL {key}"), Reply::Int(ttl)),
            (format!("MEMORY USAGE {key}"), Reply::Int(120)),
            (format!("OBJECT ENCODING {key}"), bulk("embstr")),
            (format!("OBJECT IDLETIME {key}"), Reply::Int(30)),
            (
                format!("OBJECT FREQ {key}"),
                Reply::Error(
                    "ERR An LFU maxmemory policy is not selected, access frequency not tracked."
                        .into(),
                ),
            ),
        ]
    }

    fn run(
        script: Vec<(String, Reply)>,
        argv: &[&str],
    ) -> (agent_cli_core::testing::Outcome, Vec<String>) {
        let fake = serve(
            script
                .iter()
                .map(|(command, reply)| (command.as_str(), reply.clone()))
                .collect(),
        );
        let outcome = redis(argv, &config(&[("orders-qa", fake.port)]));
        (outcome, fake.sent())
    }

    #[test]
    fn a_string_prints_decoded_with_when_it_expires_and_was_last_used() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(br#"{"user":42,"roles":["reader"]}"#).unwrap();
        let gz = gz.finish().unwrap();
        let mut script = meta("session:7f3a", "string", 3_600_000);
        script.push(("STRLEN session:7f3a".into(), Reply::Int(gz.len() as i64)));
        script.push(("GETRANGE session:7f3a 0 1048575".into(), Reply::Bulk(gz)));
        let (outcome, sent) = run(script, &["redis", "key", "get", "session:7f3a"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["value"], json!({"user": 42, "roles": ["reader"]}));
        assert_eq!(
            (
                row["type"].as_str(),
                row["format"].as_str(),
                row["compression"].as_str()
            ),
            (Some("string"), Some("json"), Some("gzip"))
        );
        assert_eq!(
            (row["ttl_s"].as_i64(), row["size_bytes"].as_i64()),
            (Some(3600), Some(120))
        );
        assert!(
            row["expires_at"].is_string()
                && row["last_access"].is_string()
                && row.get("access_frequency").is_none(),
            "{row}"
        );
        assert!(non_utc_times(&row).is_empty());
        assert!(
            sent.iter()
                .all(|command| !command.starts_with("GET ") && !command.starts_with("KEYS")),
            "{sent:?}"
        );
    }

    #[test]
    fn a_dotnet_cache_entry_shows_its_own_expiry_and_its_data() {
        let mut script = meta("orders:4417", "hash", -1);
        script.push(("HLEN orders:4417".into(), Reply::Int(3)));
        script.push((
            "HSCAN orders:4417 0 COUNT 50".into(),
            array([
                bulk("0"),
                array([
                    bulk("absexp"),
                    bulk("639000000000000000"),
                    bulk("sldexp"),
                    bulk("12000000000"),
                    bulk("data"),
                    bulk(r#"{"id":4417,"total":"129.95"}"#),
                ]),
            ]),
        ));
        let (outcome, _) = run(script, &["redis", "key", "get", "orders:4417"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = outcome.json();
        assert_eq!(row["envelope"], "IDistributedCache");
        assert_eq!(row["absolute_expires_at"], "2025-11-29T08:00:00Z");
        assert_eq!(row["sliding_s"], 1200);
        assert_eq!(row["value"], json!({"id": 4417, "total": "129.95"}));
        assert!(
            row.get("expires_at").is_none(),
            "PTTL -1: it never expires: {row}"
        );
    }

    #[test]
    fn a_big_hash_prints_its_first_members_and_a_pickle_is_named_not_loaded() {
        let mut script = meta("cart:9", "hash", -1);
        script.push(("HLEN cart:9".into(), Reply::Int(500)));
        script.push((
            "HSCAN cart:9 0 COUNT 10".into(),
            array([
                bulk("17"),
                array((0..10).flat_map(|i| [bulk(format!("sku{i}")), bulk(i.to_string())])),
            ]),
        ));
        script.extend(meta(":1:report:7", "string", -1));
        let pickle = b"\x80\x04\x95\x05\x00\x00\x00\x00\x00\x00\x00K\x07.".to_vec();
        script.push(("STRLEN :1:report:7".into(), Reply::Int(pickle.len() as i64)));
        script.push(("GETRANGE :1:report:7 0 1048575".into(), Reply::Bulk(pickle)));
        let fake = serve(
            script
                .iter()
                .map(|(command, reply)| (command.as_str(), reply.clone()))
                .collect(),
        );
        let config = config(&[("orders-qa", fake.port)]);
        let outcome = redis(&["redis", "key", "get", "cart:9", "--limit", "3"], &config);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["value"],
            json!({"sku0": "0", "sku1": "1", "sku2": "2"})
        );
        assert!(
            outcome.stderr.contains("[3 of 500 members; --limit N]"),
            "{}",
            outcome.stderr
        );
        let outcome = redis(&["redis", "key", "get", ":1:report:7"], &config);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["format"], "pickle");
        assert!(outcome.json().get("value").is_none());
        assert!(
            outcome.stderr.contains("python -m pickletools FILE"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_missing_key_is_exit_four_naming_the_list_of_its_neighbours() {
        let (outcome, _) = run(
            vec![
                ("TYPE session:nope".into(), Reply::Status("none".into())),
                ("PTTL session:nope".into(), Reply::Int(-2)),
                ("MEMORY USAGE session:nope".into(), Reply::Nil),
                ("OBJECT ENCODING session:nope".into(), Reply::Nil),
                ("OBJECT IDLETIME session:nope".into(), Reply::Nil),
                ("OBJECT FREQ session:nope".into(), Reply::Nil),
            ],
            &["redis", "key", "get", "session:nope"],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli redis key list session:* --instance orders-qa"),
            "{}",
            outcome.stderr
        );
    }
}
