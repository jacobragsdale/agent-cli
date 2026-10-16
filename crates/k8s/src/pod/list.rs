use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use serde_json::Value;

use crate::kubectl::{At, items, limited};

use super::{PodRow, csi_classes, key_vault_ids, row, status_word};

#[derive(clap::Args)]
pub struct PodListArgs {
    /// Part of the pod name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    /// Only pods with this label: app=api, or a key alone (repeatable, all must hold)
    #[arg(long)]
    label: Vec<String>,
    /// Only pods mounting this Key Vault secret: VAULT/NAME, as `kv secret list` prints it
    #[arg(long, conflicts_with = "label")]
    kv: Option<String>,
    /// Only pods on this node, as `pod list` prints it
    #[arg(long)]
    node: Option<String>,
    /// Only pods whose phase or status word is this
    #[arg(long, value_enum, ignore_case = true)]
    status: Option<Status>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A pod's phase, or a status word `pod list` prints: the ones worth
/// finding across a namespace.
#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "lower")]
enum Status {
    Running,
    Pending,
    Succeeded,
    Failed,
    Unknown,
    CrashLoopBackOff,
    ImagePullBackOff,
    ErrImagePull,
    ContainerCreating,
    Completed,
    Error,
    OomKilled,
    Terminating,
}

fn pod_list(ctx: &Ctx, args: PodListArgs) -> Result<Vec<PodRow>> {
    let target = args.at.listing(ctx)?;
    if let Some(kv) = &args.kv
        && !kv.split_once('/').is_some_and(|(vault, name)| {
            !vault.is_empty() && !name.is_empty() && !name.contains('/')
        })
    {
        return Err(Failure::usage(format!("--kv {kv:?} is not VAULT/NAME"))
            .hint("agent-cli kv secret list NAME")
            .into());
    }
    let selector = args.label.join(",");
    // With --kv, one call reads the classes too (a selector would filter
    // them out as well, hence --kv and --label are not taken together).
    let kinds = if args.kv.is_some() {
        "pods,secretproviderclasses"
    } else {
        "pods"
    };
    let mut argv = vec!["get", kinds, "-o", "json"];
    if !selector.is_empty() {
        argv.extend(["-l", selector.as_str()]);
    }
    let listed = target.json(ctx, &argv)?;
    let (pods, classes): (Vec<&Value>, Vec<&Value>) =
        items(&listed).partition(|item| item["kind"].as_str() != Some("SecretProviderClass"));
    let rows = pods
        .into_iter()
        .filter(|pod| {
            args.kv
                .as_deref()
                .is_none_or(|wanted| mounts(pod, &classes, wanted))
        })
        .filter(|item| {
            args.name.as_deref().is_none_or(|part| {
                item["metadata"]["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(part))
            })
        })
        .filter(|item| {
            args.node
                .as_deref()
                .is_none_or(|node| item["spec"]["nodeName"].as_str() == Some(node))
        })
        .filter(|item| {
            args.status.is_none_or(|wanted| {
                let wanted = clap::ValueEnum::to_possible_value(&wanted)
                    .map(|value| value.get_name().to_owned())
                    .unwrap_or_default();
                [
                    status_word(item).as_str(),
                    item["status"]["phase"].as_str().unwrap_or_default(),
                ]
                .iter()
                .any(|held| held.eq_ignore_ascii_case(&wanted))
            })
        })
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// Whether `pod` mounts the Key Vault secret `wanted` (vault/name, any case
/// as Key Vault names are) through a class in its own namespace.
fn mounts(pod: &Value, classes: &[&Value], wanted: &str) -> bool {
    let namespace = &pod["metadata"]["namespace"];
    csi_classes(pod).into_iter().any(|class| {
        classes
            .iter()
            .filter(|held| {
                held["metadata"]["name"].as_str() == Some(class)
                    && &held["metadata"]["namespace"] == namespace
            })
            .flat_map(|held| key_vault_ids(held))
            .any(|id| id.eq_ignore_ascii_case(wanted))
    })
}

command! {
    pub POD_LIST = ["k8s", "pod", "list"], Read,
    "List pods: status, ready, restarts, owner; or those mounting a Key Vault secret",
    keywords: ["containers", "restarting", "crashloop", "crash loop", "crashing", "running", "pending", "unhealthy", "deployments"],
    example: "k8s pod list --cluster qa --namespace dev --fields id,status,restarts,owner",
    run: pod_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{k8s, run};

    #[test]
    fn pod_list_reads_status_ready_restarts_and_the_deployment_behind_a_replica_set() {
        let outcome = run(&["k8s", "pod", "list"]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 5);
        assert_eq!(rows[0]["name"], "orders-api-7d9f5b-abc12");
        assert_eq!(rows[0]["status"], "Running");
        assert_eq!(rows[0]["ready"], "1/1");
        assert_eq!(rows[0]["restarts"], 2);
        assert_eq!(rows[0]["owner"], "Deployment/orders-api");
        assert_eq!(rows[0]["node"], "aks-np1-vmss000000");
        assert!(
            rows[0]["age"].as_str().unwrap().ends_with('d')
                || rows[0]["age"].as_str().unwrap().ends_with("mo")
        );
        assert!(
            rows[0].get("namespace").is_none(),
            "one namespace needs no column"
        );
        assert_eq!(rows[2]["status"], "CrashLoopBackOff");
        assert_eq!(rows[2]["ready"], "0/1");
        assert_eq!(rows[3]["ready"], "2/2");
        assert_eq!(rows[4]["owner"], "StatefulSet/redis");

        let outcome = k8s(&[
            "k8s",
            "pod",
            "list",
            "--cluster",
            "all",
            "--fields",
            "name,namespace",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(
            rows.as_array().unwrap().len(),
            13,
            "every namespace of the qa context"
        );
        assert_eq!(rows[9]["namespace"], "uat");

        let outcome = run(&["k8s", "pod", "list", "worker", "--fields", "name"]);
        assert_eq!(
            outcome.json(),
            json!([{"name": "orders-worker-5c4d3e-q8zt"}])
        );
    }

    #[test]
    fn pod_list_finds_the_pods_that_mount_a_key_vault_secret() {
        let names = |kv: &str| {
            let outcome = run(&["k8s", "pod", "list", "--kv", kv, "--fields", "name,owner"]);
            assert_eq!(outcome.code, 0, "{outcome:?}");
            outcome.json()
        };
        assert_eq!(
            names("kv-contoso-dev/db-password"),
            json!([{"name": "orders-api-7d9f5b-abc12", "owner": "Deployment/orders-api"}])
        );
        assert_eq!(
            names("KV-contoso-dev/api-cert"),
            json!([{"name": "orders-api-7d9f5b-abc12", "owner": "Deployment/orders-api"}])
        );
        assert_eq!(
            names("kv-contoso-dev/orders-signing"),
            json!([]),
            "a key is not a secret"
        );
        assert_eq!(names("kv-contoso-prod/db-password"), json!([]));

        let outcome = run(&["k8s", "pod", "list", "--kv", "db-password"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("is not VAULT/NAME"),
            "{}",
            outcome.stderr
        );
        let outcome = run(&["k8s", "pod", "list", "--kv", "kv/x", "--label", "app=x"]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn pod_list_narrows_to_a_node_and_a_status_word_or_phase() {
        let names = |argv: &[&str]| {
            let mut argv = argv.to_vec();
            argv.extend(["--fields", "name"]);
            let outcome = k8s(&argv);
            assert_eq!(outcome.code, 0, "{outcome:?}");
            outcome.json()
        };
        let all = ["k8s", "pod", "list", "--cluster", "all"];
        assert_eq!(
            names(&[&all[..], &["--status", "crashloopbackoff"]].concat()),
            json!([{"name": "orders-worker-5c4d3e-q8zt"}])
        );
        assert_eq!(
            names(&[&all[..], &["--status", "Succeeded"]].concat()),
            json!([{"name": "nightly-report-29-k8s7f"}]),
            "a phase matches too"
        );
        assert_eq!(
            names(&[&all[..], &["--node", "aks-np1-vmss000009"]].concat()),
            json!([])
        );
        let on_node = names(&[&all[..], &["--node", "aks-np1-vmss000000"]].concat());
        assert!(!on_node.as_array().unwrap().is_empty(), "{on_node}");
    }

    #[test]
    fn pod_list_selects_by_label() {
        let outcome = run(&[
            "k8s",
            "pod",
            "list",
            "--label",
            "app=billing-api",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "billing-api-1a2b3c-qq111"}])
        );
        let outcome = run(&[
            "k8s",
            "pod",
            "list",
            "--label",
            "app=orders-api",
            "--label",
            "pod-template-hash",
            "--fields",
            "name",
        ]);
        assert_eq!(outcome.json().as_array().unwrap().len(), 2, "{outcome:?}");
    }
}
