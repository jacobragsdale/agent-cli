//! What the contoso world answers of arm. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::ok;

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
    assert_eq!(counts.as_array().map(Vec::len), Some(5), "{counts}");
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
    assert_eq!(ids.len(), 6, "{listed}");
    let cluster = ok(&["arm", "resource", "get", ids[0]]);
    assert_eq!(cluster["properties"]["currentKubernetesVersion"], "1.30.4");
    let by_name = ok(&["arm", "resource", "get", "aks-contoso-prod"]);
    assert_eq!(by_name["id"], cluster["id"]);
    let subnet = ok(&["arm", "resource", "get", &format!("{VNET}/subnets/aks")]);
    assert_eq!(subnet["properties"]["addressPrefix"], "10.1.0.0/22");
}
