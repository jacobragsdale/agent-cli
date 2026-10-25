//! Every sql command against the three compose databases
//! (`scripts/db-up.sh`), through the real dispatcher, with `config.test.toml`.
//!
//! Skipped unless `AGENT_CLI_TEST_DBS=1`. Test names start with the database
//! they need, so CI can run the SQL Server half alone (`cargo test --test
//! dbs mssql`).

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use agent_cli_core::Setup;
use agent_cli_core::testing::{FakeTransport, Outcome, run};
use serde_json::{Value, json};

static CONFIG: LazyLock<String> = LazyLock::new(|| {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.toml"
    ))
    .expect("config.test.toml at the repo root")
});

fn wanted() -> bool {
    let on = std::env::var("AGENT_CLI_TEST_DBS").is_ok_and(|value| value == "1");
    if !on {
        eprintln!("skipped: set AGENT_CLI_TEST_DBS=1 with scripts/db-up.sh's databases up");
    }
    on
}

fn sql(argv: &[&str]) -> Outcome {
    let setup = Setup::fake(FakeTransport::default()).with_config(&CONFIG);
    run(&[agent_cli_sql::DOMAIN], argv, setup)
}

/// `sql query run` on `conn`, which must succeed; its JSON with nothing
/// dropped (`--raw`).
fn query(conn: &str, text: &str, extra: &[&str]) -> Value {
    let mut argv = vec!["sql", "query", "run", "--conn", conn, text, "--raw"];
    argv.extend(extra);
    let outcome = sql(&argv);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    outcome.json()
}

fn rows(result: &Value, set: usize) -> &Value {
    &result["results"][set]["rows"]
}

// ---------- SQL Server ----------

#[test]
fn mssql_every_type_renders_as_the_contract_says() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-mssql",
        "select * from bench.all_types order by id",
        &[],
    );
    let set = &result["results"][0];
    assert_eq!(set["types"][5], "bigint");
    assert_eq!(
        set["rows"][0],
        json!([
            1,
            true,
            255,
            32767,
            2_147_483_647,
            "9223372036854775807",
            "12345.6789",
            "123.45",
            "1234.5678",
            12_345_678_901.234,
            1.25,
            "abcde",
            "varchar value",
            "nvarchar value",
            "text value",
            "2024-05-17",
            "13:45:30.1234567",
            "2024-05-17T13:45:30.000",
            "2024-05-17T13:45:30.1234567",
            "2024-05-17T13:45:30.1234567+02:00",
            "6f9619ff-8b86-d011-b42d-00c04fc964ff",
            "0x0102030405060708",
            "<root><a id=\"1\">x</a></root>"
        ])
    );
    let nulls = set["rows"][1].as_array().unwrap();
    assert!(nulls[1..].iter().all(Value::is_null), "{nulls:?}");
    assert_eq!(
        (set["rows_affected"].clone(), set["truncated"].clone()),
        (json!(0), json!(false))
    );
}

#[test]
fn mssql_zero_rows_still_carry_columns_and_types() {
    if !wanted() {
        return;
    }
    let raw = query(
        "local-mssql",
        "select id, name from bench.customers where 1 = 0",
        &[],
    );
    assert_eq!(
        raw["results"][0],
        json!({"columns": ["id", "name"], "types": ["int", "nvarchar"], "rows": [],
               "rows_affected": 0, "truncated": false})
    );
    // Without --raw the output contract drops empty lists, and only those.
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "select id from bench.customers where 1 = 0",
    ]);
    assert_eq!(
        outcome.json()["results"][0],
        json!({"columns": ["id"], "types": ["int"], "rows_affected": 0, "truncated": false})
    );
}

#[test]
fn mssql_a_script_splits_only_at_go_so_a_declare_keeps_its_select() {
    if !wanted() {
        return;
    }
    let script = "declare @x int = 41\n\nselect @x + 1 as answer\ngo\nselect 2 as b; select 3 as c";
    let outcome = sql(&["sql", "query", "run", "--conn", "local-mssql", script]);
    assert_eq!(outcome.code, 2, "a declare is not a read: {outcome:?}");
    let result = query("local-mssql", script, &["--yes"]);
    assert_eq!(rows(&result, 0), &json!([[42]]));
    assert_eq!(rows(&result, 1), &json!([[2]]));
    assert_eq!(rows(&result, 2), &json!([[3]]));
}

#[test]
fn mssql_a_read_stops_at_max_rows_and_the_next_batch_reconnects() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "--max-rows",
        "3",
        "select id from bench.events order by id\ngo\nselect count(*) from bench.customers",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let result = outcome.json();
    assert_eq!(rows(&result, 0), &json!([[1], [2], [3]]));
    assert_eq!(result["results"][0]["truncated"], true);
    assert_eq!(rows(&result, 1), &json!([[50]]));
    assert!(
        outcome.stderr.contains("--max-rows 3"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn mssql_a_write_batch_runs_whole_past_max_rows() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-mssql",
        "create table #t (id int)\ninsert #t select top 5 id from bench.orders\n\
         select id from #t\nselect count(*) from #t",
        &["--max-rows", "2", "--yes"],
    );
    assert_eq!(result["results"][0]["truncated"], true);
    assert_eq!(
        rows(&result, 1),
        &json!([[5]]),
        "the batch ran past the cut"
    );
}

#[test]
fn mssql_writes_report_what_they_changed() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-mssql",
        "create table bench.agent_cli_writes (id int)\ngo\n\
         insert bench.agent_cli_writes values (1), (2), (3)\ngo\n\
         update bench.agent_cli_writes set id = id + 1 where id > 1\ngo\n\
         drop table bench.agent_cli_writes",
        &["--yes"],
    );
    let affected: Vec<&Value> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|set| &set["rows_affected"])
        .collect();
    assert_eq!(affected, [&json!(0), &json!(3), &json!(2), &json!(0)]);
}

#[test]
fn mssql_a_failure_names_the_statement_and_what_already_ran() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "select 1\ngo\nselect * from bench.nope",
    ]);
    assert_eq!(
        outcome.code, 4,
        "an unknown table is not found: {outcome:?}"
    );
    assert!(outcome.stdout.is_empty(), "{outcome:?}");
    assert_eq!(
        outcome.stderr.trim(),
        "error: statement 2 of 2 failed; statement 1 already ran and stays committed \
         (1 row returned): line 1: Invalid object name 'bench.nope'.\n\
         hint: agent-cli sql object list --conn local-mssql nope --fields id,kind"
    );
}

/// `select count(*)` of a scratch table, read on a connection of its own.
fn count(table: &str) -> Value {
    rows(
        &query("local-mssql", &format!("select count(*) from {table}"), &[]),
        0,
    )[0][0]
        .clone()
}

