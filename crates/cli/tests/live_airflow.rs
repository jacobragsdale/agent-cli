//! Every airflow command against a live Airflow, through the built binary.
//! Off unless `AGENT_CLI_TEST_AIRFLOW=1`; start the server with
//! `scripts/airflow-up.sh` (Airflow 2.9, Basic or, with `AIRFLOW_AUTH=session`,
//! session-only sign-in). `AGENT_CLI_TEST_AIRFLOW_URL` (default
//! `http://127.0.0.1:18080`), `_USER` and `_PASSWORD` (default admin, admin)
//! point it elsewhere. It writes (triggers, clears, pauses): point it only
//! at a throwaway server seeded by `scripts/airflow/seed.sh`.
//!
//! The config holds two instances on the one server, `dev` and a
//! `read_only` `prod`, so `--instance` picking and read-only refusal run
//! too. Its writes make runs of their own, leaving the seeded runs (one per
//! state) as they are.

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("{e}: not JSON: {}\n{}", self.stdout, self.stderr))
    }
}

struct Live {
    /// The config and a fresh cache, so a remembered version or sign-in
    /// from another server cannot hide a change.
    dir: tempfile::TempDir,
    password: String,
    stamp: u64,
}

impl Live {
    fn new() -> Option<Self> {
        if std::env::var("AGENT_CLI_TEST_AIRFLOW").as_deref() != Ok("1") {
            eprintln!(
                "skipped live airflow: set AGENT_CLI_TEST_AIRFLOW=1 after scripts/airflow-up.sh"
            );
            return None;
        }
        let env = |name: &str, default: &str| std::env::var(name).unwrap_or(default.to_owned());
        let url = env("AGENT_CLI_TEST_AIRFLOW_URL", "http://127.0.0.1:18080");
        let user = env("AGENT_CLI_TEST_AIRFLOW_USER", "admin");
        let dir = tempfile::tempdir().unwrap();
        let instance = |name: &str, extra: &str| {
            format!(
                "[[airflow.instance]]\nname = \"{name}\"\nbase_url = \"{url}\"\nusername = \"{user}\"\n\
                 password_env = \"AGENT_CLI_TEST_AIRFLOW_PASSWORD\"\n{extra}\n"
            )
        };
        // The hosts seed.sh gives its mssql and oracle connections.
        let config = format!(
            "{}{}\
             [[sql.connection]]\nname = \"orders\"\nkind = \"mssql\"\nhost = \"mssql.contoso.example\"\n\
             database = \"orders\"\nuser = \"etl\"\npassword_cmd = \"echo stand-in\"\n\n\
             [[sql.connection]]\nname = \"warehouse\"\nkind = \"oracle\"\nhost = \"oracle.contoso.example\"\n\
             database = \"FREEPDB1\"\nuser = \"etl\"\npassword_cmd = \"echo stand-in\"\n",
            instance("dev", ""),
            instance("prod", "read_only = true"),
        );
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        Some(Self {
            dir,
            password: env("AGENT_CLI_TEST_AIRFLOW_PASSWORD", "admin"),
            stamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        })
    }

