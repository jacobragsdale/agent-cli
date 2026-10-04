//! `airflow xcom get`: one XCom's value
//! (`GET …/xcomEntries/KEY?deserialize=true&stringify=false`), as JSON.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Airflow, At, Want, segment, stamp};

use super::{entries_path, map_query, task_id};

/// The most of a value printed, as JSON text: under core's 12,000-byte
/// output guard, which would otherwise print a stub in its place.
const MOST: usize = 10_000;

#[derive(clap::Args)]
pub struct XcomGetArgs {
    /// The XCom: DAG/RUN/TASK[:MAP]@KEY (from xcom list), or a task instance's id or URL with --key
    xcom: String,
    /// The key, when the id leaves it out (default return_value)
    #[arg(long)]
    key: Option<String>,
    /// The DAG, when the id leaves it out
    #[arg(long)]
    dag: Option<String>,
    /// The run id, when the id leaves it out
    #[arg(long)]
    run: Option<String>,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Xcom {
    /// DAG/RUN/TASK[:MAP]@KEY.
    id: String,
    key: String,
    /// When the task pushed it.
    time: Option<String>,
    /// As the task returned or pushed it; past 10,000 bytes, the start of its
    /// JSON as text.
    value: Value,
}

fn xcom_get(ctx: &Ctx, args: XcomGetArgs) -> Result<Xcom> {
    let raw = args.xcom.trim();
    let url = raw.starts_with("https://") || raw.starts_with("http://");
    let (task, held) = match raw.split_once('@') {
        Some((task, key)) if !url => (task, Some(key)),
        _ => (raw, None),
    };
    if let (Some(held), Some(flag)) = (held, args.key.as_deref())
        && held != flag
    {
        return Err(
            Failure::usage(format!("{raw} names key {held}, and --key says {flag}")).into(),
        );
    }
    let key = held.or(args.key.as_deref()).unwrap_or("return_value");
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) = airflow.locate(
        ctx,
        args.at.instance.as_deref(),
        task,
        Want::Task,
        args.dag.as_deref(),
        args.run.as_deref(),
    )?;
    // Airflow 2 has no stringify, and deserialize only when its config
    // allows: it prints the value as Python's str() of it.
    let v1 = client.v1()?;
    let mut query = if v1 {
        Vec::new()
    } else {
        vec!["deserialize=true".to_owned(), "stringify=false".to_owned()]
    };
    query.extend(Some(map_query(&id)).filter(|map| !map.is_empty()));
    let mut path = format!("{}/{}", entries_path(&id), segment(key));
    if !query.is_empty() {
        path = format!("{path}?{}", query.join("&"));
    }
    let mut entry = client
        .get(&path)
        .map_err(|error| match error.downcast::<Failure>() {
            Ok(failure) if failure.status == Some(404) => failure
                .hint(format!(
                    "agent-cli airflow xcom list {}",
                    task_id(&id, id.map)
                ))
                .into(),
            Ok(failure) => failure.into(),
            Err(error) => error,
        })?;
    let mut value = entry["value"].take();
    if v1 && let Some(text) = value.as_str() {
        value = from_python(text);
    }
    let json = value.to_string();
    if json.len() > MOST {
        let mut cut = MOST;
        while !json.is_char_boundary(cut) {
            cut -= 1;
        }
        ctx.note(format!(
            "[the value is {} bytes of JSON; value holds the first {MOST} as text]",
            json.len()
        ));
        value = Value::String(json[..cut].to_owned());
    }
    Ok(Xcom {
        id: format!("{}@{key}", task_id(&id, id.map)),
        key: key.to_owned(),
        time: stamp(&entry["timestamp"]),
        value,
    })
}

/// A value as Python's `str()` prints it, as JSON: `'` strings, `True`,
/// `False`, `None` and tuples become JSON's. Anything else (a set, a
/// datetime, a plain string) stays the text it is.
fn from_python(text: &str) -> Value {
    fn convert(text: &str) -> Option<String> {
        let mut json = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' | '"' => {
                    let mut string = String::new();
                    loop {
                        match chars.next()? {
                            end if end == c => break,
                            '\\' => match chars.next()? {
                                'n' => string.push('\n'),
                                't' => string.push('\t'),
                                'r' => string.push('\r'),
                                'x' => {
                                    let hex: String = chars.by_ref().take(2).collect();
                                    string.push(char::from(u8::from_str_radix(&hex, 16).ok()?));
                                }
                                other => string.push(other),
                            },
                            other => string.push(other),
                        }
                    }
                    json.push_str(&serde_json::to_string(&string).ok()?);
                }
                c if c.is_ascii_alphabetic() => {
                    let mut word = String::from(c);
                    while let Some(next) = chars.next_if(char::is_ascii_alphanumeric) {
                        word.push(next);
                    }
                    json.push_str(match word.as_str() {
                        "True" => "true",
                        "False" => "false",
                        "None" => "null",
                        _ => return None,
                    });
                }
                '(' => json.push('['),
                ')' | ']' | '}' => {
                    // A one-item tuple's trailing comma.
                    let kept = json.trim_end().len();
                    if json[..kept].ends_with(',') {
                        json.truncate(kept - 1);
                    }
                    json.push(if c == ')' { ']' } else { c });
                }
                other => json.push(other),
            }
        }
        Some(json)
    }
    convert(text)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_else(|| Value::String(text.to_owned()))
}

