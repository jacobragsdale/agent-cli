//! What the contoso world answers of keeper. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use agent_cli_core::testing::next_command;
use serde_json::json;
use world::{agent_cli, ok};

#[test]
fn the_world_answers_what_a_trial_asks_of_keeper() {
    for args in [
        &["keeper", "repo", "list"][..],
        &["keeper", "repo", "get", "pypi-all"],
        &[
            "keeper",
            "repo",
            "get",
            "https://keeper.contoso.example/pypi/pypi-all/simple/",
        ],
        &["keeper", "repo", "get", "pypi-proxy"],
        &["keeper", "package", "list"],
        &["keeper", "package", "list", "contoso"],
        &["keeper", "package", "list", "--repo", "pypi-proxy"],
        &["keeper", "package", "get", "pypi/contoso-orders-client"],
        &["keeper", "package", "get", "pypi-all/contoso-orders-client"],
        &["keeper", "package", "get", "pypi/contoso-etl-utils"],
        &["keeper", "package", "get", "builds/api"],
        &["keeper", "package", "get", "pypi-proxy/pyarrow"],
        &[
            "keeper",
            "version",
            "get",
            "pypi/contoso-orders-client@2.3.0",
        ],
        &[
            "keeper",
            "version",
            "get",
            "pypi/contoso-orders-client@2.2.0",
        ],
        &[
            "keeper",
            "version",
            "get",
            "pypi-all/contoso-orders-client@2.3.0",
        ],
        &["keeper", "version", "get", "builds/api@1.4.2"],
        &["keeper", "version", "get", "builds/api@1.4.1"],
        &["keeper", "version", "get", "pypi-proxy/pyarrow@17.0.0"],
        &["keeper", "version", "get", "pypi-proxy/pyarrow@16.1.0"],
        &["keeper", "file", "list", "builds"],
        &["keeper", "file", "list", "builds/api/1.4.2/"],
        &[
            "keeper",
            "file",
            "get",
            "builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz",
        ],
        &[
            "keeper",
            "file",
            "get",
            "pypi-proxy/simple/pyarrow/pyarrow-16.1.0-cp311-cp311-manylinux_2_28_x86_64.whl",
        ],
    ] {
        ok(args);
    }
}

#[test]
fn the_newest_internal_package_and_who_published_it() {
    let package = ok(&[
        "keeper",
        "package",
        "list",
        "contoso",
        "--fields",
        "id,latest,updated",
    ]);
    assert_eq!(package[0]["id"], "pypi/contoso-orders-client");
    let package = ok(&["keeper", "package", "get", "pypi/contoso-orders-client"]);
    assert_eq!(package["latest"], "2.3.0");
    assert_eq!(package["versions"][0]["created"], "2026-09-28T21:31:10Z");
    let id = package["versions"][0]["id"].as_str().unwrap();
    let version = ok(&[
        "keeper",
        "version",
        "get",
        id,
        "--fields",
        "published_by,sha256",
    ]);
    assert_eq!(version["published_by"], "svc-ado-pipelines");
}

#[test]
fn the_build_behind_the_running_image_is_in_keeper() {
    let build = ok(&["keeper", "version", "get", "builds/api@1.4.2"]);
    assert_eq!(build["created"], "2026-09-28T21:30:05Z");
    let tarball = build["files"][0]["id"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("api.tar.gz");
    let file = ok(&[
        "keeper",
        "file",
        "get",
        tarball,
        "--output",
        saved.to_str().unwrap(),
    ]);
    assert_eq!(file["saved"], saved.to_str().unwrap());
    assert!(
        std::fs::read_to_string(&saved)
            .unwrap()
            .starts_with("stand-in for the api 1.4.2")
    );
}

#[test]
fn pip_behind_the_proxy_misses_an_internal_package_and_the_hint_finds_it() {
    let pyarrow = ok(&[
        "keeper",
        "package",
        "get",
        "pypi-proxy/pyarrow",
        "--fields",
        "versions",
    ]);
    assert_eq!(
        pyarrow["versions"][0],
        json!({"id": "pypi-proxy/pyarrow@17.0.0", "version": "17.0.0",
            "created": "2024-07-17T10:43:22Z", "cached": false, "requires_python": ">=3.9"})
    );
    let missing = agent_cli(&["keeper", "package", "get", "pypi-proxy/contoso-etl-utils"]);
    assert_eq!(missing.code, 4, "{}", missing.stderr);
    assert!(
        missing
            .stderr
            .contains("https://pypi.org has no contoso-etl-utils")
    );
    let hint = missing
        .stderr
        .lines()
        .find_map(|line| line.strip_prefix("hint: agent-cli "))
        .unwrap();
    assert_eq!(hint, "keeper package list contoso-etl-utils");
    let found = ok(&hint.split(' ').collect::<Vec<_>>());
    assert_eq!(found[0]["id"], "pypi/contoso-etl-utils");
    assert_eq!(next_command(&missing.stderr), None);
}
