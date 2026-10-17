//! What reads of one resource share: an ARM id read into its place and
//! type, portal links, and a name found by Resource Graph, so `arm resource
//! get` and every `net` get take the same refs.

pub(crate) mod diff;
pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::{Ctx, Failure, Request, host_under};
use anyhow::{Result, bail};
use serde_json::Value;

use crate::arm::{SUBSCRIPTIONS_API, of_type, quote, scope, scrub};
use crate::client::{ARM, bearer, decode, refused_with, text};
use crate::config::Azure;
use crate::graph::rows;

/// An ARM id read into where it lives and what type it is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub id: String,
    pub subscription: Option<String>,
    pub group: Option<String>,
    /// `Microsoft.Network`, absent for a subscription or a group.
    pub namespace: Option<String>,
    /// The type path below the namespace: `virtualNetworks/subnets`.
    pub path: String,
}

impl Target {
    pub fn kind(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{namespace}/{}", self.path),
            None if self.group.is_some() => "Microsoft.Resources/resourceGroups".to_owned(),
            None => "Microsoft.Resources/subscriptions".to_owned(),
        }
    }
}

/// `/subscriptions/S[/resourceGroups/G][/providers/NS/TYPE/NAME[/TYPE[/NAME]]…]`,
/// or a tenant-level `/providers/NS/…`. The last `providers` decides the
/// type, so an extension resource is its own type. An odd tail is a child
/// collection, and its type is still the path of type names.
pub(crate) fn target(id: &str) -> Option<Target> {
    let id = id.trim().trim_end_matches('/');
    let segments: Vec<&str> = id.strip_prefix('/')?.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let is = |at: usize, word: &str| {
        segments
            .get(at)
            .is_some_and(|held| held.eq_ignore_ascii_case(word))
    };
    let mut at = 0;
    let mut subscription = None;
    let mut group = None;
    if is(0, "subscriptions") {
        subscription = Some(segments.get(1)?.to_string());
        at = 2;
        if is(2, "resourceGroups") {
            group = Some(segments.get(3)?.to_string());
            at = 4;
        }
    }
    if at == segments.len() {
        // A subscription or a group itself.
        return (at > 0).then(|| Target {
            id: id.to_owned(),
            subscription,
            group,
            namespace: None,
            path: String::new(),
        });
    }
    if !is(at, "providers") {
        return None;
    }
    let providers = (at..segments.len()).rev().find(|&at| is(at, "providers"))?;
    let types: Vec<&str> = segments
        .get(providers + 2..)?
        .iter()
        .step_by(2)
        .copied()
        .collect();
    if types.is_empty() {
        return None;
    }
    Some(Target {
        id: id.to_owned(),
        subscription,
        group,
        namespace: Some(segments.get(providers + 1)?.to_string()),
        path: types.join("/"),
    })
}

/// The id in a portal link, never fetched: `#@tenant/resource/ID/BLADE`, or
/// a view link holding the id percent-encoded in one segment. A blade after
/// the id (`/overview`, `/subnets`) is dropped: an id has the type and the
/// name in pairs.
fn portal(raw: &str) -> Option<String> {
    let fragment = raw.split_once('#')?.1;
    for segment in fragment.split('/') {
        let decoded = decode(segment);
        if decoded.to_ascii_lowercase().starts_with("/subscriptions/") {
            return Some(decoded.split('#').next()?.to_owned());
        }
    }
    let decoded = decode(fragment);
    let from = decoded.to_ascii_lowercase().find("/subscriptions/")?;
    let mut segments: Vec<&str> = decoded[from + 1..]
        .split(['?', '#'])
        .next()?
        .split('/')
        .collect();
    let providers = segments
        .iter()
        .rposition(|held| held.eq_ignore_ascii_case("providers"));
    let whole = match providers {
        Some(at) => at + 2 + (segments.len().saturating_sub(at + 2)) / 2 * 2,
        None if segments
            .get(2)
            .is_some_and(|held| held.eq_ignore_ascii_case("resourceGroups")) =>
        {
            4
        }
        None => 2,
    };
    segments.truncate(whole);
    Some(format!("/{}", segments.join("/")))
}

/// What a ref is read within: the types it may be, the flags that place a
/// name, and the commands a hint names.
pub(crate) struct Lookup<'a> {
    /// The types an id may be, any case; a name is searched among them.
    /// Empty: any type.
    pub kinds: &'a [&'a str],
    pub group: Option<&'a str>,
    pub subscription: Option<&'a str>,
    /// What lists the candidates, for a name found nowhere: `net vnet list`.
    pub list: &'a str,
    /// What takes one of several ids: `net vnet get`.
    pub get: &'a str,
}

