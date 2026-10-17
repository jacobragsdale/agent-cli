//! `redis key diff`: one key in two instances (qa's and prod's), or two
//! keys, by what they hold: type, whether they expire, how they are
//! stored, and the value decoded, so gzip JSON in qa and plain JSON in prod
//! compare by content.

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::{Map, Value, json};

use super::{Reach, read, unescape};
use crate::config::{Instance, Redis};
use crate::session::on;

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
const MEMBERS: usize = 10_000;

fn key_diff(ctx: &Ctx, args: KeyDiffArgs) -> Result<Diff> {
    let redis = Redis::load(ctx.config())?;
    let side = |raw: &str| -> Result<Side> {
        let (spec, key) = sided(&redis, raw, args.instance.as_deref())?;
        let reach = Reach {
            show: super::READ,
            member: super::READ,
            members: MEMBERS,
        };
        let row = on(ctx, spec, |session| read(session, &key, reach))?;
        if row.truncated {
            ctx.note(format!(
                "[{}:{} is compared by its first {} members or 1 MB]",
                spec.name, row.id, MEMBERS
            ));
        }
        let value = match row.kind.as_str() {
            // A set has no order, and a sorted set's members are its keys.
            "zset" => Value::Object(
                row.value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|pair| (pair["member"].to_string(), pair["score"].clone()))
                    .collect::<Map<_, _>>(),
            ),
            _ => row.value,
        };
        Ok(Side::new(
            format!("{}:{}", spec.name, row.id),
            json!({
                "type": row.kind,
                "expires": row.expires_at.is_some(),
                "envelope": row.envelope,
                "sliding_s": row.sliding_s,
                "compression": row.compression,
                "format": row.format,
                "value": value,
            }),
        ))
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &Rules::PLAIN, left, right)
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
    use crate::testing::{bulk, config, redis, serve};

    fn string(key: &str, value: Vec<u8>) -> Vec<(String, Reply)> {
        vec![
            (format!("TYPE {key}"), Reply::Status("string".into())),
            (format!("PTTL {key}"), Reply::Int(-1)),
            (format!("MEMORY USAGE {key}"), Reply::Int(80)),
            (format!("OBJECT ENCODING {key}"), bulk("raw")),
            (format!("OBJECT IDLETIME {key}"), Reply::Int(1)),
            (
                format!("OBJECT FREQ {key}"),
                Reply::Error("ERR not LFU".into()),
            ),
            (format!("STRLEN {key}"), Reply::Int(value.len() as i64)),
            (format!("GETRANGE {key} 0 1048575"), Reply::Bulk(value)),
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
        let paths: Vec<&str> = answer["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["compression", "value.checkout_v2"], "{answer}");
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
