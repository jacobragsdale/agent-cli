//! Every keeper command against a live Artifact Keeper, through the built
//! binary, anonymously and with a token. Off unless `AGENT_CLI_TEST_KEEPER=1`;
//! start the server with `eval "$(scripts/keeper-up.sh)"`, which seeds what
//! this reads and exports `KEEPER_TOKEN`. `AGENT_CLI_TEST_KEEPER_URL` points it
//! elsewhere. It only reads, but for the files the proxy fetches from
//! pypi.org; the proxy's checks are skipped when keeper-up found pypi.org
//! unreachable.

use std::process::Command;

use serde_json::Value;

struct Live {
    dir: tempfile::TempDir,
    url: String,
    token: String,
}

impl Live {
    fn new() -> Option<Self> {
        if std::env::var("AGENT_CLI_TEST_KEEPER").as_deref() != Ok("1") {
            eprintln!(
                "skipped live keeper: set AGENT_CLI_TEST_KEEPER=1 after eval \"$(scripts/keeper-up.sh)\""
            );
            return None;
        }
        let url = std::env::var("AGENT_CLI_TEST_KEEPER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18070".to_owned());
        let token = std::env::var("KEEPER_TOKEN")
            .expect("KEEPER_TOKEN: run eval \"$(scripts/keeper-up.sh)\" first");
        let dir = tempfile::tempdir().unwrap();
        let signed =
            format!("[keeper]\nurl = \"{url}\"\ntoken_env = \"AGENT_CLI_TEST_KEEPER_TOKEN\"\n");
        std::fs::write(dir.path().join("token.toml"), signed).unwrap();
        std::fs::write(
            dir.path().join("anonymous.toml"),
            format!("[keeper]\nurl = \"{url}\"\n"),
        )
        .unwrap();
        Some(Self { dir, url, token })
    }

    /// `agent-cli ARGS` with the token (or none): exit, stdout as JSON, stderr.
    fn run(&self, signed: bool, token: &str, args: &[&str]) -> (i32, Value, String) {
        let config = if signed {
            "token.toml"
        } else {
            "anonymous.toml"
        };
        let output = Command::new(env!("CARGO_BIN_EXE_agent-cli"))
            .args(args)
            .env("AGENT_CLI_CONFIG", self.dir.path().join(config))
            // Never a person's usage log or denied.json.
            .env("XDG_STATE_HOME", "/nonexistent/agent-cli-state")
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env("AGENT_CLI_TEST_KEEPER_TOKEN", token)
            .current_dir(self.dir.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            !stdout.contains(&self.token) && !stderr.contains(&self.token),
            "the token leaked: agent-cli {}",
            args.join(" ")
        );
        let printed = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        (output.status.code().unwrap_or(-1), printed, stderr)
    }

    /// `agent-cli keeper ARGS`, which must exit `code`.
    fn exits(&self, signed: bool, code: i32, args: &[&str]) -> (Value, String) {
        let mut argv = vec!["keeper"];
        argv.extend(args);
        let (got, printed, stderr) = self.run(signed, &self.token, &argv);
        assert_eq!(
            got,
            code,
            "agent-cli {}\n{printed}\n{stderr}",
            argv.join(" ")
        );
        (printed, stderr)
    }

    fn ok(&self, signed: bool, args: &[&str]) -> Value {
        self.exits(signed, 0, args).0
    }
}

fn ids(rows: &Value) -> Vec<&str> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect()
}

