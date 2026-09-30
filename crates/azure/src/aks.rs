//! `aks`: AKS clusters. `cluster list` reads the Resource Graph inventory;
//! `cluster connect` is az-tui's `setup` for one cluster, without the config
//! file writing: `az aks get-credentials` then `kubelogin convert-kubeconfig`,
//! and a ready-to-paste `[[k8s.scope]]` block for the k8s domain.

use std::process::Command;

use agent_cli_core::{Check, Config, Ctx, Effect, Failure, Op, Output, command, run_until};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::graph::{Cluster, inventory};
use crate::{Azure, first_line, limited, program};

/// Namespaces every AKS cluster has that nobody wants a scope for.
const SYSTEM_NAMESPACES: &[&str] = &[
    "default",
    "kube-system",
    "kube-public",
    "kube-node-lease",
    "gatekeeper-system",
    "calico-system",
    "tigera-operator",
    "azure-arc",
    "app-routing-system",
    "aks-command",
    "aks-istio-system",
    "aks-istio-ingress",
    "aks-istio-egress",
];

// ---------- aks cluster list ----------

#[derive(clap::Args)]
pub struct ClusterListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A cluster, and the name the k8s domain knows it by.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ClusterRow {
    #[serde(flatten)]
    cluster: Cluster,
    /// The `[[k8s.scope]]` name `aks cluster connect` writes for it: what
    /// `k8s … --cluster` takes (k8s also accepts the context, which is the
    /// cluster's name).
    k8s_scope: String,
}

