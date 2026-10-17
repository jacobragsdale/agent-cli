//! Answers the lake's tests share: Path List entries, and Parquet files
//! DuckDB writes.

use agent_cli_core::testing::Answer;
use serde_json::{Value, json};

/// A folder in a Path List, `path` within its container.
pub fn dir(path: &str) -> Value {
    json!({"name": path, "isDirectory": "true", "contentLength": "0", "etag": "0x8DDFF0000000001",
        "lastModified": "Mon, 29 Sep 2026 06:00:00 GMT", "owner": "$superuser", "group": "$superuser",
        "permissions": "rwxr-x---"})
}

/// A file in a Path List, written at 06:02:11 on the world's day.
pub fn file(path: &str, size: usize) -> Value {
    file_at(path, size, "Mon, 29 Sep 2026 06:02:11 GMT")
}

pub fn file_at(path: &str, size: usize, modified: &str) -> Value {
    json!({"name": path, "contentLength": size.to_string(), "etag": "0x8DDFF0000000002",
        "lastModified": modified, "owner": "$superuser", "group": "$superuser", "permissions": "rw-r-----"})
}

/// One Path List page.
pub fn listing(entries: Vec<Value>) -> Answer {
    Answer::json(&json!({"paths": entries}))
}

/// The Parquet file DuckDB writes for `select`.
pub fn parquet(select: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.parquet");
    duckdb::Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY ({select}) TO '{}' (FORMAT parquet)",
            path.display()
        ))
        .unwrap();
    std::fs::read(path).unwrap()
}

/// What a tail read of `bytes` answers: the last 64 KiB at most.
pub fn tail(bytes: &[u8]) -> Answer {
    let from = bytes.len().saturating_sub(64 * 1024);
    Answer {
        status: 206,
        ..Answer::bytes(&bytes[from..])
    }
}
