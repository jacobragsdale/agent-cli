//! What the contoso world answers of aks. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::{agent_cli, follow, ok};

const SUBNET: &str = "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/rg-contoso-prod/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod/subnets/aks";

#[test]
fn the_world_answers_what_a_trial_asks_of_aks() {
    ok(&["aks", "cluster", "list"]);
    let cluster = ok(&["aks", "cluster", "get", "aks-contoso-prod"]);
    assert_eq!(cluster["network"]["mode"], "overlay", "{cluster}");
    assert_eq!(cluster["workload_identity"], true);
    assert_eq!(
        cluster["maintenance"][0]["schedule"]["weekly"]["dayOfWeek"],
        "Sunday"
    );
    let by_id = ok(&[
        "aks",
        "cluster",
        "get",
        cluster["resource"].as_str().unwrap(),
    ]);
    assert_eq!(by_id["name"], "aks-contoso-prod");

    let pools = ok(&["aks", "nodepool", "list", "aks-contoso-prod"]);
    let ids: Vec<&str> = pools
        .as_array()
        .unwrap()
        .iter()
        .map(|pool| pool["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["aks-contoso-prod/system", "aks-contoso-prod/user"]);
    assert!(
        pools
            .as_array()
            .unwrap()
            .iter()
            .all(|pool| pool["subnet"] == SUBNET)
    );

    let upgrades = ok(&["aks", "upgrade", "list", "aks-contoso-prod"]);
    assert_eq!(upgrades[0]["target"], "control-plane", "{upgrades}");
    assert_eq!(
        upgrades[2]["newest_node_image"],
        "AKSUbuntu-2204gen2containerd-202510.03.0"
    );

    let ran = ok(&[
        "aks",
        "command",
        "run",
        "aks-contoso-prod",
        "--yes",
        "--",
        "kubectl",
        "get",
        "nodes",
    ]);
    assert_eq!(ran["exit_code"], 0, "{ran}");
}

#[test]
fn a_pool_at_its_autoscaler_maximum_leads_to_the_cluster_events_and_refuses_a_count() {
    let walked = follow(&["aks", "nodepool", "get", "aks-contoso-prod/user"]);
    let pool = walked[0].json();
    assert_eq!(pool["count"], 3, "{pool}");
    assert_eq!(pool["autoscale"]["max"], 3);
    assert_eq!(walked.len(), 2, "then k8s event list: {}", walked[0].stderr);
    assert_eq!(walked[1].code, 0, "{}", walked[1].stderr);

    let refused = agent_cli(&[
        "aks",
        "nodepool",
        "scale",
        "aks-contoso-prod/user",
        "--nodes",
        "5",
        "--yes",
    ]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert!(refused.stderr.contains("--max-count"), "{}", refused.stderr);
}
