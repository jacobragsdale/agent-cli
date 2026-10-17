//! `airflow variable diff`: two instances' Variables by key: keys on one
//! side only, descriptions, and each value compared in memory, printed only
//! as `same: true|false`; a value Airflow masks (a key named like a secret)
//! has no `same`, since its equality is unknown.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, Rules, command};
use anyhow::Result;
use serde_json::json;

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
        // Airflow 2 lists keys only; its value is the variable's own GET.
        // VERIFY(work): 2.9's collection leaves out `value`.
        let value = match item.get("value") {
            Some(value) => value.clone(),
            None => client.get(&format!("variables/{}", segment(&key)))?["value"].clone(),
        };
        values.insert(format!("{key}.value"), unmasked(&value));
        rows.insert(
            key,
            json!({"description": item["description"], "encrypted": item["is_encrypted"]}),
        );
    }
    Ok((rows, values))
}

command! {
    pub VARIABLE_DIFF = ["airflow", "variable", "diff"], Read,
    "Compare two instances' Variables: keys missing on one side, values same or not",
    keywords: ["staging", "environments", "missing", "same", "config", "settings"],
    example: "airflow variable diff qa prod",
    run: variable_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{TWO, airflow_with};

    #[test]
    fn variables_missing_on_one_side_and_values_that_differ_print_no_value() {
        let qa = json!({"variables": [
            {"key": "orders_api_token", "value": "***", "description": "Bearer token", "is_encrypted": true},
            {"key": "report_recipients", "value": "[\"qa-team@contoso.example\"]", "description": null, "is_encrypted": false},
        ], "total_entries": 2});
        let prod = json!({"variables": [
            {"key": "orders_batch_size", "value": "5000", "description": "Orders per page", "is_encrypted": false},
            {"key": "orders_api_token", "value": "***", "description": "Bearer token", "is_encrypted": true},
            {"key": "report_recipients", "value": "[\"data-eng@contoso.example\"]", "description": null, "is_encrypted": false},
        ], "total_entries": 3});
        let (outcome, _) = airflow_with(
            TWO,
            &["airflow", "variable", "diff", "qa", "prod"],
            vec![Answer::json(&qa), Answer::json(&prod)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([
                {"path": "orders_batch_size", "right": {"description": "Orders per page", "encrypted": false}},
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
