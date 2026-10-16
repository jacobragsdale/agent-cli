//! `net`: virtual networks, subnets, NSGs, NICs, route tables, private
//! endpoints, DNS zones, public IPs, load balancers and application
//! gateways, read the way `arm` reads anything (Resource Graph for lists, ARM
//! GET for one), and joined where the raw object stops short: a subnet with
//! its used and free IPs and who holds them, an NSG's rules in the order
//! Azure applies them, a NIC with its effective rules and routes, a private
//! endpoint with whether its name resolves to it, a public IP or a load
//! balancer frontend with the k8s service that owns it.
//!
//! Every ref is what `arm resource get` takes: an ARM id or URL, a portal
//! link (parsed, never fetched), or a name with `--group` and
//! `--subscription`, where two holders is exit 2 listing their ids
//! (`arm/resource/mod.rs`'s `resolve`). A subnet is its ARM id, or
//! `VNET/SUBNET`; a NIC may be a VMSS instance's (an AKS node's), whose id is
//! under `Microsoft.Compute`; a zone is a public or a private DNS zone. Reads
//! are at one api-version per provider (`API`, `DNS_API`, `PRIVATE_DNS_API`),
//! so a field never moves under a command; ids in rows are ARM ids, which
//! every get here and `arm resource get` take. A k8s service is named as k8s
//! prints its id, `cluster/namespace/name`, from the tag AKS puts on the
//! public IPs it makes (`ip/mod.rs`).

pub(crate) mod appgw;
pub(crate) mod endpoint;
pub(crate) mod ip;
pub(crate) mod lb;
pub(crate) mod nic;
pub(crate) mod nsg;
pub(crate) mod record;
pub(crate) mod route;
pub(crate) mod subnet;
pub(crate) mod vnet;
pub(crate) mod zone;

use agent_cli_core::{Ctx, Failure, Request};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::arm::resource::{Lookup, Target, resolve};
use crate::arm::{of_type, quote, scope};
use crate::client::{ARM, arm_only, bearer, refused_with, text};
use crate::config::Azure;
use crate::graph::rows;

/// A stable Microsoft.Network api-version every type here answers at.
pub(crate) const API: &str = "2024-05-01";

pub(crate) const VNET: &str = "Microsoft.Network/virtualNetworks";
pub(crate) const SUBNET: &str = "Microsoft.Network/virtualNetworks/subnets";
pub(crate) const NSG: &str = "Microsoft.Network/networkSecurityGroups";
pub(crate) const NIC: &str = "Microsoft.Network/networkInterfaces";
pub(crate) const ENDPOINT: &str = "Microsoft.Network/privateEndpoints";
pub(crate) const ZONE: &str = "Microsoft.Network/dnsZones";
pub(crate) const PRIVATE_ZONE: &str = "Microsoft.Network/privateDnsZones";
pub(crate) const PUBLIC_IP: &str = "Microsoft.Network/publicIPAddresses";
pub(crate) const LB: &str = "Microsoft.Network/loadBalancers";
pub(crate) const APPGW: &str = "Microsoft.Network/applicationGateways";

/// Public DNS's newest stable api-version.
pub(crate) const DNS_API: &str = "2018-05-01";
/// Private DNS's long-standing stable api-version.
pub(crate) const PRIVATE_DNS_API: &str = "2020-06-01";

/// Where a named resource is, or what a list keeps to.
#[derive(clap::Args, Default)]
pub struct Place {
    /// Its resource group: where a name is looked for, or the one a list keeps to
    #[arg(long)]
    group: Option<String>,
    /// Its subscription, GUID or name (within [azure] subscriptions)
    #[arg(long)]
    subscription: Option<String>,
}

impl Place {
    /// The resource `raw` names, of one of `kinds`; `list` is the command
    /// whose ids it takes, `get` the one reading it.
    pub fn find(
        &self,
        ctx: &Ctx,
        azure: &Azure,
        raw: &str,
        kinds: &[&str],
        list: &str,
        get: &str,
    ) -> Result<Target> {
        let lookup = Lookup {
            kinds,
            group: self.group.as_deref(),
            subscription: self.subscription.as_deref(),
            list,
            get,
        };
        resolve(ctx, azure, raw, &lookup)
    }

