//! Every http command against real servers: the orders API on eight OpenAPI
//! frameworks, nginx in front of it with good and bad certificates, and three
//! proxies (`scripts/openapi-up.sh`, whose `FINDINGS.md` records what each
//! framework serves). Off unless `AGENT_CLI_TEST_HTTP=1`; start everything
//! with `eval "$(scripts/openapi-up.sh)"`, which exports the `OPENAPI_*`
//! variables read here. A framework not started is skipped.

use std::process::Command;

use serde_json::Value;

struct Live {
    dir: tempfile::TempDir,
}

struct Ran {
    code: i32,
    json: Value,
    stderr: String,
}

fn var(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name}: run eval \"$(scripts/openapi-up.sh)\" first"))
}

impl Live {
    fn new() -> Option<Self> {
        if std::env::var("AGENT_CLI_TEST_HTTP").as_deref() != Ok("1") {
            eprintln!(
                "skipped live http: set AGENT_CLI_TEST_HTTP=1 after eval \"$(scripts/openapi-up.sh)\""
            );
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.toml"), "").unwrap();
        let proxy = format!(
            "[core]\nproxy = \"{}\"\nproxy_user = \"{}\"\nproxy_password_env = \"OPENAPI_PROXY_PASSWORD\"\nno_proxy = [\"localhost\", \"127.0.0.1\"]\n",
            var("OPENAPI_PROXY"),
            var("OPENAPI_PROXY_USER")
        );
        std::fs::write(dir.path().join("proxy.toml"), proxy).unwrap();
        Some(Self { dir })
    }

    fn run_with(&self, config: &str, env: &[(&str, &str)], args: &[&str]) -> Ran {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-cli"));
        command
            .args(args)
            .env("AGENT_CLI_CONFIG", self.dir.path().join(config))
            .env("XDG_STATE_HOME", "/nonexistent/agent-cli-state")
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("https_proxy")
            .env_remove("http_proxy")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .current_dir(self.dir.path());
        for (name, value) in env {
            command.env(name, value);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ran {
            code: output.status.code().unwrap_or(-1),
            json: serde_json::from_str(&stdout).unwrap_or(Value::Null),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Ran {
        self.run_with("empty.toml", &[], args)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let ran = self.run(args);
        assert_eq!(ran.code, 0, "{args:?}: {}", ran.stderr);
        ran.json
    }
}

/// The frameworks, their variable, the spec path FINDINGS.md records and a
/// docs page that names it.
const FRAMEWORKS: &[(&str, &str, &str)] = &[
    ("OPENAPI_FASTAPI_URL", "/openapi.json", "/docs"),
    (
        "OPENAPI_ASPNET_SWASHBUCKLE_URL",
        "/swagger/v1/swagger.json",
        "/swagger",
    ),
    (
        "OPENAPI_ASPNET_OPENAPI_URL",
        "/openapi/v1.json",
        "/scalar/v1",
    ),
    ("OPENAPI_SPRINGDOC_URL", "/v3/api-docs", "/swagger-ui.html"),
    ("OPENAPI_NESTJS_URL", "/api-json", "/api"),
    (
        "OPENAPI_GO_SWAG_URL",
        "/swagger/doc.json",
        "/swagger/index.html",
    ),
    (
        "OPENAPI_DRF_SPECTACULAR_URL",
        "/api/schema/",
        "/api/schema/swagger-ui/",
    ),
    ("OPENAPI_QUARKUS_URL", "/q/openapi", "/q/swagger-ui"),
];

#[test]
fn every_frameworks_spec_is_found_from_its_base_url_and_its_docs_page() {
    let Some(live) = Live::new() else { return };
    for (name, spec, docs) in FRAMEWORKS {
        let Ok(base) = std::env::var(name) else {
            eprintln!("skipped {name}: not started");
            continue;
        };
        let found = live.ok(&["http", "spec", "get", &base, "--no-cache"]);
        assert_eq!(found["id"], format!("{base}{spec}"), "{name}: {found}");
        assert_eq!(found["found"], "probed", "{name}");
        let through = live.ok(&[
            "http",
            "spec",
            "get",
            &format!("{base}{docs}"),
            "--no-cache",
        ]);
        let wanted = if name.contains("SPRINGDOC") {
            format!("{base}/v3/api-docs")
        } else {
            format!("{base}{spec}")
        };
        assert_eq!(through["id"], wanted, "{name} from {docs}: {through}");
        assert_eq!(through["found"], "docs page", "{name}");
        let posts = live.ok(&[
            "http",
            "operation",
            "list",
            &base,
            "--method",
            "POST",
            "--param",
            "customer_id",
            "--fields",
            "id,path",
        ]);
        let create = posts
            .as_array()
            .unwrap()
            .iter()
            .find(|op| {
                op["path"]
                    .as_str()
                    .is_some_and(|path| path.trim_end_matches('/') == "/orders")
            })
            .unwrap_or_else(|| panic!("{name}: no POST /orders taking customer_id in {posts}"));
        let detail = live.ok(&["http", "operation", "get", create["id"].as_str().unwrap()]);
        let fields: Vec<&str> = detail["body"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert!(fields.contains(&"lines[].sku"), "{name}: {fields:?}");
        let schemas = live.ok(&[
            "http", "schema", "list", &base, "--field", "sku", "--fields", "id",
        ]);
        assert!(!schemas.as_array().unwrap().is_empty(), "{name}");
        live.ok(&["http", "schema", "get", schemas[0]["id"].as_str().unwrap()]);
    }
}

#[test]
fn springdocs_groups_are_listed_beside_its_whole_spec() {
    let Some(live) = Live::new() else { return };
    let Ok(base) = std::env::var("OPENAPI_SPRINGDOC_URL") else {
        return;
    };
    let listed = live.ok(&[
        "http",
        "spec",
        "list",
        &base,
        "--no-cache",
        "--fields",
        "id,operations",
    ]);
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            format!("{base}/v3/api-docs"),
            format!("{base}/v3/api-docs/admin"),
            format!("{base}/v3/api-docs/orders")
        ]
    );
}

#[test]
fn version_two_names_each_change_that_breaks_a_version_one_client() {
    let Some(live) = Live::new() else { return };
    let (one, two) = (var("OPENAPI_FASTAPI_URL"), var("OPENAPI_FASTAPI_V2_URL"));
    let diff = live.ok(&["http", "spec", "diff", &one, &two, "--breaking"]);
    let breaking: Vec<&str> = diff["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["breaking"].as_str())
        .collect();
    for wanted in [
        "GET /orders's parameter query status no longer takes on_hold",
        "POST /orders's body has a new required field currency",
        "GET /orders/{}'s 200 answer lost the field note",
        "DELETE /orders/{} is gone",
    ] {
        assert!(breaking.contains(&wanted), "{wanted}: {breaking:#?}");
    }
    let all = live.ok(&["http", "spec", "diff", &one, &two]);
    let unbroken: Vec<&str> = all["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["breaking"].is_null())
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        unbroken,
        [
            "operations.GET /orders.parameters.query customer_id",
            "operations.GET /orders/{}/fulfilment"
        ]
    );
}

#[test]
fn a_call_fails_by_its_status_and_says_what_next() {
    let Some(live) = Live::new() else { return };
    let base = var("OPENAPI_FASTAPI_URL");
    for (path, code) in [
        ("/fail/400", 2),
        ("/fail/401", 3),
        ("/orders/9999", 4),
        ("/fail/409", 5),
        ("/fail/500", 1),
        ("/fail/503", 1),
    ] {
        let ran = live.run(&["http", "request", "get", &format!("{base}{path}")]);
        assert_eq!(ran.code, code, "{path}: {}", ran.stderr);
        assert!(
            ran.json["status"].is_number(),
            "{path}: the answer prints too"
        );
    }
    let traced = live.run(&["http", "request", "get", &format!("{base}/fail/503")]);
    let trace = var("OPENAPI_TRACE_ID");
    let decimal = u64::from_str_radix(&trace[16..], 16).unwrap().to_string();
    assert_eq!(traced.json["trace_id"], decimal.as_str());
    assert!(
        traced
            .stderr
            .contains(&format!("agent-cli dd span list --trace {decimal}")),
        "{}",
        traced.stderr
    );
    // The echo origin, which no other test resets between the 429 and its retry.
    let echo = var("OPENAPI_ECHO_URL");
    let throttled = live.ok(&[
        "http",
        "request",
        "get",
        &format!("{echo}/fail/429?key=live-{}", std::process::id()),
    ]);
    assert_eq!(
        throttled["status"], 200,
        "the 429's Retry-After is waited out"
    );
    let slow = live.run(&[
        "http",
        "request",
        "get",
        &format!("{base}/slow?seconds=5"),
        "--timeout",
        "2",
    ]);
    assert_eq!(slow.code, 124, "{}", slow.stderr);
    let loop_ = live.run(&[
        "http",
        "request",
        "get",
        &format!("{base}/redirect/loop"),
        "--follow",
    ]);
    assert_eq!(loop_.code, 1, "{}", loop_.stderr);
    assert!(loop_.stderr.contains("redirect loop"));
}

#[test]
fn the_sign_in_reaches_its_origin_and_no_further() {
    let Some(live) = Live::new() else { return };
    let base = var("OPENAPI_FASTAPI_URL");
    assert_eq!(
        live.run(&["http", "request", "get", &format!("{base}/secure/orders")])
            .code,
        3
    );
    live.ok(&[
        "http",
        "request",
        "get",
        &format!("{base}/secure/orders"),
        "--bearer-env",
        "OPENAPI_TOKEN",
    ]);
    let basic = format!("{}:OPENAPI_BASIC_PASSWORD", var("OPENAPI_BASIC_USER"));
    live.ok(&[
        "http",
        "request",
        "get",
        &format!("{base}/basic"),
        "--basic",
        &basic,
    ]);
    live.ok(&[
        "http",
        "request",
        "get",
        &format!("{base}/apikey"),
        "--header-env",
        "X-Api-Key=OPENAPI_API_KEY",
    ]);
    let away = live.ok(&[
        "http",
        "request",
        "get",
        &format!("{base}/redirect/external"),
        "--follow",
        "--bearer-env",
        "OPENAPI_TOKEN",
    ]);
    assert_eq!(away["hops"][0]["unsigned"], true);
    assert!(away["body"]["headers"]["authorization"].is_null(), "{away}");
    let token = live.ok(&["http", "request", "run", "POST", &format!("{base}/token")]);
    assert_eq!(token["body"]["access_token"], "***");
}

#[test]
fn writes_are_planned_guarded_and_checked_against_the_spec() {
    let Some(live) = Live::new() else { return };
    let (one, two) = (var("OPENAPI_FASTAPI_URL"), var("OPENAPI_FASTAPI_V2_URL"));
    live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{one}/admin/reset"),
    ]);
    let plan = live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{one}/orders"),
        "--body",
        r#"{"customer_id": 7, "lines": []}"#,
        "--dry-run",
    ]);
    assert_eq!(plan["dry_run"], true);
    let made = live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{one}/orders"),
        "--body",
        r#"{"customer_id": 7, "lines": [{"sku": "SKU-1", "qty": 2}]}"#,
    ]);
    assert_eq!(made["status"], 201);
    let id = made["body"]["id"].to_string();
    assert_eq!(
        live.run(&[
            "http",
            "request",
            "run",
            "DELETE",
            &format!("{one}/orders/{id}")
        ])
        .code,
        2,
        "DELETE needs --yes"
    );
    live.ok(&[
        "http",
        "request",
        "run",
        "DELETE",
        &format!("{one}/orders/{id}"),
        "--yes",
    ]);
    let refused = live.run(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{two}/orders"),
        "--spec",
        &two,
        "--body",
        r#"{"customer_id": 7, "lines": [{"qty": 1}], "currency": "GBP"}"#,
    ]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert!(
        refused
            .stderr
            .contains("body.currency is \"GBP\", not one of EUR, USD"),
        "{}",
        refused.stderr
    );
    live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{two}/orders"),
        "--spec",
        &two,
        "--body",
        r#"{"customer_id": 7, "lines": [{"sku": "S", "qty": 1}], "currency": "EUR"}"#,
    ]);
    live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{one}/admin/reset"),
    ]);
}