/// The id `raw` names: an id, an ARM URL, a portal link, else a name found by
/// Resource Graph within the lookup.
pub(crate) fn resolve(ctx: &Ctx, azure: &Azure, raw: &str, lookup: &Lookup) -> Result<Target> {
    let raw = raw.trim();
    let bad = || -> anyhow::Error {
        Failure::usage(format!(
            "{raw:?} is not an ARM id, an ARM URL, a portal link or a name"
        ))
        .hint(format!(
            "the ids are what this prints: agent-cli {}",
            lookup.list
        ))
        .into()
    };
    let id = if let Some(rest) = raw.strip_prefix("https://") {
        let host = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if host == "management.azure.com" {
            rest[host.len()..]
                .split(['?', '#'])
                .next()
                .unwrap_or_default()
                .to_owned()
        } else if host.starts_with("portal.azure.") {
            portal(raw).ok_or_else(bad)?
        } else {
            return Err(bad());
        }
    } else if raw.starts_with('/') {
        raw.split(['?', '#']).next().unwrap_or_default().to_owned()
    } else if raw.to_ascii_lowercase().starts_with("subscriptions/") {
        format!("/{raw}")
    } else if !raw.is_empty() && !raw.contains('/') {
        return named(ctx, azure, lookup, raw);
    } else {
        return Err(bad());
    };
    let target = target(&id).ok_or_else(bad)?;
    agree(ctx, azure, lookup, &target)?;
    Ok(target)
}

/// The name `raw` gives a resource of `lookup`'s one kind: itself when it is
/// a bare name, else the last segment of the ARM id, URL or portal link it
/// is, checked as `resolve` checks it. For kv and acr, which find a vault or
/// registry by name in the inventory, within its allowlist.
pub(crate) fn name_in(ctx: &Ctx, azure: &Azure, raw: &str, lookup: &Lookup) -> Result<String> {
    let raw = raw.trim();
    if !raw.is_empty() && !raw.contains('/') {
        return Ok(raw.to_owned());
    }
    let target = resolve(ctx, azure, raw, lookup)?;
    Ok(target.id.rsplit('/').next().unwrap_or_default().to_owned())
}

/// Any resource `raw` names, as a `--resource` or `--scope` flag takes it:
/// an id, a portal link, or a name within `group`.
pub(crate) fn any(ctx: &Ctx, azure: &Azure, raw: &str, group: Option<&str>) -> Result<Target> {
    let lookup = Lookup {
        kinds: &[],
        group,
        subscription: None,
        list: "arm resource list",
        get: "arm resource get",
    };
    resolve(ctx, azure, raw, &lookup)
}

/// An id and a flag (or the command's type) that disagree is exit 2; an id
/// outside `[azure] subscriptions` too, since every list keeps within it.
fn agree(ctx: &Ctx, azure: &Azure, lookup: &Lookup, target: &Target) -> Result<()> {
    let kind = target.kind();
    if !lookup.kinds.is_empty()
        && !lookup
            .kinds
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case(&kind))
    {
        return Err(Failure::usage(format!(
            "{} is a {kind}, not a {}",
            target.id,
            lookup.kinds.join(" or ")
        ))
        .into());
    }
    if let (Some(flag), Some(held)) = (lookup.group, target.group.as_deref())
        && !flag.eq_ignore_ascii_case(held)
    {
        return Err(Failure::usage(format!(
            "{} is in group {held}, and --group says {flag}",
            target.id
        ))
        .into());
    }
    let Some(held) = &target.subscription else {
        return Ok(());
    };
    if let Some(flag) = lookup.subscription
        && !scope(ctx, azure, Some(flag))?
            .iter()
            .any(|guid| guid.eq_ignore_ascii_case(held))
    {
        return Err(Failure::usage(format!(
            "{} is in subscription {held}, and --subscription says {flag}",
            target.id
        ))
        .into());
    }
    let allowed = &azure.subscriptions;
    if allowed.iter().any(|guid| !guid.trim().is_empty())
        && !allowed
            .iter()
            .any(|guid| guid.trim().eq_ignore_ascii_case(held))
    {
        return Err(Failure::usage(format!(
            "[azure] subscriptions leaves out {held}, which holds {}",
            target.id
        ))
        .hint("the ones in scope: agent-cli arm subscription list")
        .into());
    }
    Ok(())
}

