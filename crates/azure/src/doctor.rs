//! The overview's lines for kv, acr, aks and aisearch, and `agent-cli doctor`
//! for each.

use agent_cli_core::{Check, Config, Ctx};

use crate::acr::{CATALOG_SCOPE, Session};
use crate::client::{ARM, REGISTRY, VAULT, az_token, first_line, program};
use crate::config::Azure;
use crate::graph::inventory;
use crate::kv::{API_VERSION, base, get};

/// The status line for a domain whose config is an allowlist in `[azure]`.
fn allowlist_status(
    config: &agent_cli_core::Config,
    domain: &str,
    (one, many): (&str, &str),
    names: fn(&Azure) -> &[String],
) -> String {
    match config.section::<Azure>("azure") {
        Ok(azure) if names(&azure).is_empty() => format!("{domain} all {many}"),
        Ok(azure) if names(&azure).len() == 1 => format!("{domain} 1 {one}"),
        Ok(azure) => format!("{domain} {} {many}", names(&azure).len()),
        Err(_) => format!("{domain} config broken"),
    }
}

/// The checks every Azure domain starts with: `[azure]` parses, `az` hands
/// out an ARM token (so it is installed and signed in), then a token for each
/// of `audiences`. `None` when the rest cannot run.
pub(crate) fn doctor_login(
    ctx: &Ctx,
    checks: &mut Vec<Check>,
    audiences: &[(&str, &str)],
) -> Option<Azure> {
    let azure = match Azure::load(ctx) {
        Ok(azure) => azure,
        Err(error) => {
            checks.push(Check::failed(
                "config",
                format!("{error:#}"),
                "fix [azure]; `agent-cli config example azure` shows every key",
            ));
            return None;
        }
    };
    for &(check, resource) in [("az login", ARM)].iter().chain(audiences) {
        let started = std::time::Instant::now();
        match az_token(ctx, resource, false) {
            Ok(_) => checks.push(Check::ok(
                check,
                format!(
                    "token for {resource} in {} ms",
                    started.elapsed().as_millis()
                ),
            )),
            Err(error) => {
                checks.push(Check::failed(check, format!("{error:#}"), "az login"));
                return None;
            }
        }
    }
    Some(azure)
}

// ---------- kv ----------

pub(crate) fn kv_status(config: &Config) -> String {
    allowlist_status(config, "kv", ("vault", "vaults"), |azure| &azure.vaults)
}

/// The login and its two tokens, the inventory, and one page from each vault:
/// a vault Resource Graph lists can still refuse every data-plane call. Reads
/// names only, never a value.
pub(crate) fn kv_doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = doctor_login(ctx, &mut checks, &[("vault token", VAULT)]) else {
        return checks;
    };
    let inventory = match inventory(ctx, &azure) {
        Ok(inventory) => inventory,
        Err(error) => {
            checks.push(Check::failed(
                "inventory",
                format!("{error:#}"),
                "az account list; check [azure] subscriptions",
            ));
            return checks;
        }
    };
    let vaults = crate::client::allowed(&inventory.vaults, &azure.vaults, |v| &v.name);
    checks.push(Check::ok(
        "inventory",
        format!("{} vaults in reach", vaults.len()),
    ));
    for name in crate::client::missing(&inventory.vaults, &azure.vaults, |v| &v.name) {
        checks.push(Check::failed(
            format!("vault {name}"),
            "not found by Resource Graph in the subscriptions in scope",
            "fix [azure] vaults or subscriptions; `agent-cli kv vault list` shows what is there",
        ));
    }
    for vault in &vaults {
        let check = format!("vault {}", vault.name);
        if ctx.remaining().is_err() {
            checks.push(Check::failed(
                check,
                "not checked: --timeout ran out",
                "agent-cli doctor kv --timeout 120",
            ));
            continue;
        }
        let started = std::time::Instant::now();
        let url = format!(
            "{}secrets?api-version={API_VERSION}&maxresults=1",
            base(vault)
        );
        checks.push(match get(ctx, vault, &url) {
            Ok(_) => Check::ok(
                check,
                format!("answered in {} ms", started.elapsed().as_millis()),
            ),
            Err(error) => Check::failed(
                check,
                format!("{error:#}"),
                "needs the Key Vault Secrets User role, and this IP allowed by the vault firewall",
            ),
        });
    }
    checks
}

// ---------- acr ----------

pub(crate) fn acr_status(config: &Config) -> String {
    allowlist_status(config, "acr", ("registry", "registries"), |azure| {
        &azure.registries
    })
}

