use agent_cli_core::{Ctx, When, command};
use anyhow::Result;

use crate::client::{Ado, full_ref, list, query_value};

use super::{RunRow, run_row};

#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
enum RunStatus {
    InProgress,
    NotStarted,
    Cancelling,
    Completed,
    All,
}

#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
enum RunResult {
    Succeeded,
    PartiallySucceeded,
    Failed,
    Canceled,
}

/// Why a run was queued, as Azure DevOps names it.
#[derive(Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
enum RunReason {
    Manual,
    #[value(name = "individualCI")]
    IndividualCi,
    #[value(name = "batchedCI")]
    BatchedCi,
    Schedule,
    PullRequest,
    BuildCompletion,
    ResourceTrigger,
}

#[derive(clap::Args)]
pub struct RunListArgs {
    /// Pipeline name or id
    #[arg(long)]
    pipeline: Option<String>,
    /// The branch or tag it built: main, v1.4.2, refs/heads/main, refs/tags/v1.4.2
    #[arg(long)]
    branch: Option<String>,
    /// Queued after this
    #[arg(long)]
    since: Option<When>,
    /// Queued before this
    #[arg(long)]
    until: Option<When>,
    /// Runs in this state
    #[arg(long, value_enum)]
    status: Option<RunStatus>,
    /// Finished runs with this result
    #[arg(long, value_enum)]
    result: Option<RunResult>,
    /// Who queued it: name, email or @me
    #[arg(long)]
    requested_by: Option<String>,
    /// Why it ran: a push (individualCI, batchedCI), a PR, a schedule, by hand …
    #[arg(long, value_enum, ignore_case = true)]
    reason: Option<RunReason>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn run_list(ctx: &Ctx, args: RunListArgs) -> Result<Vec<RunRow>> {
    let ado = Ado::load(ctx)?;
    let mut query = format!("queryOrder=queueTimeDescending&$top={}", args.limit + 1);
    if let Some(pipeline) = &args.pipeline {
        query.push_str(&format!("&definitions={}", ado.pipeline_id(ctx, pipeline)?));
    }
    for (key, when) in [("minTime", args.since), ("maxTime", args.until)] {
        if let Some(when) = when {
            query.push_str(&format!("&{key}={}", query_value(&when.utc())));
        }
    }
    if let Some(status) = args.status {
        let status = match status {
            RunStatus::InProgress => "inProgress",
            RunStatus::NotStarted => "notStarted",
            RunStatus::Cancelling => "cancelling",
            RunStatus::Completed => "completed",
            RunStatus::All => "all",
        };
        query.push_str(&format!("&statusFilter={status}"));
    }
    if let Some(result) = args.result {
        let result = match result {
            RunResult::Succeeded => "succeeded",
            RunResult::PartiallySucceeded => "partiallySucceeded",
            RunResult::Failed => "failed",
            RunResult::Canceled => "canceled",
        };
        query.push_str(&format!("&resultFilter={result}"));
    }
    if let Some(who) = &args.requested_by {
        // The builds API takes the person by name, as `az pipelines runs
        // list --requested-for` sends them.
        let who = if who.trim().eq_ignore_ascii_case("@me") {
            ado.me(ctx)?.name
        } else {
            who.trim().to_owned()
        };
        query.push_str(&format!("&requestedFor={}", query_value(&who)));
    }
    if let Some(reason) = args
        .reason
        .and_then(|reason| clap::ValueEnum::to_possible_value(&reason))
    {
        query.push_str(&format!("&reasonFilter={}", reason.get_name()));
    }
    // A bare name is a branch or a tag (images are tagged with the git tag
    // that built them): the branch first, the tag when no branch has runs.
    let refs: Vec<Option<String>> = match args.branch.as_deref().map(str::trim) {
        None => vec![None],
        Some(full) if full.starts_with("refs/") => vec![Some(full.to_owned())],
        Some(name) => vec![Some(full_ref(name)), Some(format!("refs/tags/{name}"))],
    };
    let mut rows: Vec<RunRow> = Vec::new();
    for reference in refs {
        let mut query = query.clone();
        if let Some(reference) = reference {
            query.push_str(&format!("&branchName={}", query_value(&reference)));
        }
        let answer = ado.get(ctx, &ado.code("build/builds", &query))?;
        rows = list(&answer["value"]).iter().map(run_row).collect();
        if !rows.is_empty() {
            break;
        }
    }
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[latest {}; --limit N, or narrow with --pipeline --branch --since]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub RUN_LIST = ["ado", "run", "list"], Read,
    "List pipeline runs, newest first",
    keywords: ["builds", "history", "recent", "latest", "failed", "status", "ci", "tag", "git", "release", "started", "triggered", "queued", "manual", "scheduled", "pr"],
    example: "ado run list --branch refs/tags/v1.4.2 --fields id,pipeline,status,result,finished",
    run: run_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, build, page, urls};