/// The one resource called `name` within the lookup, or exit 4 (none) or 2
/// (several, listed by id).
fn named(ctx: &Ctx, azure: &Azure, lookup: &Lookup, name: &str) -> Result<Target> {
    let mut kql = format!("resources\n| where name =~ {}", quote(name));
    if !lookup.kinds.is_empty() {
        kql.push_str(&format!("\n| where {}", of_type(lookup.kinds)));
    }
    if let Some(group) = lookup.group {
        kql.push_str(&format!("\n| where resourceGroup =~ {}", quote(group)));
    }
    kql.push_str("\n| project id\n| order by id asc");
    let scope = scope(ctx, azure, lookup.subscription)?;
    let ids: Vec<String> = rows(ctx, &scope, &kql, usize::MAX)?
        .iter()
        .filter_map(|row| text(&row["id"]))
        .collect();
    match ids.as_slice() {
        [id] => target(id)
            .ok_or_else(|| anyhow::anyhow!("Resource Graph answered {id}, which is not an ARM id")),
        [] => Err(Failure::not_found(format!(
            "no resource named {name} in the subscriptions in scope"
        ))
        .hint(format!("the ones in reach: agent-cli {}", lookup.list))
        .into()),
        several => Err(Failure::usage(format!(
            "{} resources are named {name}: {}",
            several.len(),
            several.join(", ")
        ))
        .hint(format!(
            "narrow it with {}--group or --subscription, or pass one: agent-cli {} ID",
            if lookup.kinds.is_empty() {
                "--type, "
            } else {
                ""
            },
            lookup.get
        ))
        .into()),
    }
}

/// What ARM reads a resource group with (Resource Groups - Get).
const GROUPS_API: &str = "2021-04-01";
/// What ARM lists a provider's resource types and api-versions with.
const PROVIDERS_API: &str = "2021-04-01";

