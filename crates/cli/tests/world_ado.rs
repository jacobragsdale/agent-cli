//! What the contoso world answers of ado. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use agent_cli_core::testing::next_command;
use serde_json::{Value, json};
use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_ado() {
    let queue = ok(&["ado", "pr", "list", "--vote", "none", "--fields", "id"]);
    assert_eq!(queue, json!([]), "nothing waits on your review");
    let urgent = ok(&[
        "ado",
        "workitem",
        "list",
        "--type",
        "Bug",
        "--priority",
        "1,2",
        "--fields",
        "id,priority",
    ]);
    assert_eq!(urgent, json!([{"id": 1218, "priority": 1}]));
    let yours = ok(&[
        "ado",
        "run",
        "list",
        "--requested-by",
        "@me",
        "--fields",
        "id",
    ]);
    assert_eq!(
        yours,
        json!([{"id": 8814}, {"id": 8812}, {"id": 8809}, {"id": 8801}])
    );
    for args in [
        &["ado", "run", "get", "8809", "--fields", "failed"][..],
        &["ado", "run", "logs", "8809"],
        &["ado", "run", "list", "--since", "1d"],
        &["ado", "run", "list", "--branch", "v1.4.2"],
        &["ado", "pr", "get", "431"],
        &["ado", "pr", "list"],
        &["ado", "workitem", "get", "AB#1207"],
        &[
            "ado",
            "workitem",
            "list",
            "--assignee",
            "@me",
            "--iteration",
            "@current",
        ],
        &["ado", "approval", "list"],
        &["ado", "pipeline", "list"],
    ] {
        ok(args);
    }
}

/// The value of a field that holds a string.
fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("no {field} in {value}"))
}

#[test]
fn f1_a_review_comment_is_read_with_its_code_and_answered() {
    let threads = ok(&["ado", "thread", "list", "436"]);
    assert_eq!(threads.as_array().map(Vec::len), Some(1), "{threads}");
    let thread = &threads[0];
    assert_eq!(thread["id"], "436/7");
    assert_eq!(
        (thread["file"].as_str(), thread["line"].as_u64()),
        (Some("src/Orders/OrderClient.cs"), Some(42))
    );
    assert!(
        string(thread, "code")
            .contains("42          var jitter = Random.Shared.NextDouble() * seconds;"),
        "the thread carries the line it is on: {thread}"
    );
    let file = ok(&[
        "ado",
        "file",
        "get",
        string(thread, "at"),
        "--fields",
        "id,commit,text",
    ]);
    assert_eq!(file["commit"], "a7e3c9f1d5b2e8a4c6f0d2b4e6a8c0e2f4a6b8d0");
    assert!(
        string(&file, "text").contains("Math.Pow(2, attempt)"),
        "{file}"
    );
    let change = ok(&[
        "ado",
        "diff",
        "get",
        "436",
        "--file",
        "src/Orders/OrderClient.cs",
        "--fields",
        "hunks",
    ]);
    assert!(
        string(&change[0]["hunks"][0], "diff").contains("+        var jitter"),
        "{change}"
    );
    let reply = ok(&[
        "ado",
        "thread",
        "comment",
        string(thread, "id"),
        "Capped at 30 s in 9f1c2e4",
        "--resolve",
    ]);
    assert_eq!(
        reply,
        json!({"id": "436/7", "comment_id": 3, "status": "fixed"})
    );
}