#[test]
fn mssql_a_transaction_survives_a_read_cut_at_max_rows_and_commits() {
    if !wanted() {
        return;
    }
    let table = "bench.agent_cli_tran_cut";
    query(
        "local-mssql",
        &format!("drop table if exists {table}\ngo\ncreate table {table} (id int)"),
        &["--yes"],
    );
    let result = query(
        "local-mssql",
        &format!(
            "begin tran\ngo\ninsert {table} values (1)\ngo\n\
             select top 100 id from bench.events\ngo\ncommit\ngo\n\
             select top 1 id into #kept from bench.events\ngo\n\
             select top 100 id from bench.events\ngo\nselect count(*) from #kept"
        ),
        &["--max-rows", "2", "--yes"],
    );
    assert_eq!(result["results"][2]["truncated"], true);
    assert_eq!(
        rows(&result, 6),
        &json!([[1]]),
        "the #temp table outlived the cut"
    );
    assert_eq!(count(table), json!(1), "the insert was committed");
    query("local-mssql", &format!("drop table {table}"), &["--yes"]);
}

#[test]
fn mssql_a_transaction_left_open_is_rolled_back_and_said() {
    if !wanted() {
        return;
    }
    let table = "bench.agent_cli_tran_open";
    query(
        "local-mssql",
        &format!("drop table if exists {table}\ngo\ncreate table {table} (id int)"),
        &["--yes"],
    );
    for script in [
        format!("begin tran\ngo\ninsert {table} values (1)"),
        format!("set implicit_transactions on\ngo\ninsert {table} values (1)"),
    ] {
        let outcome = sql(&[
            "sql",
            "query",
            "run",
            "--conn",
            "local-mssql",
            "--yes",
            &script,
        ]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(outcome.stdout.is_empty(), "{outcome:?}");
        assert!(
            outcome.stderr.contains("left a transaction open")
                && outcome.stderr.contains("hint: end the script with COMMIT"),
            "{}",
            outcome.stderr
        );
        assert_eq!(count(table), json!(0), "{script}");
    }
    query("local-mssql", &format!("drop table {table}"), &["--yes"]);
}

#[test]
fn mssql_a_failure_inside_a_transaction_never_claims_a_commit() {
    if !wanted() {
        return;
    }
    let table = "bench.agent_cli_tran_fail";
    query(
        "local-mssql",
        &format!("drop table if exists {table}\ngo\ncreate table {table} (id int)"),
        &["--yes"],
    );
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "--yes",
        &format!("begin tran\ngo\ninsert {table} values (1)\ngo\nselect 1/0\ngo\ncommit"),
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome.stderr.contains(
            "statement 3 of 4 failed; statements 1-2 ran (1: 0 rows affected, 2: 1 row \
             affected), but a transaction still open at the failure is rolled back"
        ),
        "{}",
        outcome.stderr
    );
    assert_eq!(count(table), json!(0));
    query("local-mssql", &format!("drop table {table}"), &["--yes"]);
}

#[test]
fn mssql_a_write_cut_by_the_deadline_says_it_may_have_run() {
    if !wanted() {
        return;
    }
    let table = "bench.agent_cli_tran_deadline";
    query(
        "local-mssql",
        &format!("drop table if exists {table}\ngo\ncreate table {table} (id int)"),
        &["--yes"],
    );
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "--yes",
        "--timeout",
        "1",
        &format!("insert {table} values (1); waitfor delay '00:00:05'; insert {table} values (2)"),
    ]);
    assert_eq!(outcome.code, 124, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("may already have done part or all of it")
            && outcome.stderr.contains("hint: check what it changed"),
        "{}",
        outcome.stderr
    );
    assert_eq!(count(table), json!(1), "the first insert had committed");
    query("local-mssql", &format!("drop table {table}"), &["--yes"]);
}

#[test]
fn mssql_an_unknown_table_is_not_found_and_hints_the_search() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "select * from bench.nosuchthing",
    ]);
    assert_eq!(outcome.code, 4, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("Invalid object name 'bench.nosuchthing'"),
        "{}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains(
            "hint: agent-cli sql object list --conn local-mssql nosuchthing --fields id,kind"
        ),
        "{}",
        outcome.stderr
    );
}

#[test]
fn mssql_the_deadline_cancels_a_running_statement() {
    if !wanted() {
        return;
    }
    let started = Instant::now();
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "--timeout",
        "1",
        "--yes",
        "waitfor delay '00:00:10'",
    ]);
    assert_eq!(outcome.code, 124, "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn mssql_for_json_is_one_value_and_sql_variant_fails_cleanly() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-mssql",
        "select id, name from bench.customers where id <= 2 order by id for json path",
        &[],
    );
    let json: Value = serde_json::from_str(rows(&result, 0)[0][0].as_str().unwrap()).unwrap();
    assert_eq!(json[1]["name"], "Ægir Nilsen");
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        "select cast(1 as sql_variant) v",
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .starts_with("error: a column the driver cannot read")
    );
    assert!(!outcome.stderr.contains("panicked"), "{}", outcome.stderr);
}

#[test]
fn mssql_objects_schemas_and_sources() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "object",
        "list",
        "--conn",
        "local-mssql",
        "CUSTOMER",
        "--schema",
        "bench",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let listed = outcome.json();
    let names: Vec<(&str, &str)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["kind"].as_str().unwrap(), row["name"].as_str().unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            ("procedure", "sp_customer_orders"),
            ("table", "customers"),
            ("view", "v_customer_totals")
        ]
    );
    let outcome = sql(&[
        "sql",
        "object",
        "list",
        "--conn",
        "local-mssql",
        "--limit",
        "2",
    ]);
    assert_eq!(outcome.json().as_array().unwrap().len(), 2);
    assert!(outcome.stderr.starts_with("[2 of "), "{}", outcome.stderr);

    let table = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-mssql",
        "BENCH.Customers",
    ])
    .json();
    assert_eq!(table["name"], "customers");
    assert_eq!(
        table["columns"][0],
        json!({"name": "id", "type": "int", "nullable": false, "pk": true})
    );
    assert!(
        table["ddl"]
            .as_str()
            .unwrap()
            .ends_with("    PRIMARY KEY (id)\n)")
    );
    let procedure = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-mssql",
        "[bench].[sp_mark_shipped]",
    ])
    .json();
    assert_eq!(procedure["kind"], "procedure");
    assert!(
        procedure["text"]
            .as_str()
            .unwrap()
            .contains("SET status = 'SHIPPED'")
    );
    let function = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-mssql",
        "bench.tvf_orders_by_status",
    ])
    .json();
    assert_eq!(function["kind"], "function");
    let sequence = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-mssql",
        "bench.order_seq",
    ])
    .json();
    assert!(
        sequence["ddl"]
            .as_str()
            .unwrap()
            .starts_with("CREATE SEQUENCE bench.order_seq AS bigint START WITH 501")
    );
    let missing = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-mssql",
        "bench.nope",
    ]);
    assert_eq!(missing.code, 4, "{missing:?}");

    // The diff test adds bench_drift, whichever runs first.
    let schemas = sql(&["sql", "schema", "list", "--conn", "local-mssql"]).json();
    let schemas: Vec<&Value> = schemas
        .as_array()
        .unwrap()
        .iter()
        .filter(|schema| *schema != "bench_drift")
        .collect();
    assert_eq!(schemas, [&json!("bench"), &json!("dbo")]);
}

