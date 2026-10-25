//! A team's setup end to end, in process: a shared file included under a
//! person's own, `read_only` scopes, `*_kv` logins and `config pull`, what is
//! hidden from search and the overview, the usage log and doctor's release
//! check.

use agent_cli_core::testing::{Answer, FakeTransport, run};
use agent_cli_core::{Check, Config, Ctx, Domain, Effect, Request, Setup, command};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, Default)]
#[serde(default)]
struct Shop {
    store: Vec<Store>,
}

#[derive(Deserialize)]
struct Store {
    name: String,
    read_only: Option<bool>,
}

#[derive(clap::Args)]
struct OrderArgs {
    /// The order
    id: String,
    /// The [[shop.store]] name
    #[arg(long)]
    store: Option<String>,
}

/// Picks the store the way a domain picks any scope, and says so to ctx.
fn store(ctx: &Ctx, wanted: Option<&str>) -> Result<String> {
    let shop: Shop = ctx.section("shop")?;
    let store = agent_cli_core::pick("store", "--store", wanted, &shop.store, |s| &s.name)?;
    ctx.enter_scope("shop store", &store.name, store.read_only);
    Ok(store.name.clone())
}

fn update(ctx: &Ctx, args: OrderArgs) -> Result<Value> {
    let name = store(ctx, args.store.as_deref())?;
    let url = format!("https://shop.contoso.example/{name}/orders/{}", args.id);
    ctx.write(
        Effect::Write,
        Request::new(agent_cli_core::Method::Put, url).json(json!({})),
    )?;
    Ok(json!({"id": args.id}))
}

fn reveal(ctx: &Ctx, args: OrderArgs) -> Result<Value> {
    let name = store(ctx, args.store.as_deref())?;
    let url = format!("https://shop.contoso.example/{name}/keys/{}", args.id);
    let answer = ctx.write(Effect::Reveal, Request::get(url))?;
    Ok(json!({"key": answer.body}))
}

fn list(ctx: &Ctx, _: NoArgs) -> Result<Value> {
    let answer = ctx.read(Request::get("https://shop.contoso.example/orders"))?;
    answer.json()
}

#[derive(clap::Args)]
struct NoArgs {}

command! {
    UPDATE = ["shop", "order", "update"], Write,
    "Update a shop order",
    keywords: [],
    example: "shop order update 7",
    run: update,
}

command! {
    KEY = ["shop", "key", "get"], Reveal,
    "Get a store's signing key",
    keywords: [],
    example: "shop key get 7 --reveal",
    run: reveal,
}

command! {
    LIST = ["shop", "order", "list"], Read,
    "List shop orders",
    keywords: ["purchases"],
    example: "shop order list",
    run: list,
}

command! {
    WALL_LIST = ["wall", "brick", "list"], Read,
    "List bricks in the wall",
    keywords: ["purchases"],
    example: "wall brick list",
    run: list,
}

command! {
    IDLE_LIST = ["idle", "order", "list"], Read,
    "List idle orders waiting",
    keywords: ["purchases"],
    example: "idle order list",
    run: list,
}

const DOMAINS: &[Domain] = &[
    Domain {
        name: "shop",
        summary: "Shop",
        commands: &[UPDATE, LIST, KEY],
        synonyms: &[],
        status: |_| "shop 2 stores".to_owned(),
        doctor: |_| vec![Check::ok("config", "2 stores")],
    },
    Domain {
        name: "wall",
        summary: "Wall",
        commands: &[WALL_LIST],
        synonyms: &[],
        status: |_| "wall 1 site".to_owned(),
        doctor: |ctx| {
            if ctx.env("WALL_OPEN").is_some() {
                return vec![Check::ok("credential", "a token"), Check::ok("bricks", "3")];
            }
            vec![
                Check::ok("credential", "a token"),
                Check::failed(
                    "bricks",
                    "GET https://wall.contoso.example answered 403: no role",
                    "ask for Wall Reader",
                ),
            ]
        },
    },
    Domain {
        name: "idle",
        summary: "Idle",
        commands: &[IDLE_LIST],
        synonyms: &[],
        status: |_| "idle not set up".to_owned(),
        doctor: |_| Vec::new(),
    },
];

const STORES: &str = "[shop]\nread_only = true\n\n[[shop.store]]\nname = \"prod\"\n\n[[shop.store]]\nname = \"dev\"\nread_only = false\n";

