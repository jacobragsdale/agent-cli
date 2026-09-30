//! What the contoso world answers of kv. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_kv() {
    let platform = ok(&[
        "kv",
        "secret",
        "list",
        "--tag",
        "owner=platform",
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
    ] {
        ok(args);
    }
    let expiring = ok(&[
        "kv",
        "secret",
        "list",
        "--expires-within",
        "30d",
        "--fields",
        "id",
    ]);
    assert_eq!(
        expiring,
        json!([{"id": "kv-contoso-prod/db-password"}, {"id": "kv-contoso-prod/worker-db-password"}]),
        "at the world's frozen now"
    );
}