#[test]
fn a_deploy_is_waited_for_until_it_serves_its_version() {
    let Some(live) = Live::new() else { return };
    // v2, which no other test resets while this one waits.
    let base = var("OPENAPI_FASTAPI_V2_URL");
    live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{base}/admin/deploy"),
        "--query",
        "version=2.0.1",
        "--query",
        "after=3",
    ]);
    let waited = live.ok(&[
        "http",
        "request",
        "wait",
        &format!("{base}/healthz"),
        "--expect-field",
        "version=2.0.1",
        "--every",
        "1",
    ]);
    assert!(waited["tries"].as_u64().unwrap() > 1, "{waited}");
    live.ok(&[
        "http",
        "request",
        "run",
        "POST",
        &format!("{base}/admin/reset"),
    ]);
}

#[test]
fn certificates_are_read_and_judged_as_rustls_judges_them() {
    let Some(live) = Live::new() else { return };
    let ca = var("OPENAPI_CA");
    let cases = [
        ("OPENAPI_TLS_URL", None),
        ("OPENAPI_TLS_INTERMEDIATE_URL", None),
        ("OPENAPI_TLS_EXPIRED_URL", Some("it has expired")),
        (
            "OPENAPI_TLS_SELF_SIGNED_URL",
            Some("it is signed by a CA this machine does not trust"),
        ),
        (
            "OPENAPI_TLS_WRONG_HOST_URL",
            Some("it does not name localhost"),
        ),
        (
            "OPENAPI_TLS_CA_AS_LEAF_URL",
            Some("the server presents a CA's certificate as its own"),
        ),
    ];
    for (name, refusal) in cases {
        let probe = live.ok(&["http", "certificate", "get", &var(name), "--cacert", &ca]);
        assert_eq!(probe["trusted"], refusal.is_none(), "{name}: {probe}");
        assert_eq!(probe["refusal"].as_str(), refusal, "{name}");
    }
    let tls = var("OPENAPI_TLS_URL");
    assert_eq!(
        live.run(&["http", "request", "get", &format!("{tls}/healthz")])
            .code,
        3,
        "untrusted without --cacert"
    );
    live.ok(&[
        "http",
        "request",
        "get",
        &format!("{tls}/healthz"),
        "--cacert",
        &ca,
    ]);
    let mtls = var("OPENAPI_MTLS_URL");
    assert_eq!(
        live.run(&[
            "http",
            "request",
            "get",
            &format!("{mtls}/healthz"),
            "--cacert",
            &ca
        ])
        .code,
        2
    );
    live.ok(&[
        "http",
        "request",
        "get",
        &format!("{mtls}/healthz"),
        "--cacert",
        &ca,
        "--cert",
        &format!(
            "{},{}",
            var("OPENAPI_CLIENT_CERT"),
            var("OPENAPI_CLIENT_KEY")
        ),
    ]);
    let listed = live.ok(&[
        "http",
        "certificate",
        "list",
        "localhost:58443",
        "localhost:58445",
        "--cacert",
        &ca,
        "--fields",
        "id,days_left",
    ]);
    assert_eq!(listed[0]["id"], "localhost:58445", "soonest expiry first");
}