#[test]
fn a_read_only_section_refuses_changes_before_anything_is_sent_and_an_entry_can_lift_it() {
    let transport = FakeTransport::default();
    let setup = Setup::fake(transport.clone()).with_config(STORES);
    let refused = run(
        DOMAINS,
        &["shop", "order", "update", "7", "--store", "prod"],
        setup,
    );
    assert_eq!(refused.code, 2, "{refused:?}");
    assert!(
        refused
            .stderr
            .contains("error: shop store prod is read-only in the config ([shop] read_only)"),
        "{}",
        refused.stderr
    );
    assert!(
        refused
            .stderr
            .contains("hint: agent-cli config  (shows where"),
        "{}",
        refused.stderr
    );
    assert!(transport.sent().is_empty());

    let open = FakeTransport::answering([Answer::ok("{}")]);
    let setup = Setup::fake(open.clone()).with_config(STORES);
    let changed = run(
        DOMAINS,
        &["shop", "order", "update", "7", "--store", "dev"],
        setup,
    );
    assert_eq!(changed.code, 0, "{changed:?}");
    assert_eq!(open.sent().len(), 1);

    let own = "[[shop.store]]\nname = \"prod\"\nread_only = true\n";
    let setup = Setup::fake(FakeTransport::default()).with_config(own);
    let refused = run(DOMAINS, &["shop", "order", "update", "7"], setup);
    assert!(
        refused
            .stderr
            .contains("error: shop store prod is read-only in the config\n"),
        "{}",
        refused.stderr
    );
}

#[test]
fn a_dry_run_on_a_read_only_scope_prints_the_plan_and_says_it_would_be_refused() {
    let transport = FakeTransport::default();
    let setup = Setup::fake(transport.clone()).with_config(STORES);
    let planned = run(
        DOMAINS,
        &[
            "shop",
            "order",
            "update",
            "7",
            "--store",
            "prod",
            "--dry-run",
        ],
        setup,
    );
    assert_eq!(planned.code, 0, "{planned:?}");
    assert_eq!(planned.json()["would"][0]["method"], "PUT");
    assert!(
        planned
            .stderr
            .contains("[this change would be refused: shop store prod is read-only"),
        "{}",
        planned.stderr
    );
    assert!(transport.sent().is_empty());
}

#[test]
fn a_reveal_is_a_read_and_no_variable_lifts_read_only() {
    let transport = FakeTransport::answering([Answer::ok("key-1")]);
    let setup = Setup::fake(transport).with_config(STORES);
    let revealed = run(
        DOMAINS,
        &["shop", "key", "get", "7", "--store", "prod", "--reveal"],
        setup,
    );
    assert_eq!(revealed.code, 0, "{revealed:?}");

    let mut setup = Setup::fake(FakeTransport::default());
    setup.config = Config::parse(
        "config.toml",
        Some(STORES),
        vec![("AGENT_CLI_SHOP_READ_ONLY".into(), "false".into())],
    );
    let refused = run(
        DOMAINS,
        &["shop", "order", "update", "7", "--store", "dev"],
        setup,
    );
    assert_eq!(refused.code, 3, "{refused:?}");
    assert!(
        refused
            .stderr
            .contains("AGENT_CLI_SHOP_READ_ONLY cannot set read_only"),
        "{}",
        refused.stderr
    );
}

#[test]
fn a_team_file_is_included_merged_by_name_and_shown_by_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("team.toml"), STORES).unwrap();
    let mine =
        "[core]\ninclude = \"team.toml\"\n\n[[shop.store]]\nname = \"prod\"\nread_only = false\n";
    let path = dir.path().join("config.toml");
    let layered = || {
        let mut setup = Setup::fake(FakeTransport::answering([Answer::ok("{}")]));
        setup.config = Config::parse(&path, Some(mine), Vec::new());
        setup
    };
    let changed = run(
        DOMAINS,
        &["shop", "order", "update", "7", "--store", "prod"],
        layered(),
    );
    assert_eq!(changed.code, 0, "a person's own entry wins: {changed:?}");
    let shown = run(DOMAINS, &["config"], layered()).json();
    assert_eq!(shown["include"][0]["exists"], true);
    assert_eq!(shown["sections"]["shop"]["read_only"], true);
    assert_eq!(
        shown["sections"]["shop"]["store"].as_array().unwrap().len(),
        2
    );

    let gone = "[core]\ninclude = [\"missing.toml\"]\n";
    let mut setup = Setup::fake(FakeTransport::default());
    setup.config = Config::parse(&path, Some(gone), Vec::new());
    let broken = run(DOMAINS, &["shop", "order", "update", "7"], setup);
    assert_eq!(broken.code, 3);
    assert!(
        broken.stderr.contains("missing.toml does not exist"),
        "{}",
        broken.stderr
    );
    assert!(
        broken.stderr.contains("hint: rerun install.sh"),
        "{}",
        broken.stderr
    );
}