#[test]
fn mssql_bench_reports_every_phase() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "bench",
        "--conn",
        "local-mssql",
        "--runs",
        "3",
        "select top 10 id from bench.orders",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let bench = outcome.json();
    assert_eq!(
        (bench["runs"].clone(), bench["rows"].clone()),
        (json!(3), json!(10))
    );
    let phases: Vec<&str> = bench["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|phase| phase["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["connect", "first_row", "total"]);
}

#[test]
fn mssql_bench_max_rows_ends_each_read_there_and_says_the_session_reconnects() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "bench",
        "--conn",
        "local-mssql",
        "--runs",
        "3",
        "--max-rows",
        "4",
        "select id from bench.orders",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let bench = outcome.json();
    assert_eq!(
        (bench["runs"].clone(), bench["rows"].clone()),
        (json!(3), json!(4))
    );
    assert!(
        outcome
            .stderr
            .contains("each run after the first connected again"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn mssql_doctor_connects_and_says_how_long_it_took() {
    if !wanted() {
        return;
    }
    let outcome = sql(&["doctor", "sql"]);
    let checks = outcome.json();
    let mssql = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["check"] == "connection local-mssql")
        .unwrap();
    assert_eq!(mssql["ok"], true, "{mssql}");
    assert!(!outcome.stdout.contains("Bench_Pass1!"));
}

// ---------- Oracle ----------

#[test]
fn oracle_every_type_renders_as_the_contract_says() {
    if !wanted() {
        return;
    }
    let result = query("local-oracle", "select * from all_types order by id", &[]);
    assert_eq!(
        rows(&result, 0)[0],
        json!([
            1,
            "12345.6789",
            "123.45",
            1.25,
            1.234_567_890_123_4,
            "abcde",
            "varchar2 value",
            "nvarchar2 value",
            "2024-05-17T00:00:00",
            "2024-05-17T13:45:30.123456",
            "2024-05-17T13:45:30.123456+02:00",
            "+02 03:04:05.600000",
            "0x0102030405060708",
            "clob value",
            "0xaabbcc"
        ])
    );
    let numbers = query(
        "local-oracle",
        "select count(*) n, 0.5 half, cast(100.5 as number(12,2)) money from dual",
        &[],
    );
    assert_eq!(rows(&numbers, 0), &json!([[1, "0.5", "100.50"]]));
}

#[test]
fn oracle_a_script_splits_at_semicolons_and_slashes_and_keeps_plsql_whole() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-oracle",
        "select 1 a from dual;\ndeclare\n  n number;\nbegin\n  n := 1;\nend;\n/\nselect 2 b from dual",
        &["--yes"],
    );
    let sets = result["results"].as_array().unwrap();
    assert_eq!(sets.len(), 3);
    assert_eq!(rows(&result, 0), &json!([[1]]));
    assert_eq!(sets[1]["rows_affected"], 0, "a block counts nothing");
    assert_eq!(rows(&result, 2), &json!([[2]]));
}

#[test]
fn oracle_select_for_update_is_a_write_and_reads_past_its_first_fetch() {
    if !wanted() {
        return;
    }
    let text = "select id from order_items where id <= 1200 order by id for update";
    let outcome = sql(&["sql", "query", "run", "--conn", "local-oracle", text]);
    assert_eq!(outcome.code, 2, "{outcome:?}");
    let result = query("local-oracle", text, &["--yes"]);
    assert_eq!(rows(&result, 0).as_array().unwrap().len(), 1000);
    assert_eq!(result["results"][0]["truncated"], true);
}

#[test]
fn oracle_writes_commit_and_count() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-oracle",
        "create table agent_cli_writes (id number);\n\
         insert into agent_cli_writes select level from dual connect by level <= 3;\n\
         delete from agent_cli_writes where id > 1;\n\
         drop table agent_cli_writes",
        &["--yes"],
    );
    let affected: Vec<&Value> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|set| &set["rows_affected"])
        .collect();
    assert_eq!(affected, [&json!(0), &json!(3), &json!(2), &json!(0)]);
}

#[test]
fn oracle_a_broken_procedure_says_which_line() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-oracle",
        "--yes",
        "create or replace procedure agent_cli_broken is\nbegin\n  nope;\nend;",
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .starts_with("error: line 3: PLS-00201: identifier 'NOPE' must be declared"),
        "{}",
        outcome.stderr
    );
    query(
        "local-oracle",
        "drop procedure agent_cli_broken",
        &["--yes"],
    );
}

#[test]
fn oracle_an_unknown_table_is_not_found_and_hints_the_search() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-oracle",
        "select * from bench.nosuchthing",
    ]);
    assert_eq!(outcome.code, 4, "{outcome:?}");
    assert!(outcome.stderr.contains("ORA-00942"), "{}", outcome.stderr);
    assert!(
        outcome
            .stderr
            .contains("hint: agent-cli sql object list --conn local-oracle "),
        "{}",
        outcome.stderr
    );
}

#[test]
fn oracle_the_deadline_holds_even_inside_a_plsql_sleep() {
    if !wanted() {
        return;
    }
    let started = Instant::now();
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-oracle",
        "--timeout",
        "2",
        "--yes",
        "begin dbms_session.sleep(10); end;",
    ]);
    assert_eq!(outcome.code, 124, "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn oracle_objects_schemas_and_sources() {
    if !wanted() {
        return;
    }
    let listed = sql(&[
        "sql",
        "object",
        "list",
        "--conn",
        "local-oracle",
        "order",
        "--kind",
        "package",
    ])
    .json();
    assert_eq!(listed[0]["name"], "ORDER_PKG");
    let modified = listed[0]["modified"].as_str().unwrap();
    assert!(
        modified.ends_with('Z') && agent_cli_core::utc(modified) == modified,
        "{modified}"
    );

    let package = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-oracle",
        "bench.order_pkg",
    ])
    .json();
    let text = package["text"].as_str().unwrap();
    assert!(
        text.starts_with("CREATE OR REPLACE PACKAGE order_pkg AS"),
        "{text}"
    );
    assert!(
        text.contains("\n/\n\nCREATE OR REPLACE PACKAGE BODY order_pkg AS"),
        "{text}"
    );
    let view = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-oracle",
        "bench.v_customer_totals",
    ])
    .json();
    assert!(
        view["text"]
            .as_str()
            .unwrap()
            .starts_with("CREATE OR REPLACE VIEW BENCH.V_CUSTOMER_TOTALS AS")
    );
    let table = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-oracle",
        "bench.orders",
    ])
    .json();
    assert_eq!(
        table["columns"][4],
        json!({"name": "TOTAL", "type": "NUMBER(12,2)", "nullable": false, "pk": false})
    );
    let sequence = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-oracle",
        "\"BENCH\".order_seq",
    ])
    .json();
    assert!(
        sequence["ddl"]
            .as_str()
            .unwrap()
            .starts_with("CREATE SEQUENCE BENCH.ORDER_SEQ START WITH")
    );

    let schemas = sql(&["sql", "schema", "list", "--conn", "local-oracle"]).json();
    assert!(
        schemas.as_array().unwrap().contains(&json!("BENCH")),
        "{schemas}"
    );
}

