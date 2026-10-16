use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, items, limited};
use crate::shape::selects;

use super::{default_deny, pod_selector, rules, types};

#[derive(clap::Args)]
pub struct NetworkPolicyListArgs {
    #[command(flatten)]
    at: At,
    /// Only the policies that select this pod: its id, namespace/name or name
    #[arg(long)]
    pod: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetworkPolicyRow {
    /// `cluster/namespace/name`: what `networkpolicy get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    /// The pods it applies to: app=api, or every pod.
    pod_selector: String,
    /// Ingress, Egress or both.
    types: Vec<String>,
    ingress_rules: usize,
    egress_rules: usize,
    /// It blocks all traffic in a direction it names, allowing none.
    default_deny: bool,
}

fn networkpolicy_list(ctx: &Ctx, args: NetworkPolicyListArgs) -> Result<Vec<NetworkPolicyRow>> {
    let (target, pod) = match &args.pod {
        Some(raw) => {
            let (target, name) = args.at.named(ctx, raw)?;
            let pod = target.json(ctx, &["get", "pod", &name, "-o", "json"])?;
            (target, Some(pod))
        }
        None => (args.at.listing(ctx)?, None),
    };
    let listed = target.json(ctx, &["get", "networkpolicies", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|policy| {
            pod.as_ref().is_none_or(|pod| {
                selects(&policy["spec"]["podSelector"], &pod["metadata"]["labels"])
            })
        })
        .map(|policy| NetworkPolicyRow {
            id: target.id(policy),
            name: policy["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            namespace: target.row_namespace(policy),
            pod_selector: pod_selector(policy),
            types: types(policy),
            ingress_rules: rules(policy, "ingress").len(),
            egress_rules: rules(policy, "egress").len(),
            default_deny: default_deny(policy),
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub NETWORKPOLICY_LIST = ["k8s", "networkpolicy", "list"], Read,
    "List network policies, or those that apply to one pod: selector, default deny",
    keywords: ["netpol", "network", "policy", "policies", "blocked", "traffic", "deny", "allow", "egress", "ingress"],
    example: "k8s networkpolicy list --pod qa/dev/orders-api-7d9f5b-abc12 --fields id,types,default_deny",
    run: networkpolicy_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn policies_list_with_their_rule_counts_and_default_deny() {
        let outcome = run(&["k8s", "networkpolicy", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "qa/dev/default-deny", "name": "default-deny", "pod_selector": "every pod",
                    "types": ["Ingress", "Egress"], "ingress_rules": 0, "egress_rules": 0, "default_deny": true},
                {"id": "qa/dev/allow-orders-api", "name": "allow-orders-api", "pod_selector": "app=orders-api",
                    "types": ["Ingress", "Egress"], "ingress_rules": 1, "egress_rules": 2, "default_deny": false},
                {"id": "qa/dev/redis-only", "name": "redis-only", "pod_selector": "app in (redis),!tier",
                    "types": ["Ingress"], "ingress_rules": 1, "egress_rules": 0, "default_deny": false},
            ])
        );
    }

    #[test]
    fn a_pod_keeps_only_the_policies_whose_selector_picks_it() {
        let ids = |pod: &str| {
            let outcome = run(&[
                "k8s",
                "networkpolicy",
                "list",
                "--pod",
                pod,
                "--fields",
                "id",
            ]);
            assert_eq!(outcome.code, 0, "{outcome:?}");
            outcome.json()
        };
        assert_eq!(
            ids("orders-api-7d9f5b-abc12"),
            json!([{"id": "qa/dev/default-deny"}, {"id": "qa/dev/allow-orders-api"}])
        );
        assert_eq!(
            ids("qa/dev/redis-0"),
            json!([{"id": "qa/dev/default-deny"}, {"id": "qa/dev/redis-only"}])
        );
        let outcome = run(&["k8s", "networkpolicy", "list", "--pod", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
