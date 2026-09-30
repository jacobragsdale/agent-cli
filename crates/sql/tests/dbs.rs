//! Every sql command against the two compose databases (`scripts/db-up.sh`),
//! through the real dispatcher, with `config.test.toml`.
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

    let schemas = sql(&["sql", "schema", "list", "--conn", "local-mssql"]).json();
    assert_eq!(schemas, json!(["bench", "dbo"]));
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
    assert!(listed[0]["modified"].as_str().unwrap().contains('T'));

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