#[test]
fn oracle_bench_max_rows_ends_each_read_there() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "bench",
        "--conn",
        "local-oracle",
        "--runs",
        "2",
        "--max-rows",
        "4",
        "select id from bench.orders",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.json()["rows"], 4);
    assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
}

#[test]
fn oracle_bench_stops_at_the_deadline_and_says_how_far_it_got() {
    if !wanted() {
        return;
    }
    let started = Instant::now();
    let outcome = sql(&[
        "sql",
        "query",
        "bench",
        "--conn",
        "local-oracle",
        "--runs",
        "100",
        "--timeout",
        "2",
        "--yes",
        "begin dbms_session.sleep(0.3); end;",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
    let bench = outcome.json();
    let runs = bench["runs"].as_u64().unwrap();
    assert!((3..100).contains(&runs), "{bench}");
    assert_eq!(bench["requested"], 100);
    assert!(
        outcome
            .stderr
            .contains("of 100 runs at the --timeout deadline")
    );
}

// ---------- diff ----------

/// A second schema drifted from `bench`: `orders.status` wider with a
/// default, a `retry_count` column the migration added, a check on `total`
/// and an index covering it, and the procedure reordered; a comment and
/// keyword case that do not count.
const DRIFT: &str = "IF SCHEMA_ID('bench_drift') IS NULL EXEC('CREATE SCHEMA bench_drift');
GO
DROP TABLE IF EXISTS bench_drift.orders;
CREATE TABLE bench_drift.orders (
    id          int           NOT NULL PRIMARY KEY,
    customer_id int           NOT NULL REFERENCES bench.customers(id),
    ordered_at  datetime2     NOT NULL,
    status      varchar(30)   NOT NULL DEFAULT 'NEW',
    total       decimal(12,2) NOT NULL CHECK (total >= 0),
    retry_count int           NOT NULL DEFAULT 0
);
GO
CREATE INDEX ix_drift_status ON bench_drift.orders (status DESC) INCLUDE (total);
GO
CREATE OR ALTER PROCEDURE bench_drift.sp_customer_orders @customer_id int AS
BEGIN
    SET NOCOUNT ON;
    -- the newest first
    select id, ordered_at, status, total
    FROM bench.orders
    WHERE customer_id = @customer_id
    ORDER BY ordered_at DESC;
END;
GO";

#[test]
fn mssql_a_drifted_schema_differs_by_columns_defaults_and_a_procedures_order() {
    if !wanted() {
        return;
    }
    let made = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-mssql",
        DRIFT,
        "--yes",
    ]);
    assert_eq!(made.code, 0, "{made:?}");
    let table = sql(&[
        "sql",
        "object",
        "diff",
        "local-mssql:bench.orders",
        "local-mssql:bench_drift.orders",
    ]);
    assert_eq!(table.code, 0, "{table:?}");
    let changes = table.json()["changes"].clone();
    assert!(
        changes
            .as_array()
            .unwrap()
            .contains(&json!({"path": "columns.status.type",
            "left": "varchar(20)", "right": "varchar(30)"})),
        "{changes}"
    );
    assert!(
        changes
            .as_array()
            .unwrap()
            .contains(&json!({"path": "columns.status.default",
            "right": "('NEW')"})),
        "{changes}"
    );
    assert_eq!(
        changes
            .as_array()
            .unwrap()
            .iter()
            .filter(|change| change["path"] == "columns.retry_count")
            .count(),
        1,
        "{changes}"
    );
    // A check is keyed by its column, whatever name the server made up
    // (bench.orders has none, so the whole map is the right side's).
    assert!(
        changes
            .as_array()
            .unwrap()
            .contains(&json!({"path": "checks", "right": {"total": "([total]>=(0))"}})),
        "{changes}"
    );
    let procedure = sql(&[
        "sql",
        "object",
        "diff",
        "local-mssql:bench.sp_customer_orders",
        "local-mssql:bench_drift.sp_customer_orders",
    ]);
    assert_eq!(procedure.code, 0, "{procedure:?}");
    let lines = procedure.json()["changes"][0]["lines"].to_string();
    assert!(lines.contains("+    ORDER BY ordered_at DESC;"), "{lines}");
    assert!(
        !lines.contains("-    SELECT id"),
        "case and comments count only with --exact: {lines}"
    );
    // A side as the diff printed it is what object get takes.
    let side = table.json()["right"].as_str().unwrap().to_owned();
    let got = sql(&["sql", "object", "get", &side]);
    assert_eq!(got.json()["schema"], "bench_drift", "{got:?}");
    let schemas = sql(&[
        "sql",
        "schema",
        "diff",
        "local-mssql:bench",
        "local-mssql:bench_drift",
        "--limit",
        "200",
    ]);
    assert_eq!(schemas.code, 0, "{schemas:?}");
    let paths: Vec<String> = schemas.json()["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        paths.contains(&"orders.columns.retry_count".to_owned()),
        "{paths:?}"
    );
    // A schema compares each table as object diff does: indexes too.
    assert!(paths.contains(&"orders.indexes".to_owned()), "{paths:?}");
    assert!(paths.contains(&"customers".to_owned()), "{paths:?}");
    assert!(
        !paths.iter().any(|path| path.ends_with(".rows")),
        "row counts are noise: {paths:?}"
    );
}

#[test]
fn oracle_a_descending_index_reads_its_column_and_identity_sequences_are_not_objects() {
    if !wanted() {
        return;
    }
    let made = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-oracle",
        "begin execute immediate 'drop table drift_at'; exception when others then null; end;
/
create table drift_at (id number generated always as identity primary key, at timestamp not null)
/
create index ix_drift_at on drift_at (at desc)
/",
        "--yes",
    ]);
    assert_eq!(made.code, 0, "{made:?}");
    let same = sql(&[
        "sql",
        "object",
        "diff",
        "local-oracle:bench.drift_at",
        "local-oracle:bench.drift_at",
    ]);
    assert_eq!(same.json()["same"], true, "{same:?}");
    let got = sql(&[
        "sql",
        "schema",
        "diff",
        "local-oracle:bench",
        "local-oracle:bench",
    ]);
    assert_eq!(got.json()["same"], true, "{got:?}");
    let sequences = sql(&[
        "sql",
        "object",
        "list",
        "--conn",
        "local-oracle",
        "--kind",
        "sequence",
    ]);
    assert!(!sequences.stdout.contains("ISEQ$$"), "{sequences:?}");
    let table = sql(&[
        "sql",
        "object",
        "diff",
        "local-oracle:bench.orders",
        "local-oracle:bench.drift_at",
    ]);
    let index = table.json()["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "indexes")
        .cloned();
    assert_eq!(
        index.map(|change| change["right"]["IX_DRIFT_AT"]["columns"].clone()),
        Some(json!(["AT desc"])),
        "{table:?}"
    );
}

