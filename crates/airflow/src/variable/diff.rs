//! `airflow variable diff`: two instances' Variables by key: keys on one
//! side only, descriptions, and each value compared in memory, printed only
//! as `same: true|false`; a value Airflow masks (a key named like a secret)
//! has no `same`, since its equality is unknown. A JSON object's values are
//! compared one by one, so feature flags say which flag differs.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, Rules, Secret, command};
use anyhow::Result;
use serde_json::{Value, json};

use crate::client::{Client, segment};
use crate::diff::{Instances, Read, unmasked};

/// The most Variables read from each instance.
const MOST: usize = 1000;

fn variable_diff(ctx: &Ctx, args: Instances) -> Result<Diff> {
    args.compare(ctx, &Rules::PLAIN, variables)
}

fn variables(client: &Client) -> Result<Read> {
    let (items, _) = client.list("variables", "", "variables", MOST)?;
    let mut rows = BTreeMap::new();
    let mut values = BTreeMap::new();
    for item in items {
        let Some(key) = item["key"].as_str().map(str::to_owned) else {
            continue;
        };
        // A list without values gets each from the variable's own GET. 2.9
        // lists them, unmasked even where 3 sends `***`: never printed.
        let value = match item.get("value") {
            Some(value) => value.clone(),
            None => client.get(&format!("variables/{}", segment(&key)))?["value"].clone(),
        };
        let object = value
            .as_str()
            .filter(|text| text.trim_start().starts_with('{'))
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .filter(Value::is_object);
        match object {
            Some(object) => leaves(&format!("{key}.value"), &object, &mut values),
            None => {
                values.insert(format!("{key}.value"), unmasked(&value));
            }
        }
        // Whether a value is encrypted is the instance's Fernet setting, and
        // 2.9 does not list it: no difference between two Variables.
        rows.insert(key, json!({"description": item["description"]}));
    }
    Ok((rows.into_iter().collect(), values))
}

/// Each value of a JSON object by its path under `at`.
fn leaves(at: &str, value: &Value, into: &mut BTreeMap<String, Option<Secret>>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                leaves(&format!("{at}.{key}"), value, into);
            }
        }
        other => {
            into.insert(at.to_owned(), unmasked(other));
        }
    }
}

command! {
    pub VARIABLE_DIFF = ["airflow", "variable", "diff"], Read,
    "Compare two instances' Variables: keys missing on one side, values same or not",
    keywords: ["staging", "missing", "same", "config", "settings"],
    example: "airflow variable diff qa prod",
    run: variable_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{TWO, airflow_with};

    #[test]
    fn variables_missing_on_one_side_and_values_that_differ_print_no_value_and_json_ones_name_the_key()
     {
        let qa = json!({"variables": [
            {"key": "orders_api_token", "value": "***", "description": "Bearer token", "is_encrypted": true},
            {"key": "report_recipients", "value": "[\"qa-team@contoso.example\"]", "description": null, "is_encrypted": false},
            {"key": "feature_flags", "value": "{\"new_checkout\": true, \"beta\": {\"on\": false}}", "description": null},
        ], "total_entries": 3});
        let prod = json!({"variables": [
            {"key": "orders_batch_size", "value": "5000", "description": "Orders per page", "is_encrypted": false},
            {"key": "orders_api_token", "value": "***", "description": "Bearer token", "is_encrypted": true},
            {"key": "report_recipients", "value": "[\"data-eng@contoso.example\"]", "description": null, "is_encrypted": false},
            {"key": "feature_flags", "value": "{\"beta\": {\"on\": false}, \"new_checkout\": false}", "description": null, "is_encrypted": true},
        ], "total_entries": 4});
        let (outcome, _) = airflow_with(
            TWO,
            &["airflow", "variable", "diff", "qa", "prod"],
            vec![Answer::json(&qa), Answer::json(&prod)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([
                {"path": "orders_batch_size", "right": {"description": "Orders per page"}},
                {"path": "feature_flags.value.new_checkout", "redacted": true, "same": false},
                {"path": "orders_api_token.value", "redacted": true},
                {"path": "orders_batch_size.value", "right": "(set)", "redacted": true},
                {"path": "report_recipients.value", "redacted": true, "same": false},
            ])
        );
        assert!(
            !outcome.stdout.contains("contoso.example"),
            "{}",
            outcome.stdout
        );
        assert!(
            outcome.stderr.contains("[1 masked by the service"),
            "{}",
            outcome.stderr
        );
    }
}