fn cluster_list(ctx: &Ctx, args: ClusterListArgs) -> Result<Vec<ClusterRow>> {
    let azure = Azure::load(ctx)?;
    let rows = inventory(ctx, &azure)?
        .clusters
        .into_iter()
        .map(|cluster| ClusterRow {
            k8s_scope: cluster.name.clone(),
            cluster,
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub CLUSTER_LIST = ["aks", "cluster", "list"], Read,
    "List the AKS clusters the az login reaches, with version and power state",
    keywords: ["kubernetes", "clusters", "managed", "running", "stopped", "version"],
    example: "aks cluster list --fields name,k8s_scope,power_state",
    run: cluster_list,
}

// ---------- aks cluster connect ----------

#[derive(clap::Args)]
pub struct ClusterConnectArgs {
    /// The cluster's name, as `aks cluster list` shows it
    name: String,
    /// Its resource group; looked up by name when left out
    #[arg(long)]
    resource_group: Option<String>,
    /// Its subscription id; the az default, or looked up with the group
    #[arg(long)]
    subscription: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Connected {
    cluster: String,
    resource_group: String,
    subscription: Option<String>,
    /// The kubeconfig context `get-credentials` wrote: what `[[k8s.scope]]`
    /// context takes.
    context: String,
    /// Whether the kubeconfig now borrows the az login, or why not.
    kubelogin: String,
    /// The cluster's own namespaces, system ones left out.
    namespaces: Vec<String>,
    /// A `[[k8s.scope]]` block to trim and paste into config.toml.
    config: String,
}

fn cluster_connect(ctx: &Ctx, args: ClusterConnectArgs) -> Result<Connected> {
    let (resource_group, subscription) = match args.resource_group {
        Some(group) => (group, args.subscription),
        None => {
            let found = locate(ctx, &args.name, args.subscription.as_deref())?;
            (found.resource_group, Some(found.subscription))
        }
    };
    let mut credentials = program("az");
    credentials.args([
        "aks",
        "get-credentials",
        "--resource-group",
        &resource_group,
        "--name",
        &args.name,
        "--overwrite-existing",
    ]);
    if let Some(subscription) = &subscription {
        credentials.args(["--subscription", subscription]);
    }
    let mut convert = program("kubelogin");
    convert.args(["convert-kubeconfig", "-l", "azurecli"]);
    let kubelogin = ctx.write(
        Effect::Write,
        Connect {
            credentials,
            convert,
        },
    )?;
    // What the cluster calls its namespaces, for the block. A cluster that
    // will not say yet (RBAC, a device-code prompt) still gets a block.
    let mut listing = program("kubectl");
    listing.args([
        "--context",
        &args.name,
        "--request-timeout=10s",
        "get",
        "namespaces",
        "-o",
        "name",
    ]);
    let namespaces: Vec<String> = match ctx.read(listing) {
        Ok(output) if output.status.success() => output
            .stdout
            .lines()
            .filter_map(|line| line.trim().strip_prefix("namespace/"))
            .filter(|name| !SYSTEM_NAMESPACES.contains(name))
            .map(str::to_owned)
            .collect(),
        Ok(output) => {
            ctx.note(format!(
                "[namespaces not read: {}]",
                first_line(&output.stderr)
            ));
            Vec::new()
        }
        Err(error) => {
            ctx.note(format!("[namespaces not read: {error:#}]"));
            Vec::new()
        }
    };
    let quoted: Vec<String> = namespaces.iter().map(|name| format!("{name:?}")).collect();
    Ok(Connected {
        config: format!(
            "[[k8s.scope]]\nname = {:?}\ncontext = {:?}\nnamespaces = [{}]\n",
            args.name,
            args.name,
            quoted.join(", ")
        ),
        context: args.name.clone(),
        cluster: args.name,
        resource_group,
        subscription,
        kubelogin,
        namespaces,
    })
}

/// The one cluster in reach called `name`: two is exit 2 naming where each
/// is, none is exit 4.
fn locate(ctx: &Ctx, name: &str, subscription: Option<&str>) -> Result<Cluster> {
    let azure = Azure::load(ctx)?;
    let mut found: Vec<Cluster> = inventory(ctx, &azure)?
        .clusters
        .into_iter()
        .filter(|cluster| cluster.name.eq_ignore_ascii_case(name))
        .filter(|cluster| {
            subscription.is_none_or(|wanted| cluster.subscription.eq_ignore_ascii_case(wanted))
        })
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(
            Failure::not_found(format!("no AKS cluster {name} in reach of the az login"))
                .hint("agent-cli aks cluster list --fields name,resource_group,subscription")
                .into(),
        ),
        _ => {
            let places: Vec<String> = found
                .iter()
                .map(|cluster| format!("{} in {}", cluster.resource_group, cluster.subscription))
                .collect();
            Err(Failure::usage(format!(
                "there is more than one cluster {name} ({}); name one with --resource-group and --subscription",
                places.join("; ")
            ))
            .into())
        }
    }
}

/// The two commands that change the kubeconfig, as one change: `--dry-run`
/// shows both argv, and a failed `get-credentials` stops before `kubelogin`.
/// `kubelogin` failing is reported rather than fatal, as in az-tui: the
/// credentials are written, and a cluster without Entra ID sign-in needs no
/// conversion.
struct Connect {
    credentials: Command,
    convert: Command,
}

impl Op for Connect {
    type Output = String;

    fn plan(&self) -> Value {
        json!({"run": [self.credentials.plan()["run"], self.convert.plan()["run"]]})
    }

    fn writes(&self) -> bool {
        true
    }

    fn perform(self, ctx: &Ctx) -> Result<String> {
        let written = run_until(self.credentials, ctx.deadline())?;
        if !written.status.success() {
            let reason = first_line(&written.stderr);
            let failure = if reason.contains("az login") {
                Failure::setup(format!("az aks get-credentials failed: {reason}")).hint("az login")
            } else {
                Failure::new(
                    agent_cli_core::Exit::Failed,
                    format!("az aks get-credentials failed: {reason}"),
                )
            };
            return Err(failure.into());
        }
        Ok(match run_until(self.convert, ctx.deadline()) {
            Ok(Output { status, .. }) if status.success() => {
                "converted to use the az login".to_owned()
            }
            Ok(Output { stderr, .. }) => format!("not converted: {}", first_line(&stderr)),
            Err(error) => format!(
                "not converted: {error:#}; a cluster with Entra ID sign-in will ask for a device code until it is"
            ),
        })
    }
}

command! {
    pub CLUSTER_CONNECT = ["aks", "cluster", "connect"], Write,
    "Fetch an AKS cluster's kubeconfig credentials and print its [[k8s.scope]]",
    keywords: ["kubeconfig", "credentials", "get-credentials", "kubelogin", "context", "setup", "login"],
    example: "aks cluster connect aks-contoso-dev --resource-group rg-contoso --fields context,config",
    run: cluster_connect,
}

// ---------- overview and doctor ----------

pub fn status(config: &Config) -> String {
    match config.section::<Azure>("azure") {
        Ok(_) => String::new(),
        Err(_) => "aks config broken".to_owned(),
    }
}

/// The login, the clusters in reach, and `kubelogin` on PATH, which a cluster
/// with Entra ID sign-in needs.
pub fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = crate::doctor_login(ctx, &mut checks, &[]) else {
        return checks;
    };
    checks.push(match inventory(ctx, &azure) {
        Ok(found) => Check::ok(
            "clusters",
            format!("{} AKS clusters in reach", found.clusters.len()),
        ),
        Err(error) => Check::failed(
            "clusters",
            format!("{error:#}"),
            "az account list; check [azure] subscriptions",
        ),
    });
    let mut version = program("kubelogin");
    version.arg("--version");
    checks.push(match ctx.read(version) {
        Ok(output) if output.status.success() => {
            Check::ok("kubelogin", first_line(&output.stdout).to_owned())
        }
        Ok(output) => Check::failed("kubelogin", first_line(&output.stderr).to_owned(), "reinstall kubelogin"),
        Err(error) => Check::failed(
            "kubelogin",
            format!("{error:#}"),
            "install kubelogin (https://azure.github.io/kubelogin/); a cluster with Entra ID sign-in needs it",
        ),
    });
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::assert_dry_run;
    use serde_json::json;

    use crate::AKS;
    use crate::fixtures::{self, azure};

    #[test]
    fn cluster_list_reads_the_inventory() {
        let (outcome, _) = azure(
            &[AKS],
            &["aks", "cluster", "list"],
            vec![fixtures::inventory(vec![
                fixtures::cluster("aks-contoso-dev"),
                fixtures::vault("kv-contoso"),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "name": "aks-contoso-dev", "resource_group": "rg-contoso",
                "subscription": "00000000-0000-0000-0000-000000000001", "location": "eastus",
                "kubernetes_version": "1.30.4", "power_state": "Running",
                "k8s_scope": "aks-contoso-dev",
            }])
        );
    }

    #[test]
    fn a_dry_run_connect_plans_both_commands_and_runs_neither() {
        let plans = assert_dry_run(
            &[AKS],
            &[
                "aks",
                "cluster",
                "connect",
                "aks-qa",
                "--resource-group",
                "rg-contoso",
            ],
            vec![],
        );
        assert_eq!(
            plans,
            [json!({"run": [
                ["az", "aks", "get-credentials", "--resource-group", "rg-contoso", "--name", "aks-qa", "--overwrite-existing"],
                ["kubelogin", "convert-kubeconfig", "-l", "azurecli"],
            ]})]
        );
        // Without a group it is looked up first, and that read still runs.
        let plans = assert_dry_run(
            &[AKS],
            &["aks", "cluster", "connect", "aks-contoso-dev"],
            vec![fixtures::inventory(vec![fixtures::cluster(
                "aks-contoso-dev",
            )])],
        );
        assert_eq!(plans[0]["run"][0][8], "--subscription");
    }

    #[test]
    fn connect_writes_credentials_converts_and_prints_a_scope_block() {
        let (outcome, _) = azure(
            &[AKS],
            &[
                "aks",
                "cluster",
                "connect",
                "aks-qa",
                "--resource-group",
                "rg-contoso",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let connected = outcome.json();
        assert_eq!(connected["context"], "aks-qa");
        assert_eq!(connected["kubelogin"], "converted to use the az login");
        assert_eq!(connected["namespaces"], json!(["dev", "qa", "uat"]));
        assert_eq!(
            connected["config"],
            "[[k8s.scope]]\nname = \"aks-qa\"\ncontext = \"aks-qa\"\nnamespaces = [\"dev\", \"qa\", \"uat\"]\n"
        );
    }

    #[test]
    fn a_name_in_two_places_is_ambiguous_and_one_nowhere_is_not_found() {
        let mut other = fixtures::cluster("aks-contoso-dev");
        other["resourceGroup"] = json!("rg-fabrikam");
        let (outcome, _) = azure(
            &[AKS],
            &["aks", "cluster", "connect", "aks-contoso-dev", "--dry-run"],
            vec![fixtures::inventory(vec![
                fixtures::cluster("aks-contoso-dev"),
                other,
            ])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("rg-contoso in"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("rg-fabrikam in"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[AKS],
            &["aks", "cluster", "connect", "aks-nope"],
            vec![fixtures::inventory(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }

    #[test]
    fn doctor_counts_the_clusters_and_finds_kubelogin() {
        let (ctx, _) = fixtures::doctor_ctx(
            vec![fixtures::inventory(vec![fixtures::cluster(
                "aks-contoso-dev",
            )])],
            "[azure]\n",
        );
        let checks = super::doctor(&ctx);
        assert_eq!(
            fixtures::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("clusters".to_owned(), true),
                ("kubelogin".to_owned(), true),
            ]
        );
        assert_eq!(checks[1].detail, "1 AKS clusters in reach");
        assert_eq!(checks[2].detail, "kubelogin version v0.1.4-fake");
    }
}
