//! What the contoso world answers of acr. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_acr() {
    let stale = ok(&["acr", "repo", "list", "--until", "30d", "--fields", "id"]);
    assert_eq!(stale, json!([]), "both images were pushed on 09-28");
    ok(&["acr", "repo", "list"]);
}
