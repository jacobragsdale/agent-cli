//! User-assigned managed identities: Resource Graph lists them, and `get`
//! reads one with its federated credentials, each subject named as the k8s
//! serviceaccount id it trusts, and its role assignments (`role/mod.rs`).
//! `entra app get` names an app registration's federated credentials the
//! same way, through [`federated`].

pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::Ctx;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::text;
use crate::config::Azure;
use crate::graph::rows;

pub(crate) const IDENTITY: &str = "Microsoft.ManagedIdentity/userAssignedIdentities";

/// Every AKS cluster's OIDC issuer, which a federated credential trusts.
const ISSUERS: &str = "resources
| where type =~ 'microsoft.containerservice/managedclusters'
| project name, issuer = tostring(properties.oidcIssuerProfile.issuerURL)
| where isnotempty(issuer)
| order by name asc";

/// One federated credential: a token from `issuer` about `subject` signs in
/// as the identity or app without a secret.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Federated {
    pub name: String,
    pub issuer: Option<String>,
    pub subject: Option<String>,
    /// The k8s serviceaccount the subject is: `cluster/namespace/name` when the
    /// issuer is an AKS cluster's in reach, else `namespace/name`.
    pub serviceaccount: Option<String>,
}

/// Federated credentials as ARM (`properties.issuer`) or Graph (`issuer`)
/// writes them. Clusters' issuers are read only when a subject is a
/// serviceaccount.
pub(crate) fn federated(ctx: &Ctx, credentials: &[Value]) -> Result<Vec<Federated>> {
    let field =
        |held: &Value, key: &str| text(&held["properties"][key]).or_else(|| text(&held[key]));
    // ponytail: one Resource Graph query for every cluster's issuer; a
    // cached map when identities are read in bulk.
    let clusters: Vec<(String, String)> = if credentials.iter().any(|held| {
        field(held, "subject").is_some_and(|subject| subject.starts_with("system:serviceaccount:"))
    }) {
        let azure = Azure::load(ctx)?;
        rows(ctx, &azure.subscriptions, ISSUERS, usize::MAX)?
            .iter()
            .filter_map(|row| Some((text(&row["name"])?, text(&row["issuer"])?)))
            .collect()
    } else {
        Vec::new()
    };
    Ok(credentials
        .iter()
        .map(|held| {
            let issuer = field(held, "issuer");
            let subject = field(held, "subject");
            Federated {
                name: text(&held["name"]).unwrap_or_default(),
                serviceaccount: subject
                    .as_deref()
                    .and_then(|subject| serviceaccount(issuer.as_deref(), subject, &clusters)),
                issuer,
                subject,
            }
        })
        .collect())
}

/// The serviceaccount `subject` names, with the cluster whose issuer it is.
fn serviceaccount(
    issuer: Option<&str>,
    subject: &str,
    clusters: &[(String, String)],
) -> Option<String> {
    let rest = subject.strip_prefix("system:serviceaccount:")?;
    let (namespace, name) = rest.split_once(':')?;
    let same = |held: &str| {
        issuer.is_some_and(|issuer| {
            held.trim_end_matches('/')
                .eq_ignore_ascii_case(issuer.trim_end_matches('/'))
        })
    };
    Some(match clusters.iter().find(|(_, held)| same(held)) {
        Some((cluster, _)) => format!("{cluster}/{namespace}/{name}"),
        None => format!("{namespace}/{name}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUER: &str = "https://eastus.oic.prod-aks.azure.com/t/c/";

    #[test]
    fn a_subject_names_the_serviceaccount_with_its_cluster_when_the_issuer_is_one() {
        let clusters = [("aks-contoso".to_owned(), ISSUER.to_owned())];
        let subject = "system:serviceaccount:web:notifier";
        assert_eq!(
            serviceaccount(Some(ISSUER.trim_end_matches('/')), subject, &clusters).as_deref(),
            Some("aks-contoso/web/notifier")
        );
        assert_eq!(
            serviceaccount(
                Some("https://token.actions.githubusercontent.com"),
                subject,
                &clusters
            )
            .as_deref(),
            Some("web/notifier")
        );
        assert_eq!(
            serviceaccount(Some(ISSUER), "sc://contoso/Fabrikam/contoso-dev", &clusters),
            None
        );
    }
}
