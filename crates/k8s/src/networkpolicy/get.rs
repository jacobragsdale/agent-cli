use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::kubectl::{At, items};
use crate::shape::{EventRow, events_for, selector_text, selects};

use super::{default_deny, pod_selector, rules, types};

/// The most selected pods a policy prints; a policy for every pod in a big
/// namespace would otherwise print them all.
const PODS: usize = 50;

#[derive(clap::Args)]
pub struct NetworkPolicyGetArgs {
    /// The policy: its id (cluster/namespace/name), namespace/name, or name
    policy: String,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetworkPolicyDetail {
    id: String,
    name: String,
    namespace: String,
    pod_selector: String,
    /// The ids of the pods it selects now.
    pods: Vec<String>,
    types: Vec<String>,
    default_deny: bool,
    /// What it allows, a rule per entry; a direction it names with no rule
    /// allows nothing.
    rules: Vec<Rule>,
    /// The five newest events about it, newest first.
    events: Vec<EventRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Rule {
    /// ingress (from the peers) or egress (to them).
    direction: &'static str,
    /// `pods app=api`, `namespaces team=edge`, `cidr 10.0.0.0/8 except …`,
    /// or `anywhere`.
    peers: Vec<String>,
    /// `5432/TCP`, `8000-8100/TCP`, `http/TCP`, or `every port`.
    ports: Vec<String>,
}

fn networkpolicy_get(ctx: &Ctx, args: NetworkPolicyGetArgs) -> Result<NetworkPolicyDetail> {
    let (target, name) = args.at.named(ctx, &args.policy)?;
    let policy = target.json(ctx, &["get", "networkpolicy", &name, "-o", "json"])?;
    let pods = target.json(ctx, &["get", "pods", "-o", "json"])?;
    let mut selected: Vec<String> = items(&pods)
        .filter(|pod| selects(&policy["spec"]["podSelector"], &pod["metadata"]["labels"]))
        .map(|pod| target.id(pod))
        .collect();
    if selected.len() > PODS {
        ctx.note(format!("[{PODS} of {} selected pods]", selected.len()));
        selected.truncate(PODS);
    }
    let mut flat = Vec::new();
    for (direction, peers_key) in [("ingress", "from"), ("egress", "to")] {
        for rule in rules(&policy, direction) {
            flat.push(Rule {
                direction,
                peers: listed(&rule[peers_key], peer, "anywhere"),
                ports: listed(&rule["ports"], port, "every port"),
            });
        }
    }
    Ok(NetworkPolicyDetail {
        id: target.id(&policy),
        name,
        namespace: target.namespace.clone().unwrap_or_default(),
        pod_selector: pod_selector(&policy),
        pods: selected,
        types: types(&policy),
        default_deny: default_deny(&policy),
        rules: flat,
        events: events_for(ctx, &target, &policy),
    })
}

/// Each entry in words, or `all` when there are none: an empty `from`,
/// `to` or `ports` allows everything.
fn listed(entries: &Value, word: fn(&Value) -> String, all: &str) -> Vec<String> {
    let words: Vec<String> = entries.as_array().into_iter().flatten().map(word).collect();
    if words.is_empty() {
        vec![all.to_owned()]
    } else {
        words
    }
}

/// One peer: pods of this namespace, namespaces, pods in namespaces, or a
/// CIDR with its exceptions.
fn peer(peer: &Value) -> String {
    if let Some(cidr) = peer["ipBlock"]["cidr"].as_str() {
        let except: Vec<&str> = peer["ipBlock"]["except"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        return if except.is_empty() {
            format!("cidr {cidr}")
        } else {
            format!("cidr {cidr} except {}", except.join(","))
        };
    }
    let pods = peer.get("podSelector").map(|selector| {
        selector_text(selector)
            .map_or_else(|| "every pod".to_owned(), |text| format!("pods {text}"))
    });
    let namespaces = peer.get("namespaceSelector").map(|selector| {
        selector_text(selector).map_or_else(
            || "every namespace".to_owned(),
            |text| format!("namespaces {text}"),
        )
    });
    match (pods, namespaces) {
        (Some(pods), Some(namespaces)) => format!("{pods} in {namespaces}"),
        (Some(pods), None) => format!("{pods} in this namespace"),
        (None, Some(namespaces)) => namespaces,
        (None, None) => "anywhere".to_owned(),
    }
}

/// `5432/TCP`, `8000-8100/TCP` with an end port, `http/TCP` by name.
fn port(port: &Value) -> String {
    let protocol = port["protocol"].as_str().unwrap_or("TCP");
    let number = match &port["port"] {
        Value::Null => "every port".to_owned(),
        Value::String(name) => name.clone(),
        other => other.to_string(),
    };
    match port["endPort"].as_i64() {
        Some(end) => format!("{number}-{end}/{protocol}"),
        None => format!("{number}/{protocol}"),
    }
}

command! {
    pub NETWORKPOLICY_GET = ["k8s", "networkpolicy", "get"], Read,
    "Show a network policy's rules in words: peers, ports, and the pods it selects",
    keywords: ["netpol", "network", "policy", "allowed", "blocked", "traffic", "egress", "ingress", "cidr"],
    example: "k8s networkpolicy get qa/dev/allow-orders-api --fields pods,rules",
    run: networkpolicy_get,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn a_policy_reads_as_its_selected_pods_and_rules_in_words() {
        let outcome = run(&["k8s", "networkpolicy", "get", "allow-orders-api"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let policy = outcome.json();
        assert_eq!(
            policy["pods"],
            json!([
                "qa/dev/orders-api-7d9f5b-abc12",
                "qa/dev/orders-api-7d9f5b-k9x2p"
            ])
        );
        assert_eq!(
            policy["rules"],
            json!([
                {"direction": "ingress",
                    "peers": ["pods app notin (redis) in this namespace", "namespaces team=edge"],
                    "ports": ["http/TCP"]},
                {"direction": "egress", "peers": ["cidr 10.0.0.0/8 except 10.0.99.0/24"],
                    "ports": ["5432/TCP", "8000-8100/TCP"]},
                {"direction": "egress", "peers": ["anywhere"], "ports": ["every port"]},
            ])
        );
        assert_eq!(policy["default_deny"], false);

        let redis = run(&["k8s", "networkpolicy", "get", "redis-only"]).json();
        assert_eq!(redis["pods"], json!(["qa/dev/redis-0"]));
        assert_eq!(
            redis["rules"][0]["peers"],
            json!(["pods app in every namespace"])
        );

        let deny = run(&["k8s", "networkpolicy", "get", "qa/dev/default-deny"]).json();
        assert_eq!(deny["default_deny"], true);
        assert_eq!(deny["pod_selector"], "every pod");
        assert!(deny.get("rules").is_none(), "{deny}");

        let outcome = run(&["k8s", "networkpolicy", "get", "gone"]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli k8s networkpolicy list"),
            "{}",
            outcome.stderr
        );
    }
}