#[test]
fn f2_a_review_comments_on_a_hunk_and_votes() {
    let pr = ok(&[
        "ado",
        "pr",
        "get",
        "436",
        "--fields",
        "open_threads,threads",
    ]);
    let listed = ok(&["ado", "thread", "list", "436", "--fields", "id,at"]);
    assert_eq!(pr["open_threads"], 1);
    assert_eq!(
        (&pr["threads"][0]["id"], &pr["threads"][0]["at"]),
        (&listed[0]["id"], &listed[0]["at"]),
        "pr get and thread list hand on the same ids"
    );
    let names = ok(&[
        "ado",
        "diff",
        "get",
        "436",
        "--names-only",
        "--fields",
        "path,change",
    ]);
    assert_eq!(
        names,
        json!([{"path": "src/Orders/OrderClient.cs", "change": "edit"},
               {"path": "tests/Api.Tests/OrdersClientTests.cs", "change": "edit"}])
    );
    let hunks = ok(&[
        "ado",
        "diff",
        "get",
        "436",
        "--file",
        "*.cs",
        "--fields",
        "path,hunks",
    ]);
    let at = string(&hunks[0]["hunks"][0], "at");
    assert_eq!(
        at,
        "api@a7e3c9f1d5b2e8a4c6f0d2b4e6a8c0e2f4a6b8d0:src/Orders/OrderClient.cs:34-44"
    );
    let line = format!("{}:42", at.rsplit_once(':').unwrap().0);
    let posted = ok(&[
        "ado",
        "pr",
        "comment",
        "436",
        "Jitter is unbounded here",
        "--at",
        &line,
    ]);
    assert_eq!(posted, json!({"pr": 436, "thread_id": 8}));
    let vote = ok(&["ado", "pr", "vote", "436", "suggest"]);
    assert_eq!(vote["vote"], "suggestions");
}

#[test]
fn f3_code_search_finds_the_callers_and_their_neighbours() {
    let found = ok(&["ado", "code", "list", "IOrderClient", "--fields", "id,repo"]);
    let repos: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|row| string(row, "repo"))
        .collect();
    assert_eq!(repos, ["api", "api", "worker"]);
    let caller = string(&found[2], "id");
    assert_eq!(caller, "worker:src/Jobs/Retry.cs:18");
    let file = ok(&["ado", "file", "get", caller, "--fields", "text"]);
    assert!(
        string(&file, "text").contains("18      private readonly IOrderClient _orders;"),
        "{file}"
    );
    let folder = caller.rsplit_once('/').unwrap().0;
    let neighbours = ok(&["ado", "file", "list", folder, "--fields", "id"]);
    assert_eq!(neighbours[0]["id"], "worker:src/Jobs/Retry.cs");
    assert_eq!(neighbours.as_array().map(Vec::len), Some(3));
}

#[test]
fn f4_the_running_release_is_compared_with_the_one_before() {
    let deployments = ok(&["k8s", "deployment", "list", "--fields", "id,images"]);
    let image = string(&deployments[0]["images"][0], "image");
    assert!(image.ends_with("/api:v1.4.2"), "{image}");
    let range = "api@v1.4.1..v1.4.2";
    let names = ok(&[
        "ado",
        "diff",
        "get",
        range,
        "--names-only",
        "--fields",
        "path",
    ]);
    assert_eq!(
        names,
        json!([{"path": "src/Orders/OrderClient.cs"}, {"path": "tests/Api.Tests/OrdersClientTests.cs"}])
    );
    let change = ok(&[
        "ado",
        "diff",
        "get",
        range,
        "--file",
        "src/Orders/OrderClient.cs",
        "--fields",
        "at,added,removed,hunks",
    ]);
    assert_eq!(
        change[0]["at"],
        "api@4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9:src/Orders/OrderClient.cs"
    );
    assert!(
        string(&change[0]["hunks"][0], "diff")
            .contains("+                await Task.Delay(Backoff(response, attempt), cancel);"),
        "v1.4.2 holds PR 431's retry: {change}"
    );
}

