//! What the contoso world answers of lake: step 0's five trial tasks, each
//! in the calls agent-cli takes. Compiled only with the `fixtures` feature.
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
        json!({"name": "customer_id", "type": "BIGINT"})
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
fn todays_orders_without_a_customer_and_the_days_curated_never_got() {
    let nulls = ok(&[
        "lake",
        "query",
        "run",
        "select count(*) filter (where customer_id is null) as no_customer from o",
        "--table",
        &format!("o={ORDERS}/date=2026-09-29"),
    ]);
    assert_eq!(nulls["results"][0]["rows"], json!([[2]]));
    let gap = ok(&[
        "lake",
        "query",
        "run",
        "select date from (select distinct date from raw) anti join daily using (date) order by date",
        "--table",
        &format!("raw={ORDERS}/date=2026-09-2*"),
        "--table",
        "daily=contosolake/curated/sales/orders_daily",
    ]);
    assert_eq!(
        gap["results"][0]["rows"],
        json!([["2026-09-28"], ["2026-09-29"]])
    );
    assert_eq!(gap["read"]["files"], 7);
}
