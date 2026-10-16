use std::collections::BTreeSet;

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::kubectl::{At, age, items, limited, non_empty};

use super::{address, class};

#[derive(clap::Args)]
pub struct IngressListArgs {
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IngressRow {
    /// `cluster/namespace/name`: what `ingress get` takes.
    id: String,
    name: String,
    /// Only when the listing spans namespaces.
    namespace: Option<String>,
    class: Option<String>,
    hosts: Vec<String>,
    /// The TLS secrets' names.
    tls: Vec<String>,
    /// The controller's load balancer IPs or host names.
    address: Vec<String>,
    age: Option<String>,
}

fn ingress_list(ctx: &Ctx, args: IngressListArgs) -> Result<Vec<IngressRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "ingresses", "-o", "json"])?;
    let rows = items(&listed)
        .map(|item| {
            let spec = &item["spec"];
            let hosts: BTreeSet<&str> = spec["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|rule| non_empty(&rule["host"]))
                .collect();
            IngressRow {
                id: target.id(item),
                name: item["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                namespace: target.row_namespace(item),
                class: class(item),
                hosts: hosts.into_iter().map(str::to_owned).collect(),
                tls: spec["tls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tls| non_empty(&tls["secretName"]).map(str::to_owned))
                    .collect(),
                address: address(item),
                age: age(&item["metadata"]["creationTimestamp"]),
            }
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub INGRESS_LIST = ["k8s", "ingress", "list"], Read,
    "List ingresses: the hosts each serves, class, TLS secrets, address",
    keywords: ["kubernetes", "routes", "domain", "hostname", "https", "public", "url"],
    example: "k8s ingress list --cluster qa --namespace dev --fields id,hosts,address",
    run: ingress_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn ingresses_list_their_hosts_tls_secrets_and_address() {
        let outcome = run(&["k8s", "ingress", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows,
            json!([{"id": "qa/dev/orders", "name": "orders", "class": "nginx",
                "hosts": ["billing.contoso.example", "orders.contoso.example"],
                "tls": ["orders-tls", "billing-tls"], "address": ["10.240.0.60"], "age": rows[0]["age"]}])
        );
    }
}
