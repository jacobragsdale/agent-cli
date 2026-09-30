//! What the contoso world answers of ado. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_ado() {
    let queue = ok(&["ado", "pr", "list", "--vote", "none", "--fields", "id"]);
    assert_eq!(queue, json!([]), "nothing waits on your review");
    let urgent = ok(&[
        "ado",
        "workitem",
        "list",
        "--type",
        "Bug",
        "--priority",
        "1,2",
        "--fields",
        "id,priority",
    ]);
    assert_eq!(urgent, json!([{"id": 1218, "priority": 1}]));
    let yours = ok(&[
        "ado",
        "run",
        "list",
        "--requested-by",
        "@me",
        "--fields",
        "id",
    ]);
    assert_eq!(yours, json!([{"id": 8812}, {"id": 8809}, {"id": 8801}]));
    for args in [
        &["ado", "run", "get", "8809", "--fields", "failed"][..],
        &["ado", "run", "logs", "8809"],
        &["ado", "run", "list", "--since", "1d"],
        &["ado", "run", "list", "--branch", "v1.4.2"],
        &["ado", "pr", "get", "431"],
        &["ado", "pr", "list"],
        &["ado", "workitem", "get", "AB#1207"],
        &[
            "ado",
            "workitem",
            "list",
            "--assignee",
            "@me",
            "--iteration",
            "@current",
        ],
        &["ado", "approval", "list"],
        &["ado", "pipeline", "list"],
    ] {
        ok(args);
    }
}
