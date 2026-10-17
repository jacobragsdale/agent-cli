//! The Azure domains of agent-cli: Key Vault secrets, keys and
//! certificates (`kv`), container registry images and builds (`acr`) and
//! AKS clusters (`aks`), ported from az-tui, Azure AI Search (`aisearch`),
//! any ARM resource (`arm`), virtual networks (`net`), and Data Lake
//! Storage Gen2 (`lake`, behind the feature of its name: DuckDB is in it).
//!
//! There is no Azure SDK and no credential of our own. Each plane gets an
//! `az` token for its own audience (or, for an AI Search service that takes
//! keys only, its admin key from ARM, held for the run), and every call is a plain HTTPS request
//! through core's transport, which already carries az-tui's retry policy. The
//! guardrails az-tui kept are kept here:
//!
//! - a token goes only to its own hosts (`management.azure.com`,
//!   `*.vault.azure.net`, `*.azurecr.io`, `*.search.windows.net`, an
//!   account in reach's `*.dfs.core.windows.net`, `graph.microsoft.com`),
//!   checked on every URL, `nextLink`s and cached addresses included;
//! - an allowlist that reaches nothing is an error, never an empty answer;
//! - a name found in two vaults, registries or search services is exit 2,
//!   never a guess;
//! - only `kv secret get` returns a value, and no row type has a field for one.

mod acr;
mod aisearch;
mod aks;
mod arm;
mod client;
mod config;
mod doctor;
mod graph;
mod kv;
#[cfg(feature = "lake")]
mod lake;
mod net;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const KV: Domain = Domain {
    name: "kv",
    summary: "Key Vault",
    commands: &[
        kv::secret::list::SECRET_LIST,
        kv::secret::get::SECRET_GET,
        kv::secret::update::SECRET_UPDATE,
        kv::version::list::VERSION_LIST,
        kv::certificate::list::CERTIFICATE_LIST,
        kv::certificate::get::CERTIFICATE_GET,
        kv::key::list::KEY_LIST,
        kv::key::get::KEY_GET,
        kv::vault::list::VAULT_LIST,
        kv::vault::get::VAULT_GET,
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
        acr::tag::delete::TAG_DELETE,
        acr::manifest::get::MANIFEST_GET,
        acr::run::list::RUN_LIST,
        acr::run::get::RUN_GET,
        acr::run::logs::RUN_LOGS,
        acr::registry::list::REGISTRY_LIST,
        acr::registry::get::REGISTRY_GET,
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
        aks::cluster::get::CLUSTER_GET,
        aks::cluster::connect::CLUSTER_CONNECT,
        aks::nodepool::list::NODEPOOL_LIST,
        aks::nodepool::get::NODEPOOL_GET,
        aks::nodepool::scale::NODEPOOL_SCALE,
        aks::upgrade::list::UPGRADE_LIST,
        aks::command::run::COMMAND_RUN,
    ],
    synonyms: &[
        ("kubeconfig", &["connect"]),
        ("credentials", &["connect"]),
        ("get-credentials", &["connect"]),
        ("kubernetes cluster", &["aks", "cluster"]),
        ("clusters", &["cluster"]),
        ("pull images", &["kubelet"]),
    ],
    status: doctor::aks_status,
    doctor: doctor::aks_doctor,
};

pub const AISEARCH: Domain = Domain {
    name: "aisearch",
    summary: "Azure AI Search",
    commands: &[
        aisearch::service::list::SERVICE_LIST,
        aisearch::service::get::SERVICE_GET,
        aisearch::index::list::INDEX_LIST,
        aisearch::index::get::INDEX_GET,
        aisearch::index::create::INDEX_CREATE,
        aisearch::index::update::INDEX_UPDATE,
        aisearch::index::delete::INDEX_DELETE,
        aisearch::document::list::DOCUMENT_LIST,
        aisearch::document::get::DOCUMENT_GET,
        aisearch::document::create::DOCUMENT_CREATE,
        aisearch::document::update::DOCUMENT_UPDATE,
        aisearch::document::delete::DOCUMENT_DELETE,
        aisearch::indexer::list::INDEXER_LIST,
        aisearch::indexer::get::INDEXER_GET,
        aisearch::indexer::run::INDEXER_RUN,
        aisearch::indexer::wait::INDEXER_WAIT,
    ],
    synonyms: &[
        ("ai search", &["aisearch"]),
        ("azure search", &["aisearch"]),
        ("cognitive search", &["aisearch"]),
        ("search service", &["aisearch", "service"]),
        ("search index", &["aisearch", "index"]),
        ("vector", &["document", "vector"]),
        ("embedding", &["document", "vector"]),
        ("semantic", &["document", "semantic"]),
        ("hybrid", &["document", "hybrid"]),
        ("lookup", &["document", "get"]),
        ("reindex", &["indexer", "run"]),
    ],
    status: doctor::aisearch_status,
    doctor: doctor::aisearch_doctor,
};