    fn run_as(&self, password: &str, args: &[&str]) -> Ran {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
            .args(args)
            .env("AGENT_CLI_CONFIG", self.dir.path().join("config.toml"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env("AGENT_CLI_TEST_AIRFLOW_PASSWORD", password)
            .env_remove("AGENT_CLI_READ_ONLY")
            .output()
            .unwrap();
        Ran {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// `agent-cli airflow ARGS --instance dev`.
    fn run(&self, args: &[&str]) -> Ran {
        let mut args = args.to_vec();
        args.extend(["--instance", "dev"]);
        self.run_as(&self.password, &args)
    }

    /// Runs `args` on dev, which must exit `code`, and reads what it printed.
    fn exits(&self, code: i32, args: &[&str]) -> (Value, String) {
        let ran = self.run(args);
        assert_eq!(
            ran.code,
            code,
            "agent-cli {}\n{}{}",
            args.join(" "),
            ran.stdout,
            ran.stderr
        );
        let json = if ran.stdout.trim().is_empty() {
            Value::Null
        } else {
            ran.json()
        };
        (json, ran.stderr)
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.exits(0, args).0
    }

    /// The newest run of `dag`'s id.
    fn latest(&self, dag: &str) -> String {
        self.ok(&[
            "airflow",
            "run",
            "get",
            &format!("{dag}/latest"),
            "--fields",
            "id",
        ])["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// doctor, the instances, picking one, read-only, a wrong password; true
    /// when the server takes only session sign-in.
    fn setup(&self) -> bool {
        let ran = self.run_as(&self.password, &["doctor", "airflow"]);
        assert_eq!(ran.code, 0, "{}{}", ran.stdout, ran.stderr);
        let checks = ran.json();
        let credential = checks
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["check"] == "dev credential")
            .unwrap();
        let detail = credential["detail"].as_str().unwrap();
        assert!(detail.contains("signs in by"), "{detail}");
        eprintln!("live airflow: {detail}");

        let instances = self.run_as(&self.password, &["airflow", "instance", "list"]);
        assert_eq!(instances.json()[1]["read_only"], true);
        let unnamed = self.run_as(&self.password, &["airflow", "dag", "list"]);
        assert_eq!(unnamed.code, 2, "{}", unnamed.stderr);
        assert!(unnamed.stderr.contains("dev, prod"), "{}", unnamed.stderr);
        let refused = self.run_as(
            &self.password,
            &[
                "airflow",
                "dag",
                "update",
                "e2e_etl",
                "--paused",
                "true",
                "--instance",
                "prod",
            ],
        );
        assert_eq!(refused.code, 2, "{}", refused.stderr);
        assert!(refused.stderr.contains("read_only"), "{}", refused.stderr);
        let reads = self.run_as(
            &self.password,
            &["airflow", "pool", "list", "--instance", "prod"],
        );
        assert_eq!(
            reads.code, 0,
            "a read_only instance still reads: {}",
            reads.stderr
        );

        let wrong = self.run_as(
            "not-the-password",
            &["airflow", "dag", "list", "--instance", "dev"],
        );
        assert_eq!(wrong.code, 3, "{}{}", wrong.stdout, wrong.stderr);
        assert!(
            !wrong.stderr.contains("not-the-password"),
            "{}",
            wrong.stderr
        );
        detail.contains("sign-in form")
    }

    /// pools, variables, connections, DAGs, their source, import errors.
    fn catalog(&self) {
        let pools = self.ok(&["airflow", "pool", "list"]);
        let zero = pools
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "e2e_zero");
        assert_eq!(zero.unwrap()["slots"], 0, "{pools}");

        let variables = self.ok(&["airflow", "variable", "list", "orders"]);
        assert_eq!(variables[0]["id"], "e2e_orders_bucket", "{variables}");
        assert_eq!(variables[0]["description"], "Where extract_orders writes");
        assert_eq!(variables.as_array().unwrap().len(), 1, "{variables}");
        assert!(
            !self
                .run(&["airflow", "variable", "list"])
                .stdout
                .contains("not-a-real-token")
        );

        let connections = self.ok(&["airflow", "connection", "list", "e2e_%sql"]);
        assert_eq!(connections[0]["type"], "mssql", "{connections}");
        assert_eq!(connections[0]["sql_conn"], "orders", "{connections}");
        let all = self.run(&["airflow", "connection", "list"]);
        assert!(
            all.stdout.contains("\"sql_conn\":\"warehouse\""),
            "{}",
            all.stdout
        );
        assert!(
            !all.stdout.contains("not-a-real-password"),
            "{}",
            all.stdout
        );

        let dags = self.ok(&[
            "airflow", "dag", "list", "e2e", "--tag", "e2e", "--limit", "100",
        ]);
        let paused = dags
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == "e2e_paused")
            .unwrap();
        assert_eq!(paused["paused"], true);
        assert_eq!(paused["schedule"], "0 0 * * *");
        assert!(
            paused["next_run"].as_str().unwrap().ends_with('Z'),
            "{paused}"
        );
        let only_paused = self.ok(&[
            "airflow", "dag", "list", "--paused", "true", "--fields", "id",
        ]);
        assert!(
            only_paused
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["id"] != "e2e_etl")
        );

        let params = self.ok(&["airflow", "dag", "get", "e2e_params"]);
        assert_eq!(params["params"][0]["name"], "day", "{params}");
        assert_eq!(params["params"][0]["description"], "The day to load");
        assert!(
            !params["recent_runs"].as_array().unwrap().is_empty(),
            "{params}"
        );
        let (stale, notes) = self.exits(0, &["airflow", "dag", "get", "e2e_stale"]);
        assert_eq!(stale["stale"], true, "{stale}");
        assert!(notes.contains("is stale"), "{notes}");
        let (_, missing) = self.exits(4, &["airflow", "dag", "get", "e2e_nope"]);
        assert!(
            missing.contains("hint: agent-cli airflow dag list"),
            "{missing}"
        );

        let code = self.ok(&["airflow", "source", "get", "e2e_failing:9"]);
        assert_eq!(code["file"], "e2e_failing.py", "{code}");
        assert!(
            code["text"]
                .as_str()
                .unwrap()
                .contains(" 9          raise ValueError"),
            "{code}"
        );

        let errors = self.ok(&["airflow", "import-error", "list"]);
        let broken = errors
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["file"] == "e2e_broken.py");
        let broken = broken.unwrap_or_else(|| panic!("{errors}"));
        assert!(
            broken["error"]
                .as_str()
                .unwrap()
                .contains("contoso_crm_sdk"),
            "{broken}"
        );
        let error = self.ok(&["airflow", "import-error", "get", "e2e_broken"]);
        assert_eq!(error["line"], 2, "{error}");
        assert!(
            error["stack_trace"]
                .as_str()
                .unwrap()
                .starts_with("Traceback")
        );
    }

    /// The seeded runs, one per state: run list and get, task list and get,
    /// logs and XComs, and run wait's three ends.
    fn every_state(&self) {
        let runs = self.ok(&["airflow", "run", "list", "--since", "30d", "--limit", "100"]);
        for run in runs.as_array().unwrap() {
            for time in ["run_after", "logical_date", "start", "end"] {
                if let Some(stamp) = run[time].as_str() {
                    assert!(stamp.ends_with('Z') && stamp.len() == 20, "{time}: {run}");
                }
            }
        }
        let failed = self.ok(&[
            "airflow",
            "run",
            "list",
            "--dag",
            "e2e_mapped",
            "--state",
            "failed",
        ]);
        assert!(!failed.as_array().unwrap().is_empty(), "{failed}");

        let failing = self.latest("e2e_failing");
        let (run, notes) = self.exits(0, &["airflow", "run", "get", &failing]);
        if run["state"] == "failed" {
            assert_eq!(run["tasks"]["upstream_failed"], 1, "{run}");
            assert!(
                notes.contains("[next: agent-cli airflow task logs"),
                "{notes}"
            );
        }
        let logs = self.ok(&[
            "airflow",
            "task",
            "logs",
            &format!("{failing}/load_orders/1"),
        ]);
        assert_eq!(
            logs["error"], "ValueError: order 88123 has no customer_id",
            "{logs}"
        );
        assert_eq!(logs["at"], "e2e_failing:9", "{logs}");
        assert_eq!(logs["complete"], true);

        let mapped = self.latest("e2e_mapped");
        let tasks = self.ok(&["airflow", "task", "list", &mapped]);
        let ids: Vec<&str> = tasks
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["id"].as_str())
            .collect();
        assert!(ids.iter().any(|id| id.contains("/double:1")), "{ids:?}");
        let (_, unmapped) = self.exits(2, &["airflow", "task", "get", &format!("{mapped}/double")]);
        assert!(unmapped.contains("TASK:N"), "{unmapped}");
        let one = self.ok(&["airflow", "task", "logs", &format!("{mapped}/double:1/1")]);
        assert_eq!(one["error"], "ValueError: cannot double 2", "{one}");
        let xcoms = self.ok(&["airflow", "xcom", "list", &format!("{mapped}/double")]);
        assert_eq!(
            xcoms.as_array().unwrap().len(),
            2,
            "index 1 failed: {xcoms}"
        );
        let doubled = self.ok(&["airflow", "xcom", "get", &format!("{mapped}/double:2")]);
        assert_eq!(doubled["value"], 6, "{doubled}");

        let running = self.latest("e2e_running");
        let starved = self.ok(&["airflow", "task", "get", &format!("{running}/starved")]);
        assert_eq!(starved["pool"], "e2e_zero", "{starved}");
        assert!(
            ["scheduled", "queued"].contains(&starved["state"].as_str().unwrap()),
            "{starved}"
        );
        let (sleeping, notes) =
            self.exits(0, &["airflow", "task", "logs", &format!("{running}/sleep")]);
        assert_eq!(sleeping["complete"], false, "{sleeping}");
        assert!(notes.contains("is still running"), "{notes}");
        let (still, hint) =
            self.exits(124, &["airflow", "run", "wait", &running, "--timeout", "4"]);
        assert_eq!(still["state"], "running");
        assert!(hint.contains("run the same command again"), "{hint}");

        let deferred = self.latest("e2e_deferred");
        let sensor = self.ok(&[
            "airflow",
            "task",
            "get",
            &format!("{deferred}/wait_for_2099"),
        ]);
        assert_eq!(sensor["state"], "deferred", "{sensor}");
        self.ok(&[
            "airflow",
            "task",
            "logs",
            &format!("{deferred}/wait_for_2099"),
        ]);

        let retry = self.latest("e2e_retry");
        let flaky = self.ok(&["airflow", "task", "logs", &format!("{retry}/flaky/1")]);
        assert_eq!(
            flaky["error"], "ConnectionError: warehouse refused the connection",
            "{flaky}"
        );
        assert_eq!(flaky["complete"], true, "try 1 ended: {flaky}");

        let branch = self.latest("e2e_branch");
        let skipped = self.ok(&["airflow", "task", "list", &branch, "--state", "skipped"]);
        assert_eq!(skipped[0]["id"], format!("{branch}/right"), "{skipped}");

        let etl = self.latest("e2e_etl");
        let keys = self.ok(&["airflow", "xcom", "list", &format!("{etl}/extract_orders")]);
        assert_eq!(keys.as_array().unwrap().len(), 2, "{keys}");
        let orders = self.ok(&["airflow", "xcom", "get", &format!("{etl}/extract_orders")]);
        assert_eq!(orders["value"][0]["order_id"], 88122, "{orders}");
        assert_eq!(orders["value"][0]["paid"], true, "{orders}");
        let count = self.ok(&[
            "airflow",
            "xcom",
            "get",
            &format!("{etl}/extract_orders@row_count"),
        ]);
        assert_eq!(count["value"], 2);
        let (big, notes) = self.exits(
            0,
            &["airflow", "xcom", "get", &format!("{etl}/big_payload")],
        );
        assert!(
            big["value"].is_string() && notes.contains("bytes of JSON"),
            "{notes}"
        );
        let (_, missing) = self.exits(
            4,
            &[
                "airflow",
                "xcom",
                "get",
                &format!("{etl}/extract_orders@nope"),
            ],
        );
        assert!(
            missing.contains("hint: agent-cli airflow xcom list"),
            "{missing}"
        );
    }

