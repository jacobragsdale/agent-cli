//! What the contoso world answers of net. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::{agent_cli, ok};

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
