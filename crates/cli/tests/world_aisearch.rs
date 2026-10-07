//! What the contoso world answers of aisearch. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{agent_cli, follow, ok};

#[test]
fn the_world_answers_what_a_trial_asks_of_aisearch() {
    assert_eq!(
        ok(&["aisearch", "service", "list", "--fields", "id,auth"]),
        json!([{"id": "srch-contoso-prod", "auth": "token"}, {"id": "srch-contoso-dev", "auth": "key"}])
    );
    for args in [
        &["aisearch", "service", "get", "srch-contoso-prod"][..],
        &["aisearch", "service", "get", "srch-contoso-dev"],
        &[
            "aisearch",
            "index",
            "get",
            "srch-contoso-prod/orders",
            "--full",
        ],
        &["aisearch", "index", "get", "srch-contoso-dev/products"],
        &["aisearch", "document", "list", "srch-contoso-dev/products"],
        &[
            "aisearch",
            "document",
            "get",
            "srch-contoso-prod/orders/88121",
        ],
        &["aisearch", "indexer", "list"],
    ] {
        ok(args);
    }
    assert_eq!(
        ok(&["aisearch", "index", "list", "--fields", "id,documents"]),
        json!([{"id": "srch-contoso-prod/orders", "documents": 88120}, {"id": "srch-contoso-dev/products", "documents": 1200}])
    );
    let hits = ok(&[
        "aisearch",
        "document",
        "list",
        "orders",
        "late delivery",
        "--mode",
        "hybrid",
        "--fields",
        "id,doc",
    ]);
    assert_eq!(hits[0]["id"], "srch-contoso-prod/orders/88122");
    assert_eq!(hits[0]["doc"]["summary_vector"], "[1536 floats]");
}

#[test]
fn a_missing_document_leads_to_the_indexer_that_failed_it() {
    let missing = agent_cli(&[
        "aisearch",
        "document",
        "get",
        "srch-contoso-prod/orders/88123",
    ]);
    assert_eq!(missing.code, 4, "{}", missing.stderr);
    assert!(
        missing.stderr.contains(
            "hint: agent-cli aisearch indexer list --service srch-contoso-prod --failing"
        ),
        "{}",
        missing.stderr
    );
    assert_eq!(
        ok(&[
            "aisearch",
            "indexer",
            "list",
            "--service",
            "srch-contoso-prod",
            "--failing",
            "--fields",
            "id,failed"
        ]),
        json!([{"id": "srch-contoso-prod/orders-sql", "failed": 1}])
    );
    let walked = follow(&[
        "aisearch",
        "indexer",
        "get",
        "srch-contoso-prod/orders-sql",
        "--fields",
        "errors",
    ]);
    assert_eq!(walked.len(), 2, "indexer get names document get");
    assert!(
        walked[0].json()["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("customer_id"),
        "{}",
        walked[0].stdout
    );
    assert_eq!(walked[1].code, 4, "88123 is not in the index");
}

#[test]
fn the_world_plans_every_write_without_making_it() {
    for args in [
        &[
            "aisearch",
            "indexer",
            "run",
            "srch-contoso-prod/orders-sql",
            "--reset",
            "--dry-run",
        ][..],
        &[
            "aisearch",
            "index",
            "delete",
            "srch-contoso-dev/products",
            "--dry-run",
        ],
        &[
            "aisearch",
            "document",
            "delete",
            "srch-contoso-prod/orders/88121",
            "--dry-run",
        ],
        &[
            "aisearch",
            "document",
            "update",
            "srch-contoso-dev/products",
            "--docs",
            r#"{"sku": "TENT-2P", "category": "tents"}"#,
            "--dry-run",
        ],
    ] {
        let planned = ok(args);
        assert_eq!(planned["dry_run"], true, "{args:?}");
        assert!(
            !planned.to_string().contains("fixture-admin-key"),
            "{planned}"
        );
    }
}