/// The login and the registry token, the inventory, and the first catalog
/// page of each registry: the exchange is where a missing role shows.
pub(crate) fn acr_doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = doctor_login(ctx, &mut checks, &[("registry token", REGISTRY)]) else {
        return checks;
    };
    let inventory = match inventory(ctx, &azure) {
        Ok(inventory) => inventory,
        Err(error) => {
            checks.push(Check::failed(
                "inventory",
                format!("{error:#}"),
                "az account list; check [azure] subscriptions",
            ));
            return checks;
        }
    };
    let registries = crate::client::allowed(&inventory.registries, &azure.registries, |r| &r.name);
    checks.push(Check::ok(
        "inventory",
        format!("{} registries in reach", registries.len()),
    ));
    for name in crate::client::missing(&inventory.registries, &azure.registries, |r| &r.name) {
        checks.push(Check::failed(
            format!("registry {name}"),
            "not found by Resource Graph in the subscriptions in scope",
            "fix [azure] registries or subscriptions; `agent-cli acr registry list` shows what is there",
        ));
    }
    for registry in &registries {
        let check = format!("registry {}", registry.name);
        if ctx.remaining().is_err() {
            checks.push(Check::failed(
                check,
                "not checked: --timeout ran out",
                "agent-cli doctor acr --timeout 120",
            ));
            continue;
        }
        let started = std::time::Instant::now();
        let probe = Session::new(ctx, registry)
            .and_then(|session| session.get(CATALOG_SCOPE, "_catalog?n=1"));
        checks.push(match probe {
            Ok(_) => Check::ok(check, format!("{} answered in {} ms", registry.login_server, started.elapsed().as_millis())),
            Err(error) => Check::failed(check, format!("{error:#}"), "needs AcrPull (or Container Registry Repository Catalog Lister on an ABAC registry)"),
        });
    }
    checks
}

// ---------- aks ----------

pub(crate) fn aks_status(config: &Config) -> String {
    match config.section::<Azure>("azure") {
        Ok(_) => String::new(),
        Err(_) => "aks config broken".to_owned(),
    }
}

/// The login, the clusters in reach, and `kubelogin` on PATH, which a cluster
/// with Entra ID sign-in needs.
pub(crate) fn aks_doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = doctor_login(ctx, &mut checks, &[]) else {
        return checks;
    };
    checks.push(match inventory(ctx, &azure) {
        Ok(found) => Check::ok(
            "clusters",
            format!("{} AKS clusters in reach", found.clusters.len()),
        ),
        Err(error) => Check::failed(
            "clusters",
            format!("{error:#}"),
            "az account list; check [azure] subscriptions",
        ),
    });
    let mut version = program("kubelogin");
    version.arg("--version");
    checks.push(match ctx.read(version) {
        Ok(output) if output.status.success() => {
            Check::ok("kubelogin", first_line(&output.stdout).to_owned())
        }
        Ok(output) => Check::failed("kubelogin", first_line(&output.stderr).to_owned(), "reinstall kubelogin"),
        Err(error) => Check::failed(
            "kubelogin",
            format!("{error:#}"),
            "install kubelogin (https://azure.github.io/kubelogin/); a cluster with Entra ID sign-in needs it",
        ),
    });
    checks
}

// ---------- aisearch ----------

pub(crate) fn aisearch_status(config: &Config) -> String {
    allowlist_status(config, "aisearch", ("service", "services"), |azure| {
        &azure.search_services
    })
}