command! {
    pub XCOM_GET = ["airflow", "xcom", "get"], Read,
    "Show an XCom's value: what a task returned or pushed for its downstream tasks",
    keywords: ["return", "value", "output", "result", "pass", "passed", "data", "input", "upstream", "downstream"],
    example: "airflow xcom get etl_nightly/latest/extract_orders@return_value --fields value",
    run: xcom_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RUN, RUN_PATH, airflow, paths};

    #[test]
    fn xcom_get_prints_the_value_as_json_and_defaults_to_return_value() {
        let orders = json!([{"order_id": 88122, "customer_id": 4411}, {"order_id": 88123}]);
        let answer = json!({"key": "return_value", "timestamp": "2026-09-29T00:12:31.5+00:00",
            "map_index": -1, "task_id": "extract_orders", "dag_id": "etl_nightly", "run_id": RUN,
            "value": orders});
        let task = format!("etl_nightly/{RUN}/extract_orders");
        for id in [format!("{task}@return_value"), task.clone()] {
            let (outcome, transport) = airflow(
                &["airflow", "xcom", "get", &id],
                vec![Answer::json(&answer)],
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(
                outcome.json(),
                json!({"id": format!("etl_nightly/{RUN}/extract_orders@return_value"),
                    "key": "return_value", "time": "2026-09-29T00:12:31Z", "value": orders})
            );
            assert_eq!(
                paths(&transport),
                [format!(
                    "{RUN_PATH}/taskInstances/extract_orders/xcomEntries/return_value?deserialize=true&stringify=false"
                )]
            );
        }
    }

    #[test]
    fn airflow_2s_python_printed_values_read_as_json() {
        for (text, want) in [
            (
                "[{'order_id': 88122, 'paid': True, 'note': None}]",
                json!([{"order_id": 88122, "paid": true, "note": null}]),
            ),
            ("(1,)", json!([1])),
            (r#"{'a\'b': "x\ny"}"#, json!({"a'b": "x\ny"})),
            (r"'caf\xe9'", json!("café")),
            ("2", json!(2)),
            ("plain text", json!("plain text")),
            ("{1, 2}", json!("{1, 2}")),
        ] {
            assert_eq!(super::from_python(text), want, "{text}");
        }
    }

    #[test]
    fn a_huge_value_is_cut_and_a_missing_key_hints_the_list() {
        let big = json!({"key": "rows", "value": vec!["x".repeat(100); 400], "map_index": 2});
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "xcom",
                "get",
                &format!("etl_nightly/{RUN}/extract_orders:2"),
                "--key",
                "rows",
            ],
            vec![Answer::json(&big)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let value = outcome.json()["value"].as_str().unwrap().to_owned();
        assert_eq!(value.len(), 10_000);
        assert!(
            outcome.stderr.contains("the value is 41201 bytes"),
            "{}",
            outcome.stderr
        );
        assert!(
            paths(&transport)[0]
                .ends_with("xcomEntries/rows?deserialize=true&stringify=false&map_index=2")
        );
        let (outcome, _) = airflow(
            &[
                "airflow",
                "xcom",
                "get",
                &format!("etl_nightly/{RUN}/extract_orders@nope"),
            ],
            vec![Answer::status(
                404,
                r#"{"detail":"XCom entry with key: `nope` not found"}"#,
            )],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains(&format!(
                "hint: agent-cli airflow xcom list etl_nightly/{RUN}/extract_orders"
            )),
            "{}",
            outcome.stderr
        );
        let (outcome, transport) = airflow(
            &[
                "airflow",
                "xcom",
                "get",
                "etl_nightly/r1/extract_orders@a",
                "--key",
                "b",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn xcom_get_on_airflow_2_sends_no_stringify_and_reads_pythons_print() {
        let answer = json!({"key": "return_value", "timestamp": "2026-09-29T00:12:31.5+00:00",
            "map_index": 2, "task_id": "extract_orders", "dag_id": "etl_nightly",
            "value": "[{'order_id': 88122, 'paid': True}]"});
        let (outcome, transport) = crate::testing::airflow_v1(
            &[
                "airflow",
                "xcom",
                "get",
                &format!("etl_nightly/{RUN}/extract_orders:2"),
            ],
            vec![Answer::json(&answer)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["value"],
            json!([{"order_id": 88122, "paid": true}])
        );
        assert_eq!(
            paths(&transport),
            [format!(
                "{RUN_PATH}/taskInstances/extract_orders/xcomEntries/return_value?map_index=2"
            )]
        );
    }
}
