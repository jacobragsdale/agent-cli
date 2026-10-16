//! `net appgw get`: an application gateway's frontends, listeners, routing
//! rules, backend pools and settings, probes and WAF mode, its parts named
//! rather than given as ids. `--health` adds `backendhealth`, a long-running
//! POST that only reads, polled under the deadline: which backend each probe
//! finds down, and the probe's own words why.

use agent_cli_core::{Ctx, Failure, Request, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{ARM, arm_only, bearer, settle, text};
use crate::config::Azure;
use crate::net::{API, APPGW, Place, leaf, list, read, strings};

#[derive(clap::Args)]
pub struct AppgwGetArgs {
    /// The application gateway: its ARM id, portal link, or name
    appgw: String,
    /// Also ask Azure how each backend answers its probe (takes up to a minute)
    #[arg(long)]
    health: bool,
    #[command(flatten)]
    place: Place,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AppgwRow {
    /// The ARM id, which `net appgw get` takes.
    id: String,
    name: String,
    group: Option<String>,
    /// Standard_v2 or WAF_v2.
    sku: Option<String>,
    /// Running or Stopped.
    status: Option<String>,
    /// Instances, or min-max when it autoscales.
    capacity: Option<String>,
    /// The gateway's own subnet's ARM id.
    subnet: Option<String>,
    waf: Option<Waf>,
    frontends: Vec<Frontend>,
    listeners: Vec<Listener>,
    /// By priority: the first match decides.
    rules: Vec<Rule>,
    pools: Vec<Pool>,
    settings: Vec<Settings>,
    probes: Vec<Probe>,
    /// With --health: each backend as each setting's probe finds it.
    health: Vec<Health>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Waf {
    /// Detection logs; Prevention blocks.
    mode: Option<String>,
    enabled: Option<bool>,
    /// The WAF policy's ARM id, when one is attached.
    policy: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Frontend {
    name: String,
    private_ip: Option<String>,
    /// The public IP's ARM id.
    public_ip: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Listener {
    name: String,
    frontend: Option<String>,
    port: Option<u64>,
    protocol: Option<String>,
    /// Empty: every host name.
    hosts: Vec<String>,
    certificate: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Rule {
    name: String,
    priority: Option<u64>,
    /// Basic or PathBasedRouting.
    kind: Option<String>,
    listener: Option<String>,
    pool: Option<String>,
    settings: Option<String>,
    path_map: Option<String>,
    redirect: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Pool {
    name: String,
    /// IPs or FQDNs.
    addresses: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Settings {
    name: String,
    port: Option<u64>,
    protocol: Option<String>,
    /// The Host header sent to the backend.
    host: Option<String>,
    /// Whether the Host header is the backend's own address.
    backend_host: Option<bool>,
    probe: Option<String>,
    /// Seconds before a backend's answer counts as failed.
    timeout: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Probe {
    name: String,
    protocol: Option<String>,
    host: Option<String>,
    path: Option<String>,
    interval: Option<u64>,
    timeout: Option<u64>,
    /// Failed probes before a backend is taken out.
    threshold: Option<u64>,
    /// The status codes that count as healthy.
    statuses: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Health {
    pool: Option<String>,
    settings: Option<String>,
    address: Option<String>,
    /// Healthy, Unhealthy, Partial, Draining or Unknown.
    health: Option<String>,
    /// The probe's own words: the status or the error it got.
    log: Option<String>,
}

/// The backends as the probes find them, polled until the deadline.
// VERIFY(work): that backendhealth takes an empty JSON body and answers 202
// with a Location that gives the result, as the REST reference's
// final-state-via: location says.
fn health(ctx: &Ctx, id: &str) -> Result<Vec<Health>> {
    let url = format!("https://management.azure.com{id}/backendhealth?api-version={API}");
    let mint = bearer(ctx, ARM);
    let answer = ctx.read(Request::query(arm_only(&url)?, json!({})).auth(&mint))?;
    let answer = settle(ctx, answer, &mint, |_| true)?.ok_or_else(|| {
        Failure::timed_out("Azure has not finished probing the gateway's backends in time")
            .hint("give it longer: agent-cli net appgw get ID --health --timeout 300")
    })?;
    let mut found = Vec::new();
    for pool in list(&answer["backendAddressPools"]) {
        for settings in list(&pool["backendHttpSettingsCollection"]) {
            for server in list(&settings["servers"]) {
                found.push(Health {
                    pool: leaf(&pool["backendAddressPool"]["id"]),
                    settings: leaf(&settings["backendHttpSettings"]["id"]),
                    address: text(&server["address"]),
                    health: text(&server["health"]),
                    log: text(&server["healthProbeLog"]),
                });
            }
        }
    }
    Ok(found)
}

/// The WAF set on the gateway itself, else the attached policy's.
fn waf(ctx: &Ctx, props: &Value) -> Result<Option<Waf>> {
    let inline = &props["webApplicationFirewallConfiguration"];
    let policy = text(&props["firewallPolicy"]["id"]);
    if inline.is_object() {
        return Ok(Some(Waf {
            mode: text(&inline["firewallMode"]),
            enabled: inline["enabled"].as_bool(),
            policy,
        }));
    }
    let Some(policy) = policy else {
        return Ok(None);
    };
    let held = read(ctx, &policy, API, "net appgw list")?;
    let settings = &held["properties"]["policySettings"];
    Ok(Some(Waf {
        mode: text(&settings["mode"]),
        enabled: text(&settings["state"]).map(|state| state.eq_ignore_ascii_case("Enabled")),
        policy: Some(policy),
    }))
}

fn appgw_get(ctx: &Ctx, args: AppgwGetArgs) -> Result<AppgwRow> {
    let azure = Azure::load(ctx)?;
    let target = args.place.find(
        ctx,
        &azure,
        &args.appgw,
        &[APPGW],
        "net appgw list",
        "net appgw get",
    )?;
    let gateway = read(ctx, &target.id, API, "net appgw list")?;
    let props = &gateway["properties"];
    // Each part's fields, by name.
    let parts = |key: &str| -> Vec<(String, &Value)> {
        list(&props[key])
            .iter()
            .map(|held| (text(&held["name"]).unwrap_or_default(), &held["properties"]))
            .collect()
    };
    let ports = parts("frontendPorts");
    let mut rules: Vec<Rule> = parts("requestRoutingRules")
        .into_iter()
        .map(|(name, fields)| Rule {
            name,
            priority: fields["priority"].as_u64(),
            kind: text(&fields["ruleType"]),
            listener: leaf(&fields["httpListener"]["id"]),
            pool: leaf(&fields["backendAddressPool"]["id"]),
            settings: leaf(&fields["backendHttpSettings"]["id"]),
            path_map: leaf(&fields["urlPathMap"]["id"]),
            redirect: leaf(&fields["redirectConfiguration"]["id"]),
        })
        .collect();
    rules.sort_by_key(|rule| rule.priority.unwrap_or(u64::MAX));
    let sku = &props["sku"];
    let scale = &props["autoscaleConfiguration"];
    let capacity = match (scale["minCapacity"].as_u64(), sku["capacity"].as_u64()) {
        (Some(min), _) => Some(match scale["maxCapacity"].as_u64() {
            Some(max) => format!("{min}-{max}"),
            None => format!("{min}-"),
        }),
        (None, Some(count)) => Some(count.to_string()),
        (None, None) => None,
    };
    Ok(AppgwRow {
        name: text(&gateway["name"]).unwrap_or_default(),
        group: target.group.clone(),
        sku: text(&sku["name"]),
        status: text(&props["operationalState"]),
        capacity,
        subnet: list(&props["gatewayIPConfigurations"])
            .first()
            .and_then(|held| text(&held["properties"]["subnet"]["id"])),
        waf: waf(ctx, props)?,
        frontends: parts("frontendIPConfigurations")
            .into_iter()
            .map(|(name, fields)| Frontend {
                name,
                private_ip: text(&fields["privateIPAddress"]),
                public_ip: text(&fields["publicIPAddress"]["id"]),
            })
            .collect(),
        listeners: parts("httpListeners")
            .into_iter()
            .map(|(name, fields)| {
                let port = leaf(&fields["frontendPort"]["id"]);
                let mut hosts = strings(&fields["hostNames"]);
                hosts.extend(text(&fields["hostName"]));
                Listener {
                    name,
                    frontend: leaf(&fields["frontendIPConfiguration"]["id"]),
                    port: ports
                        .iter()
                        .find(|(name, _)| Some(name) == port.as_ref())
                        .and_then(|(_, fields)| fields["port"].as_u64()),
                    protocol: text(&fields["protocol"]),
                    hosts,
                    certificate: leaf(&fields["sslCertificate"]["id"]),
                }
            })
            .collect(),
        rules,
        pools: parts("backendAddressPools")
            .into_iter()
            .map(|(name, fields)| Pool {
                name,
                addresses: list(&fields["backendAddresses"])
                    .iter()
                    .filter_map(|held| text(&held["fqdn"]).or_else(|| text(&held["ipAddress"])))
                    .collect(),
            })
            .collect(),
        settings: parts("backendHttpSettingsCollection")
            .into_iter()
            .map(|(name, fields)| Settings {
                name,
                port: fields["port"].as_u64(),
                protocol: text(&fields["protocol"]),
                host: text(&fields["hostName"]),
                backend_host: fields["pickHostNameFromBackendAddress"].as_bool(),
                probe: leaf(&fields["probe"]["id"]),
                timeout: fields["requestTimeout"].as_u64(),
            })
            .collect(),
        probes: parts("probes")
            .into_iter()
            .map(|(name, fields)| Probe {
                name,
                protocol: text(&fields["protocol"]),
                host: text(&fields["host"]),
                path: text(&fields["path"]),
                interval: fields["interval"].as_u64(),
                timeout: fields["timeout"].as_u64(),
                threshold: fields["unhealthyThreshold"].as_u64(),
                statuses: strings(&fields["match"]["statusCodes"]),
            })
            .collect(),
        health: if args.health {
            health(ctx, &target.id)?
        } else {
            Vec::new()
        },
        id: target.id,
    })
}

command! {
    pub APPGW_GET = ["net", "appgw", "get"], Read,
    "Show an application gateway's routing rules, backends, probes and WAF mode",
    keywords: ["waf", "backend", "health", "unhealthy", "502", "probe"],
    example: "net appgw get agw-contoso-prod --health --fields rules,health",
    timeout: 100,
    run: appgw_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure};

    #[test]
    fn parts_are_named_rules_go_by_priority_and_health_is_polled() {
        let id = format!("{RG}/providers/Microsoft.Network/applicationGateways/agw");
        let part = |kind: &str, name: &str| json!({"id": format!("{id}/{kind}/{name}")});
        let location =
            format!("https://management.azure.com{RG}/operations/2?api-version=2024-05-01");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "appgw", "get", &id, "--health"],
            vec![
                Answer::json(&json!({"id": id, "name": "agw", "properties": {
                    "sku": {"name": "WAF_v2", "tier": "WAF_v2"},
                    "autoscaleConfiguration": {"minCapacity": 1, "maxCapacity": 3},
                    "operationalState": "Running",
                    "webApplicationFirewallConfiguration": {"enabled": true, "firewallMode": "Prevention"},
                    "frontendIPConfigurations": [{"name": "public", "properties": {"publicIPAddress": {"id": "/ip/agw"}}}],
                    "frontendPorts": [{"name": "port-443", "properties": {"port": 443}}],
                    "httpListeners": [{"name": "https", "properties": {
                        "frontendIPConfiguration": part("frontendIPConfigurations", "public"),
                        "frontendPort": part("frontendPorts", "port-443"), "protocol": "Https",
                        "hostNames": ["www.contoso.example"], "sslCertificate": part("sslCertificates", "www")}}],
                    "requestRoutingRules": [
                        {"name": "late", "properties": {"priority": 200, "ruleType": "Basic",
                            "httpListener": part("httpListeners", "https"),
                            "redirectConfiguration": part("redirectConfigurations", "to-www")}},
                        {"name": "web", "properties": {"priority": 100, "ruleType": "Basic",
                            "httpListener": part("httpListeners", "https"),
                            "backendAddressPool": part("backendAddressPools", "web"),
                            "backendHttpSettings": part("backendHttpSettingsCollection", "web-https")}}],
                    "backendAddressPools": [{"name": "web", "properties": {"backendAddresses": [
                        {"fqdn": "web-1.contoso.example"}, {"ipAddress": "10.0.6.4"}]}}],
                    "backendHttpSettingsCollection": [{"name": "web-https", "properties": {"port": 443,
                        "protocol": "Https", "pickHostNameFromBackendAddress": true,
                        "probe": part("probes", "web"), "requestTimeout": 30}}],
                    "probes": [{"name": "web", "properties": {"protocol": "Https", "path": "/healthz",
                        "interval": 30, "timeout": 30, "unhealthyThreshold": 3,
                        "match": {"statusCodes": ["200-399"]}}}]}})),
                Answer::status(202, "")
                    .with_header("Location", &location)
                    .with_header("Retry-After", "1"),
                Answer::json(&json!({"backendAddressPools": [{
                    "backendAddressPool": part("backendAddressPools", "web"),
                    "backendHttpSettingsCollection": [{
                        "backendHttpSettings": part("backendHttpSettingsCollection", "web-https"),
                        "servers": [
                            {"address": "web-1.contoso.example", "health": "Healthy", "healthProbeLog": "Success. Received 200 status code"},
                            {"address": "10.0.6.4", "health": "Unhealthy", "healthProbeLog": "Received invalid status code: 502"}]}]}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["capacity"], "1-3");
        assert_eq!(got["waf"], json!({"mode": "Prevention", "enabled": true}));
        assert_eq!(
            got["listeners"],
            json!([{"name": "https", "frontend": "public", "port": 443, "protocol": "Https",
                "hosts": ["www.contoso.example"], "certificate": "www"}])
        );
        assert_eq!(got["rules"][0]["name"], "web");
        assert_eq!(got["rules"][0]["settings"], "web-https");
        assert_eq!(got["rules"][1]["redirect"], "to-www");
        assert_eq!(
            got["pools"],
            json!([{"name": "web", "addresses": ["web-1.contoso.example", "10.0.6.4"]}])
        );
        assert_eq!(
            got["health"][1],
            json!({"pool": "web", "settings": "web-https", "address": "10.0.6.4",
                "health": "Unhealthy", "log": "Received invalid status code: 502"})
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!("https://management.azure.com{id}/backendhealth?api-version=2024-05-01")
        );
        assert_eq!(sent[1].body, Some(json!({})), "a POST that only reads");
        assert_eq!(sent[2].url, location);
    }
}
