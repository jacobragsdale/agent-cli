//! What the contoso world answers of acr. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{follow, ok};

#[test]
fn the_world_answers_what_a_trial_asks_of_acr() {
    let stale = ok(&["acr", "repo", "list", "--until", "30d", "--fields", "id"]);
    assert_eq!(stale, json!([]), "every image was pushed in September");
    ok(&["acr", "repo", "list"]);
    let registry = ok(&["acr", "registry", "get", "contosoacr"]);
    assert_eq!(registry["sku"], "Standard");
    assert_eq!(registry["admin_user"], false);
    // What k8s deployment get reports-api prints, phase 4's missing image.
    let reports = ok(&[
        "acr",
        "manifest",
        "get",
        "contosoacr.azurecr.io/reports:v0.9.3",
    ]);
    assert_eq!(reports["tags"], json!(["v0.9.3"]));
}

/// The deploy trace's image: api-ci run 8812 built `api:v1.4.2` with an
/// `az acr build`, and the run's image is the tag the pods run.
#[test]
fn the_run_that_built_api_v1_4_2_names_the_image_the_pods_run() {
    let runs = ok(&["acr", "run", "list", "--fields", "id,status,images"]);
    let built = runs
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["images"] == json!(["contosoacr.azurecr.io/api:v1.4.2"]))
        .unwrap_or_else(|| panic!("{runs}"));
    assert_eq!(built["id"], "contosoacr/ca1e");
    let manifest = ok(&["acr", "manifest", "get", "contosoacr.azurecr.io/api:v1.4.2"]);
    assert_eq!(
        manifest["digest"],
        "sha256:7a72fa7948c3b5e299c40bdb1ebeb125e313c32d4262e130a8206115054663d3"
    );
}

#[test]
fn a_failed_run_leads_to_its_log_and_the_sas_never_shows() {
    let walked = follow(&["acr", "run", "get", "contosoacr/ca1g"]);
    assert_eq!(walked.len(), 2, "run get, then run logs");
    let log = walked[1].json();
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("Assert.Equal() Failure"),
        "{log}"
    );
    for ran in &walked {
        assert!(
            !ran.stdout.contains("sig=") && !ran.stderr.contains("sig="),
            "{}{}",
            ran.stdout,
            ran.stderr
        );
    }
}
