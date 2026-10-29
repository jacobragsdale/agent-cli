use serde_json::json;

use super::*;

const DEPLOYMENT: Rules = Rules {
    keys: &[("containers", "name"), ("containers.*.env", "name")],
    noise: &["metadata.uid", "status"],
    ..Rules::PLAIN
};

fn changes(rules: &Rules, args: &DiffArgs, left: Side, right: Side) -> Value {
    let pairs = names(args, rules, &left.id, &right.id).unwrap();
    let (changes, _) = compare(args, rules, &pairs, &left, &right);
    serde_json::to_value(changes).unwrap()
}

fn args() -> DiffArgs {
    DiffArgs {
        limit: 50,
        ..DiffArgs::default()
    }
}

#[test]
fn lists_match_by_their_key_and_noise_and_empties_do_not_count() {
    let left = json!({"metadata": {"uid": "1"}, "status": {"ready": 3}, "replicas": 3,
        "containers": [{"name": "sidecar", "image": "s:1"}, {"name": "api", "image": "api:v1.4.1",
            "env": [{"name": "A", "value": "1"}, {"name": "DB_HOST", "value": "db.staging.contoso.example"}]}],
        "tolerations": []});
    let right = json!({"metadata": {"uid": "2"}, "replicas": 3,
        "containers": [{"name": "api", "image": "api:v1.4.2",
            "env": [{"name": "DB_HOST", "value": "db-prod.contoso.example"}, {"name": "A", "value": "1"}]},
            {"name": "sidecar", "image": "s:1"}]});
    let got = changes(
        &DEPLOYMENT,
        &args(),
        Side::new("staging/web/api", left.clone()),
        Side::new("prod/web/api", right.clone()),
    );
    assert_eq!(
        got,
        json!([
            {"path": "containers.api.image", "left": "api:v1.4.1", "right": "api:v1.4.2", "same": null, "lines": []},
            {"path": "containers.api.env.DB_HOST.value", "left": "db.staging.contoso.example",
                "right": "db-prod.contoso.example", "same": null, "lines": []},
        ])
    );
    let all = DiffArgs {
        all: true,
        ignore: vec!["containers.*.env".into()],
        ..args()
    };
    let got = changes(
        &DEPLOYMENT,
        &all,
        Side::new("a", left),
        Side::new("b", right),
    );
    let paths: Vec<&str> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["metadata.uid", "status", "containers.api.image"]);
}

#[test]
fn a_list_of_plain_values_differs_as_one_list_with_the_names_lined_up() {
    let left = json!({"tags": ["etl", "e2e"], "args": ["--host", "db.qa.contoso.example"],
        "ports": [{"port": 80}, {"port": 443}]});
    let right = json!({"tags": ["e2e"], "args": ["--host", "db.prod.contoso.example"],
        "ports": [{"port": 80}, {"port": 8443}]});
    let got = changes(
        &Rules::PLAIN,
        &args(),
        Side::new("qa:api", left),
        Side::new("prod:api", right),
    );
    assert_eq!(
        got,
        json!([
            {"path": "tags", "left": ["etl", "e2e"], "right": ["e2e"], "same": null, "lines": []},
            {"path": "ports.1.port", "left": 443, "right": 8443, "same": null, "lines": []},
        ])
    );
}

#[test]
fn keys_that_name_the_environment_pair_up_and_a_nested_key_field_is_followed() {
    let rules = Rules {
        keys: &[("rules", "host"), ("claims", "metadata.name")],
        ..Rules::PLAIN
    };
    let left = json!({"rules": [{"host": "api.staging.contoso.example", "path": "/"}],
        "data": {"appsettings.staging.json": "{}"},
        "claims": [{"metadata": {"name": "data"}, "size": "1Gi"}]});
    let right = json!({"rules": [{"host": "api.prod.contoso.example", "path": "/api"}],
        "data": {"appsettings.prod.json": "{}"},
        "claims": [{"metadata": {"name": "data"}, "size": "2Gi"}]});
    let got = changes(
        &rules,
        &args(),
        Side::new("staging/web/api", left),
        Side::new("prod/web/api", right),
    );
    assert_eq!(
        got,
        json!([
            {"path": "rules.api.staging.contoso.example.path", "left": "/", "right": "/api",
                "same": null, "lines": []},
            {"path": "claims.data.size", "left": "1Gi", "right": "2Gi", "same": null, "lines": []},
        ])
    );
    // A key on both sides pairs with itself before another lines up with it.
    let got = changes(
        &rules,
        &args(),
        Side::new("staging/web/api", json!({"x-staging": 1, "x-prod": 2})),
        Side::new("prod/web/api", json!({"x-prod": 2})),
    );
    assert_eq!(
        got,
        json!([{"path": "x-staging", "left": 1, "right": null, "same": null, "lines": []}])
    );
}