#[test]
fn f5_a_pipeline_edit_is_previewed_and_its_template_error_read() {
    let run = ok(&["ado", "run", "get", "8809", "--fields", "failed"]);
    assert!(run["failed"].to_string().contains("Run tests"), "{run}");
    let pipeline = ok(&["ado", "pipeline", "get", "api-ci", "--fields", "yaml"]);
    assert_eq!(pipeline["yaml"], "api:azure-pipelines.yml");
    let file = ok(&[
        "ado",
        "file",
        "get",
        string(&pipeline, "yaml"),
        "--fields",
        "text",
    ]);
    // The file as it is, without the line numbers, and the agent's edit:
    // a parameter the test template does not declare.
    let mut edited = String::new();
    for line in string(&file, "text").lines() {
        let line = &line[4..];
        edited.push_str(line);
        edited.push('\n');
        if line.ends_with("project: tests/Api.Tests") {
            edited.push_str("              testTimeout: 20\n");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("azure-pipelines.yml");
    std::fs::write(&path, edited).unwrap();
    let refused = world::agent_cli(&[
        "ado",
        "pipeline",
        "preview",
        "api-ci",
        "--yaml-file",
        path.to_str().unwrap(),
    ]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert!(
        refused
            .stderr
            .contains("Unexpected parameter 'testTimeout'")
            && refused
                .stderr
                .contains("hint: agent-cli ado file get api:templates/dotnet-test.yml:1"),
        "{}",
        refused.stderr
    );
    let template = ok(&[
        "ado",
        "file",
        "get",
        "api:templates/dotnet-test.yml:1",
        "--fields",
        "text",
    ]);
    assert!(
        string(&template, "text").contains("- name: project"),
        "{template}"
    );
    let expanded = ok(&["ado", "pipeline", "preview", "api-ci", "--fields", "yaml"]);
    assert!(
        string(&expanded, "yaml").contains("displayName: Run tests"),
        "{expanded}"
    );
}

/// The file get a walk ended on: its id, and whether `line` is in its text.
fn landed(walked: &[world::Ran], line: &str) -> String {
    let last = walked.last().unwrap();
    assert_eq!(last.code, 0, "{}{}", last.stdout, last.stderr);
    let file = last.json();
    assert!(string(&file, "text").contains(line), "{file}");
    string(&file, "id").to_owned()
}

#[test]
fn f8_a_pull_requests_broken_build_leads_by_notes_to_the_line_that_broke() {
    let walked = world::follow(&["ado", "pr", "get", "436"]);
    let steps: Vec<String> = walked
        .iter()
        .filter_map(|ran| next_command(&ran.stderr).map(|argv| argv.join(" ")))
        .collect();
    assert_eq!(
        steps,
        [
            "ado run get 8814",
            "ado file get api@a7e3c9f1d5b2e8a4c6f0d2b4e6a8c0e2f4a6b8d0:src/Orders/OrderClient.cs:42"
        ]
    );
    assert!(
        walked[1]
            .stdout
            .contains("does not contain a definition for 'Shared'"),
        "{}",
        walked[1].stdout
    );
    assert_eq!(
        landed(
            &walked,
            "42          var jitter = Random.Shared.NextDouble() * seconds;"
        ),
        "api@a7e3c9f1d5b2e8a4c6f0d2b4e6a8c0e2f4a6b8d0:src/Orders/OrderClient.cs:22-45"
    );
}

#[test]
fn f9_a_failing_test_leads_by_notes_to_the_line_it_waited_on() {
    let walked = world::follow(&["ado", "run", "get", "8809"]);
    let steps: Vec<String> = walked
        .iter()
        .filter_map(|ran| next_command(&ran.stderr).map(|argv| argv.join(" ")))
        .collect();
    assert_eq!(
        steps,
        [
            "ado test list 8809",
            "ado file get api@4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9:src/Orders/OrderClient.cs:22"
        ]
    );
    let tests = walked[1].json();
    assert_eq!(tests.as_array().map(Vec::len), Some(3), "{tests}");
    assert_eq!(tests[0]["name"], "Api.Tests.OrdersClientTests.RetriesOn429");
    assert_eq!(tests[0]["failing_since"], 8809);
    landed(
        &walked,
        "22                  await Task.Delay(Backoff(response, attempt), cancel);",
    );
}

#[test]
fn a_new_run_names_the_wait_for_it() {
    let walked = world::follow(&["ado", "run", "create", "--pipeline", "api-ci"]);
    assert_eq!(walked.len(), 2);
    assert_eq!(walked[0].json()["id"], 8815);
    assert_eq!(walked[1].code, 0, "{}", walked[1].stderr);
    assert_eq!(walked[1].json()["result"], "succeeded");
}

#[test]
fn a_container_stack_frame_is_read_as_the_repository_file_it_names() {
    for args in [
        &[
            "ado",
            "file",
            "get",
            "Fabrikam/api@4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9:/app/src/Orders/OrderClient.cs:19",
        ][..],
        &[
            "ado",
            "file",
            "get",
            "/app/src/Orders/OrderClient.cs:19",
            "--repo",
            "api",
            "--ref",
            "v1.4.2",
        ],
    ] {
        let ran = world::agent_cli(args);
        assert_eq!(ran.code, 0, "{args:?}: {}", ran.stderr);
        assert!(
            ran.stderr
                .contains("[app/src/Orders/OrderClient.cs is src/Orders/OrderClient.cs]"),
            "{}",
            ran.stderr
        );
        let file = ran.json();
        assert_eq!(file["path"], "src/Orders/OrderClient.cs");
        assert_eq!(file["commit"], "4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9");
        assert!(
            string(&file, "text")
                .contains("19              using var response = await http.GetAsync($\"orders/{id}\", cancel);"),
            "{file}"
        );
    }
}

#[test]
fn a_files_history_names_the_pull_request_behind_each_change_and_its_diff() {
    let commits = ok(&["ado", "commit", "list", "api:src/Orders/OrderClient.cs"]);
    assert_eq!(commits.as_array().map(Vec::len), Some(2), "{commits}");
    assert_eq!(
        commits[0]["commit"],
        "4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9"
    );
    assert_eq!(
        commits[0]["pr"],
        json!({"id": 431, "title": "Retry on 429 from the orders service"})
    );
    assert!(commits[1].get("pr").is_none(), "{commits}");
    let change = ok(&[
        "ado",
        "diff",
        "get",
        string(&commits[0], "diff"),
        "--file",
        "src/Orders/OrderClient.cs",
    ]);
    assert!(
        change.to_string().contains("Backoff(response, attempt)"),
        "{change}"
    );
    let pr = ok(&["ado", "pr", "get", "431", "--fields", "work_items"]);
    assert_eq!(pr["work_items"], json!([1207, 1210]));
}

#[test]
fn several_files_are_read_in_one_call_in_the_order_given() {
    const AT: &str = "api@4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9";
    let both = ok(&[
        "ado",
        "file",
        "get",
        &format!("{AT}:tests/Api.Tests/OrdersClientTests.cs:58"),
        &format!("{AT}:src/Orders/OrderClient.cs:22"),
        "--fields",
        "path,lines",
    ]);
    assert_eq!(
        both,
        json!([
            {"path": "tests/Api.Tests/OrdersClientTests.cs", "lines": "38-61"},
            {"path": "src/Orders/OrderClient.cs", "lines": "2-37"}
        ])
    );
}

#[test]
fn airflow_dags_root_holds_the_dags_and_the_requirements_that_lack_the_crm_client() {
    let root = ok(&["ado", "file", "list", "airflow-dags", "--fields", "path"]);
    assert_eq!(
        root,
        json!([{"path": "README.md"}, {"path": "dags"}, {"path": "requirements.txt"}])
    );
    let tree = ok(&["ado", "file", "list", "airflow-dags", "--recursive"]);
    let dags = tree
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| string(entry, "path").starts_with("dags/"))
        .count();
    assert_eq!(dags, 4, "{tree}");
    let requirements = ok(&["ado", "file", "get", "airflow-dags:requirements.txt"]);
    let text = string(&requirements, "text");
    assert!(text.contains("contoso-orders"), "{text}");
    assert!(!text.contains("contoso-crm"), "{text}");
}

#[test]
fn the_dag_that_fails_to_import_came_in_one_commit_with_no_pull_request() {
    let commits = ok(&[
        "ado",
        "commit",
        "list",
        "airflow-dags:dags/customer_sync.py",
    ]);
    assert_eq!(
        commits,
        json!([{
            "commit": "b5d2e8f1c4a7b0d3e6f9a2c5b8d1e4f7a0c3b6d9",
            "author": "Sam Lee",
            "date": "2026-09-29T11:53:10Z",
            "message": "Add hourly customer sync from the CRM",
            "diff": "airflow-dags@8f2a4c6e0b1d3f5a7c9e2b4d6f8a0c1e3b5d7f9a..b5d2e8f1c4a7b0d3e6f9a2c5b8d1e4f7a0c3b6d9"
        }])
    );
}

#[test]
fn a_repositorys_history_names_pr_431_on_its_merge_alone() {
    let commits = ok(&["ado", "commit", "list", "api", "--fields", "commit,pr"]);
    let commits = commits.as_array().unwrap();
    assert_eq!(commits.len(), 5, "{commits:?}");
    assert_eq!(
        commits[0],
        json!({"commit": "4be1c0d2e8f1a9b3c5d7e9f1a2b3c4d5e6f7a8b9",
            "pr": {"id": 431, "title": "Retry on 429 from the orders service"}})
    );
    assert!(commits[1..].iter().all(|commit| commit.get("pr").is_none()));
    let by_path = ok(&[
        "ado",
        "commit",
        "list",
        "api",
        "--path",
        "src/Orders/OrderClient.cs",
        "--fields",
        "commit",
    ]);
    assert_eq!(by_path.as_array().map(Vec::len), Some(2), "{by_path}");
}

#[test]
fn a_pull_requests_failed_build_log_ends_in_the_compiler_error() {
    let logs = ok(&["ado", "run", "logs", "8814"]);
    assert_eq!(logs["logs"], json!(["Build"]));
    let text = string(&logs, "text");
    assert!(
        text.contains("/home/vsts/work/1/s/src/Orders/OrderClient.cs(42,29): error CS0117: 'Random' does not contain a definition for 'Shared'"),
        "{text}"
    );
}

#[test]
fn the_run_a_create_queued_reads_back_succeeded() {
    let run = ok(&[
        "ado",
        "run",
        "get",
        "8815",
        "--fields",
        "result,failed,pr,workitems",
    ]);
    assert_eq!(run["result"], "succeeded");
    assert_eq!(run.get("failed"), None, "{run}");
    assert_eq!(run["pr"]["id"], 431);
}

#[test]
fn a_bug_and_a_story_show_their_states_moves_and_required_fields() {
    let bug = ok(&[
        "ado",
        "workitem-type",
        "get",
        "Bug",
        "--fields",
        "states,transitions",
    ]);
    let states: Vec<&str> = bug["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|state| string(state, "name"))
        .collect();
    assert_eq!(states, ["New", "Active", "Resolved", "Closed"]);
    assert_eq!(bug["transitions"]["Closed"], json!(["Active"]));
    let story = ok(&["ado", "workitem-type", "get", "User Story"]);
    let value_area = story["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "Value Area")
        .unwrap_or_else(|| panic!("{story}"));
    assert_eq!(
        (&value_area["required"], &value_area["default"]),
        (&json!(true), &json!("Business"))
    );
}

#[test]
fn a_teammate_is_found_by_name_with_the_address_assignee_takes() {
    let found = ok(&["ado", "person", "list", "--text", "priya"]);
    assert_eq!(
        found,
        json!([{"id": "priya@contoso.com", "name": "Priya Patel", "team": "Fabrikam Team"}])
    );
}

#[test]
fn the_sprint_list_has_sprint_42_current_between_41_and_43() {
    let sprints = ok(&["ado", "sprint", "list", "--fields", "name,timeframe"]);
    assert_eq!(
        sprints,
        json!([{"name": "Sprint 41", "timeframe": "past"},
            {"name": "Sprint 42", "timeframe": "current"},
            {"name": "Sprint 43", "timeframe": "future"}])
    );
}

#[test]
fn the_current_sprint_shows_jane_overloaded_and_its_note_lists_her_work() {
    let walked = world::follow(&["ado", "sprint", "get"]);
    let sprint = walked[0].json();
    assert_eq!(sprint["name"], "Sprint 42");
    assert_eq!(sprint["working_days_left"], 4);
    let jane = &sprint["people"][0];
    assert_eq!(
        (
            &jane["name"],
            &jane["remaining_work"],
            &jane["capacity_left"]
        ),
        (&json!("Jane Doe"), &json!(22.0), &json!(18.0)),
        "{sprint}"
    );
    assert_eq!(walked.len(), 2, "{}", walked[0].stderr);
    let work = walked[1].json();
    let ids: Vec<&Value> = work
        .as_array()
        .unwrap()
        .iter()
        .map(|item| &item["id"])
        .collect();
    assert_eq!(ids, [&json!(1218), &json!(1215), &json!(1207)]);
}

#[test]
fn the_stories_backlog_is_ranked_with_points() {
    let backlog = ok(&["ado", "backlog", "list", "--fields", "rank,id,points"]);
    assert_eq!(
        backlog,
        json!([{"rank": 1, "id": 1216, "points": 2.0}, {"rank": 2, "id": 1207, "points": 3.0},
            {"rank": 3, "id": 1224, "points": 3.0}, {"rank": 4, "id": 1221, "points": 5.0}])
    );
}

#[test]
fn a_tasks_history_shows_its_reassignment_and_the_hours_it_sat_new() {
    let history = ok(&["ado", "history", "get", "1215"]);
    assert_eq!(history["states"][0]["state"], "New");
    assert_eq!(history["states"][0]["hours"], 163.2);
    assert_eq!(history["states"][1]["by"], "Jane Doe");
    let handed = &history["changes"][1];
    assert_eq!(
        handed["fields"],
        json!([{"field": "Assigned To", "old": "Sam Lee", "new": "Jane Doe"}])
    );
    assert_eq!(
        handed["comment"],
        "Handing this to @<Jane Doe>: you are in the retry code already."
    );
}

#[test]
fn an_epics_tree_rolls_up_its_stories_points_and_tasks_hours() {
    let tree = ok(&["ado", "tree", "get", "1190"]);
    assert_eq!(
        tree["rollup"],
        json!({"items": 6, "done": 1, "by_state": {"Active": 3, "Closed": 1, "Resolved": 2},
            "points": 7.0, "points_done": 2.0, "remaining_work": 6.0, "percent_done": 29})
    );
    let stories: Vec<&Value> = tree["children"][0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|story| &story["id"])
        .collect();
    assert_eq!(stories, [&json!(1201), &json!(1207), &json!(1216)]);
}

#[test]
fn the_triage_query_is_found_by_name_and_lists_the_new_bugs() {
    let queries = ok(&["ado", "query", "list", "--fields", "path"]);
    assert_eq!(
        queries,
        json!([{"path": "My Queries/Assigned to me"}, {"path": "Shared Queries/Triage"}])
    );
    let bugs = ok(&["ado", "query", "run", "Triage", "--fields", "id,priority"]);
    assert_eq!(
        bugs,
        json!([{"id": 1218, "priority": 1}, {"id": 1222, "priority": 3}])
    );
}

#[test]
fn the_crash_loop_bug_is_blocked_by_the_task_that_resets_the_password() {
    let bug = ok(&["ado", "workitem", "get", "1218", "--fields", "blocked_by"]);
    assert_eq!(bug["blocked_by"], json!([1219]));
    let task = ok(&[
        "ado",
        "workitem",
        "get",
        "1219",
        "--fields",
        "blocks,assignee",
    ]);
    assert_eq!(task, json!({"blocks": [1218], "assignee": "Priya Patel"}));
}
