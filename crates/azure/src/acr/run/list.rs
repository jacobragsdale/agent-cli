//! `acr run list`: each registry's ACR Tasks runs, newest first, filtered by
//! ARM (`$filter` on the task and status, `$top`).

use agent_cli_core::{Ctx, Request, command, percent_encode};
use anyhow::Result;

use crate::acr::registries;
use crate::acr::run::{RUNS_API, RunRow, runs};
use crate::client::{ARM, bearer, limited, parallel};
use crate::config::Azure;
use crate::net::list;

#[derive(clap::Args)]
pub struct RunListArgs {
    /// Only this registry (repeatable; within [azure] registries)
    #[arg(long)]
    registry: Vec<String>,
    /// Only runs of this task
    #[arg(long)]
    task: Option<String>,
    /// Only runs in this state
    #[arg(long, value_enum)]
    status: Option<Status>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A run's state, as the runs filter spells it.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Status {
    Queued,
    Started,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Error,
    Timeout,
}

/// The single quotes of an OData string literal.
fn literal(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

fn run_list(ctx: &Ctx, args: RunListArgs) -> Result<Vec<RunRow>> {
    let azure = Azure::load(ctx)?;
    let registries = registries(ctx, &azure, &args.registry)?;
    // VERIFY(work): the filter's field names and that runs come newest first,
    // as `az acr task list-runs` assumes.
    let filter: Vec<String> = [
        args.task
            .as_deref()
            .map(|task| format!("TaskName eq {}", literal(task))),
        args.status
            .map(|status| format!("Status eq {}", literal(&format!("{status:?}")))),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut query = format!("api-version={RUNS_API}&$top={}", args.limit + 1);
    if !filter.is_empty() {
        query.push_str("&$filter=");
        percent_encode(&filter.join(" and "), &mut query);
    }
    let mint = bearer(ctx, ARM);
    // ponytail: the first page only, bounded by $top; follow nextLink when a
    // --limit past one page is wanted.
    let read = parallel(&registries, azure.parallel(), |registry| {
        let url = format!("https://management.azure.com{}?{query}", runs(registry, ""));
        let answer = ctx.read(Request::get(url).auth(&mint))?.json()?;
        Ok::<_, anyhow::Error>(
            list(&answer["value"])
                .iter()
                .filter_map(|run| RunRow::new(registry, run))
                .collect::<Vec<_>>(),
        )
    });
    let mut rows = Vec::new();
    for found in read {
        rows.extend(found?);
    }
    // RFC 3339 in UTC sorts as text.
    rows.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub RUN_LIST = ["acr", "run", "list"], Read,
    "List ACR Tasks runs, newest first: outcome, trigger and the images they pushed",
    keywords: ["tasks", "build", "history", "pushed", "trigger", "commit", "failed"],
    example: "acr run list --status failed --fields id,task,status,error",
    run: run_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::ACR;
    use crate::testing::{RG, azure, one_registry};

    #[test]
    fn runs_are_listed_newest_first_with_their_images_and_trigger() {
        let (outcome, transport) = azure(
            &[ACR],
            &[
                "acr",
                "run",
                "list",
                "--task",
                "api-build",
                "--status",
                "succeeded",
                "--limit",
                "1",
            ],
            vec![
                one_registry(),
                Answer::json(&json!({"value": [
                    {"name": "ca1e", "properties": {"runId": "ca1e", "status": "Succeeded", "task": "api-build",
                        "createTime": "2026-09-27T10:00:00+00:00"}},
                    {"name": "ca1f", "properties": {"runId": "ca1f", "status": "Succeeded", "task": "api-build",
                        "runType": "AutoRun", "createTime": "2026-09-28T21:00:51.21+00:00",
                        "startTime": "2026-09-28T21:00:52+00:00", "finishTime": "2026-09-28T21:02:41+00:00",
                        "sourceTrigger": {"eventType": "commit", "commitId": "3f9e2a1c0b", "branchName": "main"},
                        "outputImages": [{"registry": "contosoacr.azurecr.io", "repository": "api", "tag": "v1.4.2",
                            "digest": "sha256:7a"}]}},
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[1].url,
            format!(
                "https://management.azure.com{RG}/providers/Microsoft.ContainerRegistry/registries/contosoacr/runs?api-version=2019-06-01-preview&$top=2&$filter=TaskName+eq+%27api-build%27+and+Status+eq+%27Succeeded%27"
            )
        );
        assert_eq!(
            outcome.json(),
            json!([{"id": "contosoacr/ca1f", "registry": "contosoacr", "run": "ca1f", "task": "api-build",
                "status": "Succeeded", "trigger": "commit 3f9e2a1 on main",
                "images": ["contosoacr.azurecr.io/api:v1.4.2"],
                "created": "2026-09-28T21:00:51Z", "started": "2026-09-28T21:00:52Z",
                "finished": "2026-09-28T21:02:41Z", "duration": 109}])
        );
        assert!(
            outcome.stderr.contains("[1 of 2; --limit N]"),
            "{}",
            outcome.stderr
        );
    }
}
