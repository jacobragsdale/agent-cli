//! The ado work item commands against a live Azure DevOps project, through
//! the built binary. Off unless `AGENT_CLI_TEST_ADO=1`; the credential is
//! the usual one (`AZURE_DEVOPS_EXT_PAT`, else `az login`). It runs on the
//! configured project, and on `AGENT_CLI_TEST_ADO_CONFIG` (a second
//! config.toml) when that is set. It writes: point it at a sandbox seeded
//! by `scripts/ado-sandbox.py`, never at a team's real project.
//!
//! Every work item it makes is tagged `agent-cli-e2e-run` and ends in its
//! type's Removed state (else a Completed one). A check whose precondition
//! the project lacks (no team, no next sprint, no saved query) prints why it
//! skipped instead of failing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

const TAG: &str = "agent-cli-e2e-run";

/// The requirement-level type, by process: Agile, Scrum, CMMI, Basic.
const STORY_TYPES: [&str; 4] = ["User Story", "Product Backlog Item", "Requirement", "Issue"];

/// Says which test skipped, as the two configs run side by side.
fn skip(what: &str, why: &str) {
    let test = std::thread::current().name().unwrap_or("").to_owned();
    eprintln!("skipped {what} in {test}: {why}");
}

/// One run of the binary.
struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("{e}: not JSON: {}\n{}", self.stdout, self.stderr))
    }
}

struct Live {
    /// None: the environment's own config.
    config: Option<String>,
    /// A fresh cache, so a day-old field list cannot hide a change.
    cache: tempfile::TempDir,
    /// Every work item made, with its type.
    made: RefCell<Vec<(i64, String)>>,
    /// The state each type's items end in.
    end: RefCell<HashMap<String, String>>,
    stamp: u64,
}

impl Live {
    fn new(config: Option<String>) -> Option<Self> {
        if std::env::var("AGENT_CLI_TEST_ADO").as_deref() != Ok("1") {
            skip(
                "live ado",
                "set AGENT_CLI_TEST_ADO=1 on a sandbox project (scripts/ado-sandbox.py)",
            );
            return None;
        }
        Some(Self {
            config,
            cache: tempfile::tempdir().unwrap(),
            made: RefCell::new(Vec::new()),
            end: RefCell::new(HashMap::new()),
            stamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        })
    }

    fn run(&self, args: &[&str]) -> Ran {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-cli"));
        command
            .args(args)
            .env("XDG_CACHE_HOME", self.cache.path())
            .env_remove("AGENT_CLI_READ_ONLY");
        if let Some(config) = &self.config {
            command.env("AGENT_CLI_CONFIG", config);
        }
        let output = command.output().unwrap();
        Ran {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs `args`, which must succeed, and reads what it printed.
    fn ok(&self, args: &[&str]) -> Value {
        let ran = self.run(args);
        assert_eq!(
            ran.code,
            0,
            "agent-cli {}\n{}{}",
            args.join(" "),
            ran.stdout,
            ran.stderr
        );
        ran.json()
    }

    /// Makes a tagged work item, remembered for the cleanup.
    fn make(&self, kind: &str, title: &str, more: &[&str]) -> i64 {
        let title = format!("e2e {} {title}", self.stamp);
        let mut args = vec![
            "ado", "workitem", "create", "--type", kind, "--title", &title, "--tags", TAG,
        ];
        args.extend(more);
        let id = self.ok(&args)["id"].as_i64().unwrap();
        self.made.borrow_mut().push((id, kind.to_owned()));
        id
    }

    /// The state `kind`'s work ends in: its Removed category, else Completed.
    fn end_state(&self, kind: &str) -> String {
        let got = self.ok(&["ado", "workitem-type", "get", kind, "--fields", "states"]);
        let states = got["states"].as_array().unwrap();
        let named = |category: &str| {
            states
                .iter()
                .find(|s| s["category"] == category)
                .and_then(|s| s["name"].as_str())
        };
        named("Removed")
            .or_else(|| named("Completed"))
            .unwrap()
            .to_owned()
    }

    /// Every check; answers the requirement-level type.
    fn check_all(&self) -> String {
        let story = self.types();
        let team = self.people_and_sprints();
        let me = self.fields_comments_and_mentions(&story);
        let first = self.made.borrow()[0].0;
        self.rank(&story, first, team.is_some());
        self.relations_tree_and_history(first);
        self.queries();
        self.attachments(first);
        match &team {
            Some(_) => self.sprint_complete(),
            None => skip("sprint complete", "the config names no team"),
        }
        self.activity(first, &me);
        story
    }

    /// The first config on an item of this one's project, whose type and
    /// states (another process's, perhaps) are read from the item's project.
    fn seen_from_the_first_config(&self, story: &str) {
        let id = self.make(story, "seen from the first config", &[]);
        let id = id.to_string();
        let Some(first) = Live::new(None) else { return };
        first.ok(&["ado", "history", "get", &id, "--field", "State"]);
        first.ok(&["ado", "tree", "get", &id]);
        first.ok(&[
            "ado",
            "workitem",
            "comment",
            &id,
            "seen from the first config",
        ]);
        let end = self.end.borrow()[story].clone();
        let done = first.ok(&["ado", "workitem", "update", &id, "--state", &end]);
        assert_eq!(done["state"], end.as_str(), "{done}");
    }

    /// `workitem-type list|get`: `required` is only what a caller must set.
    fn types(&self) -> String {
        let types = self.ok(&["ado", "workitem-type", "list", "--fields", "name"]);
        let names: Vec<&str> = types
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"Task"), "{names:?}");
        let story = STORY_TYPES
            .into_iter()
            .find(|kind| names.contains(kind))
            .expect("a requirement-level type")
            .to_owned();
        for kind in ["Task", story.as_str()] {
            let got = self.ok(&["ado", "workitem-type", "get", kind]);
            let required: Vec<&str> = got["fields"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|f| f["required"] == true)
                .filter_map(|f| f["ref"].as_str())
                .collect();
            assert!(required.contains(&"System.Title"), "{kind}: {required:?}");
            for filled in ["System.IterationId", "System.AreaId", "System.State"] {
                assert!(!required.contains(&filled), "{kind}: {required:?}");
            }
            self.end
                .borrow_mut()
                .insert(kind.to_owned(), self.end_state(kind));
        }
        story
    }