#[test]
fn environment_names_come_from_the_ids_and_apply_only_where_they_stand_alone() {
    let rules = Rules::PLAIN;
    let none = args();
    assert_eq!(
        names(&none, &rules, "staging/web/api", "prod/web/api").unwrap(),
        [("staging".to_owned(), "prod".to_owned())]
    );
    assert!(
        names(&none, &rules, "job-29312", "job-29313")
            .unwrap()
            .is_empty()
    );
    assert!(
        names(&none, &rules, "dbo.orders", "prod:dbo.orders")
            .unwrap()
            .is_empty()
    );
    let runs = Rules {
        infer: false,
        ..Rules::PLAIN
    };
    assert!(names(&none, &runs, "etl/a", "etl/b").unwrap().is_empty());
    let off = DiffArgs {
        as_: vec!["none".into()],
        ..args()
    };
    assert!(names(&off, &rules, "qa:x", "prod:x").unwrap().is_empty());
    let chosen = DiffArgs {
        as_: vec!["db-qa=dbprod".into()],
        ..args()
    };
    assert_eq!(names(&chosen, &rules, "a", "b").unwrap()[0].1, "dbprod");
    let bad = DiffArgs {
        as_: vec!["qa".into()],
        ..args()
    };
    assert!(names(&bad, &rules, "a", "b").is_err());

    let pairs = [("qa".to_owned(), "prod".to_owned())];
    assert_eq!(
        swap("db-qa.contoso.example/qa_x qa", &pairs),
        "db-prod.contoso.example/prod_x prod"
    );
    assert_eq!(swap("quality aqa qa1 qaqa", &pairs), "quality aqa qa1 qaqa");
    // Oracle folds `shop_qa` to `SHOP_QA`: one name in either case.
    assert_eq!(swap("SHOP_QA.ORDERS Qa", &pairs), "SHOP_PROD.ORDERS prod");
    assert_eq!(
        names(
            &none,
            &rules,
            "ora-qa:SHOP_QA.ORDERS",
            "ora-prod:SHOP_PROD.ORDERS"
        )
        .unwrap(),
        [("qa".to_owned(), "prod".to_owned())]
    );
    let got = changes(
        &rules,
        &none,
        Side::new(
            "kv-contoso-staging/db",
            json!({"url": "https://kv-contoso-staging.vault.azure.net/"}),
        ),
        Side::new(
            "kv-contoso-prod/db",
            json!({"url": "https://kv-contoso-prod.vault.azure.net/"}),
        ),
    );
    assert_eq!(got, json!([]));
}