/// The newest api-version without a suffix that the provider lists for the
/// type, cached for `[azure] refresh`, and whether only previews were there.
fn api_version(ctx: &Ctx, azure: &Azure, target: &Target) -> Result<(String, bool)> {
    let Some(namespace) = &target.namespace else {
        let fixed = if target.group.is_some() {
            GROUPS_API
        } else {
            SUBSCRIPTIONS_API
        };
        return Ok((fixed.to_owned(), false));
    };
    let key = format!("azure:api-version:{}", target.kind().to_ascii_lowercase());
    if let Some(held) = ctx.cache().get::<(String, bool)>(&key) {
        return Ok(held);
    }
    let scope = target
        .subscription
        .as_deref()
        .map(|guid| format!("/subscriptions/{guid}"))
        .unwrap_or_default();
    let url = format!(
        "https://management.azure.com{scope}/providers/{namespace}?api-version={PROVIDERS_API}"
    );
    if !host_under(&url, "management.azure.com") {
        bail!("{url} is not ARM; the ARM token is not sent there");
    }
    let provider = ctx
        .read(Request::get(&url).auth(&bearer(ctx, ARM)))?
        .json()?;
    // VERIFY(work): that a provider lists a type nested deeper than two
    // (`servers/databases/backupShortTermRetentionPolicies`) by its full
    // path; `az resource show` matches only the top parent and the leaf.
    let versions: Vec<&str> = provider["resourceTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|held| {
            held["resourceType"]
                .as_str()
                .is_some_and(|kind| kind.eq_ignore_ascii_case(&target.path))
        })
        .and_then(|held| held["apiVersions"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // A stable version is a bare date; anything after it (-preview, -beta)
    // is not, and dates sort as text.
    let stable = versions.iter().filter(|version| version.len() == 10).max();
    let chosen = match stable {
        Some(version) => (version.to_string(), false),
        // VERIFY(work): which version to read a preview-only type with; the
        // newest preview is a guess that a type's newest shape is its best.
        None => match versions.iter().max() {
            Some(version) => (version.to_string(), true),
            None => {
                return Err(Failure::not_found(format!(
                    "{namespace} lists no resource type {}",
                    target.path
                ))
                .hint(format!(
                    "what exists: agent-cli arm resource list --type {}",
                    target.kind()
                ))
                .into());
            }
        },
    };
    ctx.cache().put(&key, &chosen, azure.refresh());
    Ok(chosen)
}

/// Any resource as ARM answers it, at the newest stable api-version its
/// provider lists, scrubbed of secrets: what `resource get` prints and
/// `resource diff` compares.
pub(crate) fn fetch(
    ctx: &Ctx,
    azure: &Azure,
    raw: &str,
    lookup: &Lookup,
) -> Result<(Target, Value)> {
    let target = resolve(ctx, azure, raw, lookup)?;
    let (version, preview) = api_version(ctx, azure, &target)?;
    if preview {
        ctx.note(format!(
            "[{} has only preview api-versions; read with {version}]",
            target.kind()
        ));
    }
    let url = format!(
        "https://management.azure.com{}?api-version={version}",
        target.id
    );
    if !host_under(&url, "management.azure.com") {
        bail!("{url} is not ARM; the ARM token is not sent there");
    }
    let answer = ctx
        .read(Request::get(&url).auth(&bearer(ctx, ARM)))
        .map_err(|error| {
            if refused_with(&error) == Some(404) {
                return Failure::not_found(format!("{error:#}"))
                    .hint(format!(
                        "what exists: agent-cli arm resource list --type {}",
                        target.kind()
                    ))
                    .into();
            }
            error
        })?;
    // ponytail: a child collection is ARM's first page, its nextLink left
    // in the answer; follow it when a collection outgrows a page.
    Ok((target, scrub(answer.json()?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUB: &str = "00000000-0000-4000-8000-000000000001";

    fn rg(path: &str) -> String {
        format!("/subscriptions/{SUB}/resourceGroups/rg-contoso-prod{path}")
    }

    fn kind_of(id: &str) -> Option<String> {
        target(id).map(|target| target.kind())
    }

    #[test]
    fn an_id_is_read_into_its_type_path_whatever_its_depth() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod");
        for (id, kind) in [
            (
                format!("/subscriptions/{SUB}"),
                "Microsoft.Resources/subscriptions",
            ),
            (rg(""), "Microsoft.Resources/resourceGroups"),
            (vnet.clone(), "Microsoft.Network/virtualNetworks"),
            (
                format!("{vnet}/subnets/aks/"),
                "Microsoft.Network/virtualNetworks/subnets",
            ),
            (
                rg("/providers/Microsoft.DBforPostgreSQL/flexibleServers/pg-contoso/firewallRules"),
                "Microsoft.DBforPostgreSQL/flexibleServers/firewallRules",
            ),
            (
                format!("{vnet}/providers/Microsoft.Insights/diagnosticSettings/to-law"),
                "Microsoft.Insights/diagnosticSettings",
            ),
            (
                "/providers/Microsoft.Management/managementGroups/mg-contoso".to_owned(),
                "Microsoft.Management/managementGroups",
            ),
        ] {
            assert_eq!(kind_of(&id).as_deref(), Some(kind), "{id}");
        }
        let subnet = target(&format!("{vnet}/subnets/aks")).unwrap();
        assert_eq!(subnet.subscription.as_deref(), Some(SUB));
        assert_eq!(subnet.group.as_deref(), Some("rg-contoso-prod"));
        assert_eq!(subnet.path, "virtualNetworks/subnets");
        for bad in [
            "/".to_owned(),
            "/subscriptions".to_owned(),
            "/subscriptions//resourceGroups/g".to_owned(),
            rg("/virtualNetworks/x"),
            rg("/providers/Microsoft.Network"),
        ] {
            assert_eq!(target(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_portal_link_gives_its_id_without_the_blade() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod");
        assert_eq!(
            portal(&format!(
                "https://portal.azure.com/#@contoso.onmicrosoft.com/resource{vnet}/subnets"
            )),
            Some(vnet.clone())
        );
        assert_eq!(
            portal(&format!(
                "https://portal.azure.com/#resource{vnet}/overview?x=1"
            )),
            Some(vnet.clone())
        );
        assert_eq!(
            portal(&format!(
                "https://portal.azure.com/#@contoso.onmicrosoft.com/resource{}/overview",
                rg("")
            )),
            Some(rg(""))
        );
        let encoded = vnet.replace('/', "%2F");
        assert_eq!(
            portal(&format!(
                "https://portal.azure.com/#view/Microsoft_Azure_Network/VnetBlade/id/{encoded}/location/westeurope"
            )),
            Some(vnet)
        );
        assert_eq!(portal("https://portal.azure.com/#home"), None);
    }
}
