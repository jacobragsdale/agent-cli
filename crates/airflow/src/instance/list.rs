use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::Airflow;

#[derive(clap::Args)]
pub struct NoArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstanceRow {
    /// What --instance takes.
    name: String,
    base_url: String,
    /// The kind of credential and where it comes from, never its value.
    auth: String,
    /// Every change on it is refused.
    read_only: bool,
    /// The [[k8s.scope]] its task pods run in.
    k8s_scope: Option<String>,
    k8s_namespace: Option<String>,
}

fn instance_list(ctx: &Ctx, _: NoArgs) -> Result<Vec<InstanceRow>> {
    let airflow = Airflow::load(ctx.config())?;
    Ok(airflow
        .instances
        .iter()
        .map(|instance| InstanceRow {
            name: instance.name.clone(),
            base_url: instance.base_url.clone(),
            auth: instance.auth_source(),
            read_only: instance.read_only,
            k8s_scope: instance.k8s_scope.clone(),
            k8s_namespace: instance
                .k8s_scope
                .as_ref()
                .map(|_| instance.k8s_namespace.clone()),
        })
        .collect())
}

command! {
    pub INSTANCE_LIST = ["airflow", "instance", "list"], Read,
    "List the configured Airflow instances (from config, no network)",
    keywords: ["environment", "environments", "server", "deployment", "configured", "astro", "composer", "mwaa"],
    example: "airflow instance list --fields name,base_url,read_only",
    run: instance_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{CONFIG, airflow_with};

    #[test]
    fn instance_list_shows_the_auth_kind_and_never_the_secret() {
        let config = format!(
            "{CONFIG}\n[[airflow.instance]]\nname = \"dev\"\nbase_url = \"http://localhost:8080/\"\n\
             username = \"agent\"\npassword = \"in-file-password-1\"\nread_only = true\n"
        );
        let (outcome, transport) = airflow_with(&config, &["airflow", "instance", "list"], vec![]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"name": "prod", "base_url": "https://airflow.contoso.example",
                 "auth": "token (token_env AIRFLOW_TOKEN)", "read_only": false,
                 "k8s_scope": "prod", "k8s_namespace": "web"},
                {"name": "dev", "base_url": "http://localhost:8080",
                 "auth": "password for agent (password (in the config file))", "read_only": true}
            ])
        );
        assert!(!outcome.stdout.contains("in-file-password-1"));
        assert!(transport.sent().is_empty());
    }
}
