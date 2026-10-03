use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, Kind, list, text};
use crate::ids::arg;
use crate::run::{build_url, built, built_tree, line_at};

use super::test_runs;

#[derive(Clone, Copy, clap::ValueEnum)]
enum Outcome {
    Failed,
    Passed,
    All,
}

#[derive(clap::Args)]
pub struct TestListArgs {
    /// The run (build) whose tests to show: 8809, #8809 or its web URL
    run: String,
    /// Which results
    #[arg(long, value_enum, default_value_t = Outcome::Failed)]
    outcome: Outcome,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// One test's result in a run.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TestResult {
    /// The automated test's full name.
    name: Option<String>,
    /// Failed, Passed, NotExecuted …
    outcome: Option<String>,
    duration_ms: Option<u64>,
    /// The error message.
    error: Option<String>,
    /// The innermost frame in the repository, as file get takes it.
    at: Option<String>,
    /// The run it began failing in (this one for a new failure), for run get.
    failing_since: Option<i64>,
    /// The stack trace, its first 20 lines.
    stack: Option<String>,
}

/// Lines of a stack trace kept.
const STACK_LINES: usize = 20;

/// Each frame's path and line, innermost first: .NET (`in PATH:line N`)
/// and JavaScript (`at f (PATH:L:C)`) print the innermost first, Python
/// (`File "PATH", line N`) last. A frame without a path (framework code
/// built without sources) is skipped.
fn frames(stack: &str) -> Vec<(String, usize)> {
    let number = |raw: &str| raw.trim().parse::<usize>().ok().filter(|n| *n > 0);
    let mut python = false;
    let mut found = Vec::new();
    for line in stack.lines().map(str::trim) {
        let frame = if let Some(rest) = line.strip_prefix("File \"") {
            python = true;
            rest.split_once("\", line ").and_then(|(path, rest)| {
                Some((path, number(rest.split(',').next().unwrap_or(rest))?))
            })
        } else if let Some((path, rest)) = line
            .rsplit_once(" in ")
            .and_then(|(_, place)| place.rsplit_once(":line "))
        {
            number(rest).map(|line| (path, line))
        } else if let Some(rest) = line.strip_prefix("at ") {
            let place = rest
                .rsplit_once('(')
                .map_or(rest, |(_, place)| place)
                .trim_end_matches(')');
            let mut parts = place.rsplitn(3, ':');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(_column), Some(line), Some(path)) => number(line).map(|line| (path, line)),
                _ => None,
            }
        } else {
            None
        };
        if let Some((path, line)) = frame {
            let path = path.strip_prefix("file://").unwrap_or(path);
            found.push((path.to_owned(), line));
        }
    }
    if python {
        found.reverse();
    }
    found
}