#[test]
fn json_strings_compare_as_json_and_other_texts_by_lines_without_comments() {
    let got = changes(
        &Rules::PLAIN,
        &args(),
        Side::new(
            "a",
            json!({"settings": "{\"Retry\": 3, \"Log\": {\"Level\": \"Info\"}}"}),
        ),
        Side::new(
            "b",
            json!({"settings": "{\"Log\": {\"Level\": \"Debug\"},\n \"Retry\": 3}"}),
        ),
    );
    assert_eq!(got[0]["path"], "settings.Log.Level");
    assert_eq!(got.as_array().unwrap().len(), 1);

    let sql = Rules {
        comments: &["--", "/*"],
        fold_case: true,
        ..Rules::PLAIN
    };
    let left = "CREATE PROCEDURE p AS\n  SELECT a  FROM t -- old\n/* note\n more */\nRETURN";
    let right = "create procedure p as\nselect a from t\n\n\nreturn";
    let text = |value: &str| json!({ "text": value });
    let same = changes(
        &sql,
        &args(),
        Side::new("p", text(left)),
        Side::new("p", text(right)),
    );
    assert_eq!(same, json!([]));
    let exact = DiffArgs {
        exact: true,
        ..args()
    };
    let got = changes(
        &sql,
        &exact,
        Side::new("p", text(left)),
        Side::new("p", text(right)),
    );
    assert_eq!(got[0]["path"], "text");
    assert_eq!(got[0]["left"], Value::Null);
    assert_eq!(got[0]["lines"][0], "@@ -1,5 +1,5 @@");
    let changed = changes(
        &sql,
        &args(),
        Side::new("p", text(left)),
        Side::new("p", text(&right.replace(" a ", " b "))),
    );
    assert_eq!(
        changed[0]["lines"],
        json!([
            "@@ -1,5 +1,5 @@",
            " CREATE PROCEDURE p AS",
            "-  SELECT a  FROM t -- old",
            "+select b from t",
            " /* note",
            "  more */",
            " RETURN"
        ])
    );
}

#[test]
fn secrets_say_only_whether_they_are_the_same() {
    let mut left = Side::new("staging/web/db", json!({"type": "Opaque"}));
    let mut right = Side::new("prod/web/db", json!({"type": "Opaque"}));
    let mut both = |path: &str, a: Option<&str>, b: Option<&str>| {
        left.secrets.insert(path.into(), a.map(Secret::new));
        right.secrets.insert(path.into(), b.map(Secret::new));
    };
    both("data.password", Some("same-value-1"), Some("same-value-1"));
    both("data.api-key", Some("left-key-123"), Some("right-key-456"));
    both("data.token", None, None);
    right
        .secrets
        .insert("data.extra".into(), Some(Secret::new("extra-value")));
    let (got, masked) = compare(&args(), &Rules::PLAIN, &[], &left, &right);
    assert_eq!(masked, 1);
    let got = serde_json::to_value(got).unwrap();
    assert_eq!(
        got,
        json!([
            {"path": "data.api-key", "left": null, "right": null, "redacted": true, "same": false, "lines": []},
            {"path": "data.extra", "left": null, "right": "(set)", "redacted": true, "same": null, "lines": []},
            {"path": "data.token", "left": null, "right": null, "redacted": true, "same": null, "lines": []},
        ])
    );
    assert!(!got.to_string().contains("key-"), "{got}");
}

#[test]
fn a_scope_prefix_is_split_off_but_a_run_ids_colon_is_not() {
    assert_eq!(scoped("qa:dbo.orders"), (Some("qa"), "dbo.orders"));
    assert_eq!(scoped("dbo.orders"), (None, "dbo.orders"));
    assert_eq!(
        scoped("etl/scheduled__2026-09-28T00:00:00+00:00"),
        (None, "etl/scheduled__2026-09-28T00:00:00+00:00")
    );
    assert_eq!(
        scoped("qa:etl/scheduled__2026-09-28T00:00:00+00:00"),
        (Some("qa"), "etl/scheduled__2026-09-28T00:00:00+00:00")
    );
}

#[test]
fn a_breaking_rule_marks_the_changes_it_names() {
    fn gone(change: &Change) -> Option<String> {
        change
            .right
            .is_none()
            .then(|| format!("{} is gone", change.path))
    }
    let rules = Rules {
        breaking: Some(gone),
        ..Rules::PLAIN
    };
    let ctx = crate::testing::ctx(crate::ctx::Setup::fake(
        crate::testing::FakeTransport::default(),
    ));
    let compared = diff(
        &ctx,
        &args(),
        &rules,
        Side::new("a", json!({"x": 1, "y": 1})),
        Side::new("b", json!({"y": 2})),
    )
    .unwrap();
    let marked: Vec<(&str, Option<&str>)> = compared
        .changes
        .iter()
        .map(|c| (c.path.as_str(), c.breaking.as_deref()))
        .collect();
    assert_eq!(marked, [("x", Some("x is gone")), ("y", None)]);
}