/// The login, the services in reach, and for the first three the auth they
/// take and `GET /servicestats`: the cheapest call that proves it works.
pub(crate) fn aisearch_doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("azure") {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let Some(azure) = doctor_login(ctx, &mut checks, &[]) else {
        return checks;
    };
    let services = match crate::aisearch::reach(ctx, &azure, &[]) {
        Ok(services) => services,
        Err(error) => {
            checks.push(Check::failed(
                "inventory",
                format!("{error:#}"),
                "az account list; check [azure] subscriptions and search_services",
            ));
            return checks;
        }
    };
    checks.push(Check::ok(
        "inventory",
        format!("{} search services in reach", services.len()),
    ));
    for service in services.into_iter().take(3) {
        let check = format!("service {}", service.name);
        let auth = crate::aisearch::auth_mode(&service);
        let started = std::time::Instant::now();
        let probe = crate::aisearch::Search::new(ctx, service)
            .and_then(|search| search.get("/servicestats", crate::aisearch::Role::Definitions));
        checks.push(match probe {
            Ok(_) => Check::ok(
                check,
                format!(
                    "servicestats answered in {} ms (auth by {auth})",
                    started.elapsed().as_millis()
                ),
            ),
            Err(error) => Check::failed(
                check,
                format!("{error:#} (auth by {auth})"),
                "a token needs Reader (definitions) and Search Index Data Reader (documents); a key needs Contributor or Search Service Contributor to fetch it",
            ),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::{self, exchanged, issued, listing, one_registry, two_vaults};

    #[test]
    fn doctor_checks_the_login_the_inventory_and_each_vault_without_reading_a_value() {
        let (ctx, transport) = testing::doctor_ctx(
            vec![
                two_vaults(),
                listing("kv-contoso-dev", &["db-password"]),
                Answer::status(
                    403,
                    r#"{"error":{"code":"Forbidden","message":"Client address is not authorized"}}"#,
                ),
            ],
            "[azure]\nvaults = [\"kv-contoso-dev\", \"kv-contoso-prod\", \"kv-gone\"]\n",
        );
        let checks = kv_doctor(&ctx);
        assert_eq!(
            testing::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("vault token".to_owned(), true),
                ("inventory".to_owned(), true),
                ("vault kv-gone".to_owned(), false),
                ("vault kv-contoso-dev".to_owned(), true),
                ("vault kv-contoso-prod".to_owned(), false),
            ]
        );
        assert!(checks[5].detail.contains("firewall"), "{:?}", checks[5]);
        let sent = transport.sent();
        assert!(
            sent[1..]
                .iter()
                .all(|sent| sent.url.ends_with("secrets?api-version=7.4&maxresults=1"))
        );

        let (ctx, _) = testing::doctor_ctx(vec![], "");
        assert!(kv_doctor(&ctx).is_empty(), "no [azure], nothing to check");
    }

    #[test]
    fn the_overview_counts_the_allowlist_from_config_alone() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(
            kv_status(&config("[azure]\nvaults = [\"a\", \"b\", \"c\"]\n")),
            "kv 3 vaults"
        );
        assert_eq!(
            kv_status(&config("[azure]\nvaults = \"a\"\n")),
            "kv 1 vault"
        );
        assert_eq!(kv_status(&config("")), "kv all vaults");
        assert_eq!(
            kv_status(&config("[azure]\nvault = \"typo\"\n")),
            "kv config broken"
        );
        assert_eq!(
            acr_status(&config("[azure]\nregistries = \"contosoacr\"\n")),
            "acr 1 registry"
        );
    }

    #[test]
    fn doctor_reads_one_catalog_page_per_registry() {
        let (ctx, transport) = testing::doctor_ctx(
            vec![
                one_registry(),
                exchanged(),
                issued("t"),
                Answer::json(&json!({"repositories": ["team/api"]})),
            ],
            "[azure]\n",
        );
        let checks = acr_doctor(&ctx);
        assert_eq!(
            testing::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("registry token".to_owned(), true),
                ("inventory".to_owned(), true),
                ("registry contosoacr".to_owned(), true),
            ]
        );
        assert_eq!(
            transport.sent()[3].url,
            "https://contosoacr.azurecr.io/acr/v1/_catalog?n=1"
        );
    }

    #[test]
    fn doctor_counts_the_clusters_and_finds_kubelogin() {
        let (ctx, _) = testing::doctor_ctx(
            vec![testing::inventory(vec![testing::cluster(
                "aks-contoso-dev",
            )])],
            "[azure]\n",
        );
        let checks = aks_doctor(&ctx);
        assert_eq!(
            testing::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("clusters".to_owned(), true),
                ("kubelogin".to_owned(), true),
            ]
        );
        assert_eq!(checks[1].detail, "1 AKS clusters in reach");
        assert_eq!(checks[2].detail, "kubelogin version v0.1.4-fake");
    }

    #[test]
    fn doctor_probes_each_search_service_with_the_auth_it_takes() {
        let refused = || {
            Answer::status(
                403,
                r#"{"error":{"code":"","message":"Authorization failed."}}"#,
            )
        };
        let (ctx, transport) = testing::doctor_ctx(
            vec![
                testing::inventory(vec![
                    testing::search_service("srch-contoso-dev", "apiKeyOnly"),
                    testing::search_service("srch-contoso-prod", "aadOrApiKey"),
                ]),
                testing::admin_keys(),
                Answer::json(&json!({"counters": {}})),
                refused(),
                testing::admin_keys(),
                refused(),
            ],
            "[azure]\nparallel = 1\n",
        );
        let checks = aisearch_doctor(&ctx);
        assert_eq!(
            testing::rows(&checks),
            [
                ("az login".to_owned(), true),
                ("inventory".to_owned(), true),
                ("service srch-contoso-dev".to_owned(), true),
                ("service srch-contoso-prod".to_owned(), false),
            ]
        );
        assert!(
            checks[2].detail.ends_with("(auth by key)"),
            "{:?}",
            checks[2]
        );
        assert!(
            checks[3]
                .detail
                .contains("needs Reader or Search Service Contributor")
                && checks[3].detail.contains("its admin key was refused too"),
            "{:?}",
            checks[3]
        );
        assert_eq!(
            transport.sent().len(),
            6,
            "one fallback to the key, no more"
        );
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(
            aisearch_status(&config("[azure]\nsearch_services = [\"a\", \"b\"]\n")),
            "aisearch 2 services"
        );
        assert_eq!(aisearch_status(&config("")), "aisearch all services");
    }
}