#[test]
fn search_and_the_overview_leave_out_a_domain_not_set_up_and_search_all_finds_it() {
    let found = run(
        DOMAINS,
        &["search", "list", "purchases"],
        Setup::fake(FakeTransport::default()),
    );
    assert!(!found.stdout.contains("agent-cli idle"), "{}", found.stdout);
    assert!(
        found.stderr.contains(
            "[1 more in domains not set up here: idle; agent-cli search --all 'list purchases']"
        ),
        "{}",
        found.stderr
    );
    let all = run(
        DOMAINS,
        &["search", "--all", "list", "purchases"],
        Setup::fake(FakeTransport::default()),
    );
    assert!(
        all.stdout.contains("agent-cli idle order list"),
        "{}",
        all.stdout
    );
    let overview = run(DOMAINS, &[], Setup::fake(FakeTransport::default())).stdout;
    assert!(
        overview.contains("Domains:     shop(3)  wall(1)\n"),
        "{overview}"
    );
    assert!(
        overview.contains("Hidden:      idle (not set up); agent-cli search --all WORDS"),
        "{overview}"
    );
}

#[test]
fn doctor_marks_a_403_denied_hides_the_domain_and_a_passing_doctor_clears_it() {
    let state = tempfile::tempdir().unwrap();
    let setup = || Setup::fake(FakeTransport::default()).with_state_dir(state.path());
    let doctor = run(DOMAINS, &["doctor", "wall"], setup());
    let rows = doctor.json();
    assert_eq!(rows[2]["denied"], true, "{rows}");
    let denied: Value =
        serde_json::from_slice(&std::fs::read(state.path().join("denied.json")).unwrap()).unwrap();
    assert!(denied["wall"].as_str().unwrap().contains("403"));
    let overview = run(DOMAINS, &[], setup()).stdout;
    assert!(overview.contains("wall (denied)"), "{overview}");
    let search = run(DOMAINS, &["search", "bricks"], setup());
    assert!(
        search
            .stderr
            .contains("domains not set up here or denied: wall"),
        "{}",
        search.stderr
    );
    assert_eq!(
        run(DOMAINS, &["wall", "brick", "list"], setup()).code,
        1,
        "a hidden domain still runs"
    );

    let open = Setup::fake(FakeTransport::default())
        .with_state_dir(state.path())
        .with_env("WALL_OPEN", "1");
    assert_eq!(run(DOMAINS, &["doctor", "wall"], open).code, 0);
    let overview = run(DOMAINS, &[], setup()).stdout;
    assert!(!overview.contains("wall (denied)"), "{overview}");
}

#[test]
fn every_run_appends_names_and_counts_never_values_and_usage_sums_them() {
    let state = tempfile::tempdir().unwrap();
    let setup = |answers: Vec<Answer>| {
        Setup::fake(FakeTransport::answering(answers))
            .with_config(STORES)
            .with_state_dir(state.path())
    };
    let ok = run(
        DOMAINS,
        &["shop", "order", "list"],
        setup(vec![Answer::ok("[1,2]")]),
    );
    assert_eq!(ok.code, 0, "{ok:?}");
    let refused = run(
        DOMAINS,
        &["shop", "order", "update", "secret-order-9", "--store=prod"],
        setup(vec![]),
    );
    assert_eq!(refused.code, 2);
    let log = std::fs::read_to_string(state.path().join("usage.jsonl")).unwrap();
    assert!(
        !log.contains("secret-order-9") && !log.contains("prod"),
        "{log}"
    );
    let lines: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["command"], "shop order list");
    assert_eq!(
        (lines[0]["requests"].as_u64(), lines[0]["bytes"].as_u64()),
        (Some(1), Some(5))
    );
    assert_eq!(lines[1]["flags"], json!(["--store"]));
    assert_eq!(lines[1]["exit"], 2);

    let summary = run(DOMAINS, &["usage", "--since", "1d"], setup(vec![])).json();
    assert_eq!(summary["calls"], 2);
    assert_eq!(
        summary["exit_2"][0],
        json!({"command": "shop order update", "flags": "--store", "count": 1})
    );

    let off = setup(vec![Answer::ok("[]")]).with_env("AGENT_CLI_USAGE", "off");
    let _ = run(DOMAINS, &["shop", "order", "list"], off);
    let after = std::fs::read_to_string(state.path().join("usage.jsonl")).unwrap();
    assert_eq!(
        after.lines().count(),
        3,
        "the usage run itself is logged, the off one is not"
    );
}

