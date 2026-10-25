//! Every confluence command against a live Confluence Data Center, through
//! the built binary. Off unless `AGENT_CLI_TEST_CONFLUENCE=1`; start the site
//! with `scripts/confluence-up.sh` (9.2 behind the `/confluence` context
//! path, Jane Doe's token in `target/confluence-live/token`).
//! `AGENT_CLI_TEST_CONFLUENCE_URL` and `AGENT_CLI_TEST_CONFLUENCE_TOKEN` point
//! it elsewhere. It writes (pages, comments, files, moves, the trash): point
//! it only at a throwaway site.
//!
//! Each run makes its own pages, titled with the time it started, under
//! ENG's homepage, and leaves them for the next run to ignore.

use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Live {
    dir: tempfile::TempDir,
    token: String,
    url: String,
    stamp: u64,
}

impl Live {
    fn new() -> Option<Self> {
        if std::env::var("AGENT_CLI_TEST_CONFLUENCE").as_deref() != Ok("1") {
            eprintln!(
                "skipped live confluence: set AGENT_CLI_TEST_CONFLUENCE=1 after scripts/confluence-up.sh"
            );
            return None;
        }
        let url = std::env::var("AGENT_CLI_TEST_CONFLUENCE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18090/confluence".to_owned());
        let token = std::env::var("AGENT_CLI_TEST_CONFLUENCE_TOKEN").unwrap_or_else(|_| {
            let file = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/confluence-live/token"
            );
            std::fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("{file}: {e}; run scripts/confluence-up.sh"))
        });
        let dir = tempfile::tempdir().unwrap();
        let config = format!(
            "[confluence]\nurl = \"{url}\"\ntoken_env = \"AGENT_CLI_TEST_CONFLUENCE_TOKEN\"\n"
        );
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        Some(Self {
            dir,
            token: token.trim().to_owned(),
            url,
            stamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        })
    }

    fn run_with(&self, token: &str, args: &[&str], stdin: Option<&str>) -> Ran {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-cli"));
        command
            .args(args)
            .env("AGENT_CLI_CONFIG", self.dir.path().join("config.toml"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env("AGENT_CLI_TEST_CONFLUENCE_TOKEN", token)
            .current_dir(self.dir.path());
        let output = match stdin {
            None => command.output().unwrap(),
            Some(text) => {
                use std::io::Write;
                let mut child = command
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(text.as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            }
        };
        let ran = Ran {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        assert!(
            !ran.stdout.contains(&self.token) && !ran.stderr.contains(&self.token),
            "the token leaked: agent-cli {}",
            args.join(" ")
        );
        ran
    }

    /// `agent-cli confluence ARGS`, which must exit `code`: what it printed
    /// and said.
    fn exits(&self, code: i32, args: &[&str]) -> (Value, String) {
        self.exits_with(code, args, None)
    }

    fn exits_with(&self, code: i32, args: &[&str], stdin: Option<&str>) -> (Value, String) {
        let mut argv = vec!["confluence"];
        argv.extend(args);
        let ran = self.run_with(&self.token, &argv, stdin);
        assert_eq!(
            ran.code,
            code,
            "agent-cli {}\n{}{}",
            argv.join(" "),
            ran.stdout,
            ran.stderr
        );
        let json = if ran.stdout.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&ran.stdout)
                .unwrap_or_else(|e| panic!("{e}: not JSON: {}\n{}", ran.stdout, ran.stderr))
        };
        (json, ran.stderr)
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.exits(0, args).0
    }

    fn title(&self, what: &str) -> String {
        format!("{what} {}", self.stamp)
    }

    /// A new page under `parent` (ENG's homepage when `None`): its id.
    fn page(&self, title: &str, parent: Option<&str>, markdown: &str) -> String {
        let mut args = vec!["page", "create", "--title", title, "--body", markdown];
        match parent {
            Some(parent) => args.extend(["--parent", parent]),
            None => args.extend(["--space", "ENG"]),
        }
        let made = self.ok(&args);
        assert_eq!(made["space"], "ENG", "{made}");
        assert_eq!(made["version"], 1, "{made}");
        made["id"].as_str().unwrap().to_owned()
    }
}

fn s(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a string: {value}"))
}

const RUNBOOK: &str = "## First checks\n\nLook at the failed run.\n\n## An order without customer_id\n\nFix the order in the CRM, then retry the load once. Ask @<Sam Lee>.\n\n```bash\nairflow tasks clear etl_nightly -t load_orders\n```\n\n> [!WARNING]\n> Prod only.\n\n## Escalation\n\nPage the owner.";

#[test]
fn every_confluence_command_runs_against_a_live_data_center() {
    let Some(live) = Live::new() else { return };

    // doctor and the spaces
    let ran = live.run_with(&live.token, &["doctor", "confluence"], None);
    assert_eq!(ran.code, 0, "{}{}", ran.stdout, ran.stderr);
    let checks: Value = serde_json::from_str(&ran.stdout).unwrap();
    let connection = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["domain"] == "confluence" && row["check"] == "connection")
        .unwrap();
    assert_eq!(connection["detail"], "signed in as Jane Doe (jane)");
    let wrong = live.run_with("not-a-token", &["confluence", "space", "list"], None);
    assert_eq!(wrong.code, 3, "{}", wrong.stderr);
    assert!(
        wrong
            .stderr
            .contains("/plugins/personalaccesstokens/usertokens.action"),
        "{}",
        wrong.stderr
    );
    let spaces = live.ok(&["space", "list"]);
    let eng = spaces
        .as_array()
        .unwrap()
        .iter()
        .find(|space| space["id"] == "ENG")
        .unwrap_or_else(|| panic!("{spaces}"));
    let home = s(&eng["homepage"]).to_owned();
    assert_eq!(
        live.ok(&["space", "list", "operat", "--fields", "id"]),
        json!([{"id": "OPS"}])
    );

    // create, and read back every way a page is named
    let runbooks = live.page(&live.title("Runbooks"), None, "Every runbook.");
    let title = live.title("Runbook: etl_nightly");
    let made = live.ok(&[
        "page",
        "create",
        "--title",
        &title,
        "--parent",
        &runbooks,
        "--body",
        RUNBOOK,
        "--labels",
        "runbook, airflow",
    ]);
    let id = s(&made["id"]).to_owned();
    let url = s(&made["url"]).to_owned();
    assert!(
        url.starts_with(&format!("{}/spaces/ENG/pages/{id}/", live.url)),
        "{url}"
    );
    let got = live.ok(&["page", "get", &id]);
    assert_eq!(
        (
            &got["parent"],
            &got["author"],
            &got["updated_by"],
            &got["version"]
        ),
        (
            &json!(runbooks),
            &json!("Jane Doe"),
            &json!("Jane Doe"),
            &json!(1)
        )
    );
    let mut labels: Vec<&str> = got["labels"].as_array().unwrap().iter().map(s).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["airflow", "runbook"]);
    let body = s(&got["body"]);
    for part in [
        "## An order without customer_id",
        "Ask @<Sam Lee>.",
        "```bash\nairflow tasks clear",
        "> [!WARNING]",
    ] {
        assert!(body.contains(part), "{part}: {body}");
    }
    assert!(got["lossy"].is_null(), "nothing lost: {got}");
    for named in [
        url.clone(),
        format!("ENG:{title}"),
        format!("{}/pages/viewpage.action?pageId={id}", live.url),
        format!(
            "{}/display/ENG/{}",
            live.url,
            title.replace(' ', "+").replace(':', "%3A")
        ),
    ] {
        assert_eq!(
            live.ok(&["page", "get", &named, "--fields", "id"])["id"],
            id,
            "{named}"
        );
    }
    let (_, said) = live.exits(4, &["page", "get", "ENG:No such page here"]);
    assert!(said.contains("no page titled"), "{said}");

    // edits: a section, an append from stdin, a title, labels, a move
    let edited = live.ok(&[
        "page",
        "update",
        &id,
        "--section",
        "first checks",
        "--body",
        "## First checks\n\nLook at the failed run's **log**.",
        "--message",
        "the log",
    ]);
    assert_eq!(edited["version"], 2);
    let (appended, _) = live.exits_with(
        0,
        &[
            "page",
            "update",
            &id,
            "--section",
            "Escalation",
            "--append",
            "-",
        ],
        Some("Then tell @<sam> and @<Priya Patel>."),
    );
    assert_eq!(appended["version"], 3);
    let storage = live.ok(&["page", "get", &id, "--storage", "--fields", "body"]);
    assert!(
        s(&storage["body"]).matches("<ri:user ri:userkey=").count() >= 3,
        "{storage}"
    );
    let (_, said) = live.exits(
        5,
        &["page", "update", &id, "--body", "x", "--if-version", "2"],
    );
    assert!(said.contains("changed since version 2 (now 3)"), "{said}");
    let retitled = live.ok(&["page", "update", &id, "--title", &format!("{title} (v2)")]);
    assert_eq!(retitled["version"], 4);
    assert!(
        s(&live.ok(&["page", "get", &id, "--fields", "body"])["body"]).contains("Prod only."),
        "a new title keeps the body"
    );
    live.ok(&["page", "update", &id, "--labels", "runbook,etl"]);
    let mut labels: Vec<String> = live.ok(&["page", "get", &id, "--fields", "labels"])["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| s(label).to_owned())
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, ["etl", "runbook"]);
    let other = live.page(&live.title("Incidents"), None, "Where postmortems go.");
    let moved = live.ok(&["page", "update", &id, "--parent", &other]);
    assert_eq!(moved["version"], 5);
    assert_eq!(
        live.ok(&["page", "get", &id, "--fields", "parent"])["parent"],
        other
    );
    live.ok(&["page", "update", &id, "--parent", &runbooks]);
    let ops_home = live.ok(&["tree", "get", "OPS", "--depth", "1"])["nodes"][0]["id"].clone();
    let (_, said) = live.exits(2, &["page", "update", &id, "--parent", s(&ops_home)]);
    assert!(said.contains("only within its space"), "{said}");

    // versions and diffs
    let versions = live.ok(&["version", "list", &id, "--fields", "id,version,message"]);
    assert_eq!(versions[0]["version"], 6, "{versions}");
    assert_eq!(versions[4]["message"], "the log", "{versions}");
    assert_eq!(versions.as_array().unwrap().len(), 6);
    let first = live.ok(&["page", "get", &format!("{id}@1")]);
    assert_eq!(
        (&first["status"], &first["version"]),
        (&json!("historical"), &json!(1))
    );
    assert!(s(&first["body"]).contains("Look at the failed run."));
    let diff = live.ok(&["page", "diff", &id, "--version", "1"]);
    let changes = diff["changes"].to_string();
    assert!(
        changes.contains("**log**") && changes.contains("(v2)"),
        "{diff}"
    );

    // comments: footer, a reply, inline with --match, an inline reply, resolving
    live.ok(&[
        "page",
        "update",
        &id,
        "--append",
        "Retry the load, and if it fails, retry the load again.",
    ]);
    let footer = live.ok(&["page", "comment", &id, "Rolled out **clean**."]);
    assert_eq!(footer["kind"], "footer");
    let footer_id = s(&footer["id"]).to_owned();
    let reply = live.ok(&["page", "comment", &id, "Agreed.", "--reply-to", &footer_id]);
    assert_eq!(reply["kind"], "footer");
    let (_, said) = live.exits(
        2,
        &["page", "comment", &id, "why?", "--on", "retry the load"],
    );
    assert!(said.contains("--match 1 to"), "{said}");
    let inline = live.ok(&[
        "page",
        "comment",
        &id,
        "Only once the CRM fix has synced.",
        "--on",
        "retry the load",
        "--match",
        "2",
    ]);
    assert_eq!(inline["kind"], "inline");
    let inline_id = s(&inline["id"]).to_owned();
    assert!(
        s(&inline["url"]).contains(&format!("focusedCommentId={inline_id}")),
        "{inline}"
    );
    let threaded = live.ok(&[
        "page",
        "comment",
        &id,
        "On it.",
        "--reply-to",
        s(&inline["url"]),
    ]);
    assert_eq!(threaded["kind"], "inline", "an inline reply stays inline");
    let marked = live.ok(&["page", "get", &id, "--fields", "lossy,version"]);
    assert_eq!(marked["lossy"], json!(["inline comment marks"]));
    let (_, said) = live.exits(
        2,
        &[
            "page",
            "update",
            &id,
            "--body",
            "x",
            "--if-version",
            &marked["version"].to_string(),
        ],
    );
    assert!(
        said.contains("(inline comment marks)"),
        "the loss gate: {said}"
    );
    let comments = live.ok(&["comment", "list", &id]);
    let row = |id: &str| {
        comments
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("{id}: {comments}"))
            .clone()
    };
    assert_eq!(
        comments[0]["id"], threaded["id"],
        "newest first: {comments}"
    );
    assert_eq!(
        (
            row(&inline_id)["selection"].clone(),
            row(&inline_id)["resolution"].clone()
        ),
        (json!("retry the load"), json!("open"))
    );
    assert_eq!(row(s(&threaded["id"]))["parent"], inline_id);
    assert_eq!(row(s(&reply["id"]))["parent"], footer_id);
    assert_eq!(row(&footer_id)["body"], "Rolled out **clean**.");
    assert_eq!(row(&footer_id)["author"], "Jane Doe");
    let (_, said) = live.exits(
        2,
        &["comment", "update", &footer_id, "--status", "resolved"],
    );
    assert!(said.contains("only inline comments resolve"), "{said}");
    let (_, said) = live.exits(
        2,
        &[
            "comment",
            "update",
            s(&threaded["id"]),
            "--status",
            "resolved",
        ],
    );
    assert!(
        said.contains(&format!("comment update {inline_id} --status resolved")),
        "{said}"
    );
    let resolved = live.ok(&["comment", "update", &inline_id, "--status", "resolved"]);
    assert_eq!(resolved["resolution"], "resolved");
    assert_eq!(live.ok(&["comment", "list", &id, "--open"]), json!([]));
    let reopened = live.ok(&["comment", "update", &inline_id, "--status", "open"]);
    assert_eq!(reopened["resolution"], "reopened");
    let open = live.ok(&[
        "comment", "list", &id, "--open", "--kind", "inline", "--fields", "id",
    ]);
    assert_eq!(
        open.as_array().unwrap().len(),
        2,
        "the thread is open again: {open}"
    );

    // attachments: text, a second version, binary
    let file = live.dir.path().join("changes.txt");
    std::fs::write(&file, "PR 431: retry on 429\n").unwrap();
    let path = file.to_str().unwrap();
    let attached = live.ok(&[
        "attachment",
        "create",
        &id,
        "--file",
        path,
        "--comment",
        "Changelog",
    ]);
    assert_eq!(attached["version"], 1);
    let file_id = s(&attached["id"]).to_owned();
    std::fs::write(&file, "PR 431: retry on 429\nPR 433: log the run id\n").unwrap();
    let again = live.ok(&["attachment", "create", &id, "--file", path]);
    assert_eq!(
        (&again["id"], &again["version"]),
        (&json!(file_id), &json!(2))
    );
    let text = live.ok(&["attachment", "get", &file_id]);
    assert_eq!(
        text["text"],
        "PR 431: retry on 429\nPR 433: log the run id\n"
    );
    let png = live.dir.path().join("rollout.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
    live.ok(&["attachment", "create", &id, "--file", png.to_str().unwrap()]);
    let files = live.ok(&["attachment", "list", &id]);
    assert_eq!(files[0]["name"], "rollout.png", "newest first: {files}");
    assert_eq!(files[1]["size"], 44);
    let png_id = s(&files[0]["id"]).to_owned();
    live.exits(2, &["attachment", "get", &png_id]);
    let saved = live.dir.path().join("saved.png");
    live.ok(&[
        "attachment",
        "get",
        &png_id,
        "--output",
        saved.to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read(&saved).unwrap(), std::fs::read(&png).unwrap());

    // the tree, and search once the index has the page
    let tree = live.ok(&["tree", "get", &runbooks]);
    assert_eq!(tree["nodes"][0]["id"], id, "{tree}");
    let space = live.ok(&["tree", "get", "ENG", "--depth", "2"]);
    assert!(
        space["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == runbooks
                && node["parent"] == home.as_str()
                && node["depth"] == 2),
        "{space}"
    );
    let word = format!("{}", live.stamp);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let found = live.ok(&["page", "list", &word, "--space", "ENG", "--fields", "id"]);
        if found.as_array().unwrap().iter().any(|row| row["id"] == id) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "search never found {id}: {found}"
        );
        std::thread::sleep(Duration::from_secs(2));
    }
    let labelled = live.ok(&[
        "page", "list", "--label", "etl", "--author", "@me", "--since", "1h",
    ]);
    assert!(
        labelled
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == id && row["updated_by"] == "Jane Doe"),
        "{labelled}"
    );

    // the trash
    let (_, said) = live.exits(2, &["page", "delete", &other]);
    assert!(said.contains("--yes"), "{said}");
    let trashed = live.ok(&["page", "delete", &other, "--yes"]);
    assert_eq!(trashed["status"], "trashed");
    live.exits(4, &["page", "get", &other]);
}
