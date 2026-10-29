//! What the contoso world answers of http: the api's spec on both
//! environments, its orders, prod's failing POST, and the certificates its
//! hosts serve. Compiled only with the `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{agent_cli, ok};

const PROD: &str = "https://api.contoso.example";
const STAGING: &str = "https://api-staging.contoso.example";

#[test]
fn the_api_spec_is_found_from_its_base_url_and_its_docs_page() {
    let spec = ok(&["http", "spec", "get", PROD]);
    assert_eq!(spec["id"], format!("{PROD}/swagger/v1/swagger.json"));
    assert_eq!(spec["framework"], "ASP.NET Core (Swashbuckle)");
    assert_eq!(spec["docs"], format!("{PROD}/swagger/index.html"));
    let page = ok(&["http", "spec", "get", &format!("{PROD}/swagger/index.html")]);
    assert_eq!(page["id"], spec["id"]);
    assert_eq!(page["found"], "docs page");
    let ops = ok(&[
        "http",
        "operation",
        "list",
        PROD,
        "--param",
        "customer_id",
        "--fields",
        "id",
    ]);
    assert_eq!(
        ops,
        json!([{"id": format!("{PROD}/swagger/v1/swagger.json#POST /orders")}])
    );
}

#[test]
fn what_1_4_2_changed_is_one_breaking_rule_and_one_new_operation() {
    let diff = ok(&["http", "spec", "diff", STAGING, PROD]);
    let breaking: Vec<&str> = diff["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["breaking"].as_str())
        .collect();
    assert_eq!(
        breaking,
        ["POST /orders's body field lines[].sku is now required"]
    );
    let added = diff["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == "operations.GET /orders/{}/fulfilment" && c["left"].is_null());
    assert!(added, "{diff}");
}

#[test]
fn order_42_differs_between_staging_and_prod_in_its_status() {
    let diff = ok(&[
        "http",
        "request",
        "diff",
        &format!("{STAGING}/orders/42"),
        &format!("{PROD}/orders/42"),
    ]);
    assert_eq!(
        diff["changes"],
        json!([{"path": "body.status", "left": "open", "right": "shipped"}])
    );
}

#[test]
fn prods_failing_post_names_its_trace_in_datadog() {
    let ran = agent_cli(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{PROD}/orders"),
        "--body",
        r#"{"customerId": 7, "lines": [{"sku": "SKU-1042", "qty": 2}]}"#,
    ]);
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    assert_eq!(ran.json()["trace_id"], "1846223019175527734");
    assert!(
        ran.stderr.contains(
            "hint: the trace in Datadog: agent-cli dd span list --trace 1846223019175527734"
        ),
        "{}",
        ran.stderr
    );
    let staging = ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{STAGING}/orders"),
        "--body",
        r#"{"customerId": 7, "lines": []}"#,
    ]);
    assert_eq!(staging["status"], 201);
}

#[test]
fn the_api_serves_cert_managers_certificate_and_www_the_one_key_vault_holds() {
    let api = ok(&["http", "certificate", "get", "api.contoso.example"]);
    assert_eq!(api["leaf"]["not_after"], "2026-11-30T03:00:00Z");
    assert_eq!(api["trusted"], true);
    let www = ok(&["http", "certificate", "get", "www.contoso.example"]);
    let kv = ok(&["kv", "certificate", "get", "www-contoso-example"]);
    assert_eq!(www["leaf"]["thumbprint"], kv["thumbprint"]);
    let due = ok(&[
        "http",
        "certificate",
        "list",
        "api.contoso.example",
        "www.contoso.example",
        "--expires-within",
        "30d",
        "--fields",
        "id",
    ]);
    assert_eq!(due, json!([{"id": "www.contoso.example:443"}]));
}

#[test]
fn doctor_sees_the_office_network_intercept_tls() {
    let doctor = ok(&["doctor", "http", "--fields", "check,detail"]);
    assert_eq!(doctor[2]["check"], "tls");
    assert!(
        doctor[2]["detail"]
            .as_str()
            .unwrap()
            .starts_with("TLS is intercepted"),
        "{doctor}"
    );
}
