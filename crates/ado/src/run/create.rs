use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use serde_json::{Map, json};

use crate::client::{Ado, full_ref, short_branch, stamp, text};

use super::RunRow;

#[derive(clap::Args)]
pub struct RunCreateArgs {
    /// Pipeline name or id
    #[arg(long)]
    pipeline: String,
    /// The branch to build (default: the pipeline's)
    #[arg(long)]
    branch: Option<String>,
    /// A template parameter as name=value (repeatable)
    #[arg(long)]
    param: Vec<String>,
}

fn run_create(ctx: &Ctx, args: RunCreateArgs) -> Result<RunRow> {
    let ado = Ado::load(ctx)?;
    let mut parameters = Map::new();
    for pair in &args.param {
        let Some((name, value)) = pair.split_once('=') else {
            return Err(Failure::usage(format!("--param takes name=value, not {pair:?}")).into());
        };
        parameters.insert(name.trim().to_owned(), value.into());
    }
    let pipeline = ado.pipeline_id(ctx, &args.pipeline)?;
    let mut body = json!({"templateParameters": parameters});
    if let Some(branch) = &args.branch {
        body["resources"] = json!({"repositories": {"self": {"refName": full_ref(branch)}}});
    }
    let url = ado.code(&format!("pipelines/{pipeline}/runs"), "");
    let run = ado.change(ctx, Effect::Write, Method::Post, &url, body)?;
    // The pipelines endpoint answers in its own shape.
    Ok(RunRow {
        id: run["id"].as_i64().unwrap_or_default(),
        pipeline: text(&run["pipeline"]["name"]),
        pipeline_id: run["pipeline"]["id"].as_i64().or(Some(pipeline)),
        build_number: text(&run["name"]),
        status: text(&run["state"]),
        result: text(&run["result"]),
        branch: run["resources"]["repositories"]["self"]["refName"]
            .as_str()
            .map(short_branch),
        commit: None,
        requested_by: None,
        reason: None,
        queued: stamp(&run["createdDate"]),
        started: None,
        finished: None,
        url: text(&run["_links"]["web"]["href"]),
    })
}

command! {
    pub RUN_CREATE = ["ado", "run", "create"], Write,
    "Start a pipeline run on a branch, with template parameters",
    keywords: ["trigger", "queue", "start", "kick", "off", "build", "deploy", "launch"],
    example: "ado run create --pipeline web-ci --branch 42-fix-login",
    run: run_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, page};

    #[test]
    fn create_resolves_the_pipeline_then_plans_the_run_with_its_parameters() {
        let plans = dry_run(
            &[
                "ado",
                "run",
                "create",
                "--pipeline",
                "web-ci",
                "--branch",
                "42-fix",
                "--param",
                "env=staging",
                "--param",
                "debug=",
            ],
            vec![page(vec![json!({"id": 12, "name": "web-ci"})])],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/pipelines/12/runs?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"templateParameters": {"env": "staging", "debug": ""},
                "resources": {"repositories": {"self": {"refName": "refs/heads/42-fix"}}}})
        );

        let (outcome, _) = ado(
            &["ado", "run", "create", "--pipeline", "12"],
            vec![Answer::json(
                &json!({"id": 995, "name": "20260929.4", "state": "inProgress",
                "pipeline": {"id": 12, "name": "web-ci"}, "createdDate": "2026-09-29T11:00:00Z",
                "resources": {"repositories": {"self": {"refName": "refs/heads/main"}}}}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": 995, "pipeline": "web-ci", "pipeline_id": 12, "build_number": "20260929.4",
                "status": "inProgress", "branch": "main", "queued": "2026-09-29T11:00:00Z"})
        );
        let (outcome, _) = ado(
            &[
                "ado",
                "run",
                "create",
                "--pipeline",
                "12",
                "--param",
                "oops",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