    /// Resource Graph's rows of `kind` within the place, `project` (KQL
    /// after `| project`) then sorted by name and id.
    pub fn list(
        &self,
        ctx: &Ctx,
        azure: &Azure,
        kind: &str,
        project: &str,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let tail = format!("\n| project {project}\n| order by name asc, id asc");
        self.rows(ctx, azure, &[kind], &tail, limit)
    }

    /// Resource Graph's rows of `kinds` within the place, then `tail` (KQL
    /// from a newline and a pipe on).
    pub fn rows(
        &self,
        ctx: &Ctx,
        azure: &Azure,
        kinds: &[&str],
        tail: &str,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let mut kql = format!("resources\n| where {}", of_type(kinds));
        if let Some(group) = &self.group {
            kql.push_str(&format!("\n| where resourceGroup =~ {}", quote(group)));
        }
        kql.push_str(tail);
        let scope = scope(ctx, azure, self.subscription.as_deref())?;
        rows(ctx, &scope, &kql, limit)
    }
}

/// ARM's GET of `path` at `api`, or `None` for a 404: what may well not be
/// there, such as a DNS record.
pub(crate) fn found(ctx: &Ctx, path: &str, api: &str) -> Result<Option<Value>> {
    let url = format!("https://management.azure.com{path}?api-version={api}");
    match ctx.read(Request::get(arm_only(&url)?).auth(&bearer(ctx, ARM))) {
        Ok(answer) => answer.json().map(Some),
        Err(error) if refused_with(&error) == Some(404) => Ok(None),
        Err(error) => Err(error),
    }
}

/// The name at the end of the id at `value`: a sub-resource's, as an
/// application gateway's rule names its listener.
pub(crate) fn leaf(value: &Value) -> Option<String> {
    text(value).and_then(|id| id.rsplit('/').next().map(str::to_owned))
}

/// ARM's GET of `path` at `api`; a 404 is exit 4 naming `list`.
pub(crate) fn read(ctx: &Ctx, path: &str, api: &str, list: &str) -> Result<Value> {
    let url = format!("https://management.azure.com{path}?api-version={api}");
    ctx.read(Request::get(arm_only(&url)?).auth(&bearer(ctx, ARM)))
        .map_err(|error| {
            if refused_with(&error) == Some(404) {
                return Failure::not_found(format!("{error:#}"))
                    .hint(format!("the ones in reach: agent-cli {list}"))
                    .into();
            }
            error
        })?
        .json()
}

/// A subnet's used and free IPs, from its vnet's `usages`, by subnet id in
/// lower case.
// VERIFY(work): that `limit` already leaves out Azure's five reserved
// addresses (251 for a /24), so limit - used is what can still be handed out.
// ponytail: the first page of usages, one entry per subnet; follow nextLink
// when a vnet outgrows a page.
pub(crate) fn usages(ctx: &Ctx, vnet: &str) -> Result<Vec<(String, u64, u64)>> {
    let answer = read(ctx, &format!("{vnet}/usages"), API, "net vnet list")?;
    Ok(list(&answer["value"])
        .iter()
        .filter_map(|usage| {
            let used = usage["currentValue"].as_u64()?;
            let limit = usage["limit"].as_u64()?;
            Some((
                text(&usage["id"])?.to_ascii_lowercase(),
                used,
                limit.saturating_sub(used),
            ))
        })
        .collect())
}

/// The array at `value`, or none.
pub(crate) fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// The `id` of each object in the array at `value`.
pub(crate) fn ids(value: &Value) -> Vec<String> {
    list(value)
        .iter()
        .filter_map(|held| text(&held["id"]))
        .collect()
}

/// The strings in the array at `value`.
pub(crate) fn strings(value: &Value) -> Vec<String> {
    list(value).iter().filter_map(text).collect()
}

