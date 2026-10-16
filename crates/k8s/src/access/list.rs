use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, limited};

use super::{AccessRow, rules};

#[derive(clap::Args)]
pub struct AccessListArgs {
    /// Whose access: a serviceaccount's id (cluster/namespace/name), namespace/name or name; @me (the default) is yourself
    #[arg(long = "as")]
    subject: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn access_list(ctx: &Ctx, args: AccessListArgs) -> Result<Vec<AccessRow>> {
    let (target, subject) = match args.subject.as_deref().filter(|held| *held != "@me") {
        Some(account) => {
            let (target, name) = args.at.named(ctx, account)?;
            let namespace = target.namespace.clone().unwrap_or_default();
            (
                target,
                Some(format!("system:serviceaccount:{namespace}:{name}")),
            )
        }
        None => (args.at.one(ctx)?, None),
    };
    let rows = rules(ctx, &target, subject.as_deref())?;
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub ACCESS_LIST = ["k8s", "access", "list"], Read,
    "List what RBAC allows you or a service account to do in a namespace (auth can-i)",
    keywords: ["rbac", "permissions", "can-i", "allowed", "forbidden", "role", "rolebinding", "authorized"],
    example: "k8s access list --as prod/web/worker --fields resource,names,verbs",
    run: access_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn access_lists_what_you_or_a_service_account_may_do() {
        let outcome = run(&["k8s", "access", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"resource": "pods", "verbs": ["get", "list", "watch"]},
                {"resource": "secrets", "verbs": ["get", "list"]}])
        );
        let mine = run(&["k8s", "access", "list", "--as", "@me"]);
        assert_eq!(mine.json(), outcome.json());

        let outcome = k8s(&[
            "k8s",
            "access",
            "list",
            "--as",
            "qa/dev/orders-api",
            "--fields",
            "resource,names",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"resource": "configmaps", "names": ["orders-config"]},
                {"resource": "secrets", "names": ["orders-db"]}])
        );
        let outcome = k8s(&[
            "k8s",
            "access",
            "list",
            "--as",
            "qa/dev/orders-api",
            "--namespace",
            "qa",
        ]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
