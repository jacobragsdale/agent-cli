//! Every redis command against `scripts/redis-up.sh`'s two instances (qa:
//! plain TCP, a read-only ACL user, LRU; prod: TLS with a self-signed
//! certificate, LFU), through the real dispatcher.
//!
//! Skipped unless `AGENT_CLI_TEST_REDIS=1`.

use std::io::{Read, Write};
use std::net::TcpStream;

use agent_cli_core::Setup;
use agent_cli_core::testing::{FakeTransport, Outcome, run};
use serde_json::{Value, json};

const CONFIG: &str = r#"
[[redis.instance]]
name = "orders-qa"
host = "127.0.0.1"
port = 16379
user = "agent"
password = "agent-pass"

[[redis.instance]]
name = "orders-prod"
host = "localhost"
port = 16380
tls = true
trust_cert = true
password = "prod-pass"

[[redis.instance]]
name = "untrusted"
host = "localhost"
port = 16380
tls = true
password = "prod-pass"

[[redis.instance]]
name = "nobody"
host = "127.0.0.1"
port = 16379
user = "nobody"
password = "nobody-pass"

[[redis.instance]]
name = "plain-to-tls"
host = "127.0.0.1"
port = 16380
password = "prod-pass"
"#;

fn wanted() -> bool {
    let on = std::env::var("AGENT_CLI_TEST_REDIS").is_ok_and(|value| value == "1");
    if !on {
        eprintln!("skipped: set AGENT_CLI_TEST_REDIS=1 with scripts/redis-up.sh's instances up");
    }
    on
}

fn redis(argv: &[&str]) -> Outcome {
    let setup = Setup::fake(FakeTransport::default()).with_config(CONFIG);
    run(&[agent_cli_redis::DOMAIN], argv, setup)
}

fn ok(argv: &[&str]) -> (Value, String) {
    let outcome = redis(argv);
    assert_eq!(outcome.code, 0, "{argv:?}: {outcome:?}");
    (outcome.json(), outcome.stderr)
}

#[test]
fn health_reads_both_instances_over_plain_tcp_and_tls() {
    if !wanted() {
        return;
    }
    let (rows, _) = ok(&["redis", "instance", "list"]);
    for row in rows.as_array().unwrap() {
        if ["orders-qa", "orders-prod"].contains(&row["id"].as_str().unwrap()) {
            assert!(row["version"].as_str().unwrap().starts_with('7'), "{row}");
            assert!(row["keys"].as_i64().unwrap() > 22_000, "{row}");
        } else {
            assert!(row["error"].is_string(), "{row}");
        }
    }
    let (qa, _) = ok(&["redis", "instance", "get", "orders-qa"]);
    assert_eq!(qa["memory"]["policy"], "allkeys-lru");
    assert_eq!(qa["memory"]["maxmemory_bytes"], 64 << 20);
    assert_eq!(qa["keyspace"][0]["db"], 0);
    let (prod, _) = ok(&["redis", "instance", "get", "orders-prod"]);
    assert_eq!(prod["memory"]["policy"], "allkeys-lfu");
    let two = redis(&["redis", "instance", "get", "rediss://:x@localhost:16380/0"]);
    assert_eq!(two.code, 2, "{two:?}");
    assert!(
        two.stderr.contains(
            "localhost is more than one instance; name one with --instance: orders-prod, untrusted"
        ),
        "{}",
        two.stderr
    );
}

