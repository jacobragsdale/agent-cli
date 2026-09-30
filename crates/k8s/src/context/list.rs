use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{finished, program, scopes};

#[derive(clap::Args)]
pub struct NoArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContextRow {
    /// What --cluster takes.
    name: String,
    /// The kubeconfig context it runs kubectl against.
    context: String,
    /// Empty: every namespace.
    namespaces: Vec<String>,
    /// Whether the kubeconfig has that context.
    known: bool,
}

fn context_list(ctx: &Ctx, _: NoArgs) -> Result<Vec<ContextRow>> {
    let scopes = scopes(ctx.config())?;
    let mut contexts = program("kubectl");
    contexts.args(["config", "get-contexts", "-o", "name"]);
    let known = finished(ctx.read(contexts)?)?;
    let known: Vec<&str> = known.lines().map(str::trim).collect();
    Ok(scopes
        .into_iter()
        .map(|scope| ContextRow {
            known: known.contains(&scope.context()),
            context: scope.context().to_owned(),
            name: scope.name,
            namespaces: scope.namespaces,
        })
        .collect())
}

command! {
    pub CONTEXT_LIST = ["k8s", "context", "list"], Read,
    "List the configured cluster scopes and whether kubectl knows each context",
    keywords: ["kubeconfig", "clusters", "namespaces", "configured", "scope", "which"],
    example: "k8s context list --fields name,context,namespaces,known",
    run: context_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{SCOPES, k8s_with};

    #[test]
    fn context_list_says_which_contexts_kubectl_knows() {
        let config = format!("{SCOPES}\n[[k8s.scope]]\nname = \"gone\"\n");
        let outcome = k8s_with(&["k8s", "context", "list"], &config);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows[0],
            json!({"name": "qa", "context": "aks-qa", "namespaces": ["dev", "qa", "uat"], "known": true})
        );
        assert_eq!(
            rows[3],
            json!({"name": "gone", "context": "gone", "known": false})
        );
    }
}