    /// run create (conf, run id, note, logical date, a duplicate, a paused
    /// DAG), run wait, run retry, task retry and dag update, on runs it makes.
    fn writes(&self) {
        let conf = format!("e2e_conf_{}", self.stamp);
        let (made, notes) = self.exits(
            0,
            &[
                "airflow",
                "run",
                "create",
                "e2e_params",
                "--conf",
                r#"{"day": "2026-09-30", "full": true}"#,
                "--run-id",
                &conf,
                "--note",
                "from the live test",
            ],
        );
        assert_eq!(made["id"], format!("e2e_params/{conf}"));
        assert!(
            notes.contains("[next: agent-cli airflow run wait"),
            "{notes}"
        );
        let (_, duplicate) = self.exits(
            5,
            &["airflow", "run", "create", "e2e_params", "--run-id", &conf],
        );
        assert!(
            duplicate.contains("hint: agent-cli airflow run list --dag e2e_params"),
            "{duplicate}"
        );
        let done = self.ok(&[
            "airflow",
            "run",
            "wait",
            &format!("e2e_params/{conf}"),
            "--timeout",
            "90",
        ]);
        assert_eq!(done["state"], "success", "{done}");
        let run = self.ok(&["airflow", "run", "get", &format!("e2e_params/{conf}")]);
        assert_eq!(run["note"], "from the live test", "{run}");
        let shown = self.ok(&["airflow", "xcom", "get", &format!("e2e_params/{conf}/show")]);
        assert_eq!(
            shown["value"],
            serde_json::json!({"day": "2026-09-30", "full": true})
        );
        let dated = self.ok(&[
            "airflow",
            "run",
            "create",
            "e2e_params",
            "--logical-date",
            &format!(
                "2020-01-01T00:{:02}:{:02}Z",
                self.stamp / 60 % 60,
                self.stamp % 60
            ),
        ]);
        assert!(
            dated["logical_date"]
                .as_str()
                .unwrap()
                .starts_with("2020-01-01T00:"),
            "{dated}"
        );

        let fails = format!("e2e_fails_{}", self.stamp);
        let id = format!("e2e_failing/{fails}");
        self.ok(&[
            "airflow",
            "run",
            "create",
            "e2e_failing",
            "--run-id",
            &fails,
        ]);
        let (waited, hint) = self.exits(1, &["airflow", "run", "wait", &id, "--timeout", "90"]);
        assert_eq!(
            waited["failed"][0],
            format!("{id}/load_orders/1"),
            "{waited}"
        );
        assert!(
            hint.contains(&format!("agent-cli airflow task logs {id}/load_orders/1")),
            "{hint}"
        );
        let (plan, _) = self.exits(0, &["airflow", "run", "retry", &id, "--dry-run"]);
        assert_eq!(plan["dry_run"], true, "{plan}");
        let retried = self.ok(&["airflow", "run", "retry", &id]);
        assert_eq!(retried["cleared"].as_array().unwrap().len(), 2, "{retried}");
        let again = self
            .exits(1, &["airflow", "run", "wait", &id, "--timeout", "90"])
            .0;
        assert_eq!(again["failed"][0], format!("{id}/load_orders/2"), "{again}");
        let first = self.ok(&["airflow", "task", "logs", &format!("{id}/load_orders/1")]);
        assert_eq!(first["complete"], true, "an earlier try has ended: {first}");
        let (_, unconfirmed) = self.exits(
            2,
            &["airflow", "task", "retry", &format!("{id}/load_orders")],
        );
        assert!(unconfirmed.contains("--yes"), "{unconfirmed}");
        let cleared = self.ok(&[
            "airflow",
            "task",
            "retry",
            &format!("{id}/load_orders"),
            "--no-downstream",
            "--yes",
        ]);
        assert_eq!(
            cleared["cleared"],
            serde_json::json!([format!("{id}/load_orders")])
        );
        self.exits(
            4,
            &["airflow", "task", "retry", &format!("{id}/nope"), "--yes"],
        );

        let (queued, notes) = self.exits(0, &["airflow", "run", "create", "e2e_paused"]);
        assert!(notes.contains("is paused"), "{notes}");
        let queued = queued["id"].as_str().unwrap().to_owned();
        let (_, hint) = self.exits(124, &["airflow", "run", "wait", &queued, "--timeout", "4"]);
        assert!(
            hint.contains("dag update e2e_paused --paused false"),
            "{hint}"
        );
        let (_, next) = self.exits(0, &["airflow", "run", "get", &queued]);
        assert!(
            next.contains("[next: agent-cli airflow dag update e2e_paused --paused false"),
            "{next}"
        );

        let pause = self.ok(&["airflow", "dag", "update", "e2e_etl", "--paused", "true"]);
        assert_eq!(pause["paused"], true);
        let resume = self.ok(&["airflow", "dag", "update", "e2e_etl", "--paused", "false"]);
        assert_eq!(resume["paused"], false);
    }
}

#[test]
fn every_airflow_command_holds_on_the_live_server() {
    let Some(live) = Live::new() else { return };
    if live.setup() {
        // Session-only: each command signs in through the web form, which
        // allows 5 sign-ins in 40 s, so only a read and a write follow.
        std::thread::sleep(Duration::from_secs(41));
        live.ok(&["airflow", "dag", "list", "--fields", "id"]);
        let pause = live.ok(&["airflow", "dag", "update", "e2e_etl", "--paused", "true"]);
        assert_eq!(pause["paused"], true);
        live.ok(&["airflow", "dag", "update", "e2e_etl", "--paused", "false"]);
        return;
    }
    live.catalog();
    live.every_state();
    live.writes();
}