#[test]
fn oracle_a_table_against_itself_is_the_same_and_sql_server_is_refused() {
    if !wanted() {
        return;
    }
    let same = sql(&[
        "sql",
        "object",
        "diff",
        "local-oracle:bench.orders",
        "local-oracle:bench.orders",
    ]);
    assert_eq!(same.code, 0, "{same:?}");
    assert_eq!(same.json()["same"], true);
    let package = sql(&[
        "sql",
        "object",
        "diff",
        "local-oracle:bench.order_pkg",
        "local-oracle:bench.order_pkg",
    ]);
    assert_eq!(package.json()["same"], true, "{package:?}");
    let refused = sql(&[
        "sql",
        "object",
        "diff",
        "local-oracle:bench.orders",
        "local-mssql:bench.orders",
    ]);
    assert_eq!(refused.code, 2, "{refused:?}");
}

// ---------- read-only connections and a rotated password ----------

/// `config.test.toml` and one more connection to the database `like` names,
/// with `extra` in place of its password.
fn with_connection(name: &str, like: &str, extra: &str) -> String {
    let (kind, rest) = match like {
        "local-oracle" => (
            "oracle",
            "port = 1521\nservice = \"FREEPDB1\"\nuser = \"bench\"",
        ),
        "local-postgres" => (
            "postgres",
            "port = 5433\ndatabase = \"bench\"\nuser = \"bench\"\ntrust_cert = true",
        ),
        _ => (
            "mssql",
            "port = 1433\ndatabase = \"bench\"\nuser = \"sa\"\ntrust_cert = true",
        ),
    };
    format!(
        "{}\n[[sql.connection]]\nname = \"{name}\"\nkind = \"{kind}\"\nhost = \"localhost\"\n{rest}\n{extra}\n",
        *CONFIG
    )
}

#[test]
fn oracle_a_read_only_connection_reads_inside_a_read_only_transaction_and_refuses_writes() {
    if !wanted() {
        return;
    }
    let config = with_connection(
        "ro-oracle",
        "local-oracle",
        "password = \"bench\"\nread_only = true",
    );
    let on = |argv: &[&str]| {
        run(
            &[agent_cli_sql::DOMAIN],
            argv,
            Setup::fake(FakeTransport::default()).with_config(&config),
        )
    };
    // A select begins no transaction; SET TRANSACTION READ ONLY does.
    let transaction = "select nvl(dbms_transaction.local_transaction_id, 'none') t from dual";
    let id = |conn: &str| {
        let outcome = on(&["sql", "query", "run", "--conn", conn, transaction]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        outcome.json()["results"][0]["rows"][0][0].clone()
    };
    assert_eq!(id("local-oracle"), "none");
    assert_ne!(id("ro-oracle"), "none", "the read ran inside a transaction");

    let outcome = on(&[
        "sql",
        "query",
        "run",
        "--conn",
        "ro-oracle",
        "insert into customers (id) values (-1)",
        "--yes",
    ]);
    assert_eq!(outcome.code, 2, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("sql connection ro-oracle is read-only in the config"),
        "{}",
        outcome.stderr
    );
    // The catalog reads every other command makes run in it too.
    for argv in [
        ["sql", "object", "list", "--conn", "ro-oracle", "customers"].as_slice(),
        &[
            "sql",
            "object",
            "get",
            "--conn",
            "ro-oracle",
            "bench.customers",
        ],
        &["sql", "schema", "list", "--conn", "ro-oracle"],
        &[
            "sql",
            "schema",
            "diff",
            "ro-oracle:bench",
            "local-oracle:bench",
        ],
    ] {
        let outcome = on(argv);
        assert_eq!(outcome.code, 0, "{argv:?}: {outcome:?}");
    }
}

/// A `password_kv` whose kept value is stale and whose Key Vault secret is
/// right: the refused login reads the vault again, signs in with the new
/// value and keeps it. An unchanged vault value leaves the refusal standing.
fn a_rotated_password_is_read_again_and_kept(like: &str, right: &str, select: &str) {
    use agent_cli_core::testing::Answer;
    let config = with_connection("rotated", like, "password_kv = \"contoso-kv/bench-db\"");
    let dir = tempfile::tempdir().unwrap();
    let secrets = dir.path().join("secrets.toml");
    let attempt = |vault: &str| {
        std::fs::write(&secrets, "\"contoso-kv/bench-db\" = \"stale-password\"\n").unwrap();
        let transport = FakeTransport::answering([Answer::json(&json!({"value": vault}))]);
        let setup = Setup::fake(transport.clone())
            .with_config(&config)
            .with_secrets(&secrets);
        let outcome = run(
            &[agent_cli_sql::DOMAIN],
            &["sql", "query", "run", "--conn", "rotated", select],
            setup,
        );
        (outcome, transport.sent().len())
    };

    let (outcome, sent) = attempt("stale-password");
    assert_eq!(outcome.code, 3, "{outcome:?}");
    assert!(
        ["ORA-01017", "(error 18456)", "(SQLSTATE 28P01)"]
            .iter()
            .any(|code| outcome.stderr.contains(code)),
        "{}",
        outcome.stderr
    );
    assert_eq!(sent, 1, "the vault was read again once");

    let (outcome, sent) = attempt(right);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.json()["results"][0]["rows"][0][0], 1);
    assert_eq!(sent, 1);
    let kept = std::fs::read_to_string(&secrets).unwrap();
    assert!(
        kept.contains(right) && !kept.contains("stale-password"),
        "{kept}"
    );
}

#[test]
fn oracle_a_refused_login_with_a_rotated_password_signs_in_again() {
    if !wanted() {
        return;
    }
    a_rotated_password_is_read_again_and_kept("local-oracle", "bench", "select 1 from dual");
}

#[test]
fn mssql_a_refused_login_with_a_rotated_password_signs_in_again() {
    if !wanted() {
        return;
    }
    a_rotated_password_is_read_again_and_kept("local-mssql", "Bench_Pass1!", "select 1");
}

#[test]
fn postgres_a_refused_login_with_a_rotated_password_signs_in_again() {
    if !wanted() {
        return;
    }
    a_rotated_password_is_read_again_and_kept("local-postgres", "bench", "select 1");
}

// ---------- Postgres ----------