#[test]
fn doctor_names_the_server_and_how_it_signs_in() {
    let Some(live) = Live::new() else { return };
    for (signed, access) in [
        (true, "signed in as admin"),
        (false, "anonymous reads allowed"),
    ] {
        let (code, rows, stderr) = live.run(signed, &live.token, &["doctor", "keeper"]);
        assert_eq!(code, 0, "{rows}\n{stderr}");
        let detail = |check: &str| {
            rows.as_array()
                .unwrap()
                .iter()
                .find(|row| row["domain"] == "keeper" && row["check"] == check)
                .unwrap()["detail"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(detail("server").starts_with("Artifact Keeper 1."), "{rows}");
        assert!(detail("access").starts_with(access), "{rows}");
    }
    let (code, rows, _) = live.run(true, "nope_0000", &["doctor", "keeper"]);
    assert_eq!(code, 1, "a refused token fails doctor: {rows}");
}

#[test]
fn repositories_list_and_show_by_key_and_url() {
    let Some(live) = Live::new() else { return };
    let all = live.ok(true, &["repo", "list"]);
    for key in [
        "agent-cli",
        "builds",
        "internal",
        "pypi",
        "pypi-proxy",
        "pypi-all",
    ] {
        assert!(ids(&all).contains(&key), "{key}: {all}");
    }
    let public = live.ok(false, &["repo", "list"]);
    assert!(!ids(&public).contains(&"internal"), "{public}");
    let proxies = live.ok(
        false,
        &["repo", "list", "--type", "remote", "--format", "pypi"],
    );
    assert_eq!(ids(&proxies), ["pypi-proxy"]);
    assert_eq!(proxies[0]["upstream"], "https://pypi.org");

    let index = format!("{}/pypi/pypi-all/simple/", live.url);
    let virtual_repo = live.ok(true, &["repo", "get", &index]);
    assert_eq!(virtual_repo["type"], "virtual");
    assert_eq!(ids(&virtual_repo["members"]), ["pypi", "pypi-proxy"]);
    let (anonymous, said) = live.exits(false, 0, &["repo", "get", "pypi-all"]);
    assert!(
        anonymous.get("members").is_none() && said.contains("need a token"),
        "{said}"
    );

    let proxy = live.ok(
        false,
        &[
            "repo",
            "get",
            &format!("{}/repositories/pypi-proxy", live.url),
        ],
    );
    assert!(proxy["cache_ttl"].as_u64().is_some(), "{proxy}");
    let (_, said) = live.exits(false, 4, &["repo", "get", "internal"]);
    assert!(said.contains("it is private"), "{said}");
    assert_eq!(
        live.ok(true, &["repo", "get", "internal"])["visibility"],
        "private"
    );
}

#[test]
fn hosted_packages_list_their_versions_and_who_published_them() {
    let Some(live) = Live::new() else { return };
    let found = live.ok(false, &["package", "list", "orders"]);
    assert_eq!(ids(&found), ["pypi/contoso-orders-client"]);
    assert_eq!(found[0]["latest"], "2.3.0");

    let package = live.ok(
        false,
        &["package", "get", "Contoso_Orders_Client", "--repo", "pypi"],
    );
    assert_eq!(package["id"], "pypi/contoso-orders-client");
    assert_eq!(
        ids(&package["versions"]),
        [
            "pypi/contoso-orders-client@2.3.0",
            "pypi/contoso-orders-client@2.2.0"
        ]
    );
    let through = live.ok(false, &["package", "get", "pypi-all/contoso-orders-client"]);
    assert_eq!(through["type"], "virtual");
    assert_eq!(through["versions"].as_array().unwrap().len(), 2);

    let version = live.ok(
        false,
        &["version", "get", "pypi/contoso-orders-client@2.3.0"],
    );
    assert_eq!(version["published_by"], "admin");
    assert_eq!(version["sha256"], version["files"][0]["sha256"]);
    let release = format!(
        "{}/pypi/pypi/pypi/contoso-orders-client/2.2.0/json",
        live.url
    );
    assert_eq!(
        live.ok(false, &["version", "get", &release])["version"],
        "2.2.0"
    );
    let (_, said) = live.exits(
        false,
        4,
        &["version", "get", "pypi/contoso-orders-client@9.9.9"],
    );
    assert!(
        said.contains("agent-cli keeper package get pypi/contoso-orders-client"),
        "{said}"
    );

    let api = live.ok(false, &["package", "get", "builds/api"]);
    assert_eq!(
        ids(&api["versions"]),
        ["builds/api@1.4.2", "builds/api@1.4.1"]
    );
    let build = live.ok(false, &["version", "get", "builds/api@1.4.2"]);
    assert_eq!(build["files"].as_array().unwrap().len(), 2, "{build}");

    let (_, said) = live.exits(false, 4, &["package", "get", "internal/etl"]);
    assert!(said.contains("it is private"), "{said}");
    let (_, said) = live.exits(true, 4, &["package", "get", "internal/etl-tools"]);
    assert!(said.contains("no package etl-tools in internal"), "{said}");
    let ref_and_flag = [
        "version",
        "get",
        "pypi/contoso-orders-client@2.3.0",
        "--repo",
        "builds",
    ];
    let (_, said) = live.exits(false, 2, &ref_and_flag);
    assert!(said.contains("--repo says builds"), "{said}");
    assert_eq!(
        live.ok(true, &["package", "get", "internal/etl"])["latest"],
        "2.0.1"
    );
}

#[test]
fn files_list_show_and_download_anonymously_or_with_the_token() {
    let Some(live) = Live::new() else { return };
    let listed = live.ok(false, &["file", "list", "builds/api/1.4.2/"]);
    assert_eq!(
        ids(&listed),
        [
            "builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz",
            "builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz.sha256"
        ]
    );
    let (cut, said) = live.exits(false, 0, &["file", "list", "builds", "--limit", "1"]);
    assert_eq!(cut.as_array().unwrap().len(), 1);
    assert!(said.contains("--limit N"), "{said}");

    let file = live.ok(
        false,
        &["file", "get", "builds/api/1.4.2/api-1.4.2-linux-x64.tar.gz"],
    );
    assert_eq!(
        (&file["package"], &file["version"]),
        (&"api".into(), &"1.4.2".into())
    );
    assert_eq!(file["origin"], "hosted");

    let saved = live.dir.path().join("api.tar.gz");
    let download = format!(
        "{}/api/v1/repositories/builds/download/api/1.4.2/api-1.4.2-linux-x64.tar.gz",
        live.url
    );
    let file = live.ok(
        false,
        &[
            "file",
            "get",
            &download,
            "--output",
            saved.to_str().unwrap(),
        ],
    );
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "api 1.4.2\n");
    assert_eq!(file["saved"], saved.to_str().unwrap());

    let (_, said) = live.exits(
        false,
        4,
        &["file", "get", "internal/etl/2.0.1/etl-2.0.1.whl"],
    );
    assert!(said.contains("it is private"), "{said}");
    let wheel = live.dir.path().join("etl.whl");
    live.ok(
        true,
        &[
            "file",
            "get",
            "etl/2.0.1/etl-2.0.1.whl",
            "--repo",
            "internal",
            "--output",
            wheel.to_str().unwrap(),
        ],
    );
    assert_eq!(std::fs::read_to_string(&wheel).unwrap(), "etl 2.0.1\n");
    let (_, said) = live.exits(false, 4, &["file", "get", "builds/api/9.9.9/api.tar.gz"]);
    assert!(
        said.contains("agent-cli keeper file list builds/api/9.9.9/"),
        "{said}"
    );
}

#[test]
fn the_pypi_proxy_says_what_it_cached_and_what_upstream_lacks() {
    let Some(live) = Live::new() else { return };
    let cached = live.ok(false, &["package", "list", "--repo", "pypi-proxy"]);
    if !ids(&cached).contains(&"pypi-proxy/six") {
        eprintln!("skipped the proxy: keeper-up found pypi.org unreachable");
        return;
    }
    let six = live.ok(
        false,
        &["package", "get", "pypi-proxy/six", "--limit", "200"],
    );
    assert_eq!(six["upstream"], "https://pypi.org");
    let versions = six["versions"].as_array().unwrap();
    let of = |version: &str| {
        versions
            .iter()
            .find(|row| row["version"] == version)
            .unwrap_or_else(|| panic!("{version}: {six}"))["cached"]
            .clone()
    };
    assert_eq!(of("1.17.0"), true);
    assert_eq!(of("1.16.0"), false);

    let release = live.ok(false, &["version", "get", "pypi-proxy/six@1.17.0"]);
    let files = release["files"].as_array().unwrap();
    let wheel = files
        .iter()
        .find(|file| file["id"] == "pypi-proxy/simple/six/six-1.17.0-py2.py3-none-any.whl")
        .unwrap();
    assert_eq!(wheel["cached"], true);
    let fetched = live.ok(false, &["file", "get", wheel["id"].as_str().unwrap()]);
    assert_eq!(fetched["sha256"], wheel["sha256"]);
    let (_, said) = live.exits(
        false,
        4,
        &["file", "get", "pypi-proxy/simple/six/six-1.16.0.tar.gz"],
    );
    assert!(said.contains("has not cached"), "{said}");

    let (_, said) = live.exits(
        false,
        4,
        &["package", "get", "pypi-proxy/contoso-orders-client"],
    );
    assert!(
        said.contains("https://pypi.org has no contoso-orders-client"),
        "{said}"
    );
    assert!(
        said.contains("agent-cli keeper package list contoso-orders-client"),
        "{said}"
    );
}
