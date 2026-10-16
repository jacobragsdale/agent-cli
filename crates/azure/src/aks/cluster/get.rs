//! `aks cluster get`: ARM's managedClusters GET, read into what a cluster
//! question turns on (network, API server reach, identity, upgrades), with
//! its maintenance windows from `maintenanceConfigurations`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aks::{Place, locate};
use crate::client::text;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct ClusterGetArgs {
    /// The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
    cluster: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClusterDetail {
    /// What the aks commands take.
    name: String,
    resource_group: String,
    subscription: String,
    location: Option<String>,
    /// Its ARM id, which `arm resource get` takes.
    resource: String,
    kubernetes_version: Option<String>,
    /// Free, Standard or Premium.
    tier: Option<String>,
    /// Running or Stopped.
    power_state: Option<String>,
    /// Succeeded, or what ARM is doing or failed at.
    provisioning_state: Option<String>,
    network: Network,
    api_server: ApiServer,
    /// The issuer URL a federated credential trusts; absent when off.
    oidc_issuer: Option<String>,
    workload_identity: bool,
    /// The kubelet's managed identity (ARM id), which pulls images and
    /// reads disks.
    identity: Option<String>,
    /// The add-ons switched on.
    addons: Vec<String>,
    /// none, patch, stable, rapid or node-image.
    auto_upgrade: Option<String>,
    node_os_upgrade: Option<String>,
    maintenance: Vec<Window>,
    /// Where AKS keeps the nodes' VM scale sets, disks and load balancer.
    node_resource_group: Option<String>,
    /// The k8s scope `aks cluster connect` writes for it.
    k8s_scope: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Network {
    /// azure, kubenet or none.
    plugin: Option<String>,
    /// overlay, when Azure CNI hands pods addresses outside the subnet.
    mode: Option<String>,
    /// azure, calico or cilium.
    policy: Option<String>,
    dataplane: Option<String>,
    pod_cidr: Option<String>,
    service_cidr: Option<String>,
    dns_service_ip: Option<String>,
    /// loadBalancer, userDefinedRouting, managedNATGateway …
    outbound_type: Option<String>,
    /// The public IPs (ARM ids) its egress leaves from.
    outbound_ips: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiServer {
    private: bool,
    /// The private FQDN for a private cluster, else the public one.
    fqdn: Option<String>,
    /// Who may reach a public API server; empty is everyone.
    authorized_ranges: Vec<String>,
}

/// One planned-maintenance configuration, as ARM keeps it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Window {
    /// default, aksManagedAutoUpgradeSchedule or aksManagedNodeOSUpgradeSchedule.
    name: String,
    /// One of weekly, absoluteMonthly, relativeMonthly or daily.
    schedule: Option<Value>,
    start_time: Option<String>,
    utc_offset: Option<String>,
    duration_hours: Option<u64>,
    /// The legacy `default` configuration's allowed hours.
    time_in_week: Option<Value>,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(text)
        .collect()
}

fn cluster_get(ctx: &Ctx, args: ClusterGetArgs) -> Result<ClusterDetail> {
    let azure = Azure::load(ctx)?;
    let (located, _) = locate(ctx, &azure, &args.cluster, &args.place)?;
    let cluster = located.get(ctx, "")?;
    let windows = located.get(ctx, "/maintenanceConfigurations")?;
    let props = &cluster["properties"];
    let net = &props["networkProfile"];
    let access = &props["apiServerAccessProfile"];
    let private = access["enablePrivateCluster"].as_bool().unwrap_or(false);
    let fqdn_key = if private { "privateFQDN" } else { "fqdn" };
    let detail = ClusterDetail {
        location: text(&cluster["location"]),
        resource: located.id(),
        kubernetes_version: text(&props["currentKubernetesVersion"])
            .or_else(|| text(&props["kubernetesVersion"])),
        tier: text(&cluster["sku"]["tier"]),
        power_state: text(&props["powerState"]["code"]),
        provisioning_state: text(&props["provisioningState"]),
        network: Network {
            plugin: text(&net["networkPlugin"]),
            mode: text(&net["networkPluginMode"]),
            policy: text(&net["networkPolicy"]),
            dataplane: text(&net["networkDataplane"]),
            pod_cidr: text(&net["podCidr"]),
            service_cidr: text(&net["serviceCidr"]),
            dns_service_ip: text(&net["dnsServiceIP"]),
            outbound_type: text(&net["outboundType"]),
            outbound_ips: net["loadBalancerProfile"]["effectiveOutboundIPs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|ip| text(&ip["id"]))
                .collect(),
        },
        api_server: ApiServer {
            private,
            fqdn: text(&props[fqdn_key]),
            authorized_ranges: strings(&access["authorizedIPRanges"]),
        },
        oidc_issuer: text(&props["oidcIssuerProfile"]["issuerURL"]),
        workload_identity: props["securityProfile"]["workloadIdentity"]["enabled"]
            .as_bool()
            .unwrap_or(false),
        identity: text(&props["identityProfile"]["kubeletidentity"]["resourceId"]),
        addons: props["addonProfiles"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, addon)| addon["enabled"].as_bool() == Some(true))
            .map(|(name, _)| name.clone())
            .collect(),
        auto_upgrade: text(&props["autoUpgradeProfile"]["upgradeChannel"]),
        node_os_upgrade: text(&props["autoUpgradeProfile"]["nodeOSUpgradeChannel"]),
        // ponytail: the first page of configurations; a cluster has at most three.
        maintenance: windows["value"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|held| {
                let window = &held["properties"]["maintenanceWindow"];
                Window {
                    name: text(&held["name"]).unwrap_or_default(),
                    schedule: Some(window["schedule"].clone()).filter(|held| !held.is_null()),
                    start_time: text(&window["startTime"]),
                    utc_offset: text(&window["utcOffset"]),
                    duration_hours: window["durationHours"].as_u64(),
                    time_in_week: Some(held["properties"]["timeInWeek"].clone())
                        .filter(|held| !held.is_null()),
                }
            })
            .collect(),
        node_resource_group: text(&props["nodeResourceGroup"]),
        k8s_scope: located.name.clone(),
        name: located.name,
        resource_group: located.resource_group,
        subscription: located.subscription,
    };
    next(ctx, &detail);
    Ok(detail)
}

/// A cluster that failed to provision says where to look next; a stopped
/// one says nothing in it will answer.
fn next(ctx: &Ctx, detail: &ClusterDetail) {
    let state = detail.provisioning_state.as_deref().unwrap_or("Succeeded");
    if state.eq_ignore_ascii_case("Failed") {
        ctx.note(format!(
            "[next: agent-cli aks nodepool list {}  (the cluster's last operation failed; each pool's state)]",
            detail.name
        ));
    } else if detail.power_state.as_deref() == Some("Stopped") {
        ctx.note(format!(
            "[stopped: no node or API server answers until `az aks start --name {} --resource-group {}`]",
            detail.name, detail.resource_group
        ));
    }
}

command! {
    pub CLUSTER_GET = ["aks", "cluster", "get"], Read,
    "Show an AKS cluster: network, private API server, identity, addons, auto-upgrade",
    keywords: ["cni", "oidc", "issuer", "workload", "kubelet", "maintenance", "window", "outbound", "fqdn", "plugin"],
    example: "aks cluster get aks-contoso-prod --fields network,api_server,identity",
    run: cluster_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AKS;
    use crate::testing::{self, azure};

    use crate::testing::AKS_ID as ID;

    fn cluster(state: &str) -> Answer {
        Answer::json(&json!({
            "id": ID, "name": "aks-contoso-dev", "location": "eastus",
            "sku": {"name": "Base", "tier": "Standard"},
            "properties": {
                "provisioningState": state, "powerState": {"code": "Running"},
                "kubernetesVersion": "1.30", "currentKubernetesVersion": "1.30.4",
                "fqdn": "aks-contoso-dev-1.hcp.eastus.azmk8s.io",
                "privateFQDN": "aks-contoso-dev-1.privatelink.eastus.azmk8s.io",
                "apiServerAccessProfile": {"enablePrivateCluster": true},
                "networkProfile": {"networkPlugin": "azure", "networkPluginMode": "overlay",
                    "networkPolicy": "cilium", "networkDataplane": "cilium",
                    "podCidr": "10.244.0.0/16", "serviceCidr": "10.0.0.0/16", "dnsServiceIP": "10.0.0.10",
                    "outboundType": "loadBalancer",
                    "loadBalancerProfile": {"effectiveOutboundIPs": [{"id": "/subscriptions/s/resourceGroups/mc/providers/Microsoft.Network/publicIPAddresses/out-1"}]}},
                "oidcIssuerProfile": {"enabled": true, "issuerURL": "https://eastus.oic.prod-aks.azure.com/t/c/"},
                "securityProfile": {"workloadIdentity": {"enabled": true}},
                "identityProfile": {"kubeletidentity": {"resourceId": "/subscriptions/s/resourceGroups/mc/providers/Microsoft.ManagedIdentity/userAssignedIdentities/aks-contoso-dev-agentpool", "clientId": "c"}},
                "addonProfiles": {"azureKeyvaultSecretsProvider": {"enabled": true}, "azurepolicy": {"enabled": false}},
                "autoUpgradeProfile": {"upgradeChannel": "patch", "nodeOSUpgradeChannel": "NodeImage"},
                "nodeResourceGroup": "MC_rg-contoso_aks-contoso-dev_eastus",
            },
        }))
    }

    fn windows() -> Answer {
        Answer::json(
            &json!({"value": [{"name": "aksManagedAutoUpgradeSchedule", "properties": {"maintenanceWindow": {
            "schedule": {"weekly": {"dayOfWeek": "Sunday", "intervalWeeks": 1}},
            "durationHours": 4, "startTime": "02:00", "utcOffset": "+00:00"}}}]}),
        )
    }

    #[test]
    fn get_reads_the_cluster_and_its_maintenance_windows() {
        let (outcome, transport) = azure(
            &[AKS],
            &["aks", "cluster", "get", "aks-contoso-dev"],
            vec![
                testing::inventory(vec![testing::cluster("aks-contoso-dev")]),
                cluster("Succeeded"),
                windows(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["resource"], ID);
        assert_eq!(got["tier"], "Standard");
        assert_eq!(
            got["api_server"],
            json!({"private": true, "fqdn": "aks-contoso-dev-1.privatelink.eastus.azmk8s.io"})
        );
        assert_eq!(got["network"]["policy"], "cilium");
        assert_eq!(
            got["network"]["outbound_ips"][0]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next(),
            Some("out-1")
        );
        assert_eq!(
            got["identity"].as_str().unwrap().rsplit('/').next(),
            Some("aks-contoso-dev-agentpool")
        );
        assert_eq!(got["addons"], json!(["azureKeyvaultSecretsProvider"]));
        assert_eq!(
            got["maintenance"][0]["schedule"]["weekly"]["dayOfWeek"],
            "Sunday"
        );
        assert_eq!(got["k8s_scope"], "aks-contoso-dev");
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!("https://management.azure.com{ID}?api-version=2026-07-01")
        );
        assert_eq!(
            sent[2].url,
            format!(
                "https://management.azure.com{ID}/maintenanceConfigurations?api-version=2026-07-01"
            )
        );
    }

    #[test]
    fn an_id_or_portal_link_skips_the_inventory_and_a_failed_cluster_names_its_pools() {
        let link =
            format!("https://portal.azure.com/#@contoso.onmicrosoft.com/resource{ID}/overview");
        for raw in [ID.to_owned(), link] {
            let (outcome, transport) = azure(
                &[AKS],
                &["aks", "cluster", "get", &raw],
                vec![cluster("Failed"), windows()],
            );
            assert_eq!(outcome.code, 0, "{outcome:?}");
            assert_eq!(transport.sent().len(), 2);
            assert!(
                outcome
                    .stderr
                    .contains("[next: agent-cli aks nodepool list aks-contoso-dev"),
                "{}",
                outcome.stderr
            );
        }
        let (outcome, transport) = azure(
            &[AKS],
            &["aks", "cluster", "get", ID, "--resource-group", "rg-other"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