/// `sql query run` on a config of `config.test.toml` and one more
/// connection like `local-postgres`, with `extra` in place of its password.
fn on_postgres(name: &str, extra: &str, argv: &[&str]) -> Outcome {
    run(
        &[agent_cli_sql::DOMAIN],
        argv,
        Setup::fake(FakeTransport::default()).with_config(&with_connection(
            name,
            "local-postgres",
            extra,
        )),
    )
}

#[test]
fn postgres_every_type_renders_as_the_contract_says() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-postgres",
        "select * from bench.all_types order by id",
        &[],
    );
    let set = &result["results"][0];
    assert_eq!(
        set["types"],
        json!([
            "int4",
            "bool",
            "int2",
            "int4",
            "int8",
            "numeric",
            "float4",
            "float8",
            "bpchar",
            "varchar",
            "text",
            "date",
            "time",
            "timestamp",
            "timestamptz",
            "interval",
            "uuid",
            "bytea",
            "jsonb",
            "int4[]"
        ])
    );
    assert_eq!(
        set["rows"][0],
        json!([
            1,
            true,
            32767,
            2_147_483_647,
            "9223372036854775807",
            "12345.6789",
            1.25,
            12_345_678_901.234,
            "abcde",
            "varchar value",
            "text value",
            "2024-05-17",
            "13:45:30.123456",
            "2024-05-17T13:45:30.123456",
            "2024-05-17T11:45:30.123456+00:00",
            "1 day 02:03:04",
            "6f9619ff-8b86-d011-b42d-00c04fc964ff",
            "0x0102030405060708",
            "{\"a\": [1, 2]}",
            "{1,2,3}"
        ])
    );
    let nulls = set["rows"][1].as_array().unwrap();
    assert!(nulls[1..].iter().all(Value::is_null), "{nulls:?}");
    let numbers = query(
        "local-postgres",
        "select count(*) n, 0.5 half, 100.5::numeric(12,2) money where false",
        &[],
    );
    assert_eq!(
        numbers["results"][0],
        json!({"columns": ["n", "half", "money"], "types": ["int8", "numeric", "numeric"],
               "rows": [[0, "0.5", "100.50"]], "rows_affected": 0, "truncated": false}),
        "an aggregate answers one row even of nothing"
    );
}

#[test]
fn postgres_a_script_splits_at_semicolons_and_keeps_dollar_quoted_and_atomic_bodies_whole() {
    if !wanted() {
        return;
    }
    let script = "create or replace function bench.agent_cli_twice(n int) returns int \
                  language plpgsql as $$\nbegin\n  return n * 2;\nend;\n$$;\n\
                  create or replace function bench.agent_cli_thrice(n int) returns int \
                  language sql begin atomic\n  select n * 3;\nend;\n\
                  select bench.agent_cli_twice(21) a, bench.agent_cli_thrice(1) b;\n\
                  do $do$ begin perform 1; end $do$;\n\
                  drop function bench.agent_cli_twice; drop function bench.agent_cli_thrice";
    let outcome = sql(&["sql", "query", "run", "--conn", "local-postgres", script]);
    assert_eq!(outcome.code, 2, "a create is not a read: {outcome:?}");
    let result = query("local-postgres", script, &["--yes"]);
    let sets = result["results"].as_array().unwrap();
    assert_eq!(sets.len(), 6, "{result}");
    assert_eq!(rows(&result, 2), &json!([[42, 3]]));
}

#[test]
fn postgres_a_read_stops_at_max_rows_and_the_next_statement_reconnects() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        "--max-rows",
        "3",
        "select id from bench.events order by id; select count(*) from bench.customers",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let result = outcome.json();
    assert_eq!(rows(&result, 0), &json!([[1], [2], [3]]));
    assert_eq!(result["results"][0]["truncated"], true);
    assert_eq!(rows(&result, 1), &json!([[50]]));
}

#[test]
fn postgres_writes_report_what_they_changed_and_a_transaction_survives_a_write_past_max_rows() {
    if !wanted() {
        return;
    }
    let result = query(
        "local-postgres",
        "drop table if exists bench.agent_cli_writes;\n\
         create table bench.agent_cli_writes (id int);\n\
         insert into bench.agent_cli_writes values (1), (2), (3);\n\
         update bench.agent_cli_writes set id = id + 1 where id > 1;\n\
         drop table bench.agent_cli_writes",
        &["--yes"],
    );
    let affected: Vec<&Value> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|set| &set["rows_affected"])
        .collect();
    assert_eq!(
        affected,
        [&json!(0), &json!(0), &json!(3), &json!(2), &json!(0)]
    );
    // After a write a read is read whole, so the session and its
    // transaction live on to the commit.
    let result = query(
        "local-postgres",
        "begin; create temp table agent_cli_kept as select id from bench.orders limit 5;\n\
         select id from bench.orders; select count(*) from agent_cli_kept; commit",
        &["--max-rows", "2", "--yes"],
    );
    assert_eq!(result["results"][2]["truncated"], true);
    assert_eq!(rows(&result, 3), &json!([[5]]));
}

/// `select count(*)` of a scratch table on Postgres.
fn pg_count(table: &str) -> Value {
    rows(
        &query(
            "local-postgres",
            &format!("select count(*) from {table}"),
            &[],
        ),
        0,
    )[0][0]
        .clone()
}

#[test]
fn postgres_a_failure_names_the_statement_its_line_and_what_already_ran() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        "select 1; select * from bench.nope",
    ]);
    assert_eq!(outcome.code, 4, "{outcome:?}");
    assert_eq!(
        outcome.stderr.trim(),
        "error: statement 2 of 2 failed; statement 1 already ran and stays committed \
         (1 row returned): line 1: relation \"bench.nope\" does not exist\n\
         hint: agent-cli sql object list --conn local-postgres nope --fields id,kind"
    );
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        "select id,\n  nme\nfrom bench.customers c",
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome.stderr.starts_with(
            "error: line 2: column \"nme\" does not exist; Perhaps you meant to reference the \
             column \"c.name\"."
        ),
        "{}",
        outcome.stderr
    );
}

#[test]
fn postgres_a_transaction_left_open_or_failed_is_rolled_back_and_said() {
    if !wanted() {
        return;
    }
    let table = "bench.agent_cli_tran";
    query(
        "local-postgres",
        &format!("drop table if exists {table}; create table {table} (id int)"),
        &["--yes"],
    );
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        "--yes",
        &format!("begin; insert into {table} values (1)"),
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("left a transaction open (a BEGIN with no COMMIT)"),
        "{}",
        outcome.stderr
    );
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        "--yes",
        &format!("begin; insert into {table} values (1); select 1/0; commit"),
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome.stderr.contains(
            "statement 3 of 4 failed; statements 1-2 ran (1: 0 rows affected, 2: 1 row \
             affected), but a transaction still open at the failure is rolled back"
        ),
        "{}",
        outcome.stderr
    );
    assert_eq!(pg_count(table), json!(0));
    query("local-postgres", &format!("drop table {table}"), &["--yes"]);
}

