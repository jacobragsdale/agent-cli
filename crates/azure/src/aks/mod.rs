//! `aks`: AKS clusters. `cluster list` reads the Resource Graph inventory;
//! `cluster connect` is az-tui's `setup` for one cluster, without the config
//! file writing: `az aks get-credentials` then `kubelogin convert-kubeconfig`,
//! and a ready-to-paste `[[k8s.scope]]` block for the k8s domain. The rest
//! read and change the cluster through ARM's `managedClusters`, at one
//! api-version, so a field never moves under a command.
//!
//! A cluster is its name, ARM id or portal link (`locate`, `Place`); a pool
//! is `cluster/pool`. `k8s_scope` in its rows is the k8s scope id.

pub(crate) mod cluster;
pub(crate) mod command;
pub(crate) mod nodepool;
pub(crate) mod upgrade;

use agent_cli_core::{Ctx, Failure, Request};
use anyhow::Result;
use serde_json::Value;

use crate::client::{ARM, bearer, decode, refused_with};
use crate::config::Azure;
use crate::graph::{Cluster, inventory};

/// The newest stable Microsoft.ContainerService api-version in Microsoft's
/// REST docs (October 2026).
pub(crate) const API: &str = "2026-07-01";

/// Where a cluster is, as the flags of every command that takes one say it.
#[derive(clap::Args)]
pub struct Place {
    /// Its resource group, for a name two groups hold
    #[arg(long)]
    resource_group: Option<String>,
    /// Its subscription id, for a name two subscriptions hold
    #[arg(long)]
    subscription: Option<String>,
}

/// A cluster's ARM coordinates.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Located {
    pub name: String,
    pub resource_group: String,
    pub subscription: String,
}

impl Located {
    /// The cluster's ARM id.
    pub fn id(&self) -> String {
        format!(
            "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.ContainerService/managedClusters/{}",
            self.subscription, self.resource_group, self.name
        )
    }

    /// ARM's GET of `below` the cluster (`""`, `/agentPools`), as JSON.
    pub fn get(&self, ctx: &Ctx, below: &str) -> Result<Value> {
        let url = self.url(below);
        ctx.read(Request::get(&url).auth(&bearer(ctx, ARM)))
            .map_err(|error| {
                if refused_with(&error) == Some(404) {
                    return Failure::not_found(format!("{error:#}"))
                        .hint("the clusters in reach: agent-cli aks cluster list")
                        .into();
                }
                error
            })?
            .json()
    }

    pub fn url(&self, below: &str) -> String {
        format!(
            "https://management.azure.com{}{below}?api-version={API}",
            self.id()
        )
    }
}

/// The cluster (and pool, for `cluster/pool` or a pool's ARM id) `raw`
/// names: a name, `cluster/pool`, an ARM id or URL, or a portal link, never
/// fetched. A name is found in the inventory within the flags; an id and a
/// flag that disagree is exit 2, as is an id outside `[azure]
/// subscriptions`.
pub(crate) fn locate(
    ctx: &Ctx,
    azure: &Azure,
    raw: &str,
    place: &Place,
) -> Result<(Located, Option<String>)> {
    let raw = raw.trim();
    let id = if let Some(rest) = raw.strip_prefix("https://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if host.eq_ignore_ascii_case("management.azure.com") {
            Some(rest[host.len()..].to_owned())
        } else if host.to_ascii_lowercase().starts_with("portal.azure.") {
            let fragment = decode(raw.split_once('#').map_or("", |(_, held)| held));
            let at = fragment.to_ascii_lowercase().find("/subscriptions/");
            Some(at.map_or_else(String::new, |at| fragment[at..].to_owned()))
        } else {
            Some(String::new())
        }
    } else if raw.starts_with('/') {
        Some(raw.to_owned())
    } else {
        None
    };
    let Some(id) = id else {
        let (name, pool) = match raw.split_once('/') {
            Some((name, pool)) => (name, Some(pool.to_owned())),
            None => (raw, None),
        };
        let found = find(
            ctx,
            azure,
            name,
            place.resource_group.as_deref(),
            place.subscription.as_deref(),
        )?;
        let located = Located {
            name: found.name,
            resource_group: found.resource_group,
            subscription: found.subscription,
        };
        return Ok((located, pool));
    };
    let (located, pool) = parse(&id).ok_or_else(|| {
        Failure::usage(format!(
            "{raw:?} is not an AKS cluster's or node pool's name, ARM id or portal link"
        ))
        .hint("the clusters: agent-cli aks cluster list")
    })?;
    let differ = |flag: &Option<String>, held: &str| {
        flag.as_deref()
            .is_some_and(|flag| !flag.eq_ignore_ascii_case(held))
    };
    if differ(&place.resource_group, &located.resource_group)
        || differ(&place.subscription, &located.subscription)
    {
        return Err(Failure::usage(format!(
            "{raw} is in group {} of subscription {}; --resource-group and --subscription must agree with it",
            located.resource_group, located.subscription
        ))
        .into());
    }
    let allowed: Vec<&str> = azure
        .subscriptions
        .iter()
        .map(|held| held.trim())
        .filter(|held| !held.is_empty())
        .collect();
    if !allowed.is_empty()
        && !allowed
            .iter()
            .any(|held| held.eq_ignore_ascii_case(&located.subscription))
    {
        return Err(Failure::usage(format!(
            "[azure] subscriptions leaves out {}, which holds {}",
            located.subscription, located.name
        ))
        .hint("the clusters in scope: agent-cli aks cluster list")
        .into());
    }
    Ok((located, pool))
}