    /// `person list`, `sprint list|get`, `backlog list`, and `workitem list`
    /// by sprint, team, mention and follow. The team is the first person's.
    fn people_and_sprints(&self) -> Option<String> {
        let people = self.ok(&["ado", "person", "list", "--limit", "5"]);
        let Some(team) = people[0]["team"].as_str().map(str::to_owned) else {
            skip("sprints and backlog", "person list found no team");
            return None;
        };
        assert!(people[0]["id"].is_string(), "{people}");
        let sprints = self.ok(&["ado", "sprint", "list", "--team", &team]);
        assert!(!sprints.as_array().unwrap().is_empty(), "{sprints}");
        let current = self.run(&["ado", "sprint", "get", "--team", &team]);
        match current.code {
            0 => assert!(current.json()["path"].is_string()),
            _ => skip("sprint get @current", current.stderr.trim()),
        }
        let first = sprints[0]["path"].as_str().unwrap();
        let got = self.ok(&["ado", "sprint", "get", first, "--team", &team]);
        assert_eq!(got["path"], first);
        let name = sprints[0]["name"].as_str().unwrap();
        let listed = self.ok(&[
            "ado",
            "workitem",
            "list",
            "--iteration",
            name,
            "--team",
            &team,
            "--limit",
            "5",
        ]);
        assert!(listed.is_array());
        assert!(
            self.ok(&["ado", "backlog", "list", "--limit", "5"])
                .is_array()
        );
        for when in ["@next", "@previous"] {
            let ran = self.run(&[
                "ado",
                "workitem",
                "list",
                "--iteration",
                when,
                "--team",
                &team,
                "--limit",
                "5",
            ]);
            match ran.code {
                0 => assert!(ran.json().is_array()),
                4 => skip(
                    &format!("workitem list --iteration {when}"),
                    ran.stderr.trim(),
                ),
                _ => panic!("--iteration {when}: {}{}", ran.stdout, ran.stderr),
            }
        }
        assert!(
            self.ok(&["ado", "workitem", "list", "--following", "--limit", "5"])
                .is_array()
        );
        Some(team)
    }