/// One security rule, as written or as Azure applies it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Rule {
    /// Inbound or Outbound.
    pub direction: String,
    /// Lower goes first; the first match decides.
    pub priority: u64,
    pub name: String,
    /// Allow or Deny.
    pub access: String,
    pub protocol: String,
    /// Prefixes, service tags or application security group ids.
    pub source: Vec<String>,
    pub source_ports: Vec<String>,
    pub destination: Vec<String>,
    pub destination_ports: Vec<String>,
    /// One of Azure's own rules, which every NSG has at 65000 and up.
    pub default: bool,
    /// The NSG it comes from, when a NIC's subnet and the NIC both have one.
    pub nsg: Option<String>,
}

/// A rule from its fields: an NSG rule's `properties`, or an effective rule,
/// whose name is `securityRules/NAME` or `defaultSecurityRules/NAME`.
pub(crate) fn rule(name: &str, fields: &Value, default: bool, nsg: Option<&str>) -> Rule {
    // One field, its plural, or (in an NSG) application security groups.
    let either = |one: &str, many: &str, groups: &str| {
        let mut found = strings(&fields[many]);
        if found.is_empty() {
            found.extend(text(&fields[one]));
        }
        found.extend(ids(&fields[groups]));
        found
    };
    let (default, name) = match name.split_once('/') {
        Some((kind, name)) => (kind.eq_ignore_ascii_case("defaultSecurityRules"), name),
        None => (default, name),
    };
    Rule {
        direction: text(&fields["direction"]).unwrap_or_default(),
        priority: fields["priority"].as_u64().unwrap_or_default(),
        name: name.to_owned(),
        access: text(&fields["access"]).unwrap_or_default(),
        protocol: text(&fields["protocol"]).unwrap_or_default(),
        source: either(
            "sourceAddressPrefix",
            "sourceAddressPrefixes",
            "sourceApplicationSecurityGroups",
        ),
        source_ports: either("sourcePortRange", "sourcePortRanges", ""),
        destination: either(
            "destinationAddressPrefix",
            "destinationAddressPrefixes",
            "destinationApplicationSecurityGroups",
        ),
        destination_ports: either("destinationPortRange", "destinationPortRanges", ""),
        default,
        nsg: nsg.map(str::to_owned),
    }
}

/// An NSG's custom and default rules, as written.
pub(crate) fn nsg_rules(nsg: &Value, tagged: bool) -> Vec<Rule> {
    let id = text(&nsg["id"]);
    let id = id.as_deref().filter(|_| tagged);
    let custom = list(&nsg["properties"]["securityRules"])
        .iter()
        .map(|held| (held, false));
    let defaults = list(&nsg["properties"]["defaultSecurityRules"])
        .iter()
        .map(|held| (held, true));
    custom
        .chain(defaults)
        .map(|(held, default)| {
            let name = text(&held["name"]).unwrap_or_default();
            rule(&name, &held["properties"], default, id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_rule_reads_plurals_groups_and_the_default_prefix() {
        let fields = json!({"direction": "Inbound", "priority": 200, "access": "Deny",
            "protocol": "Tcp", "sourceAddressPrefix": "", "sourceAddressPrefixes": ["10.0.0.0/8", "Internet"],
            "sourcePortRange": "*", "destinationAddressPrefix": "*",
            "destinationApplicationSecurityGroups": [{"id": "/asg/web"}],
            "destinationPortRanges": ["22", "3389"]});
        let rule = rule("securityRules/deny-admin", &fields, true, None);
        assert_eq!(rule.name, "deny-admin");
        assert!(!rule.default, "the prefix says it is a custom rule");
        assert_eq!(rule.source, ["10.0.0.0/8", "Internet"]);
        assert_eq!(rule.destination, ["*", "/asg/web"]);
        assert_eq!(rule.destination_ports, ["22", "3389"]);
        assert!(super::rule("defaultSecurityRules/DenyAllInBound", &fields, false, None).default);
    }
}
