//! `airflow variable list`: Variables' keys and descriptions
//! (`GET variables?variable_key_pattern=…`), never their values.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, At, note_more, query_value, text};

#[derive(clap::Args)]
pub struct VariableListArgs {
    /// Only keys containing this (% and _ are wildcards)
    pattern: Option<String>,
    #[command(flatten)]
    at: At,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A Variable without its value: no field here can hold one.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VariableRow {
    /// The key, what a DAG reads with Variable.get.
    id: String,
    description: Option<String>,
    /// Stored encrypted with the Fernet key.
    encrypted: bool,
}

fn variable_list(ctx: &Ctx, args: VariableListArgs) -> Result<Vec<VariableRow>> {
    let airflow = Airflow::load(ctx.config())?;
    let client = airflow.open(ctx, args.at.instance.as_deref())?;
    let query = args
        .pattern
        .map(|pattern| format!("variable_key_pattern={}", query_value(pattern.trim())))
        .unwrap_or_default();
    let (variables, total) = client.list("variables", &query, "variables", args.limit)?;
    note_more(ctx, variables.len(), total);
    Ok(variables
        .iter()
        .map(|variable| VariableRow {
            id: variable["key"].as_str().unwrap_or_default().to_owned(),
            description: text(&variable["description"]),
            encrypted: variable["is_encrypted"].as_bool().unwrap_or_default(),
        })
        .collect())
}

command! {
    pub VARIABLE_LIST = ["airflow", "variable", "list"], Read,
    "List Airflow Variables by key and description, never their values",
    keywords: ["variables", "settings", "keys", "config", "names", "defined", "get"],
    example: "airflow variable list orders --fields id,description",
    run: variable_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{airflow, paths};

    #[test]
    fn variable_list_never_prints_a_value() {
        let (outcome, transport) = airflow(
            &["airflow", "variable", "list", "orders"],
            vec![Answer::json(&json!({"variables": [
                {"key": "orders_batch_size", "value": "5000", "description": "Rows per insert",
                 "is_encrypted": false},
                {"key": "orders_api_token", "value": "s3cr3t-variable-value", "description": null,
                 "is_encrypted": true}], "total_entries": 2}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "orders_batch_size", "description": "Rows per insert", "encrypted": false},
                {"id": "orders_api_token", "encrypted": true}])
        );
        assert!(!outcome.stdout.contains("s3cr3t") && !outcome.stdout.contains("5000"));
        assert_eq!(
            paths(&transport),
            ["variables?variable_key_pattern=orders&limit=50&offset=0"]
        );
    }
}
