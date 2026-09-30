//! What the contoso world answers of aks. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_aks() {
    ok(&["aks", "cluster", "list"]);
}