    /// create and update with `--field`, `--comment` and a mention; the
    /// mention lands in `workitem list --mentioned`. Returns who signed in.
    fn fields_comments_and_mentions(&self, story: &str) -> String {
        let id = self.make(
            story,
            "fields",
            &["--assignee", "@me", "--field", "Priority=3"],
        );
        let created = self.ok(&["ado", "workitem", "get", &id.to_string()]);
        assert_eq!(created["priority"], 3, "{created}");
        let me = created["assignee"].as_str().unwrap().to_owned();
        let mention = format!("Checked by @<{me}> in the live suite");
        let updated = self.ok(&[
            "ado",
            "workitem",
            "update",
            &id.to_string(),
            "--field",
            "Microsoft.VSTS.Common.Priority=2",
            "--comment",
            &mention,
        ]);
        assert_eq!(updated["priority"], 2, "{updated}");
        let got = self.ok(&["ado", "workitem", "get", &id.to_string(), "--comments", "1"]);
        let text = got["comments"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains(&format!("@<{me}>")), "{text}");
        // The mention index trails the write by a few seconds.
        let mut found = false;
        for _ in 0..10 {
            let mentioned = self.ok(&["ado", "workitem", "list", "--mentioned", "--limit", "200"]);
            found = mentioned
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == id);
            if found {
                break;
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        assert!(found, "workitem list --mentioned never listed {id}");
        me
    }

    /// `--below` then `--above` put it right next to the other item.
    fn rank(&self, story: &str, first: i64, team: bool) {
        if !team {
            skip("--above/--below", "the config names no team");
            return;
        }
        let other = self.make(story, "rank", &[]);
        let place = |args: &[&str]| -> Option<(i64, i64)> {
            let mut argv = vec!["ado", "workitem", "update"];
            argv.extend(args);
            self.ok(&argv);
            let backlog = self.ok(&[
                "ado", "backlog", "list", "--limit", "1000", "--fields", "id,rank",
            ]);
            let rank = |id: i64| {
                backlog
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"] == id)
                    .and_then(|row| row["rank"].as_i64())
            };
            Some((rank(first)?, rank(other)?))
        };
        let (a, b) = (first.to_string(), other.to_string());
        let Some((moved, anchor)) = place(&[&a, "--below", &b]) else {
            skip(
                "--above/--below",
                "the team's backlog does not hold new items",
            );
            return;
        };
        assert_eq!(moved, anchor + 1, "--below");
        let (moved, anchor) = place(&[&a, "--above", &b]).unwrap();
        assert_eq!(moved + 1, anchor, "--above");
    }

    /// `relation create|delete` with their exit codes, `tree get`, `history get`.
    fn relations_tree_and_history(&self, parent: i64) {
        let child = self.make("Task", "child", &[]);
        let (p, c) = (parent.to_string(), child.to_string());
        let link =
            |verb: &str, other: &str| self.run(&["ado", "relation", verb, &p, "--child", other]);
        assert_eq!(link("create", &c).json()["already_linked"], false);
        assert_eq!(link("create", &c).json()["already_linked"], true);
        let tree = self.ok(&["ado", "tree", "get", &p]);
        assert!(
            tree["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kid| kid["id"] == child),
            "{tree}"
        );
        assert_eq!(link("create", &p).code, 2, "linked to itself");
        assert_eq!(
            link("create", "2147483000").code,
            4,
            "a work item nobody has"
        );
        assert_eq!(link("delete", &c).json()["removed"], true);
        assert_eq!(link("delete", &c).code, 4, "a link that is gone");

        let history = self.ok(&["ado", "history", "get", &p]);
        let changes = history["changes"].as_array().unwrap();
        assert!(
            changes.iter().any(|c| c["comment"].is_string()),
            "{history}"
        );
        let priority = self.ok(&["ado", "history", "get", &p, "--field", "Priority"]);
        assert!(
            !priority["changes"].as_array().unwrap().is_empty(),
            "{priority}"
        );
    }

    /// `query list|run` on the seeded shared query, else the first one listed.
    fn queries(&self) {
        let queries = self.ok(&["ado", "query", "list", "--limit", "200"]);
        let queries = queries.as_array().unwrap();
        let chosen = queries
            .iter()
            .find(|q| q["name"] == "agent-cli e2e open")
            .or_else(|| queries.iter().find(|q| q["type"] == "flat"));
        let Some(id) = chosen.and_then(|q| q["id"].as_str()) else {
            skip("query run", "the project has no saved flat query");
            return;
        };
        assert!(
            self.ok(&["ado", "query", "run", id, "--limit", "5"])
                .is_array()
        );
    }

    /// `attachment create|list|get`: text prints, bytes go to --output.
    fn attachments(&self, id: i64) {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("e2e.log");
        std::fs::write(&text, "hello from the live suite\n").unwrap();
        let binary = dir.path().join("e2e.bin");
        let bytes: Vec<u8> = (0..=255u8).collect();
        std::fs::write(&binary, &bytes).unwrap();
        let item = id.to_string();
        let path = |p: &std::path::Path| p.to_str().unwrap().to_owned();
        let made = self.ok(&[
            "ado",
            "attachment",
            "create",
            &item,
            "--file",
            &path(&text),
            "--comment",
            "live suite",
        ]);
        let text_id = made["id"].as_str().unwrap().to_owned();
        let made = self.ok(&[
            "ado",
            "attachment",
            "create",
            &item,
            "--file",
            &path(&binary),
        ]);
        let binary_id = made["id"].as_str().unwrap().to_owned();
        let listed = self.ok(&["ado", "attachment", "list", &item]);
        for name in ["e2e.log", "e2e.bin"] {
            assert!(
                listed.as_array().unwrap().iter().any(|a| a["name"] == name),
                "{listed}"
            );
        }
        let got = self.ok(&["ado", "attachment", "get", &text_id]);
        assert_eq!(got["text"], "hello from the live suite\n");
        assert_eq!(
            self.run(&["ado", "attachment", "get", &binary_id]).code,
            2,
            "bytes need --output"
        );
        let saved = dir.path().join("saved.bin");
        self.ok(&[
            "ado",
            "attachment",
            "get",
            &binary_id,
            "--output",
            &path(&saved),
        ]);
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
    }

    /// A real `sprint complete`: an item made in a sprint whose other work is
    /// all finished (a dry run moves nothing) moves to the sprint before it,
    /// and nothing else does.
    fn sprint_complete(&self) {
        let sprints = self.ok(&["ado", "sprint", "list", "--fields", "path"]);
        let paths: Vec<&str> = sprints
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["path"].as_str())
            .collect();
        let complete = |from: &str, to: &str, how: &str| {
            self.ok(&["ado", "sprint", "complete", from, "--move-to", to, how])
        };
        let mut pairs = paths.windows(2).rev().map(|w| (w[0], w[1]));
        let Some(&(last_to, last_from)) = pairs.clone().next().as_ref() else {
            skip("sprint complete", "the team has fewer than two sprints");
            return;
        };
        let free = pairs.find(|(to, from)| {
            complete(from, to, "--dry-run")["would"]
                .as_array()
                .is_none_or(Vec::is_empty)
        });
        let (to, from) = free.unwrap_or((last_to, last_from));
        let id = self.make("Task", "rollover", &["--iteration", from]);
        if free.is_none() {
            let plan = complete(from, to, "--dry-run");
            let planned = plan["would"].as_array().unwrap().iter().any(|w| {
                w["url"]
                    .as_str()
                    .is_some_and(|url| url.contains(&format!("/{id}?")))
            });
            assert!(planned, "{plan}");
            skip(
                "sprint complete --yes",
                "every sprint holds unfinished work this suite did not make; the dry run planned the move",
            );
            return;
        }
        let done = complete(from, to, "--yes");
        let moved: Vec<&Value> = done["moved"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| &m["id"])
            .collect();
        if moved.is_empty() {
            skip(
                "sprint complete",
                "the team's sprint does not hold new items",
            );
            return;
        }
        assert_eq!(moved, [&Value::from(id)], "{done}");
        let got = self.ok(&["ado", "workitem", "get", &id.to_string()]);
        assert_eq!(got["iteration"], to);
    }

    /// `activity list` shows this run's work item changes.
    fn activity(&self, id: i64, me: &str) {
        let rows = self.ok(&["ado", "activity", "list", "--since", "2h", "--limit", "500"]);
        let rows = rows.as_array().unwrap();
        for kind in ["workitem", "comment"] {
            assert!(
                rows.iter()
                    .any(|row| row["id"].as_str() == Some(&*id.to_string()) && row["kind"] == kind),
                "no {kind} row for {id}: {rows:?}"
            );
        }
        let named = self.ok(&["ado", "activity", "list", "--person", me, "--since", "2h"]);
        assert!(!named.as_array().unwrap().is_empty());
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let end = self.end.borrow();
        for (id, kind) in self.made.borrow().iter() {
            let Some(state) = end.get(kind) else { continue };
            let ran = self.run(&[
                "ado",
                "workitem",
                "update",
                &id.to_string(),
                "--state",
                state,
            ]);
            if ran.code != 0 {
                eprintln!("cleanup of {id} failed: {}", ran.stderr);
            }
        }
    }
}

#[test]
fn the_work_item_commands_hold_on_the_configured_project() {
    if let Some(live) = Live::new(None) {
        live.check_all();
    }
}

#[test]
fn the_work_item_commands_hold_on_the_second_config() {
    let Ok(config) = std::env::var("AGENT_CLI_TEST_ADO_CONFIG") else {
        skip(
            "the second config",
            "set AGENT_CLI_TEST_ADO_CONFIG to a config.toml",
        );
        return;
    };
    if let Some(live) = Live::new(Some(config)) {
        let story = live.check_all();
        live.seen_from_the_first_config(&story);
    }
}
