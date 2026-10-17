//! `airflow instance diff`: two instances' Airflow version and installed
//! provider packages with their versions (`GET version`, `GET providers`):
//! a task that works on qa and fails on prod may be a provider apart.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, Rules, command};
use anyhow::Result;
use serde_json::{Map, Value};

use crate::diff::Instances;

fn instance_diff(ctx: &Ctx, args: Instances) -> Result<Diff> {
    args.compare(ctx, &Rules::PLAIN, |client| {
        let version = client.get("version")?["version"].clone();
        let (items, _) = client.list("providers", "", "providers", 1000)?;
        let providers: Map<String, Value> = items
            .iter()
            .filter_map(|provider| {
                Some((
                    provider["package_name"].as_str()?.to_owned(),
                    provider["version"].clone(),
                ))
            })
            .collect();
        // The version first: the note names the first differences.
        let mut rows = Map::new();
        rows.insert("version".to_owned(), version);
        rows.insert("providers".to_owned(), Value::Object(providers));
        Ok((rows, BTreeMap::new()))
    })
}

command! {
    pub INSTANCE_DIFF = ["airflow", "instance", "diff"], Read,
    "Compare two instances' Airflow version and installed provider packages",
    keywords: ["staging", "providers", "packages", "upgrade", "version"],
    example: "airflow instance diff qa prod",
    run: instance_diff,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{TWO, airflow_with, paths};

    #[test]
    fn two_instances_differ_by_version_and_a_provider() {
        let providers = |version: &str| {
            Answer::json(&json!({"providers": [
                {"package_name": "apache-airflow-providers-microsoft-mssql", "version": version, "description": "MSSQL"},
            ], "total_entries": 1}))
        };
        let (outcome, transport) = airflow_with(
            TWO,
            &["airflow", "instance", "diff", "qa", "prod"],
            vec![
                Answer::json(&json!({"version": "3.3.1"})),
                providers("4.3.0"),
                Answer::json(&json!({"version": "3.3.2"})),
                providers("4.2.1"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["changes"],
            json!([
                {"path": "version", "left": "3.3.1", "right": "3.3.2"},
                {"path": "providers.apache-airflow-providers-microsoft-mssql", "left": "4.3.0", "right": "4.2.1"},
            ])
        );
        assert_eq!(paths(&transport)[2], "version");
    }
}
