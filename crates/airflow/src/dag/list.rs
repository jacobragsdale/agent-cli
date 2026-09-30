use agent_cli_core::{Ctx, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;

use crate::client::{Airflow, At, note_more, query_value};

use super::{DagRow, RUN_STATES, dag_row};

#[derive(clap::Args)]
pub struct DagListArgs {
    /// Only DAG ids containing this
    pattern: Option<String>,
    /// Only DAGs with this tag (repeatable; any of them)
    #[arg(long)]
    tag: Vec<String>,
    /// true for paused DAGs only, false for active ones only
    #[arg(long)]
    paused: Option<bool>,
    /// Only DAGs whose last run ended in this state
    #[arg(long, value_parser = PossibleValuesParser::new(RUN_STATES))]
    last_state: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn dag_list(ctx: &Ctx, args: DagListArgs) -> Result<Vec<DagRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let mut query = vec!["order_by=dag_id".to_owned()];
    if let Some(pattern) = &args.pattern {
        query.push(format!("dag_id_pattern={}", query_value(pattern.trim())));
    }
    for tag in &args.tag {
        query.push(format!("tags={}", query_value(tag)));
    }
    if let Some(paused) = args.paused {
        query.push(format!("paused={paused}"));
    }
    if let Some(state) = &args.last_state {
        query.push(format!("last_dag_run_state={state}"));
    }
    let (dags, total) = client.list("dags", &query.join("&"), "dags", args.limit)?;
    note_more(ctx, dags.len(), total);
    Ok(dags.iter().map(dag_row).collect())
}

command! {
    pub DAG_LIST = ["airflow", "dag", "list"], Read,
    "List DAGs with their schedule, next run and whether they are paused",
    keywords: ["dags", "workflows", "paused", "tags", "schedule", "owner", "failing", "all"],
    example: "airflow dag list --last-state failed --fields id,schedule,next_run",
    run: dag_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, run};
    use serde_json::{Value, json};

    use crate::DOMAIN;
    use crate::testing::{CONFIG, airflow, airflow_with, dag, paths};

    #[test]
    fn dag_list_filters_server_side_and_notes_the_rest() {
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "dag",
                "list",
                "etl",
                "--tag",
                "etl",
                "--tag",
                "orders team",
                "--paused",
                "false",
                "--last-state",
                "failed",
                "--limit",
                "1",
            ],
            vec![Answer::json(
                &json!({"dags": [dag("etl_nightly", false)], "total_entries": 3}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "etl_nightly", "paused": false, "schedule": "0 0 * * *",
                "next_run": "2026-09-30T00:00:00Z", "tags": ["etl"], "owners": ["data-eng"],
                "file": "etl_nightly.py", "import_errors": false}])
        );
        assert_eq!(
            paths(&transport),
            [
                "dags?order_by=dag_id&dag_id_pattern=etl&tags=etl&tags=orders+team&paused=false&last_dag_run_state=failed&limit=1&offset=0"
            ]
        );
        assert_eq!(outcome.stderr, "[1 of 3; --limit N]\n");
        let sent = &transport.sent()[0];
        assert_eq!(
            sent.authorization.as_deref(),
            Some("Bearer fixture-token-1")
        );
    }

    #[test]
    fn dag_list_pages_past_the_servers_cap_of_100() {
        let page = |n: usize, from: usize| {
            let dags: Vec<Value> = (from..from + n)
                .map(|i| dag(&format!("d{i:03}"), false))
                .collect();
            Answer::json(&json!({"dags": dags, "total_entries": 130}))
        };
        let (outcome, transport) = airflow(
            &["airflow", "dag", "list", "--limit", "150", "--fields", "id"],
            vec![page(100, 0), page(30, 100)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json().as_array().unwrap().len(), 130);
        assert_eq!(
            paths(&transport),
            [
                "dags?order_by=dag_id&limit=100&offset=0",
                "dags?order_by=dag_id&limit=50&offset=100"
            ]
        );
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
    }

    #[test]
    fn a_broken_instance_is_exit_3_naming_it() {
        for (body, want) in [
            (
                "base_url = \"http://airflow.contoso.example\"\ntoken_env = \"T\"",
                "plain http only to localhost",
            ),
            (
                "base_url = \"https://airflow.contoso.example/api/v2\"\ntoken_env = \"T\"",
                "ends in /api/v2",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\nusername = \"a\"",
                "username needs",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"",
                "no credential",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\ntoken_env = \"T\"\ntoken_cmd = \"x\"",
                "give one of token",
            ),
            (
                "base_url = \"https://airflow.contoso.example\"\ntoken_env = \"T\"\nbogus = 1",
                "unknown field",
            ),
        ] {
            let config = format!("[[airflow.instance]]\nname = \"prod\"\n{body}\n");
            let (outcome, _) = airflow_with(&config, &["airflow", "dag", "list"], vec![]);
            assert_eq!(outcome.code, 3, "{body}: {outcome:?}");
            assert!(outcome.stderr.contains(want), "{body}: {}", outcome.stderr);
        }
    }

    #[test]
    fn a_password_signs_in_once_and_again_after_a_401_and_the_jwt_never_prints() {
        let config = "[[airflow.instance]]\nname = \"dev\"\nbase_url = \"http://localhost:8080\"\n\
                      username = \"agent\"\npassword_env = \"AIRFLOW_PASSWORD\"\n";
        let page = json!({"dags": [], "total_entries": 0});
        let transport = FakeTransport::answering([
            Answer::status(201, r#"{"access_token":"eyJhbGciOi.first-jwt.sig1"}"#),
            Answer::status(401, r#"{"detail":"Token expired"}"#),
            Answer::status(201, r#"{"access_token":"eyJhbGciOi.second-jwt.sig2"}"#),
            Answer::json(&page),
        ]);
        let setup = Setup::fake(transport.clone())
            .with_config(config)
            .with_env("AIRFLOW_PASSWORD", "dev-password-1");
        let outcome = run(&[DOMAIN], &["airflow", "dag", "list"], setup);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        let calls: Vec<(&str, &str, Option<&str>)> = sent
            .iter()
            .map(|s| (s.method.wire(), s.url.as_str(), s.authorization.as_deref()))
            .collect();
        let dags = "http://localhost:8080/api/v2/dags?order_by=dag_id&limit=50&offset=0";
        assert_eq!(
            calls,
            [
                ("POST", "http://localhost:8080/auth/token", None),
                ("GET", dags, Some("Bearer eyJhbGciOi.first-jwt.sig1")),
                ("POST", "http://localhost:8080/auth/token", None),
                ("GET", dags, Some("Bearer eyJhbGciOi.second-jwt.sig2")),
            ]
        );
        assert!(sent[0].method.is_read(), "the sign-in is a read");
        assert_eq!(
            sent[0].body,
            Some(json!({"username": "agent", "password": "dev-password-1"}))
        );
        for secret in ["first-jwt", "second-jwt", "dev-password-1"] {
            assert!(!outcome.stdout.contains(secret) && !outcome.stderr.contains(secret));
        }

        let refused =
            FakeTransport::answering([Answer::status(401, r#"{"detail":"Invalid credentials"}"#)]);
        let setup = Setup::fake(refused)
            .with_config(config)
            .with_env("AIRFLOW_PASSWORD", "wrong-password-1");
        let outcome = run(&[DOMAIN], &["airflow", "dag", "list"], setup);
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Invalid credentials")
                && outcome
                    .stderr
                    .contains("`agent-cli doctor airflow` checks it"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn refusals_read_as_the_next_step() {
        for (answer, code, hint) in [
            (
                Answer::status(401, r#"{"detail":"Invalid token"}"#),
                3,
                "`agent-cli doctor airflow` checks it",
            ),
            (
                Answer::status(403, r#"{"detail":"Forbidden"}"#),
                1,
                "lacks this permission",
            ),
            (
                Answer::status(307, "").with_header("Location", "https://login.contoso.example/"),
                3,
                "base_url is wrong",
            ),
        ] {
            // A 401 is retried once with a fresh token, so it is answered twice.
            let answers = if answer.status == 401 {
                vec![answer.clone(), answer]
            } else {
                vec![answer]
            };
            let (outcome, _) = airflow_with(CONFIG, &["airflow", "dag", "list"], answers);
            assert_eq!(outcome.code, code, "{outcome:?}");
            assert!(outcome.stderr.contains(hint), "{}", outcome.stderr);
        }
    }

    #[test]
    fn two_instances_and_no_flag_is_exit_2_naming_both_before_any_request() {
        let config = format!(
            "{CONFIG}\n[[airflow.instance]]\nname = \"qa\"\nbase_url = \"https://qa.contoso.example\"\ntoken_env = \"T\"\n"
        );
        let (outcome, transport) = airflow_with(&config, &["airflow", "dag", "list"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("prod, qa"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());
    }
}
