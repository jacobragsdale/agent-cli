//! What the contoso world answers of lake: step 0's trial tasks that read
//! the storage itself (queries over the files go to Synapse, through
//! `sql`). Compiled only with the `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{agent_cli, ok};

const ORDERS: &str = "contosolake/raw/sales/orders";

#[test]
fn every_container_comes_with_its_data_domains_and_their_tables() {
    let containers = ok(&["lake", "container", "list", "--fields", "id,folders"]);
    assert_eq!(
        containers,
        json!([
            {"id": "contosolake/curated", "folders": ["contosolake/curated/finance", "contosolake/curated/sales"]},
            {"id": "contosolake/raw", "folders": ["contosolake/raw/finance", "contosolake/raw/sales"]},
            {"id": "contosolakedev/scratch"},
        ])
    );
    let tables = ok(&["lake", "table", "list", "--fields", "id,partitioned_by"]);
    assert_eq!(tables.as_array().map(Vec::len), Some(4), "{tables}");
    assert_eq!(tables[3], json!({"id": ORDERS, "partitioned_by": ["date"]}));
    let accounts = ok(&["lake", "account", "list", "--fields", "id,endpoints"]);
    assert!(
        accounts[0]["endpoints"][0]
            .as_str()
            .unwrap()
            .ends_with("/pe-contosolake-dfs"),
        "{accounts}"
    );
    ok(&[
        "lake",
        "path",
        "list",
        "abfss://raw@contosolake.dfs.core.windows.net/sales",
        "--recursive",
    ]);
}

#[test]
fn todays_partition_landed_at_six_and_one_day_is_missing() {
    let partitions = agent_cli(&[
        "lake",
        "partition",
        "list",
        ORDERS,
        "--fields",
        "id,rows,modified",
    ]);
    assert_eq!(partitions.code, 0, "{}", partitions.stderr);
    assert_eq!(
        partitions.json()[0],
        json!({"id": format!("{ORDERS}/date=2026-09-29"), "rows": 7, "modified": "2026-09-29T06:02:11Z"})
    );
    assert!(
        partitions
            .stderr
            .contains("[missing between 2026-09-23 and 2026-09-29: date=2026-09-27]"),
        "{}",
        partitions.stderr
    );
    let file = ok(&[
        "lake",
        "path",
        "get",
        &format!("{ORDERS}/date=2026-09-29/part-0001.parquet"),
        "--fields",
        "rows,writer,schema",
    ]);
    assert_eq!(file["rows"], 7);
    assert_eq!(
        file["schema"][1],
        json!({"name": "customer_id", "type": "INT64"})
    );
}

#[test]
fn the_ledgers_schema_and_rows_come_from_its_footer() {
    let ledger = ok(&[
        "lake",
        "table",
        "get",
        "contosolake/curated/finance/ledger",
        "--fields",
        "schema,rows",
    ]);
    assert_eq!(ledger["rows"], 120);
    assert_eq!(
        ledger["schema"][2],
        json!({"name": "amount", "type": "DECIMAL(12,2)"})
    );
}

#[test]
fn a_tables_bulk_path_is_what_synapse_reads_its_partitions_with() {
    let table = ok(&[
        "lake",
        "table",
        "get",
        ORDERS,
        "--fields",
        "partitioned_by,bulk",
    ]);
    assert_eq!(
        table["bulk"],
        "https://contosolake.dfs.core.windows.net/raw/sales/orders/date=*/*.parquet"
    );
    let partitions = ok(&[
        "lake",
        "partition",
        "list",
        ORDERS,
        "--limit",
        "1",
        "--fields",
        "bulk",
    ]);
    assert_eq!(
        partitions[0]["bulk"],
        "https://contosolake.dfs.core.windows.net/raw/sales/orders/date=2026-09-29/*.parquet"
    );
}

/// Today's partition has yesterday's columns; raw orders and the curated
/// daily table share none.
#[test]
fn todays_partition_has_yesterdays_schema_and_curated_has_its_own() {
    let days = ok(&[
        "lake",
        "table",
        "diff",
        &format!("{ORDERS}/date=2026-09-28"),
        &format!("{ORDERS}/date=2026-09-29"),
    ]);
    assert_eq!(days["same"], true, "{days}");
    let zones = ok(&[
        "lake",
        "table",
        "diff",
        ORDERS,
        "contosolake/curated/sales/orders_daily",
        "--fields",
        "changes",
    ]);
    assert_eq!(
        zones["changes"][5],
        json!({"path": "columns.date", "right": {"type": "DATE"}})
    );
}