pub const ARM: Domain = Domain {
    name: "arm",
    summary: "Azure Resource Manager",
    commands: &[
        arm::resource::list::RESOURCE_LIST,
        arm::resource::get::RESOURCE_GET,
        arm::query::run::QUERY_RUN,
        arm::activity::list::ACTIVITY_LIST,
        arm::change::list::CHANGE_LIST,
        arm::metric::list::METRIC_LIST,
        arm::role::list::ROLE_LIST,
        arm::identity::list::IDENTITY_LIST,
        arm::identity::get::IDENTITY_GET,
        arm::policy::list::POLICY_LIST,
        arm::group::list::GROUP_LIST,
        arm::subscription::list::SUBSCRIPTION_LIST,
    ],
    synonyms: &[
        ("resource graph", &["query"]),
        ("kusto", &["query"]),
        ("resource group", &["group"]),
        ("resource groups", &["group"]),
        ("rg", &["group"]),
        ("subscriptions", &["subscription"]),
        ("azure resource", &["arm", "resource"]),
        ("azure resources", &["arm", "resource"]),
        ("who deleted", &["activity"]),
        ("who changed", &["activity"]),
    ],
    status: doctor::arm_status,
    doctor: doctor::arm_doctor,
};

pub const NET: Domain = Domain {
    name: "net",
    summary: "Azure networking",
    commands: &[
        net::vnet::list::VNET_LIST,
        net::vnet::get::VNET_GET,
        net::subnet::get::SUBNET_GET,
        net::nsg::list::NSG_LIST,
        net::nsg::get::NSG_GET,
        net::nic::get::NIC_GET,
        net::route::list::ROUTE_LIST,
        net::endpoint::list::ENDPOINT_LIST,
        net::endpoint::get::ENDPOINT_GET,
        net::zone::list::ZONE_LIST,
        net::record::list::RECORD_LIST,
        net::ip::list::IP_LIST,
        net::lb::list::LB_LIST,
        net::lb::get::LB_GET,
        net::appgw::list::APPGW_LIST,
        net::appgw::get::APPGW_GET,
    ],
    synonyms: &[
        ("virtual network", &["vnet"]),
        ("vnets", &["vnet"]),
        ("network security group", &["nsg"]),
        ("security rules", &["nsg", "rules"]),
        ("network interface", &["nic"]),
        ("route table", &["route"]),
        ("routes", &["route"]),
        ("udr", &["route"]),
        ("peering", &["vnet", "peerings"]),
        ("ip addresses", &["subnet", "ips"]),
        ("exhaustion", &["subnet", "free_ips"]),
    ],
    status: doctor::net_status,
    doctor: doctor::arm_doctor,
};

#[cfg(feature = "lake")]
pub const LAKE: Domain = Domain {
    name: "lake",
    summary: "Data Lake Storage",
    commands: &[
        lake::account::list::ACCOUNT_LIST,
        lake::container::list::CONTAINER_LIST,
        lake::path::list::PATH_LIST,
        lake::path::get::PATH_GET,
        lake::table::list::TABLE_LIST,
        lake::table::get::TABLE_GET,
        lake::partition::list::PARTITION_LIST,
        lake::access::get::ACCESS_GET,
        lake::query::run::QUERY_RUN,
    ],
    synonyms: &[
        ("adls", &["lake"]),
        ("datalake", &["lake"]),
        ("data lake", &["lake"]),
        ("abfss", &["lake", "path"]),
        ("blob", &["lake", "path"]),
        ("storage", &["lake", "account"]),
        ("parquet", &["lake"]),
        ("folder", &["path"]),
        ("partitions", &["partition"]),
        ("tables", &["table"]),
    ],
    status: doctor::lake_status,
    doctor: doctor::lake_doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        let domains = [
            KV,
            ACR,
            AKS,
            AISEARCH,
            ARM,
            NET,
            #[cfg(feature = "lake")]
            LAKE,
        ];
        assert_eq!(check_registry(&domains), Vec::<String>::new());
        assert_eq!(check_layout(&domains), Vec::<String>::new());
    }
}
