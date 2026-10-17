//! `redis key diff`: one key in two instances (qa's and prod's), or two
//! keys, by what they hold: type, whether they expire, how they are
//! stored, and the value decoded, so gzip JSON in qa and plain JSON in prod
//! compare by content. A set compares by member, a sorted set by each
//! member's score and a stream by its entries' fields; a pickle or a string
//! past 1 MB by a digest of its bytes as well.

use std::hash::{DefaultHasher, Hasher};

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::{Map, Value, json};

use super::{Reach, read, unescape};
use crate::config::{Instance, Redis};
use crate::session::{Session, cmd, on};

#[derive(clap::Args)]
pub struct KeyDiffArgs {
    /// One side: [INSTANCE:]KEY (orders-qa:config:flags); no prefix is --instance's
    left: String,
    /// The other side, as left
    right: String,
    /// Instance name from instance list; defaults to the only one
    #[arg(long)]
    instance: Option<String>,
    #[command(flatten)]
    diff: DiffArgs,
}

/// Members compared of a collection; a bigger one compares its first ones.
// ponytail: HSCAN and SSCAN order differs between two instances, so past
// 10,000 members two hashes or sets compare only the members both sides
// read, and their lengths. Read the left's fields from the right (HMGET,
// SMISMEMBER) if big ones come up.
const MEMBERS: usize = 10_000;

fn key_diff(ctx: &Ctx, args: KeyDiffArgs) -> Result<Diff> {
    let redis = Redis::load(ctx.config())?;
    let side = |raw: &str| -> Result<(Side, bool)> {
        let (spec, key) = sided(&redis, raw, args.instance.as_deref())?;
        let reach = Reach {
            show: super::READ,
            member: super::READ,
            members: MEMBERS,
        };
        let (row, digest) = on(ctx, spec, |session| {
            let row = read(session, &key, reach)?;
            // A pickle prints no value and a long string only its first
            // MB: the bytes themselves say whether two are the same.
            let opaque = row.truncated || row.format.as_deref() == Some("pickle");
            let digest = if row.kind == "string" && opaque {
                Some(digest(session, &key, row.length.unwrap_or_default())?)
            } else {
                None
            };
            Ok((row, digest))
        })?;
        let cut = row.truncated && digest.is_none();
        if cut {
            ctx.note(format!(
                "[{}:{} is compared by its length and the first {MEMBERS} members read]",
                spec.name, row.id
            ));
        }
        let mut compared = json!({
            "type": row.kind,
            "expires": row.expires_at.is_some(),
            "envelope": row.envelope,
            "sliding_s": row.sliding_s,
            "compression": row.compression,
            "format": row.format,
            "value": value(&row.kind, row.value),
            "digest": digest,
        });
        if row.kind == "stream" {
            let (ids, fields): (Vec<Value>, Vec<Value>) = compared["value"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| (entry["id"].clone(), entry["fields"].clone()))
                .unzip();
            (compared["ids"], compared["value"]) = (json!(ids), json!(fields));
        }
        if cut {
            compared["length"] = json!(row.length);
        }
        Ok((
            Side::new(format!("{}:{}", spec.name, row.id), compared),
            cut,
        ))
    };
    let ((mut left, left_cut), (mut right, right_cut)) = (side(&args.left)?, side(&args.right)?);
    // A digest or a length on one side only would be a difference of its
    // own: each compares only where both sides have one.
    let digests = !left.row["digest"].is_null() && !right.row["digest"].is_null();
    for row in [&mut left.row, &mut right.row] {
        if !digests {
            row["digest"] = Value::Null;
        }
        if left_cut != right_cut {
            row["length"] = Value::Null;
        }
    }
    // Two hashes or sets cut short hold different members, scanned in each
    // instance's own order: one read on one side only says nothing.
    if left_cut || right_cut {
        let both = |row: &Value, other: &Value| -> Option<Value> {
            let (mine, theirs) = (row["value"].as_object()?, other["value"].as_object()?);
            let kept = mine.iter().filter(|(key, _)| theirs.contains_key(*key));
            Some(Value::Object(
                kept.map(|(k, v)| (k.clone(), v.clone())).collect(),
            ))
        };
        let (l, r) = (both(&left.row, &right.row), both(&right.row, &left.row));
        if let (Some(l), Some(r)) = (l, r) {
            (left.row["value"], right.row["value"]) = (l, r);
        }
    }
    diff(ctx, &args.diff, &RULES, left, right)
}

/// A stream's entry ids are the times they were added, different in every
/// instance: noise unless `--all`.
const RULES: Rules = Rules {
    noise: &["ids"],
    ..Rules::PLAIN
};

/// A set as its members and a sorted set as its members' scores, keyed by
/// member, so one member more is one difference wherever it sorts.
fn value(kind: &str, value: Value) -> Value {
    let key = |member: &Value| match member {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let items = value.as_array().into_iter().flatten();
    match kind {
        "set" => Value::Object(items.map(|one| (key(one), json!(true))).collect()),
        "zset" => Value::Object(
            items
                .map(|pair| (key(&pair["member"]), pair["score"].clone()))
                .collect::<Map<_, _>>(),
        ),
        _ => value,
    }
}

/// A string's whole bytes hashed a MB at a time, never held at once.
fn digest(session: &mut Session, key: &[u8], length: i64) -> Result<String> {
    let mut hasher = DefaultHasher::new();
    let mut at = 0;
    while at < length {
        let end = at + super::READ as i64 - 1;
        let got = session.call(cmd!("GETRANGE", key, at.to_string(), end.to_string()))?;
        let bytes = got.bytes().unwrap_or_default();
        if bytes.is_empty() {
            break;
        }
        hasher.write(bytes);
        at += bytes.len() as i64;
    }
    Ok(format!("{:016x}", hasher.finish()))
}

/// `INSTANCE:KEY` when the text before the first colon names a configured
/// instance; else the whole text is a key on `--instance`, since keys hold
/// colons of their own (`session:42`, Django's `:1:report`).
fn sided<'a>(redis: &'a Redis, raw: &str, flag: Option<&str>) -> Result<(&'a Instance, Vec<u8>)> {
    if let Some((scope, key)) = raw.split_once(':')
        && let Some(spec) = redis.instances.iter().find(|spec| spec.name == scope)
        && !key.is_empty()
    {
        return Ok((spec, unescape(key)));
    }
    Ok((redis.instance(flag)?, unescape(raw)))
}