#[test]
fn keys_are_scanned_described_and_ranked() {
    if !wanted() {
        return;
    }
    let (rows, notes) = ok(&[
        "redis",
        "key",
        "list",
        "session:*",
        "--instance",
        "orders-qa",
        "--limit",
        "5",
    ]);
    assert_eq!(rows.as_array().unwrap().len(), 5);
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .all(|row| row["type"] == "string" && row["expires_at"].is_string())
    );
    assert!(notes.contains("more match"), "{notes}");
    let (rows, _) = ok(&[
        "redis",
        "key",
        "list",
        "--instance",
        "orders-qa",
        "--sort",
        "size",
        "--limit",
        "2",
        "--fields",
        "id",
    ]);
    assert_eq!(rows, json!([{"id": "big:blob"}, {"id": "big:hash"}]));
    let (rows, _) = ok(&[
        "redis",
        "key",
        "list",
        "--instance",
        "orders-prod",
        "--type",
        "zset",
    ]);
    assert_eq!(rows[0]["id"], "leaderboard");
    let (rows, _) = ok(&["redis", "key", "list", "bin:*", "--instance", "orders-qa"]);
    let id = rows[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(id, "bin:\\xff\\x00");
    let (key, _) = ok(&["redis", "key", "get", &id, "--instance", "orders-qa"]);
    assert_eq!(
        (key["format"].as_str(), key["value"].as_str()),
        (Some("base64"), Some("AAH/"))
    );
}

#[test]
fn every_value_shape_decodes() {
    if !wanted() {
        return;
    }
    let get = |key: &str| ok(&["redis", "key", "get", key, "--instance", "orders-qa"]);
    let (session, _) = get("session:00001");
    assert_eq!(
        session["value"],
        json!({"user": 1, "roles": ["reader"], "env": "qa"})
    );
    assert!(session["ttl_s"].as_i64().unwrap() > 0 && session["last_access"].is_string());
    for (key, compression) in [("gz:report", "gzip"), ("zlib:report", "zlib")] {
        let (row, _) = get(key);
        assert_eq!(
            (row["compression"].as_str(), row["format"].as_str()),
            (Some(compression), Some("json")),
            "{row}"
        );
        assert_eq!(row["value"]["rows"][29]["orders"], 129);
    }
    let (zstd, _) = get("zstd:blob");
    assert_eq!(
        (zstd["compression"].as_str(), zstd["format"].as_str()),
        (Some("zstd"), Some("base64"))
    );
    let (pickle, notes) = get(":1:report:7");
    assert_eq!(pickle["format"], "pickle");
    assert!(
        pickle.get("value").is_none() && notes.contains("pickletools"),
        "{notes}"
    );
    let (dotnet, _) = get("dotnet:orders:4417");
    assert_eq!(
        (dotnet["envelope"].as_str(), dotnet["sliding_s"].as_i64()),
        (Some("IDistributedCache"), Some(1200))
    );
    assert_eq!(dotnet["value"], json!({"id": 4417, "total": "129.95"}));
    assert!(dotnet["absolute_expires_at"].is_string() && dotnet["expires_at"].is_string());
    let (persistent, _) = get("persistent:settings");
    assert!(persistent.get("expires_at").is_none());
    let (big, notes) = get("big:blob");
    assert_eq!(
        (big["truncated"].as_bool(), big["length"].as_i64()),
        (Some(true), Some(2 << 20))
    );
    assert!(notes.contains("--output FILE"), "{notes}");
    for (key, kind, length) in [
        ("big:hash", "hash", 5000),
        ("queue:emails", "list", 100),
        ("tags:popular", "set", 50),
        ("leaderboard", "zset", 100),
        ("events", "stream", 50),
    ] {
        let (row, notes) = ok(&[
            "redis",
            "key",
            "get",
            key,
            "--instance",
            "orders-qa",
            "--limit",
            "3",
        ]);
        assert_eq!(
            (row["type"].as_str(), row["length"].as_i64()),
            (Some(kind), Some(length)),
            "{row}"
        );
        assert_eq!(
            row["value"]
                .as_array()
                .map_or_else(|| row["value"].as_object().unwrap().len(), Vec::len),
            3,
            "{row}"
        );
        assert!(
            notes.contains(&format!("[3 of {length} members")),
            "{notes}"
        );
    }
    let (lfu, _) = ok(&[
        "redis",
        "key",
        "get",
        "session:00001",
        "--instance",
        "orders-prod",
    ]);
    assert!(
        lfu["access_frequency"].is_i64() && lfu.get("last_access").is_none(),
        "{lfu}"
    );
    let missing = redis(&[
        "redis",
        "key",
        "get",
        "session:nope",
        "--instance",
        "orders-qa",
    ]);
    assert_eq!(missing.code, 4, "{missing:?}");
}