    #[test]
    fn run_list_filters_by_who_queued_it_and_why() {
        let me = Answer::json(&json!({"authenticatedUser": {"id": "u-1",
            "providerDisplayName": "Jane Doe", "properties": {"Account": {"$value": "jane@contoso.com"}}}}));
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--requested-by",
                "@me",
                "--reason",
                "manual",
                "--since",
                "2026-09-29",
                "--fields",
                "id,requested_by,reason",
            ],
            vec![me, page(vec![build(991, "completed", Some("succeeded"))])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": 991, "requested_by": "Jane Doe", "reason": "individualCI"}])
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&minTime=2026-09-29T00%3A00%3A00Z&requestedFor=Jane+Doe&reasonFilter=manual&api-version=7.1"
            )
        );

        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--requested-by",
                "sam@contoso.com",
                "--reason",
                "individualci",
            ],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            urls(&transport)[0]
                .contains("&requestedFor=sam%40contoso.com&reasonFilter=individualCI&"),
            "{:?}",
            urls(&transport)
        );
        let (outcome, _) = ado(&["ado", "run", "list", "--reason", "push"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn run_list_resolves_the_pipeline_by_name_and_filters_server_side() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--pipeline",
                "WEB-CI",
                "--branch",
                "main",
                "--status",
                "completed",
                "--result",
                "partiallySucceeded",
                "--limit",
                "1",
            ],
            vec![
                page(vec![
                    json!({"id": 12, "name": "web-ci", "path": "\\"}),
                    json!({"id": 14, "name": "web-ci-2"}),
                ]),
                page(vec![
                    build(991, "completed", Some("partiallySucceeded")),
                    build(990, "completed", Some("partiallySucceeded")),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows[0]["id"], 991);
        assert_eq!(rows[0]["pipeline"], "web-ci");
        assert_eq!(rows[0]["branch"], "main");
        assert_eq!(rows[0]["requested_by"], "Jane Doe");
        assert!(
            outcome.stderr.starts_with("[latest 1;"),
            "{}",
            outcome.stderr
        );
        let sent = urls(&transport);
        assert_eq!(
            sent[0],
            format!("{CODE}/build/definitions?name=WEB-CI&api-version=7.1")
        );
        assert_eq!(
            sent[1],
            format!(
                "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=2&definitions=12&statusFilter=completed&resultFilter=partiallySucceeded&branchName=refs%2Fheads%2Fmain&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &["ado", "run", "list", "--pipeline", "nope"],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado pipeline list nope"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_bare_name_is_a_branch_then_a_tag_and_a_window_is_minimum_and_maximum_time() {
        let mut tagged = build(8812, "completed", Some("succeeded"));
        tagged["sourceBranch"] = json!("refs/tags/v1.4.2");
        let (outcome, transport) = ado(
            &[
                "ado",
                "run",
                "list",
                "--branch",
                "v1.4.2",
                "--since",
                "2026-09-28",
                "--until",
                "2026-09-29T12:00:00+02:00",
            ],
            vec![page(vec![]), page(vec![tagged.clone()])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0]["branch"],
            "refs/tags/v1.4.2",
            "a tag keeps its refs/tags/, so it pastes back into --branch"
        );
        let window = "minTime=2026-09-28T00%3A00%3A00Z&maxTime=2026-09-29T10%3A00%3A00Z";
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&{window}&branchName=refs%2Fheads%2Fv1.4.2&api-version=7.1"
                ),
                format!(
                    "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&{window}&branchName=refs%2Ftags%2Fv1.4.2&api-version=7.1"
                ),
            ]
        );

        let (outcome, transport) = ado(
            &["ado", "run", "list", "--branch", "refs/tags/v1.4.2"],
            vec![page(vec![tagged])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 1, "a full ref is asked for once");
    }
}
