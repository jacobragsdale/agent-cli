//! `arm resource get`: ARM `GET {id}` for any resource type, at the newest
//! stable api-version its provider lists, so no type needs code of its own.

use agent_cli_core::{Ctx, Failure, Request, command, host_under};
use anyhow::{Result, bail};
use serde_json::Value;

use crate::arm::{SUBSCRIPTIONS_API, quote, scope, scrub};
use crate::client::{ARM, bearer, decode, refused_with, text};
use crate::config::Azure;
use crate::graph::rows;

/// What ARM reads a resource group with (Resource Groups - Get).
const GROUPS_API: &str = "2021-04-01";
/// What ARM lists a provider's resource types and api-versions with.
const PROVIDERS_API: &str = "2021-04-01";

#[derive(clap::Args)]
pub struct ResourceGetArgs {
    /// The resource: its ARM id (a child collection such as SERVER_ID/firewallRules too), its portal link, or its name
    resource: String,
    /// Its type, for a name: Microsoft.Network/virtualNetworks
    #[arg(long = "type")]
    kind: Option<String>,
    /// Its resource group, for a name two groups hold
    #[arg(long)]
    group: Option<String>,
    /// Its subscription, GUID or name, for a name two subscriptions hold
    #[arg(long)]
    subscription: Option<String>,
}

/// An ARM id read into where it lives and what type it is.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    id: String,
    subscription: Option<String>,
    group: Option<String>,
    /// `Microsoft.Network`, absent for a subscription or a group.
    namespace: Option<String>,
    /// The type path below the namespace: `virtualNetworks/subnets`.
    path: String,
}

