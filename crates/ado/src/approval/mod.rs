//! The approvals a run waits on: deployment gates.

pub(crate) mod approve;
pub(crate) mod list;
pub(crate) mod reject;

use agent_cli_core::{Ctx, Effect, Method};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, PREVIEW_API, text};

#[derive(clap::Args)]
pub struct AnswerArgs {
    /// The approval's id, from approval list
    id: String,
    /// Why, for the record
    #[arg(long)]
    comment: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Answered {
    id: String,
    /// approved or rejected, as Azure DevOps recorded it.
    status: Option<String>,
}

fn answer(ctx: &Ctx, args: AnswerArgs, status: &str) -> Result<Answered> {
    let ado = Ado::load(ctx)?;
    let url = ado.api(
        Some(&ado.code_project),
        "pipelines/approvals",
        "",
        PREVIEW_API,
    );
    let body = json!([{
        "approvalId": args.id,
        "status": status,
        "comment": args.comment.unwrap_or_default(),
    }]);
    let answered = ado.change(ctx, Effect::Destructive, Method::Patch, &url, body)?;
    Ok(Answered {
        id: args.id,
        status: text(&answered["value"][0]["status"]),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{CODE, ado, dry_run, page, urls};

    #[test]
    fn approvals_list_the_pending_gates_and_answering_one_is_destructive() {
        let (outcome, transport) = ado(
            &["ado", "approval", "list"],
            vec![page(vec![
                json!({"id": "a-1", "status": "pending", "instructions": "Check staging",
                "createdOn": "2026-09-29T10:10:00Z",
                "pipeline": {"id": 12, "name": "web-deploy", "owner": {"id": 991, "name": "20260929.3"}},
                "steps": [{"assignedApprover": {"displayName": "Release Managers"}, "status": "pending"}]}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "a-1", "pipeline": "web-deploy", "run_id": 991, "run": "20260929.3",
                "instructions": "Check staging", "created": "2026-09-29T10:10:00Z",
                "approvers": [{"name": "Release Managers", "status": "pending"}]}])
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/pipelines/approvals?state=pending&$expand=steps&top=51&api-version=7.1-preview.1"
            )]
        );

        let plans = dry_run(
            &["ado", "approval", "approve", "a-1", "--comment", "Checked"],
            vec![],
        );
        assert_eq!(plans[0]["method"], "PATCH");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/pipelines/approvals?api-version=7.1-preview.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([{"approvalId": "a-1", "status": "approved", "comment": "Checked"}])
        );

        let (outcome, _) = ado(
            &["ado", "approval", "reject", "a-1", "--yes"],
            vec![page(vec![json!({"id": "a-1", "status": "rejected"})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"id": "a-1", "status": "rejected"}));
        let (outcome, _) = ado(&["ado", "approval", "approve", "a-1"], vec![]);
        assert_eq!(outcome.code, 2, "a gate needs --yes: {outcome:?}");
    }
}