#[test]
fn config_pull_reads_every_kv_key_and_keeps_them_without_printing_one() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = dir.path().join("secrets.toml");
    let config = "[shop]\ntoken_kv = \"kv-contoso-team/shop-token\"\n\n[[shop.store]]\nname = \"prod\"\npassword_kv = \"kv-contoso-team/shop-prod\"\n\n[[shop.store]]\nname = \"dev\"\npassword_kv = \"kv-contoso-team/shop-token\"\n";
    let vault = |value: &str| Answer::json(&json!({"value": value}));
    let transport = FakeTransport::answering([vault("pw-prod-1"), vault("tok-1")]);
    let setup = Setup::fake(transport.clone())
        .with_config(config)
        .with_secrets(&secrets);
    let pulled = run(DOMAINS, &["config", "pull"], setup);
    assert_eq!(pulled.code, 0, "{pulled:?}");
    assert!(!pulled.stdout.contains("tok-1") && !pulled.stdout.contains("pw-prod-1"));
    let rows = pulled.json();
    assert_eq!(rows.as_array().unwrap().len(), 3);
    assert_eq!(rows[0]["key"], "store.prod.password_kv");
    assert_eq!(rows[0]["changed"], true);
    assert_eq!(rows[2]["key"], "token_kv");
    assert_eq!(transport.sent().len(), 2, "one read per secret");
    let kept = std::fs::read_to_string(&secrets).unwrap();
    assert!(
        kept.contains("\"kv-contoso-team/shop-prod\" = \"pw-prod-1\""),
        "{kept}"
    );

    let refused = FakeTransport::answering([Answer::status(403, "{}"), vault("tok-1")]);
    let setup = Setup::fake(refused)
        .with_config(config)
        .with_secrets(&secrets);
    let again = run(DOMAINS, &["config", "pull"], setup);
    assert_eq!(again.code, 1);
    let rows = again.json();
    assert_eq!(
        (rows[0]["ok"].as_bool(), rows[1]["changed"].as_bool()),
        (Some(false), Some(false))
    );
    assert!(
        std::fs::read_to_string(&secrets)
            .unwrap()
            .contains("pw-prod-1"),
        "a failed key keeps its value"
    );
    let shown = run(
        DOMAINS,
        &["config"],
        Setup::fake(FakeTransport::default()).with_config(config),
    )
    .json();
    assert_eq!(
        shown["sections"]["shop"]["token_kv"],
        "kv-contoso-team/shop-token"
    );
}

#[test]
fn a_kv_key_set_to_true_reads_the_secret_named_for_its_entry_and_a_persons_own_credential_replaces_it()
 {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("team.toml"),
        "[core]\nvault = \"kv-contoso-dev\"\n\n[[shop.store]]\nname = \"Reporting_Prod\"\npassword_kv = true\n\n[[shop.store]]\nname = \"dev\"\npassword_kv = true\n",
    )
    .unwrap();
    let mine =
        "[core]\ninclude = \"team.toml\"\n\n[[shop.store]]\nname = \"dev\"\npassword = \"mine\"\n";
    let path = dir.path().join("config.toml");
    let layered = |transport: FakeTransport| {
        let mut setup = Setup::fake(transport).with_secrets(&dir.path().join("secrets.toml"));
        setup.config = Config::parse(&path, Some(mine), Vec::new());
        setup
    };
    let shown = run(DOMAINS, &["config"], layered(FakeTransport::default())).json();
    let stores = &shown["sections"]["shop"]["store"];
    assert_eq!(
        stores[0]["password_kv"],
        "kv-contoso-dev/agentcli-shop-reporting-prod-password"
    );
    assert_eq!(stores[1]["password"], "***");
    assert!(stores[1].get("password_kv").is_none(), "{stores}");

    let transport = FakeTransport::answering([Answer::json(&json!({"value": "pw-1"}))]);
    let pulled = run(DOMAINS, &["config", "pull"], layered(transport.clone()));
    assert_eq!(pulled.code, 0, "{pulled:?}");
    assert_eq!(pulled.json().as_array().unwrap().len(), 1);
    assert_eq!(
        transport.sent()[0].url,
        "https://kv-contoso-dev.vault.azure.net/secrets/agentcli-shop-reporting-prod-password?api-version=7.4"
    );

    std::fs::write(
        dir.path().join("team.toml"),
        "[[shop.store]]\nname = \"prod\"\npassword_kv = true\n",
    )
    .unwrap();
    let broken = run(
        DOMAINS,
        &["shop", "order", "update", "7"],
        layered(FakeTransport::default()),
    );
    assert_eq!(broken.code, 3);
    assert!(
        broken
            .stderr
            .contains("shop.prod.password_kv = true names its secret in [core] vault"),
        "{}",
        broken.stderr
    );
}
