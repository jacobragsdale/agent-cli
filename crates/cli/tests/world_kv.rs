//! What the contoso world answers of kv. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{follow, ok};

#[test]
fn the_world_answers_what_a_trial_asks_of_kv() {
    let platform = ok(&[
        "kv",
        "secret",
        "list",
        "--tag",
        "owner=platform",
        "--vault",
        "kv-contoso-prod",
        "--fields",
        "name",
    ]);
    assert_eq!(
        platform,
        json!([{"name": "db-password"}, {"name": "orders-db-conn"}, {"name": "worker-db-password"}])
    );
    for args in [
        &["kv", "secret", "list", "--expires-within", "30d"][..],
        &["kv", "version", "list", "kv-contoso-prod/db-password"],
        &["kv", "key", "list"],
        &["kv", "certificate", "list"],
    ] {
        ok(args);
    }
    let expiring = ok(&[
        "kv",
        "secret",
        "list",
        "--expires-within",
        "30d",
        "--managed",
        "false",
        "--fields",
        "id",
    ]);
    assert_eq!(
        expiring,
        json!([{"id": "kv-contoso-prod/db-password"}, {"id": "kv-contoso-prod/worker-db-password"}]),
        "at the world's frozen now"
    );
    let deleted = ok(&[
        "kv",
        "secret",
        "list",
        "--deleted",
        "--fields",
        "id,deleted,purge",
    ]);
    assert_eq!(
        deleted,
        json!([{"id": "kv-contoso-prod/orders-api-key-old",
            "deleted": "2026-09-20T10:15:00Z", "purge": "2026-12-19T10:15:00Z"}])
    );
}

/// Trace 1 starts from the vault: the firewall denies by default and its
/// private endpoint is the way in, which `net endpoint get` takes.
#[test]
fn the_vault_names_its_private_endpoint_for_net() {
    let vault = ok(&["kv", "vault", "get", "kv-contoso-prod"]);
    assert_eq!(vault["authorization"], "rbac");
    assert_eq!(vault["network"]["default_action"], "Deny");
    let endpoint = vault["endpoints"][0]["id"].as_str().unwrap();
    assert!(
        endpoint.ends_with("/privateEndpoints/pe-kv-contoso-prod"),
        "{vault}"
    );
    let got = ok(&["net", "endpoint", "get", endpoint, "--fields", "ips"]);
    assert_eq!(got["ips"], json!(["10.1.4.4"]), "{got}");
}

#[test]
fn a_key_shows_its_rotation_and_a_certificate_expiring_soon_names_its_versions() {
    let key = ok(&["kv", "key", "get", "kv-contoso-prod/orders-cmk"]);
    assert_eq!(key["size"], 2048);
    assert_eq!(
        key["rotation"]["actions"][0],
        json!({"action": "Rotate", "after_create": "P90D"})
    );

    let soon = ok(&[
        "kv",
        "certificate",
        "list",
        "--expires-within",
        "14d",
        "--fields",
        "id,issuer,autorenew,expires",
    ]);
    assert_eq!(
        soon,
        json!([{"id": "kv-contoso-prod/www-contoso-example", "issuer": "Unknown",
            "autorenew": false, "expires": "2026-10-08T00:00:00Z"}])
    );
    let walked = follow(&[
        "kv",
        "certificate",
        "get",
        "kv-contoso-prod/www-contoso-example",
    ]);
    assert_eq!(walked.len(), 2, "certificate get, then version list");
    assert_eq!(walked[1].code, 0, "{}", walked[1].stderr);
    assert_eq!(
        walked[1].json().as_array().map(Vec::len),
        Some(1),
        "no renewal yet"
    );
}

/// Staging's vault lacks prod's connection string and certificates; its
/// `db-password` holds prod's value, its `api-key` does not, and no value
/// is printed.
#[test]
fn the_staging_vault_differs_from_prods_by_names_and_values_never_shown() {
    let vaults = ok(&[
        "kv",
        "vault",
        "diff",
        "kv-contoso-staging",
        "kv-contoso-prod",
    ]);
    let paths: Vec<&str> = vaults["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            "db-password.expires",
            "worker-db-password.expires",
            "internal-contoso-example",
            "orders-db-conn",
            "www-contoso-example",
            "api-key.value",
            "worker-db-password.value",
        ]
    );
    assert_eq!(
        vaults["changes"][5],
        json!({"path": "api-key.value", "redacted": true, "same": false})
    );
    assert!(!vaults.to_string().contains("stand-in"), "{vaults}");
}
