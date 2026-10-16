//! What the contoso world answers of net. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::{agent_cli, follow, ok};

const RG: &str =
    "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/rg-contoso-prod";
const NODE_NIC: &str = "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/MC_rg-contoso-prod_aks-contoso-prod_westeurope/providers/Microsoft.Compute/virtualMachineScaleSets/aks-system-31415926-vmss/virtualMachines/1/networkInterfaces/aks-system-31415926-vmss";

fn vnet() -> String {
    format!("{RG}/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod")
}

#[test]
fn the_world_answers_what_a_trial_asks_of_net() {
    let vnets = ok(&["net", "vnet", "list"]);
    assert_eq!(vnets[0]["id"], vnet(), "{vnets}");
    assert_eq!(vnets[0]["peerings"][0]["state"], "Connected");

    let got = ok(&["net", "vnet", "get", "vnet-contoso-prod"]);
    assert_eq!(got["subnets"][0]["name"], "aks");
    assert_eq!(
        (
            got["subnets"][0]["used_ips"].clone(),
            got["subnets"][0]["free_ips"].clone()
        ),
        (6.into(), 1013.into())
    );
    assert_eq!(
        ok(&["net", "vnet", "get", vnets[0]["id"].as_str().unwrap()])["id"],
        vnet()
    );

    let routes = ok(&["net", "route", "list"]);
    assert_eq!(
        (
            routes[0]["prefix"].as_str(),
            routes[0]["next_hop_ip"].as_str()
        ),
        (Some("0.0.0.0/0"), Some("10.100.1.4")),
        "{routes}"
    );
    assert_eq!(routes[0]["subnets"][0], format!("{}/subnets/aks", vnet()));
}

#[test]
fn a_subnet_names_its_holders_by_ids_other_commands_take() {
    let aks = ok(&[
        "net",
        "subnet",
        "get",
        "vnet-contoso-prod/aks",
        "--group",
        "rg-contoso-prod",
    ]);
    assert_eq!(aks["id"], format!("{}/subnets/aks", vnet()));
    let pool = aks["holders"][0]["pool"].as_str().unwrap();
    assert_eq!(pool, "aks-contoso-prod/system");
    ok(&["aks", "nodepool", "get", pool]);
    let nsg = ok(&["net", "nsg", "get", aks["nsg"].as_str().unwrap()]);
    let custom: Vec<(&str, &str)> = nsg["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|rule| rule["default"] == false)
        .map(|rule| {
            (
                rule["name"].as_str().unwrap(),
                rule["access"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        custom,
        [
            ("allow-https-inbound", "Allow"),
            ("deny-ssh-inbound", "Deny")
        ]
    );

    let endpoints = ok(&[
        "net",
        "subnet",
        "get",
        &format!("{}/subnets/private-endpoints", vnet()),
    ]);
    assert_eq!(
        endpoints["holders"][0]["id"],
        format!("{RG}/providers/Microsoft.Network/privateEndpoints/pe-kv-contoso-prod")
    );
    let listed = ok(&["net", "nsg", "list"]);
    assert_eq!(listed[0]["id"], nsg["id"]);
    assert_eq!(
        ok(&["net", "nsg", "get", "nsg-contoso-aks"])["id"],
        nsg["id"]
    );
}

#[test]
fn an_aks_nodes_nic_falls_back_to_its_subnets_rules_and_routes_and_says_so() {
    let ran = agent_cli(&["net", "nic", "get", NODE_NIC]);
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let nic = ran.json();
    assert_eq!(nic["ips"][0]["private_ip"], "10.1.0.5");
    assert!(
        nic["effective_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["name"] == "deny-ssh-inbound"),
        "{nic}"
    );
    assert_eq!(nic["effective_routes"][0]["next_hop"], "VirtualAppliance");
    assert!(
        ran.stderr.contains("Azure gave no effective rules")
            && ran.stderr.contains("Azure gave no effective routes"),
        "{}",
        ran.stderr
    );
    assert!(
        nic["vm"]
            .as_str()
            .unwrap()
            .ends_with("aks-system-31415926-vmss/virtualMachines/1"),
        "the node's VM, as k8s prints it from providerID: {nic}"
    );
}

#[test]
fn the_vaults_endpoint_resolves_from_the_hub_only_and_its_note_shows_the_link() {
    let walked = follow(&["net", "endpoint", "get", "pe-kv-contoso-prod"]);
    let endpoint = walked[0].json();
    assert_eq!(endpoint["ips"][0], "10.1.4.4", "{endpoint}");
    assert_eq!(endpoint["fqdns"][0], "kv-contoso-prod.vault.azure.net");
    assert_eq!(
        (
            endpoint["dns"][0]["resolves"].clone(),
            endpoint["dns"][0]["linked"].clone()
        ),
        (true.into(), false.into()),
        "the record is right, the link is missing: {endpoint}"
    );
    assert_eq!(walked.len(), 2, "then net zone list: {}", walked[0].stderr);
    let zones = walked[1].json();
    assert_eq!(zones[0]["name"], "privatelink.vaultcore.azure.net");
    assert!(
        zones[0]["links"][0]["vnet"]
            .as_str()
            .unwrap()
            .ends_with("/vnet-contoso-hub"),
        "{zones}"
    );
    assert_eq!(
        ok(&["net", "zone", "list", "--vnet", "vnet-contoso-prod"]),
        serde_json::json!([]),
        "no private zone is linked to the AKS vnet"
    );
    let records = ok(&[
        "net",
        "record",
        "list",
        zones[0]["id"].as_str().unwrap(),
        "kv",
    ]);
    assert_eq!(records[0]["values"][0], "10.1.4.4", "{records}");
    let listed = ok(&["net", "endpoint", "list"]);
    assert_eq!(listed[0]["id"], endpoint["id"]);
}

#[test]
fn the_ingress_ip_names_its_k8s_service_and_the_gateway_shows_a_backend_down() {
    let ips = ok(&["net", "ip", "list"]);
    let ingress = ips
        .as_array()
        .unwrap()
        .iter()
        .find(|ip| ip["address"] == "203.0.113.10")
        .unwrap_or_else(|| panic!("{ips}"));
    assert_eq!(
        ingress["service"],
        "aks-contoso-prod/app-routing-system/nginx"
    );
    let lbs = ok(&["net", "lb", "list"]);
    let lb = ok(&["net", "lb", "get", lbs[0]["id"].as_str().unwrap()]);
    assert_eq!(lb["frontends"][1]["service"], ingress["service"], "{lb}");
    assert_eq!(lb["pools"][0]["members"], 6);
    assert_eq!(ok(&["net", "lb", "get", "kubernetes"])["id"], lb["id"]);

    let gateways = ok(&["net", "appgw", "list"]);
    assert_eq!(gateways[0]["waf_mode"], "Prevention");
    let gateway = ok(&["net", "appgw", "get", "agw-contoso-prod", "--health"]);
    let down: Vec<&str> = gateway["health"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|server| server["health"] != "Healthy")
        .map(|server| server["address"].as_str().unwrap())
        .collect();
    assert_eq!(down, ["web-2.contoso.example"], "{gateway}");
}