impl Target {
    fn kind(&self) -> String {
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
fn target(id: &str) -> Option<Target> {
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

/// The id `resource` names: an id, an ARM URL, a portal link, else a name
/// found by Resource Graph within the flags.
fn resolve(ctx: &Ctx, azure: &Azure, args: &ResourceGetArgs) -> Result<Target> {
    let raw = args.resource.trim();
    let bad = || -> anyhow::Error {
        Failure::usage(format!(
            "{raw:?} is not an ARM id, an ARM URL, a portal link or a name"
        ))
        .hint("the ids are what this prints: agent-cli arm resource list NAME")
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
        return named(ctx, azure, args, raw);
    } else {
        return Err(bad());
    };
    let target = target(&id).ok_or_else(bad)?;
    agree(ctx, azure, args, &target)?;
    Ok(target)
}

/// An id and a flag that disagree is exit 2; an id outside `[azure]
/// subscriptions` too, since every other arm command keeps within it.
fn agree(ctx: &Ctx, azure: &Azure, args: &ResourceGetArgs, target: &Target) -> Result<()> {
    let differ = |flag: &Option<String>, held: Option<&str>| {
        flag.as_deref()
            .zip(held)
            .is_some_and(|(flag, held)| !flag.eq_ignore_ascii_case(held))
    };
    let kind = target.kind();
    if differ(&args.kind, Some(&kind)) || differ(&args.group, target.group.as_deref()) {
        return Err(Failure::usage(format!(
            "{} is a {kind} in group {}; --type and --group must agree with it",
            target.id,
            target.group.as_deref().unwrap_or("none")
        ))
        .into());
    }
    let Some(held) = &target.subscription else {
        return Ok(());
    };
    if let Some(flag) = &args.subscription
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

/// The one resource called `name` within the flags, or exit 4 (none) or 2
/// (several, listed by id).
fn named(ctx: &Ctx, azure: &Azure, args: &ResourceGetArgs, name: &str) -> Result<Target> {
    let mut kql = format!("resources\n| where name =~ {}", quote(name));
    if let Some(kind) = &args.kind {
        kql.push_str(&format!("\n| where type =~ {}", quote(kind)));
    }
    if let Some(group) = &args.group {
        kql.push_str(&format!("\n| where resourceGroup =~ {}", quote(group)));
    }
    kql.push_str("\n| project id\n| order by id asc");
    let scope = scope(ctx, azure, args.subscription.as_deref())?;
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
        .hint(format!(
            "names that contain it: agent-cli arm resource list {name}"
        ))
        .into()),
        several => Err(Failure::usage(format!(
            "{} resources are named {name}: {}",
            several.len(),
            several.join(", ")
        ))
        .hint("narrow it with --type, --group or --subscription, or pass one: agent-cli arm resource get ID")
        .into()),
    }
}

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

fn resource_get(ctx: &Ctx, args: ResourceGetArgs) -> Result<Value> {
    let azure = Azure::load(ctx)?;
    let target = resolve(ctx, &azure, &args)?;
    let (version, preview) = api_version(ctx, &azure, &target)?;
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
    Ok(scrub(answer.json()?))
}

command! {
    pub RESOURCE_GET = ["arm", "resource", "get"], Read,
    "Show any Azure resource as ARM returns it, by id, portal link or name",
    keywords: ["properties", "config", "settings", "firewall", "rules", "show", "json", "arm"],
    example: "arm resource get vnet-contoso-prod --type Microsoft.Network/virtualNetworks --fields properties.subnets",
    run: resource_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::{azure, azure_with, inventory};

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

    fn provider(path: &str, versions: &[&str]) -> Answer {
        Answer::json(&json!({"namespace": "Microsoft.Network", "resourceTypes": [
            {"resourceType": "virtualNetworks", "apiVersions": ["2099-01-01"]},
            {"resourceType": path, "apiVersions": versions},
        ]}))
    }

    #[test]
    fn a_nested_type_is_read_at_its_newest_stable_api_version() {
        let subnet =
            rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod/subnets/aks");
        let link =
            format!("https://portal.azure.com/#@contoso.onmicrosoft.com/resource{subnet}/overview");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &link],
            vec![
                provider(
                    "virtualNetworks/subnets",
                    &[
                        "2024-07-01-preview",
                        "2024-05-01",
                        "2023-11-01",
                        "2024-01-01",
                    ],
                ),
                Answer::json(&json!({"id": subnet, "name": "aks",
                    "properties": {"addressPrefix": "10.1.0.0/22",
                        "keyVaultSecretId": "https://kv-contoso-prod.vault.azure.net/secrets/tls"}})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["properties"]["addressPrefix"], "10.1.0.0/22");
        assert_eq!(
            outcome.json()["properties"]["keyVaultSecretId"],
            "https://kv-contoso-prod.vault.azure.net/secrets/tls",
            "a reference to a secret is not one"
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!(
                "https://management.azure.com/subscriptions/{SUB}/providers/Microsoft.Network?api-version=2021-04-01"
            )
        );
        assert_eq!(
            sent[1].url,
            format!("https://management.azure.com{subnet}?api-version=2024-05-01")
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@https://management.azure.com/")
        );
    }

    #[test]
    fn a_preview_only_type_is_read_with_its_newest_preview_and_says_so() {
        let id = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod/subnets");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &id],
            vec![
                provider(
                    "virtualNetworks/subnets",
                    &["2024-01-01-preview", "2024-03-01-preview"],
                ),
                Answer::json(&json!({"value": []})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("only preview api-versions; read with 2024-03-01-preview"),
            "{}",
            outcome.stderr
        );
        assert!(
            transport.sent()[1]
                .url
                .ends_with("api-version=2024-03-01-preview")
        );
    }

    #[test]
    fn a_name_is_found_by_resource_graph_and_two_holders_are_listed_not_guessed() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "resource",
                "get",
                "vnet-contoso-prod",
                "--type",
                "Microsoft.Network/virtualNetworks",
            ],
            vec![
                inventory(vec![json!({ "id": vnet })]),
                provider("virtualNetworks", &["2024-05-01"]),
                Answer::json(&json!({ "id": vnet })),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert_eq!(
            query,
            "resources\n| where name =~ 'vnet-contoso-prod'\n| where type =~ 'Microsoft.Network/virtualNetworks'\n| project id\n| order by id asc"
        );

        let other = vnet.replace("rg-contoso-prod", "rg-contoso-dev");
        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", "vnet-contoso-prod"],
            vec![inventory(vec![
                json!({ "id": other }),
                json!({ "id": vnet }),
            ])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(&other) && outcome.stderr.contains(&vnet),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", "vnet-gone"],
            vec![inventory(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("arm resource list vnet-gone"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn an_id_the_flags_or_the_allowlist_disagree_with_is_a_usage_error() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-contoso-prod");
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &vnet, "--group", "rg-contoso-dev"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());

        let (outcome, transport) = azure_with(
            &[crate::ARM],
            &["arm", "resource", "get", &vnet],
            vec![],
            "[azure]\nsubscriptions = \"00000000-0000-4000-8000-000000000002\"\n",
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("leaves out"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());

        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", "https://example.com/x"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_missing_resource_is_exit_4_naming_the_list_to_run() {
        let vnet = rg("/providers/Microsoft.Network/virtualNetworks/vnet-gone");
        let url = format!("https://management.azure.com{vnet}?api-version=2020-01-01");
        let (outcome, _) = azure(
            &[crate::ARM],
            &["arm", "resource", "get", &url],
            vec![
                provider("virtualNetworks", &["2024-05-01"]),
                Answer::status(
                    404,
                    r#"{"error":{"code":"ResourceNotFound","message":"The Resource 'Microsoft.Network/virtualNetworks/vnet-gone' under resource group 'rg-contoso-prod' was not found."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("arm resource list --type Microsoft.Network/virtualNetworks"),
            "{}",
            outcome.stderr
        );
    }
}