#[test]
fn output_saves_a_whole_value_decompressed() {
    if !wanted() {
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("blob");
    let path = path.to_str().unwrap();
    ok(&[
        "redis",
        "key",
        "get",
        "big:blob",
        "--instance",
        "orders-qa",
        "--output",
        path,
    ]);
    assert_eq!(std::fs::metadata(path).unwrap().len(), 2 << 20);
    ok(&[
        "redis",
        "key",
        "get",
        "gz:report",
        "--instance",
        "orders-qa",
        "--output",
        path,
    ]);
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["rows"][0], json!({"day": 0, "orders": 100}));
}

#[test]
fn a_key_in_qa_and_prod_compares_by_content() {
    if !wanted() {
        return;
    }
    let (answer, _) = ok(&[
        "redis",
        "key",
        "diff",
        "orders-qa:config:feature-flags",
        "orders-prod:config:feature-flags",
    ]);
    assert_eq!(answer["same"], false);
    let paths: Vec<&str> = answer["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["compression", "value.checkout_v2"]);
    let (answer, _) = ok(&[
        "redis",
        "key",
        "diff",
        "orders-qa:big:hash",
        "orders-prod:big:hash",
    ]);
    assert_eq!(answer["same"], true, "{answer}");
}

#[test]
fn the_slow_log_and_the_clients_print_no_value() {
    if !wanted() {
        return;
    }
    let mut other = TcpStream::connect("127.0.0.1:16379").unwrap();
    other
        .write_all(b"*3\r\n$4\r\nAUTH\r\n$5\r\nagent\r\n$10\r\nagent-pass\r\n*3\r\n$6\r\nCLIENT\r\n$7\r\nSETNAME\r\n$10\r\norders-api\r\n")
        .unwrap();
    let mut answer = [0; 10];
    other.read_exact(&mut answer).unwrap();
    assert_eq!(&answer, b"+OK\r\n+OK\r\n");
    let (clients, _) = ok(&[
        "redis",
        "client",
        "list",
        "--instance",
        "orders-qa",
        "--limit",
        "500",
    ]);
    assert!(
        clients
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "orders-api" && row["user"] == "agent"),
        "{clients}"
    );
    let outcome = redis(&[
        "redis",
        "slowlog",
        "list",
        "--instance",
        "orders-qa",
        "--limit",
        "1024",
        "--raw",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert!(
        !outcome.stdout.contains("s3cret") && !outcome.stdout.contains("agent-pass"),
        "a value printed"
    );
    assert!(
        outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["command"] == "CLIENT SETNAME")
    );
}

#[test]
fn refusals_name_their_fix() {
    if !wanted() {
        return;
    }
    for (instance, code, says) in [
        ("untrusted", 3, "trust_cert = true"),
        ("nobody", 3, "+@read -@dangerous"),
        ("plain-to-tls", 1, "set tls = true"),
    ] {
        let outcome = redis(&["redis", "instance", "get", instance, "--timeout", "5"]);
        assert_eq!(outcome.code, code, "{instance}: {outcome:?}");
        assert!(
            outcome.stderr.contains(says),
            "{instance}: {}",
            outcome.stderr
        );
    }
    let outcome = redis(&["doctor", "redis"]);
    let checks: Vec<(String, bool)> = outcome
        .json()
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["domain"] == "redis")
        .map(|check| {
            (
                check["check"].as_str().unwrap().to_owned(),
                check["ok"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(
        checks.contains(&("instance orders-qa".to_owned(), true)),
        "{checks:?}"
    );
    assert!(
        checks.contains(&("instance orders-prod".to_owned(), true)),
        "{checks:?}"
    );
    assert!(
        checks.contains(&("instance untrusted".to_owned(), false)),
        "{checks:?}"
    );
}