command! {
    pub KEY_DIFF = ["redis", "key", "diff"], Read,
    "Compare a Redis key in two instances (qa and prod): value decoded, type, expiry",
    keywords: ["environments", "staging", "drift", "cached", "value", "flags", "config"],
    example: "redis key diff orders-qa:config:feature-flags orders-prod:config:feature-flags",
    run: key_diff,
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;

    use crate::session::Reply;
    use crate::testing::{array, bulk, config, redis, serve};

    fn meta(key: &str, kind: &str) -> Vec<(String, Reply)> {
        vec![
            (format!("TYPE {key}"), Reply::Status(kind.into())),
            (format!("PTTL {key}"), Reply::Int(-1)),
            (format!("MEMORY USAGE {key}"), Reply::Int(80)),
            (format!("OBJECT ENCODING {key}"), bulk("raw")),
            (format!("OBJECT IDLETIME {key}"), Reply::Int(1)),
            (
                format!("OBJECT FREQ {key}"),
                Reply::Error("ERR not LFU".into()),
            ),
        ]
    }

    fn string(key: &str, value: Vec<u8>) -> Vec<(String, Reply)> {
        let mut script = meta(key, "string");
        script.push((format!("STRLEN {key}"), Reply::Int(value.len() as i64)));
        script.push((format!("GETRANGE {key} 0 1048575"), Reply::Bulk(value)));
        script
    }

    fn set(key: &str, members: &[&str]) -> Vec<(String, Reply)> {
        let mut script = meta(key, "set");
        script.push((format!("SCARD {key}"), Reply::Int(members.len() as i64)));
        script.push((
            format!("SSCAN {key} 0 COUNT 1000"),
            array([bulk("0"), array(members.iter().map(bulk))]),
        ));
        script
    }

    fn paths(answer: &serde_json::Value) -> Vec<&str> {
        answer["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["path"].as_str().unwrap())
            .collect()
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
    fn one_key_in_qa_and_prod_compares_decoded_and_names_the_environments() {
        let flags = br#"{"checkout_v2":true,"search":"legacy"}"#;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(br#"{"checkout_v2":false,"search":"legacy"}"#)
            .unwrap();
        let qa = fake(string("config:feature-flags", flags.to_vec()));
        let prod = fake(string("config:feature-flags", gz.finish().unwrap()));
        let outcome = redis(
            &[
                "redis",
                "key",
                "diff",
                "orders-qa:config:feature-flags",
                "orders-prod:config:feature-flags",
            ],
            &config(&[("orders-qa", qa.port), ("orders-prod", prod.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let answer = outcome.json();
        assert_eq!(answer["left"], "orders-qa:config:feature-flags");
        assert_eq!(answer["as"], json!(["qa=prod"]));
        assert_eq!(
            paths(&answer),
            ["compression", "value.checkout_v2"],
            "{answer}"
        );
    }

    #[test]
    fn a_set_differs_by_the_member_one_side_lacks_wherever_it_sorts() {
        let qa = fake(set("regions", &["ap-south", "eu-west", "us-east"]));
        let prod = fake(set("regions", &["us-east", "eu-west"]));
        let outcome = redis(
            &["redis", "key", "diff", "qa:regions", "prod:regions"],
            &config(&[("qa", qa.port), ("prod", prod.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let answer = outcome.json();
        assert_eq!(paths(&answer), ["value.ap-south"], "{answer}");
        assert_eq!(answer["changes"][0]["left"], true);
    }

    #[test]
    fn two_pickles_compare_by_their_bytes_since_neither_prints_a_value() {
        let qa = fake(string(":1:report", b"\x80\x05rows=[1,2,3]".to_vec()));
        let prod = fake(string(":1:report", b"\x80\x05rows=[1,2,4]".to_vec()));
        let argv = ["redis", "key", "diff", "qa::1:report", "prod::1:report"];
        let outcome = redis(&argv, &config(&[("qa", qa.port), ("prod", prod.port)]));
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let answer = outcome.json();
        assert_eq!(answer["same"], false, "{answer}");
        assert_eq!(paths(&answer), ["digest"], "{answer}");
    }

    #[test]
    fn a_side_without_an_instance_prefix_is_a_key_on_the_instance_flag() {
        let one = fake(
            [
                string(":1:report:7", b"a".to_vec()),
                string(":1:report:8", b"a".to_vec()),
            ]
            .concat(),
        );
        let outcome = redis(
            &[
                "redis",
                "key",
                "diff",
                ":1:report:7",
                ":1:report:8",
                "--instance",
                "qa",
            ],
            &config(&[("qa", one.port), ("prod", 9)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["same"], true);
    }
}