fn test_list(ctx: &Ctx, args: TestListArgs) -> Result<Vec<TestResult>> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::Run, &args.run)?;
    let build = ado.get(ctx, &build_url(&ado, id))?;
    let outcomes = match args.outcome {
        Outcome::Failed => "outcomes=Failed&",
        Outcome::Passed => "outcomes=Passed&",
        Outcome::All => "",
    };
    let mut results = Vec::new();
    for run in test_runs(ctx, &ado, id)? {
        let Some(run) = run["id"].as_i64() else {
            continue;
        };
        if results.len() > args.limit {
            break;
        }
        let url = ado.code(
            &format!("test/runs/{run}/results"),
            &format!("{outcomes}$top={}", args.limit.saturating_add(1)),
        );
        results.extend(list(&ado.get(ctx, &url)?["value"]).iter().cloned());
    }
    let more = results.len() > args.limit;
    results.truncate(args.limit);
    let stacks: Vec<Vec<(String, usize)>> = results
        .iter()
        .map(|result| frames(result["stackTrace"].as_str().unwrap_or_default()))
        .collect();
    let built = built(&build);
    let files = built_tree(
        ctx,
        &ado,
        built.as_ref(),
        stacks.iter().any(|s| !s.is_empty()),
    )?;
    let rows: Vec<TestResult> = results
        .iter()
        .zip(&stacks)
        .map(|(result, frames)| TestResult {
            name: text(&result["automatedTestName"]).or_else(|| text(&result["testCaseTitle"])),
            outcome: text(&result["outcome"]),
            duration_ms: result["durationInMs"].as_f64().map(|ms| ms.round() as u64),
            error: text(&result["errorMessage"]),
            at: built.as_ref().and_then(|built| {
                frames
                    .iter()
                    .find_map(|(path, line)| line_at(built, &files, path, *line))
            }),
            failing_since: {
                let build = &result["failingSince"]["build"]["id"];
                build
                    .as_i64()
                    .or_else(|| build.as_str().and_then(|id| id.parse().ok()))
            },
            stack: text(&result["stackTrace"]).map(|stack| {
                stack
                    .lines()
                    .take(STACK_LINES)
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        })
        .collect();
    if more {
        ctx.note(format!("[first {}; --limit N for more]", args.limit));
    }
    // Its rows are failures to act on, not a page: the first one's line is
    // where to look next.
    if let Some(at) = rows
        .iter()
        .filter(|row| row.outcome.as_deref() == Some("Failed"))
        .find_map(|row| row.at.as_deref())
    {
        ctx.note(format!("[next: agent-cli ado file get {}]", arg(at)));
    }
    Ok(rows)
}

command! {
    pub TEST_LIST = ["ado", "test", "list"], Read,
    "List a run's failing tests: message, stack and the repository line",
    keywords: ["tests", "failed", "failing", "unit", "stack", "trace", "assert", "results", "flaky", "since"],
    example: "ado test list 8809 --fields name,error,at,failing_since",
    run: test_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::frames;
    use crate::testing::{CODE, ado, build, page, urls};

    #[test]
    fn frames_come_innermost_first_from_dotnet_python_and_javascript() {
        let dotnet = "   at System.Threading.Tasks.Task.Delay(TimeSpan delay)\n   at Contoso.OrderClient.GetAsync(Int32 id) in /home/vsts/work/1/s/src/Orders/OrderClient.cs:line 22\n   at Api.Tests.OrdersClientTests.RetriesOn429() in /home/vsts/work/1/s/tests/OrdersClientTests.cs:line 58\n--- End of stack trace from previous location ---";
        assert_eq!(
            frames(dotnet),
            [
                (
                    "/home/vsts/work/1/s/src/Orders/OrderClient.cs".to_owned(),
                    22
                ),
                (
                    "/home/vsts/work/1/s/tests/OrdersClientTests.cs".to_owned(),
                    58
                )
            ]
        );
        let python = "Traceback (most recent call last):\n  File \"/app/tests/test_x.py\", line 12, in test_x\n    load()\n  File \"/app/src/x.py\", line 5, in load\n    raise ValueError\nValueError";
        assert_eq!(
            frames(python),
            [
                ("/app/src/x.py".to_owned(), 5),
                ("/app/tests/test_x.py".to_owned(), 12)
            ]
        );
        let js = "Error: boom\n    at load (/app/src/x.js:10:5)\n    at /app/test/x.test.js:3:1\n    at process.processTicksAndRejections (node:internal/process/task_queues:95:5)";
        assert_eq!(
            frames(js)[..2],
            [
                ("/app/src/x.js".to_owned(), 10),
                ("/app/test/x.test.js".to_owned(), 3)
            ]
        );
    }

    #[test]
    fn a_runs_failures_carry_their_repository_line_and_the_first_is_the_next_step() {
        let mut built = build(8809, "completed", Some("failed"));
        built["repository"] = json!({"id": "r-1", "type": "TfsGit", "name": "web"});
        let stack = "   at Web.Login.Check() in /home/vsts/work/1/s/src/Login.cs:line 7\n   at Web.Tests.LoginTests.Rejects() in /home/vsts/work/1/s/tests/LoginTests.cs:line 30";
        let (outcome, transport) = ado(
            &["ado", "test", "list", "8809", "--limit", "1"],
            vec![
                Answer::json(&built),
                page(vec![json!({"id": 501, "totalTests": 9, "passedTests": 7})]),
                page(vec![
                    json!({"automatedTestName": "Web.Tests.LoginTests.Rejects", "outcome": "Failed",
                        "durationInMs": 1203.6, "errorMessage": "Assert.False() Failure",
                        "stackTrace": stack, "failingSince": {"build": {"id": 8807}}}),
                    json!({"automatedTestName": "Web.Tests.LoginTests.Locks", "outcome": "Failed"}),
                ]),
                page(vec![
                    json!({"path": "/src/Login.cs"}),
                    json!({"path": "/tests/LoginTests.cs"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "Web.Tests.LoginTests.Rejects", "outcome": "Failed", "duration_ms": 1204,
                "error": "Assert.False() Failure", "at": "web@abc123:src/Login.cs:7",
                "failing_since": 8807, "stack": stack.trim()}])
        );
        assert!(
            outcome
                .stderr
                .contains("[next: agent-cli ado file get web@abc123:src/Login.cs:7]"),
            "{}",
            outcome.stderr
        );
        assert!(outcome.stderr.contains("[first 1; --limit N for more]"));
        let sent = urls(&transport);
        assert_eq!(
            sent[1],
            format!(
                "{CODE}/test/runs?buildUri=vstfs%3A%2F%2F%2FBuild%2FBuild%2F8809&api-version=7.1"
            )
        );
        assert_eq!(
            sent[2],
            format!("{CODE}/test/runs/501/results?outcomes=Failed&$top=2&api-version=7.1")
        );
        assert!(sent[3].contains("recursionLevel=Full"), "{}", sent[3]);
        assert!(sent[3].contains("versionDescriptor.version=abc123"));
    }

    #[test]
    fn passed_tests_have_no_stack_so_the_tree_is_not_read() {
        let (outcome, transport) = ado(
            &["ado", "test", "list", "8809", "--outcome", "passed"],
            vec![
                Answer::json(&build(8809, "completed", Some("succeeded"))),
                page(vec![json!({"id": 501})]),
                page(vec![
                    json!({"automatedTestName": "A.B", "outcome": "Passed"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "A.B", "outcome": "Passed"}])
        );
        assert!(!outcome.stderr.contains("[next:"), "{}", outcome.stderr);
        assert!(urls(&transport)[2].contains("outcomes=Passed&"));
    }
}