#[test]
fn postgres_the_deadline_cancels_the_statement_on_the_server() {
    if !wanted() {
        return;
    }
    let started = Instant::now();
    let outcome = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        "--timeout",
        "1",
        "select pg_sleep(10.25)",
    ]);
    assert_eq!(outcome.code, 124, "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    let left = query(
        "local-postgres",
        "select count(*) from pg_stat_activity \
         where query = 'select pg_sleep(10.25)' and state = 'active'",
        &[],
    );
    assert_eq!(rows(&left, 0), &json!([[0]]), "the server stopped it");
}

#[test]
fn postgres_objects_schemas_and_sources() {
    if !wanted() {
        return;
    }
    let listed = sql(&[
        "sql",
        "object",
        "list",
        "--conn",
        "local-postgres",
        "CUSTOMER",
        "--schema",
        "BENCH",
    ])
    .json();
    let names: Vec<(&str, &str)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["kind"].as_str().unwrap(), row["name"].as_str().unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            ("function", "sp_customer_orders"),
            ("table", "customers"),
            ("view", "v_customer_totals")
        ]
    );
    let views = sql(&[
        "sql",
        "object",
        "list",
        "--conn",
        "local-postgres",
        "--kind",
        "view",
        "--schema",
        "bench",
    ])
    .json();
    assert_eq!(
        views.as_array().unwrap().len(),
        3,
        "a materialized view is a view: {views}"
    );

    let table = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-postgres",
        "BENCH.Customers",
    ])
    .json();
    assert_eq!(table["name"], "customers");
    assert_eq!(
        table["columns"][5],
        json!({"name": "credit_limit", "type": "numeric(12,2)", "nullable": true, "pk": false})
    );
    assert!(
        table["ddl"]
            .as_str()
            .unwrap()
            .starts_with("CREATE TABLE bench.customers (\n    id integer NOT NULL,")
    );
    let procedure = sql(&[
        "sql",
        "object",
        "get",
        "local-postgres:bench.sp_mark_shipped",
    ])
    .json();
    assert_eq!(procedure["kind"], "procedure");
    let text = procedure["text"].as_str().unwrap();
    assert!(
        text.starts_with(
            "CREATE OR REPLACE PROCEDURE bench.sp_mark_shipped(IN p_order_id integer)"
        ) && text.contains("set status = 'SHIPPED'"),
        "{text}"
    );
    let function = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-postgres",
        "bench.tvf_orders_by_status",
    ])
    .json();
    assert!(
        function["text"].as_str().unwrap().contains("BEGIN ATOMIC"),
        "{function}"
    );
    let view = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-postgres",
        "bench.mv_daily_orders",
    ])
    .json();
    assert!(
        view["text"]
            .as_str()
            .unwrap()
            .starts_with("CREATE MATERIALIZED VIEW bench.mv_daily_orders AS\n"),
        "{view}"
    );
    let sequence = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-postgres",
        "bench.order_seq",
    ])
    .json();
    assert!(
        sequence["ddl"]
            .as_str()
            .unwrap()
            .starts_with("CREATE SEQUENCE bench.order_seq AS bigint START WITH 501"),
        "{sequence}"
    );
    let missing = sql(&[
        "sql",
        "object",
        "get",
        "--conn",
        "local-postgres",
        "bench.nope",
    ]);
    assert_eq!(missing.code, 4, "{missing:?}");
    let schemas = sql(&["sql", "schema", "list", "--conn", "local-postgres"]).json();
    let schemas: Vec<&Value> = schemas
        .as_array()
        .unwrap()
        .iter()
        .filter(|schema| *schema != "bench_drift")
        .collect();
    assert_eq!(schemas, [&json!("bench"), &json!("public")]);
}