/// `/subscriptions/S/resourceGroups/G/providers/Microsoft.ContainerService/managedClusters/N[/agentPools/P][/…]`.
fn parse(id: &str) -> Option<(Located, Option<String>)> {
    let path = id.split(['?', '#']).next()?.trim_end_matches('/');
    let segments: Vec<&str> = path.strip_prefix('/')?.split('/').collect();
    let is = |at: usize, word: &str| {
        segments
            .get(at)
            .is_some_and(|held| held.eq_ignore_ascii_case(word))
    };
    let named = |at: usize| segments.get(at).filter(|held| !held.is_empty());
    if !(is(0, "subscriptions")
        && is(2, "resourceGroups")
        && is(4, "providers")
        && is(5, "Microsoft.ContainerService")
        && is(6, "managedClusters"))
    {
        return None;
    }
    let located = Located {
        subscription: named(1)?.to_string(),
        resource_group: named(3)?.to_string(),
        name: named(7)?.to_string(),
    };
    let pool = is(8, "agentPools")
        .then(|| named(9).map(|pool| pool.to_string()))
        .flatten();
    Some((located, pool))
}

/// The one cluster in reach called `name` within the flags: two is exit 2
/// naming where each is, none is exit 4.
pub(crate) fn find(
    ctx: &Ctx,
    azure: &Azure,
    name: &str,
    group: Option<&str>,
    subscription: Option<&str>,
) -> Result<Cluster> {
    let same =
        |flag: Option<&str>, held: &str| flag.is_none_or(|flag| flag.eq_ignore_ascii_case(held));
    let mut found: Vec<Cluster> = inventory(ctx, azure)?
        .clusters
        .into_iter()
        .filter(|cluster| cluster.name.eq_ignore_ascii_case(name))
        .filter(|cluster| same(group, &cluster.resource_group))
        .filter(|cluster| same(subscription, &cluster.subscription))
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

#[cfg(test)]
mod tests {
    use super::*;

    const SUB: &str = "00000000-0000-0000-0000-000000000001";

    #[test]
    fn a_cluster_or_pool_id_is_read_into_its_place_and_anything_else_is_not() {
        let cluster = format!(
            "/subscriptions/{SUB}/resourceGroups/rg-contoso/providers/Microsoft.ContainerService/managedClusters/aks-contoso-dev"
        );
        let (located, pool) = parse(&cluster).unwrap();
        assert_eq!(located.id(), cluster);
        assert_eq!(pool, None);
        let (_, pool) = parse(&format!(
            "{cluster}/agentPools/user/upgradeProfiles/default"
        ))
        .unwrap();
        assert_eq!(pool.as_deref(), Some("user"));
        for bad in [
            format!("/subscriptions/{SUB}/resourceGroups/rg-contoso"),
            cluster.replace("managedClusters", "fleets"),
            format!(
                "/subscriptions/{SUB}/resourceGroups/rg-contoso/providers/Microsoft.ContainerService/managedClusters/"
            ),
        ] {
            assert_eq!(parse(&bad), None, "{bad}");
        }
    }
}
