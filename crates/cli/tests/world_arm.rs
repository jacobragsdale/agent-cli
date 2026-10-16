//! What the contoso world answers of arm. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::ok;

const KV: &str = "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/rg-contoso-prod/providers/Microsoft.KeyVault/vaults/kv-contoso-prod";
/// What `k8s pvc get prod/web/data-orders-db-0` prints as `pv.disk`.
const DISK: &str = "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/MC_rg-contoso-prod_aks-contoso-prod_westeurope/providers/Microsoft.Compute/disks/pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a";
const VNET: &str = "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/rg-contoso-prod/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod";

#[test]
fn the_world_answers_what_a_trial_asks_of_arm() {
    ok(&["arm", "subscription", "list"]);
    ok(&["arm", "group", "list"]);
    let vnets = ok(&[
        "arm",
        "resource",
        "list",
        "--type",
        "Microsoft.Network/virtualNetworks",
    ]);
    assert_eq!(vnets[0]["id"], VNET, "{vnets}");
    let counts = ok(&[
        "arm",
        "query",
        "run",
        "resources | summarize count() by type",
    ]);
    assert_eq!(counts.as_array().map(Vec::len), Some(17), "{counts}");
    assert!(
        counts
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({"type": "microsoft.keyvault/vaults", "count_": 2})),
        "kv-contoso-prod and kv-contoso-billing: {counts}"
    );
}

#[test]
fn every_resource_list_id_is_what_resource_get_takes() {
    let listed = ok(&["arm", "resource", "list"]);
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 26, "{listed}");
    let aks = ids
        .iter()
        .find(|id| id.ends_with("/aks-contoso-prod"))
        .unwrap();
    let cluster = ok(&["arm", "resource", "get", aks]);
    assert_eq!(cluster["properties"]["currentKubernetesVersion"], "1.30.4");
    let by_name = ok(&["arm", "resource", "get", "aks-contoso-prod"]);
    assert_eq!(by_name["id"], cluster["id"]);
    let subnet = ok(&["arm", "resource", "get", &format!("{VNET}/subnets/aks")]);
    assert_eq!(subnet["properties"]["addressPrefix"], "10.1.0.0/22");
}

/// Trace 4's Azure half: the client id the serviceaccount `web/notifier`
/// names leads to an identity the cluster's issuer trusts, whose roles hold
/// nothing on the vault, because dana deleted that assignment on 2026-09-28.
#[test]
fn the_notifier_identity_has_lost_its_key_vault_role_and_the_log_says_who() {
    let identity = ok(&[
        "arm",
        "identity",
        "get",
        "00000000-0000-4000-8000-00000000b0f1",
    ]);
    assert_eq!(identity["name"], "id-contoso-notifier", "{identity}");
    assert_eq!(
        identity["federated_credentials"][0]["serviceaccount"],
        "aks-contoso-prod/web/notifier"
    );
    let principal = identity["principal_id"].as_str().unwrap();
    let roles = ok(&["arm", "role", "list", "--principal", principal]);
    assert_eq!(roles, identity["roles"]);
    assert!(
        roles
            .as_array()
            .unwrap()
            .iter()
            .all(|role| role["role"] != "Key Vault Secrets User"),
        "{roles}"
    );
    let at_vault = ok(&["arm", "role", "list", "--scope", "kv-contoso-prod"]);
    assert!(
        at_vault
            .as_array()
            .unwrap()
            .iter()
            .all(|role| role["principal_id"] != principal || role["inherited"] == true),
        "{at_vault}"
    );
    let log = ok(&["arm", "activity", "list", "--resource", "kv-contoso-prod"]);
    assert_eq!(
        log[0]["operation"],
        "Microsoft.Authorization/roleAssignments/delete"
    );
    assert_eq!(log[0]["caller"], "dana@contoso.example");
    assert_eq!(log[0]["when"], "2026-09-28T16:42:11Z");
    let changes = ok(&["arm", "change", "list", "--resource", KV]);
    assert_eq!(changes[0]["change_type"], "Delete");
    assert_eq!(changes[0]["resource"], log[0]["resource"]);
    assert_eq!(
        ok(&["arm", "identity", "list"]).as_array().map(Vec::len),
        Some(3)
    );
}

#[test]
fn the_orders_db_disk_answers_with_its_metrics_and_a_policy_and_a_denial_are_on_record() {
    let disk = ok(&["arm", "resource", "get", DISK]);
    assert!(
        disk["managedBy"]
            .as_str()
            .unwrap()
            .ends_with("aks-user-27182818-vmss/virtualMachines/1"),
        "{disk}"
    );
    let metrics = ok(&["arm", "metric", "list", DISK]);
    assert!(
        metrics
            .as_array()
            .unwrap()
            .iter()
            .any(|metric| metric["metric"] == "Composite Disk Write Operations/sec"),
        "{metrics}"
    );
    let writes = ok(&[
        "arm",
        "metric",
        "list",
        DISK,
        "--metric",
        "Composite Disk Write Operations/sec",
        "--interval",
        "5m",
    ]);
    assert_eq!(writes[0]["points"][0]["value"], 182.4, "{writes}");
    let vmss = DISK.replace(
        "disks/pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a",
        "virtualMachineScaleSets/aks-user-27182818-vmss",
    );
    let consumed = ok(&[
        "arm",
        "metric",
        "list",
        &vmss,
        "--metric",
        "Data Disk IOPS Consumed Percentage",
        "--interval",
        "5m",
    ]);
    assert_eq!(consumed[0]["unit"], "Percent", "{consumed}");

    let policy = ok(&["arm", "policy", "list", "--group", "rg-contoso-prod"]);
    assert_eq!(policy, ok(&["arm", "policy", "list"]));
    assert_eq!(policy[0]["resource"].as_str().unwrap(), KV.to_lowercase());
    let denied = ok(&["arm", "activity", "list", "--status", "failed"]);
    assert!(
        denied[0]["error"]
            .as_str()
            .unwrap()
            .starts_with("RequestDisallowedByPolicy"),
        "{denied}"
    );
    assert_eq!(
        ok(&["arm", "activity", "list", "--group", "rg-contoso-prod"]),
        ok(&["arm", "activity", "list"])
    );
    assert_eq!(
        ok(&["arm", "change", "list"]).as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        ok(&["arm", "role", "list"]).as_array().map(Vec::len),
        Some(4)
    );
}