#[test]
fn postgres_bench_reports_every_phase_and_says_a_cut_read_reconnects() {
    if !wanted() {
        return;
    }
    let outcome = sql(&[
        "sql",
        "query",
        "bench",
        "--conn",
        "local-postgres",
        "--runs",
        "3",
        "select id from bench.orders limit 10",
    ]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let bench = outcome.json();
    assert_eq!(
        (bench["runs"].clone(), bench["rows"].clone()),
        (json!(3), json!(10))
    );
    assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
    let outcome = sql(&[
        "sql",
        "query",
        "bench",
        "--conn",
        "local-postgres",
        "--runs",
        "3",
        "--max-rows",
        "4",
        "select id from bench.orders",
    ]);
    assert_eq!(outcome.json()["rows"], 4, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("ends a Postgres session: each run after the first connected again"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn postgres_doctor_connects_over_tls_and_says_how_long_it_took() {
    if !wanted() {
        return;
    }
    let outcome = sql(&["doctor", "sql"]);
    let checks = outcome.json();
    let postgres = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["check"] == "connection local-postgres")
        .unwrap();
    assert_eq!(postgres["ok"], true, "{postgres}");
    let tls = query(
        "local-postgres",
        "select ssl from pg_stat_ssl where pid = pg_backend_pid()",
        &[],
    );
    assert_eq!(rows(&tls, 0), &json!([[true]]), "encrypt is on by default");
}

#[test]
fn postgres_tls_is_checked_against_the_system_store_and_off_with_encrypt_false() {
    if !wanted() {
        return;
    }
    let ssl = "select ssl from pg_stat_ssl where pid = pg_backend_pid()";
    let outcome = run(
        &[agent_cli_sql::DOMAIN],
        &["sql", "query", "run", "--conn", "verified", ssl],
        Setup::fake(FakeTransport::default()).with_config(&format!(
            "{}\n[[sql.connection]]\nname = \"verified\"\nkind = \"postgres\"\nhost = \"localhost\"\n\
             port = 5433\ndatabase = \"bench\"\nuser = \"bench\"\npassword = \"bench\"\n",
            *CONFIG
        )),
    );
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("invalid peer certificate: UnknownIssuer")
            && outcome
                .stderr
                .contains("set trust_cert = true in \"verified\""),
        "{}",
        outcome.stderr
    );
    let outcome = on_postgres(
        "plain",
        "password = \"bench\"\nencrypt = false",
        &["sql", "query", "run", "--conn", "plain", ssl],
    );
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.json()["results"][0]["rows"], json!([[false]]));
}

#[test]
fn postgres_a_read_only_connection_is_read_only_on_the_server_and_refuses_writes() {
    if !wanted() {
        return;
    }
    let ro = |argv: &[&str]| {
        on_postgres(
            "ro-postgres",
            "password = \"bench\"\nread_only = true",
            argv,
        )
    };
    let outcome = ro(&[
        "sql",
        "query",
        "run",
        "--conn",
        "ro-postgres",
        "show transaction_read_only",
    ]);
    assert_eq!(
        outcome.json()["results"][0]["rows"],
        json!([["on"]]),
        "{outcome:?}"
    );
    let writable = query("local-postgres", "show transaction_read_only", &[]);
    assert_eq!(rows(&writable, 0), &json!([["off"]]));

    let outcome = ro(&[
        "sql",
        "query",
        "run",
        "--conn",
        "ro-postgres",
        "insert into bench.customers (id) values (-1)",
        "--yes",
    ]);
    assert_eq!(outcome.code, 2, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("sql connection ro-postgres is read-only in the config"),
        "{}",
        outcome.stderr
    );
    // A write the classifier cannot see, inside a function: the server
    // refuses it.
    query(
        "local-postgres",
        "create or replace function bench.agent_cli_sneaky() returns int language sql as \
         $$ insert into bench.events values (-1, 1, 'x', 'x', now()) returning 1 $$",
        &["--yes"],
    );
    let outcome = ro(&[
        "sql",
        "query",
        "run",
        "--conn",
        "ro-postgres",
        "select bench.agent_cli_sneaky()",
    ]);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert!(
        outcome
            .stderr
            .contains("cannot execute INSERT in a read-only transaction"),
        "{}",
        outcome.stderr
    );
    assert_eq!(pg_count("bench.events where id = -1"), json!(0));
    query(
        "local-postgres",
        "drop function bench.agent_cli_sneaky",
        &["--yes"],
    );
    for argv in [
        [
            "sql",
            "object",
            "list",
            "--conn",
            "ro-postgres",
            "customers",
        ]
        .as_slice(),
        &[
            "sql",
            "object",
            "get",
            "--conn",
            "ro-postgres",
            "bench.customers",
        ],
        &["sql", "schema", "list", "--conn", "ro-postgres"],
        &[
            "sql",
            "schema",
            "diff",
            "ro-postgres:bench",
            "local-postgres:bench",
        ],
    ] {
        let outcome = ro(argv);
        assert_eq!(outcome.code, 0, "{argv:?}: {outcome:?}");
    }
}

/// `bench` drifted, as `DRIFT` drifts it on SQL Server, and a procedure
/// with a comment and keyword case that do not count.
const PG_DRIFT: &str = "create schema if not exists bench_drift;
drop table if exists bench_drift.orders cascade;
create table bench_drift.orders (
    id          int           not null primary key,
    customer_id int           not null references bench.customers(id),
    ordered_at  timestamp     not null,
    status      varchar(30)   not null default 'NEW',
    total       numeric(12,2) not null check (total >= 0),
    retry_count int           not null default 0
);
create index ix_drift_status on bench_drift.orders (status desc) include (total);
grant select on bench_drift.orders to bench_reader;
create or replace function bench_drift.fn_order_total(p_order_id int)
returns numeric(12,2) language plpgsql stable as $$
BEGIN
    -- every item, cancelled or not
    RETURN (SELECT coalesce(sum(qty * unit_price), 0)
            FROM bench.order_items WHERE order_id = p_order_id);
END;
$$;
create or replace function bench_drift.sp_customer_orders(p_customer_id int)
returns table (id int, ordered_at timestamp, status varchar, total numeric)
language sql stable as $$
    select id, ordered_at, status, total
    from bench.orders
    where customer_id = p_customer_id
    order by ordered_at desc;
$$";

#[test]
fn postgres_a_drifted_schema_differs_by_columns_defaults_checks_indexes_grants_and_source() {
    if !wanted() {
        return;
    }
    let made = sql(&[
        "sql",
        "query",
        "run",
        "--conn",
        "local-postgres",
        PG_DRIFT,
        "--yes",
    ]);
    assert_eq!(made.code, 0, "{made:?}");
    let table = sql(&[
        "sql",
        "object",
        "diff",
        "local-postgres:bench.orders",
        "local-postgres:bench_drift.orders",
    ]);
    assert_eq!(table.code, 0, "{table:?}");
    let changes = table.json()["changes"].clone();
    for want in [
        json!({"path": "columns.status.type", "left": "character varying(20)",
               "right": "character varying(30)"}),
        json!({"path": "columns.status.default", "right": "'NEW'::character varying"}),
        json!({"path": "columns.retry_count",
               "right": {"type": "integer", "nullable": false, "default": "0"}}),
        json!({"path": "checks", "right": {"total": "CHECK (total >= 0::numeric)"}}),
        json!({"path": "indexes", "right": {"ix_drift_status":
               {"unique": false, "columns": ["status desc"], "include": ["total"]}}}),
        json!({"path": "grants", "right": {"bench_reader": ["SELECT"]}}),
    ] {
        assert!(
            changes.as_array().unwrap().contains(&want),
            "{want}: {changes}"
        );
    }
    let foreign = sql(&["sql", "object", "get", "local-postgres:bench_drift.orders"]);
    assert_eq!(foreign.json()["schema"], "bench_drift", "{foreign:?}");

    // A comment and keyword case are no difference; a changed order is.
    let total = sql(&[
        "sql",
        "object",
        "diff",
        "local-postgres:bench.fn_order_total",
        "local-postgres:bench_drift.fn_order_total",
    ]);
    let lines = total.json()["changes"][0]["lines"].to_string();
    assert!(
        !lines.contains("RETURN (SELECT") && lines.contains("bench_drift.fn_order_total"),
        "only the name differs: {lines}"
    );
    let orders = sql(&[
        "sql",
        "object",
        "diff",
        "local-postgres:bench.sp_customer_orders",
        "local-postgres:bench_drift.sp_customer_orders",
    ]);
    let lines = orders.json()["changes"][0]["lines"].to_string();
    assert!(lines.contains("+    order by ordered_at desc;"), "{lines}");

    let schemas = sql(&[
        "sql",
        "schema",
        "diff",
        "local-postgres:bench",
        "local-postgres:bench_drift",
        "--limit",
        "200",
    ]);
    assert_eq!(schemas.code, 0, "{schemas:?}");
    let paths: Vec<String> = schemas.json()["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect();
    for want in [
        "orders.columns.retry_count",
        "orders.indexes",
        "orders.grants",
        "customers",
        "mv_daily_orders",
    ] {
        assert!(paths.contains(&want.to_owned()), "{want}: {paths:?}");
    }
    assert!(
        !paths.iter().any(|path| path.ends_with(".rows")),
        "row counts are noise: {paths:?}"
    );
    // bench_drift, which no other test writes to while this one runs.
    let same = sql(&[
        "sql",
        "schema",
        "diff",
        "local-postgres:bench_drift",
        "local-postgres:bench_drift",
    ]);
    assert_eq!(same.json()["same"], true, "{same:?}");
    let refused = sql(&[
        "sql",
        "object",
        "diff",
        "local-postgres:bench.orders",
        "local-mssql:bench.orders",
    ]);
    assert_eq!(refused.code, 2, "{refused:?}");
}