#[test]
fn requests_and_probes_go_through_the_proxy_and_its_refusals_say_why() {
    let Some(live) = Live::new() else { return };
    let ca = var("OPENAPI_CA");
    let behind = "https://api.contoso.test:58443/healthz";
    let password = var("OPENAPI_PROXY_PASSWORD");
    let through = live.run_with(
        "proxy.toml",
        &[("OPENAPI_PROXY_PASSWORD", &password)],
        &["http", "request", "get", behind, "--cacert", &ca],
    );
    assert_eq!(through.code, 0, "{}", through.stderr);
    assert!(
        through.json["proxy"]
            .as_str()
            .unwrap()
            .ends_with("([core] proxy)")
    );
    let probe = live.run_with(
        "proxy.toml",
        &[("OPENAPI_PROXY_PASSWORD", &password)],
        &[
            "http",
            "certificate",
            "get",
            "api.contoso.test:58449",
            "--cacert",
            &ca,
        ],
    );
    assert_eq!(probe.code, 0, "{}", probe.stderr);
    assert_eq!(
        probe.json["chain"][0]["subject"],
        "CN=intermediate, O=Contoso"
    );
    let unsigned = live.run_with(
        "empty.toml",
        &[("HTTPS_PROXY", &var("OPENAPI_PROXY"))],
        &["http", "request", "get", behind, "--cacert", &ca],
    );
    assert_eq!(unsigned.code, 3, "{}", unsigned.stderr);
    assert!(
        unsigned.stderr.contains("the proxy wants a sign-in"),
        "{}",
        unsigned.stderr
    );
    let negotiate = live.run_with(
        "empty.toml",
        &[("HTTPS_PROXY", &var("OPENAPI_PROXY_NEGOTIATE"))],
        &["http", "request", "get", behind, "--cacert", &ca],
    );
    assert_eq!(negotiate.code, 3, "{}", negotiate.stderr);
    assert!(
        negotiate.stderr.contains("run px or cntlm"),
        "{}",
        negotiate.stderr
    );
    let exempt = live.run_with(
        "empty.toml",
        &[
            ("HTTPS_PROXY", &var("OPENAPI_PROXY_NEGOTIATE")),
            ("NO_PROXY", "localhost"),
        ],
        &[
            "http",
            "request",
            "get",
            &format!("{}/healthz", var("OPENAPI_TLS_URL")),
            "--cacert",
            &ca,
        ],
    );
    assert_eq!(exempt.code, 0, "NO_PROXY goes direct: {}", exempt.stderr);
    let doctor = live.run_with(
        "proxy.toml",
        &[("OPENAPI_PROXY_PASSWORD", &password)],
        &["doctor"],
    );
    let rows = doctor.json.as_array().unwrap();
    assert!(
        rows.iter().any(|row| row["check"] == "proxy"
            && row["detail"].as_str().unwrap().contains("signed in as")),
        "{rows:?}"
    );
}
