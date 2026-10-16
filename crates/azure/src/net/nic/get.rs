//! `net nic get`: a NIC's IPs, subnet and NSG, then what Azure actually
//! applies to it: `effectiveNetworkSecurityGroups` (the subnet's NSG and the
//! NIC's together) and `effectiveRouteTable` (system, user and BGP routes).
//! Both are long-running POSTs that only read, polled under the deadline.
//! When Azure gives no answer, the subnet's and the NIC's NSGs and the
//! subnet's route table stand in, and a note says so.

use agent_cli_core::{Ctx, Failure, Request, Response, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{ARM, arm_only, bearer, first_line, refused_with, settle, text};
use crate::config::Azure;
use crate::net::{API, NIC, Place, Rule, list, nsg_rules, read, rule, strings};

/// A scale set instance's NIC (an AKS node's), which lives under Compute.
const VMSS_NIC: &str =
    "Microsoft.Compute/virtualMachineScaleSets/virtualMachines/networkInterfaces";
// VERIFY(work): that a scale set instance's NIC answers at 2018-10-01 only,
// as Microsoft.Network's REST reference lists it and az asks for it.
const VMSS_NIC_API: &str = "2018-10-01";
/// What lists NIC ids, for a hint.
const LIST: &str = "arm resource list --type Microsoft.Network/networkInterfaces";

#[derive(clap::Args)]
pub struct NicGetArgs {
    /// The NIC: its ARM id (a scale set instance's too, as an AKS node's), portal link, or name
    nic: String,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NicRow {
    /// The ARM id, which `net nic get` takes.
    id: String,
    name: String,
    /// The VM's or scale set instance's ARM id (an AKS node's is what
    /// `k8s node get` prints as `vm`).
    vm: Option<String>,
    private_endpoint: Option<String>,
    /// The primary IP's subnet, which `net subnet get` takes.
    subnet: Option<String>,
    /// The NIC's own NSG; the subnet may have another.
    nsg: Option<String>,
    ips: Vec<Ip>,
    /// Inbound then outbound, in the order Azure applies them: inbound the
    /// subnet's NSG before the NIC's, outbound the other way round.
    effective_rules: Vec<Rule>,
    effective_routes: Vec<Route>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Ip {
    name: Option<String>,
    private_ip: Option<String>,
    /// The public IP's ARM id.
    public_ip: Option<String>,
    primary: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Route {
    name: Option<String>,
    /// Default (Azure's), User (a route table) or VirtualNetworkGateway (BGP).
    source: Option<String>,
    /// Active, or Invalid when a more specific route wins.
    state: Option<String>,
    prefixes: Vec<String>,
    next_hop: Option<String>,
    next_hop_ips: Vec<String>,
}

/// Starts one effective read; `Err(why)` when Azure refuses it.
// VERIFY(work): a 202 whose Location answers 200 with the result, as the
// REST reference's final-state-via: location says.
fn start(ctx: &Ctx, id: &str, action: &str, api: &str) -> Result<Result<Response, String>> {
    let url = format!("https://management.azure.com{id}/{action}?api-version={api}");
    let mint = bearer(ctx, ARM);
    match ctx.read(Request::query(arm_only(&url)?, json!({})).auth(&mint)) {
        Ok(answer) => Ok(Ok(answer)),
        // 403: a Reader may not run the action; 400, 404, 405: not for this NIC.
        Err(error) if matches!(refused_with(&error), Some(400 | 403 | 404 | 405 | 409)) => {
            // The status and Azure's words, without the URL the row has.
            let said = format!("{error:#}");
            let said = first_line(&said);
            Ok(Err(said
                .split_once("answered ")
                .map_or(said, |(_, words)| words)
                .to_owned()))
        }
        Err(error) => Err(error),
    }
}

/// The started read's result, polled until the deadline.
fn finish(ctx: &Ctx, answer: Response) -> Result<Value> {
    settle(ctx, answer, &bearer(ctx, ARM), |_| true)?.ok_or_else(|| {
        Failure::timed_out("Azure has not worked out the NIC's effective rules and routes in time")
            .hint("give it longer: agent-cli net nic get ID --timeout 100")
            .into()
    })
}

/// Sorts `(from_subnet, rule)` into the order Azure applies them.
fn ordered(mut rules: Vec<(bool, Rule)>) -> Vec<Rule> {
    rules.sort_by_key(|(from_subnet, rule)| {
        let inbound = rule.direction.eq_ignore_ascii_case("Inbound");
        (!inbound, *from_subnet != inbound, rule.priority)
    });
    rules.into_iter().map(|(_, rule)| rule).collect()
}

fn nic_get(ctx: &Ctx, args: NicGetArgs) -> Result<NicRow> {
    let azure = Azure::load(ctx)?;
    let target = args.place.find(
        ctx,
        &azure,
        &args.nic,
        &[NIC, VMSS_NIC],
        LIST,
        "net nic get",
    )?;
    let scale_set = target.kind().eq_ignore_ascii_case(VMSS_NIC);
    let api = if scale_set { VMSS_NIC_API } else { API };
    let nic = read(ctx, &target.id, api, LIST)?;
    let props = &nic["properties"];
    let configs = list(&props["ipConfigurations"]);
    let primary = configs
        .iter()
        .find(|held| held["properties"]["primary"].as_bool() == Some(true))
        .or(configs.first());
    let subnet = primary.and_then(|held| text(&held["properties"]["subnet"]["id"]));
    let nsg = text(&props["networkSecurityGroup"]["id"]);

    // Both first, so Azure works them out at once.
    let rules = start(ctx, &target.id, "effectiveNetworkSecurityGroups", api)?;
    let routes = start(ctx, &target.id, "effectiveRouteTable", api)?;
    // VERIFY(work): whether Azure answers either for a scale set instance's
    // NIC (an AKS node); if it refuses, what stands in is below.
    let written = match (&subnet, rules.is_err() || routes.is_err()) {
        (Some(subnet), true) => read(ctx, subnet, API, LIST)?,
        _ => Value::Null,
    };
    let effective_rules = match rules {
        Ok(answer) => {
            let answer = finish(ctx, answer)?;
            let mut found = Vec::new();
            for group in list(&answer["value"]) {
                let from_subnet = group["association"]["subnet"].is_object();
                let id = text(&group["networkSecurityGroup"]["id"]);
                for held in list(&group["effectiveSecurityRules"]) {
                    let name = text(&held["name"]).unwrap_or_default();
                    found.push((from_subnet, rule(&name, held, false, id.as_deref())));
                }
            }
            ordered(found)
        }
        Err(why) => {
            ctx.note(format!(
                "[Azure gave no effective rules for this NIC ({why}); these are its subnet's and its own NSG rules as written]"
            ));
            let mut found = Vec::new();
            let from_subnet = text(&written["properties"]["networkSecurityGroup"]["id"]);
            for (is_subnet, id) in [(true, from_subnet), (false, nsg.clone())] {
                if let Some(id) = id {
                    let held = read(ctx, &id, API, "net nsg list")?;
                    found.extend(
                        nsg_rules(&held, true)
                            .into_iter()
                            .map(|rule| (is_subnet, rule)),
                    );
                }
            }
            ordered(found)
        }
    };
    let effective_routes = match routes {
        Ok(answer) => list(&finish(ctx, answer)?["value"])
            .iter()
            .map(|held| Route {
                name: text(&held["name"]),
                source: text(&held["source"]),
                state: text(&held["state"]),
                prefixes: strings(&held["addressPrefix"]),
                next_hop: text(&held["nextHopType"]),
                next_hop_ips: strings(&held["nextHopIpAddress"]),
            })
            .collect(),
        Err(why) => {
            ctx.note(format!(
                "[Azure gave no effective routes for this NIC ({why}); these are its subnet's route table, without Azure's system routes]"
            ));
            match text(&written["properties"]["routeTable"]["id"]) {
                Some(table) => {
                    list(&read(ctx, &table, API, "net route list")?["properties"]["routes"])
                        .iter()
                        .map(|held| Route {
                            name: text(&held["name"]),
                            source: Some("User".to_owned()),
                            state: None,
                            prefixes: text(&held["properties"]["addressPrefix"])
                                .into_iter()
                                .collect(),
                            next_hop: text(&held["properties"]["nextHopType"]),
                            next_hop_ips: text(&held["properties"]["nextHopIpAddress"])
                                .into_iter()
                                .collect(),
                        })
                        .collect()
                }
                None => Vec::new(),
            }
        }
    };
    let vm = text(&props["virtualMachine"]["id"]).or_else(|| {
        let at = target
            .id
            .to_ascii_lowercase()
            .rfind("/networkinterfaces/")?;
        scale_set.then(|| target.id[..at].to_owned())
    });
    Ok(NicRow {
        name: text(&nic["name"]).unwrap_or_default(),
        vm,
        private_endpoint: text(&props["privateEndpoint"]["id"]),
        subnet,
        nsg,
        ips: configs
            .iter()
            .map(|held| Ip {
                name: text(&held["name"]),
                private_ip: text(&held["properties"]["privateIPAddress"]),
                public_ip: text(&held["properties"]["publicIPAddress"]["id"]),
                primary: held["properties"]["primary"].as_bool(),
            })
            .collect(),
        effective_rules,
        effective_routes,
        id: target.id,
    })
}

command! {
    pub NIC_GET = ["net", "nic", "get"], Read,
    "Show a network interface's IPs and the security rules and routes in effect",
    keywords: ["effective", "interface", "node", "blocked", "port", "reachable", "next", "hop", "vmss"],
    example: "net nic get nic-contoso-jump --fields ips,effective_rules",
    timeout: 100,
    run: nic_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure};

    const VNET: &str = "/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/rg-contoso/providers/Microsoft.Network/virtualNetworks/v";

    fn nic(id: &str, nsg: Option<&str>) -> Answer {
        Answer::json(&json!({"id": id, "name": "nic-0", "properties": {
            "networkSecurityGroup": nsg.map(|nsg| json!({"id": nsg})),
            "ipConfigurations": [{"name": "ipconfig1", "properties": {"primary": true,
                "privateIPAddress": "10.1.0.5", "subnet": {"id": format!("{VNET}/subnets/aks")}}}]}}))
    }

    fn effective_rule(name: &str, direction: &str, priority: u64) -> serde_json::Value {
        json!({"name": name, "direction": direction, "priority": priority, "access": "Allow",
            "protocol": "All", "sourceAddressPrefix": "*", "sourcePortRange": "0-65535",
            "destinationAddressPrefix": "*", "destinationPortRange": "0-65535"})
    }

    #[test]
    fn effective_rules_and_routes_are_polled_and_put_in_azures_order() {
        let id = format!("{RG}/providers/Microsoft.Network/networkInterfaces/nic-0");
        let location =
            format!("https://management.azure.com{RG}/operations/1?api-version=2024-05-01");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "nic", "get", &id],
            vec![
                nic(&id, Some("/nsg/nic")),
                Answer::status(202, "")
                    .with_header("Location", &location)
                    .with_header("Retry-After", "1"),
                Answer::json(&json!({"value": [{"source": "User", "state": "Active",
                    "addressPrefix": ["0.0.0.0/0"], "nextHopType": "VirtualAppliance",
                    "nextHopIpAddress": ["10.0.1.4"], "name": "to-firewall"}]})),
                Answer::json(&json!({"value": [
                    {"networkSecurityGroup": {"id": "/nsg/nic"},
                        "association": {"networkInterface": {"id": id}},
                        "effectiveSecurityRules": [
                            effective_rule("securityRules/nic-in", "Inbound", 100),
                            effective_rule("securityRules/nic-out", "Outbound", 100)]},
                    {"networkSecurityGroup": {"id": "/nsg/subnet"},
                        "association": {"subnet": {"id": format!("{VNET}/subnets/aks")}},
                        "effectiveSecurityRules": [
                            effective_rule("defaultSecurityRules/DenyAllInBound", "Inbound", 65500),
                            effective_rule("securityRules/subnet-in", "Inbound", 300),
                            effective_rule("securityRules/subnet-out", "Outbound", 100)]},
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        let names: Vec<&str> = got["effective_rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| rule["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "subnet-in",
                "DenyAllInBound",
                "nic-in",
                "nic-out",
                "subnet-out"
            ]
        );
        assert_eq!(got["effective_rules"][0]["nsg"], "/nsg/subnet");
        assert_eq!(
            got["effective_routes"],
            json!([{"name": "to-firewall", "source": "User", "state": "Active",
                "prefixes": ["0.0.0.0/0"], "next_hop": "VirtualAppliance", "next_hop_ips": ["10.0.1.4"]}])
        );
        assert_eq!(got["subnet"], format!("{VNET}/subnets/aks"));
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!(
                "https://management.azure.com{id}/effectiveNetworkSecurityGroups?api-version=2024-05-01"
            )
        );
        assert_eq!(sent[1].body, Some(json!({})), "a POST that only reads");
        assert!(sent[2].url.contains("/effectiveRouteTable?"));
        assert_eq!(sent[3].url, location, "the 202 is polled at its Location");
    }

    #[test]
    fn a_scale_set_nic_azure_will_not_answer_for_gets_its_nsgs_and_route_table_with_a_note() {
        let vmss = "/subscriptions/00000000-0000-0000-0000-000000000001/resourceGroups/MC_rg/providers/Microsoft.Compute/virtualMachineScaleSets/aks-system-1-vmss/virtualMachines/1";
        let id = format!("{vmss}/networkInterfaces/aks-system-1-vmss");
        let refused = || {
            Answer::status(
                404,
                r#"{"error":{"code":"NotFound","message":"Not supported for this NIC."}}"#,
            )
        };
        let nsg = |name: &str, priority: u64, direction: &str| {
            Answer::json(
                &json!({"id": format!("/nsg/{name}"), "properties": {"securityRules": [
                {"name": name, "properties": {"direction": direction, "priority": priority,
                    "access": "Deny", "protocol": "Tcp", "sourceAddressPrefix": "*",
                    "sourcePortRange": "*", "destinationAddressPrefix": "*", "destinationPortRange": "22"}}]}}),
            )
        };
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "nic", "get", &id],
            vec![
                nic(&id, Some("/nsg/own")),
                refused(),
                refused(),
                Answer::json(
                    &json!({"properties": {"networkSecurityGroup": {"id": "/nsg/aks"},
                    "routeTable": {"id": "/rt/aks"}}}),
                ),
                nsg("aks", 200, "Inbound"),
                nsg("own", 100, "Inbound"),
                Answer::json(
                    &json!({"properties": {"routes": [{"name": "to-firewall", "properties": {
                    "addressPrefix": "0.0.0.0/0", "nextHopType": "VirtualAppliance",
                    "nextHopIpAddress": "10.0.1.4"}}]}}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["vm"], vmss);
        assert_eq!(
            (
                got["effective_rules"][0]["name"].clone(),
                got["effective_rules"][1]["nsg"].clone()
            ),
            (json!("aks"), json!("/nsg/own")),
            "inbound, the subnet's NSG goes first whatever the priorities"
        );
        assert_eq!(
            got["effective_routes"][0]["next_hop_ips"],
            json!(["10.0.1.4"])
        );
        assert!(
            outcome.stderr.contains("Not supported for this NIC.")
                && outcome
                    .stderr
                    .contains("its subnet's and its own NSG rules")
                && outcome.stderr.contains("without Azure's system routes"),
            "{}",
            outcome.stderr
        );
        assert!(
            transport.sent()[0].url.ends_with("api-version=2018-10-01"),
            "a scale set NIC is read at Compute's version"
        );
    }
}
