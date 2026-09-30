//! The Azure domains of agent-cli, ported from az-tui: Key Vault secrets
//! (`kv`), container registry images (`acr`) and AKS clusters (`aks`).
//!
//! There is no Azure SDK and no credential of our own. Each plane gets an
//! `az` token for its own audience, and every call is a plain HTTPS request
//! through core's transport, which already carries az-tui's retry policy. The
//! guardrails az-tui kept are kept here:
//!
//! - a token goes only to its own hosts (`management.azure.com`,
//!   `*.vault.azure.net`, `*.azurecr.io`), checked on every URL, `nextLink`s
//!   and cached addresses included;
//! - an allowlist that reaches nothing is an error, never an empty answer;
//! - a name found in two vaults or registries is exit 2, never a guess;
//! - only `kv secret get` returns a value, and no row type has a field for one.

mod acr;
mod aks;
mod client;
mod config;
mod doctor;
mod graph;
mod kv;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const KV: Domain = Domain {
    name: "kv",
    summary: "Key Vault",
    commands: &[
        kv::secret::list::SECRET_LIST,
        kv::secret::get::SECRET_GET,
        kv::version::list::VERSION_LIST,
        kv::vault::list::VAULT_LIST,
    ],
    synonyms: &[
        ("password", &["secret"]),
        ("passwords", &["secret"]),
        ("credential", &["secret"]),
        ("connection string", &["secret"]),
        ("api key", &["secret"]),
        ("keyvault", &["kv", "vault"]),
        ("key vault", &["kv", "vault"]),
        ("expire", &["expires"]),
        ("expiring", &["expires"]),
        ("rotated", &["version"]),
        ("history", &["version"]),
    ],
    status: doctor::kv_status,
    doctor: doctor::kv_doctor,
};

pub const ACR: Domain = Domain {
    name: "acr",
    summary: "Container Registry",
    commands: &[
        acr::repo::list::REPO_LIST,
        acr::tag::list::TAG_LIST,
        acr::manifest::get::MANIFEST_GET,
        acr::registry::list::REGISTRY_LIST,
    ],
    synonyms: &[
        ("image", &["repo", "tag"]),
        ("images", &["repo", "tag"]),
        ("container image", &["repo", "tag", "manifest"]),
        ("docker", &["acr", "repo"]),
        ("repository", &["repo"]),
        ("repositories", &["repo"]),
        ("digest", &["manifest"]),
        ("sha256", &["manifest"]),
        ("architecture", &["manifest"]),
        ("pushed", &["tag", "updated"]),
    ],
    status: doctor::acr_status,
    doctor: doctor::acr_doctor,
};

pub const AKS: Domain = Domain {
    name: "aks",
    summary: "AKS",
    commands: &[
        aks::cluster::list::CLUSTER_LIST,
        aks::cluster::connect::CLUSTER_CONNECT,
    ],
    synonyms: &[
        ("kubeconfig", &["connect"]),
        ("credentials", &["connect"]),
        ("get-credentials", &["connect"]),
        ("kubernetes cluster", &["aks", "cluster"]),
        ("clusters", &["cluster"]),
    ],
    status: doctor::aks_status,
    doctor: doctor::aks_doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[KV, ACR, AKS]), Vec::<String>::new());
        assert_eq!(check_layout(&[KV, ACR, AKS]), Vec::<String>::new());
    }
}
