use std::path::PathBuf;

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method, command, status_of};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{API, Ado, full_ref, list, query_value, short_branch};

use super::{link_pr, pr_row, pr_work_items};

#[derive(clap::Args)]
pub struct PrCreateArgs {
    /// The repository, by name
    #[arg(long)]
    repo: String,
    /// The branch it merges from
    #[arg(long)]
    source: String,
    /// The branch it merges into (default: the repo's default branch)
    #[arg(long)]
    target: Option<String>,
    /// The pull request's title
    #[arg(long)]
    title: String,
    /// Markdown; - reads stdin
    #[arg(long, allow_hyphen_values = true)]
    description: Option<String>,
    /// The description from a Markdown file
    #[arg(long)]
    description_file: Option<PathBuf>,
    /// A work item to link (repeatable)
    #[arg(long)]
    workitem: Vec<i64>,
    /// Open it as a draft
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    draft: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrCreated {
    id: i64,
    url: Option<String>,
    repo: String,
    source: Option<String>,
    target: Option<String>,
    status: Option<String>,
    is_draft: bool,
    /// False when one was already open between these branches and reused.
    created: bool,
    work_items: Vec<i64>,
}

/// One pull request between two branches, whatever happened last time. None
/// open: one is opened with the links asked for. One open: it is kept as it
/// is and only the links it lacks are added. Two open is a question, not a
/// guess. Azure DevOps does not promise the links land with the pull request,
/// so they are read back, repaired, and read back again.
fn pr_create(ctx: &Ctx, args: PrCreateArgs) -> Result<PrCreated> {
    let ado = Ado::load(ctx)?;
    let title = args.title.trim();
    if title.is_empty() {
        return Err(Failure::usage("a pull request needs a title").into());
    }
    let description = ctx.long_text(
        "description",
        args.description.as_deref(),
        args.description_file.as_deref(),
        None,
    )?;
    // A work item that is not there would fail its link after the pull
    // request is open, so each is read before anything is written.
    for work_item in &args.workitem {
        let url = ado.api(
            None,
            &format!("wit/workitems/{work_item}"),
            "fields=System.Id",
            API,
        );
        ado.get(ctx, &url)
            .map_err(|error| match error.downcast::<Failure>() {
                Ok(failure) if failure.status == Some(404) => failure
                    .hint("agent-cli ado workitem list --text WORDS --fields id,title  (its id)")
                    .into(),
                Ok(failure) => failure.into(),
                Err(error) => error,
            })?;
    }
    let repo = ado.repo(ctx, &args.repo)?;
    let source = full_ref(&args.source);
    let target = match &args.target {
        Some(target) => full_ref(target),
        None => repo.default_branch.clone().ok_or_else(|| {
            Failure::usage(format!(
                "{} has no default branch; pass --target",
                repo.name
            ))
        })?,
    };
    let url = ado.code(&format!("git/repositories/{}/pullrequests", repo.id), "");
    let open = || {
        ado.get(
            ctx,
            &ado.code(
                &format!("git/repositories/{}/pullrequests", repo.id),
                &format!(
                    "searchCriteria.status=active&searchCriteria.sourceRefName={}&searchCriteria.targetRefName={}",
                    query_value(&source),
                    query_value(&target)
                ),
            ),
        )
    };
    let (pr, created) = match list(&open()?["value"]) {
        [] => {
            let body = json!({
                "sourceRefName": source,
                "targetRefName": target,
                "title": title,
                "description": description.map(|description| description.text).unwrap_or_default(),
                "isDraft": args.draft.unwrap_or(false),
                "workItemRefs": args.workitem.iter().map(|id| json!({"id": id.to_string()})).collect::<Vec<_>>(),
            });
            match ado.change(ctx, Effect::Write, Method::Post, &url, body) {
                Ok(pr) => (pr, true),
                // Another run opened it between the read and this write.
                Err(error) if status_of(&error) == Some(409) => match list(&open()?["value"]) {
                    [one] => (one.clone(), false),
                    _ => return Err(error),
                },
                // TF401398: a source or target that is not a branch (any more).
                Err(error)
                    if error
                        .downcast_ref::<Failure>()
                        .is_some_and(|failure| failure.message.contains("TF401398")) =>
                {
                    return Err(Failure::not_found(format!(
                        "{} or {} is not a branch of {}",
                        short_branch(&source),
                        short_branch(&target),
                        repo.name
                    ))
                    .hint(format!(
                        "agent-cli ado repo get {} --fields branches",
                        crate::ids::arg(&repo.name)
                    ))
                    .into());
                }
                Err(error) => return Err(error),
            }
        }
        [one] => (one.clone(), false),
        several => {
            let ids: Vec<String> = several
                .iter()
                .filter_map(|pr| pr["pullRequestId"].as_i64())
                .map(|id| id.to_string())
                .collect();
            return Err(Failure::conflict(format!(
                "{} active pull requests already go from {} into {} in {}: {}",
                several.len(),
                short_branch(&source),
                short_branch(&target),
                repo.name,
                ids.join(", ")
            ))
            .hint("abandon the extra ones (agent-cli ado pr abandon ID --yes), or pass the id to the command you meant")
            .into());
        }
    };
    let id = pr["pullRequestId"]
        .as_i64()
        .context("Azure DevOps answered with a pull request it did not number")?;
    let project_id = repo
        .project_id
        .clone()
        .context("the repository came back without its project id")?;
    let mut linked = pr_work_items(ctx, &ado, &repo.id, id)?;
    let mut repaired = false;
    for work_item in &args.workitem {
        if !linked.contains(work_item) {
            link_pr(ctx, &ado, &project_id, &repo.id, id, *work_item)?;
            repaired = true;
        }
    }
    if repaired {
        linked = pr_work_items(ctx, &ado, &repo.id, id)?;
    }
    let row = pr_row(&ado, &pr);
    let missing: Vec<String> = args
        .workitem
        .iter()
        .filter(|work_item| !linked.contains(work_item))
        .map(ToString::to_string)
        .collect();
    let made = PrCreated {
        id,
        url: row.url,
        repo: repo.name,
        source: row.source,
        target: row.target,
        status: row.status,
        is_draft: row.is_draft,
        created,
        work_items: linked,
    };
    if !missing.is_empty() {
        // The pull request is real either way, so it prints either way.
        return Err(Failure::new(
            Exit::Failed,
            format!(
                "pull request {id} is open, but work item {} did not link",
                missing.join(", ")
            ),
        )
        .hint("run the same command again to repair the links")
        .with_data(made)
        .into());
    }
    Ok(made)
}

command! {
    pub PR_CREATE = ["ado", "pr", "create"], Write,
    "Open a pull request linked to work items, or reuse the one already open",
    keywords: ["new", "open", "raise", "submit", "merge", "request", "branch"],
    example: "ado pr create --repo web --source 42-fix-login --title 'Fix login' --workitem 42",
    run: pr_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, CODE, ado, dry_run, page, pr, repo, urls};

    fn read_42() -> Answer {
        Answer::json(&json!({"id": 42, "fields": {"System.Id": 42}}))
    }

    #[test]
    fn a_work_item_that_is_not_there_is_exit_4_before_anything_is_written() {
        let gone = Answer::status(
            404,
            r#"{"message":"TF401232: Work item 42 does not exist."}"#,
        );
        let argv = [
            "ado",
            "pr",
            "create",
            "--repo",
            "web",
            "--source",
            "x",
            "--title",
            "T",
            "--workitem",
            "42",
        ];
        let (outcome, transport) = ado(&argv, vec![gone]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado workitem list --text WORDS"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn a_pull_request_another_run_just_opened_is_reused() {
        let taken = Answer::status(
            409,
            r#"{"message":"TF401179: An active pull request for the source and target branch already exists."}"#,
        );
        let argv = [
            "ado",
            "pr",
            "create",
            "--repo",
            "web",
            "--source",
            "42-fix-login",
            "--title",
            "T",
        ];
        let (outcome, _) = ado(
            &argv,
            vec![
                repo(),
                page(vec![]),
                taken,
                page(vec![pr(17, false)]),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            (
                outcome.json()["id"].clone(),
                outcome.json()["created"].clone()
            ),
            (json!(17), json!(false))
        );
    }

    #[test]
    fn pr_create_opens_one_with_its_work_items_when_none_is_open() {
        let plans = dry_run(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--workitem",
                "42",
                "--draft",
            ],
            vec![read_42(), repo(), page(vec![])],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{CODE}/git/repositories/r-1/pullrequests?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!({"sourceRefName": "refs/heads/42-fix-login", "targetRefName": "refs/heads/main",
                "title": "Fix login", "description": "", "isDraft": true, "workItemRefs": [{"id": "42"}]})
        );
    }

    #[test]
    fn pr_create_reuses_the_open_one_and_repairs_a_missing_link() {
        let work_item = json!({"id": 42, "rev": 5, "fields": {"System.Title": "Fix"}});
        let (outcome, transport) = ado(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--workitem",
                "42",
            ],
            vec![
                read_42(),
                repo(),
                page(vec![pr(17, false)]),
                page(vec![]),
                Answer::json(&work_item),
                Answer::json(&work_item),
                page(vec![json!({"id": "42"})]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], 17);
        assert_eq!(got["created"], false);
        assert_eq!(got["work_items"], json!([42]));
        let patch = &transport.sent()[5];
        assert_eq!(
            patch.url,
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            patch.body.as_ref().unwrap()[1]["value"]["url"],
            "vstfs:///Git/PullRequestId/p-1%2Fr-1%2F17"
        );
        assert_eq!(
            urls(&transport)[2],
            format!(
                "{CODE}/git/repositories/r-1/pullrequests?searchCriteria.status=active&searchCriteria.sourceRefName=refs%2Fheads%2F42-fix-login&searchCriteria.targetRefName=refs%2Fheads%2Fmain&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &[
                "ado",
                "pr",
                "create",
                "--repo",
                "web",
                "--source",
                "42-fix-login",
                "--title",
                "Fix login",
                "--workitem",
                "42",
            ],
            vec![
                read_42(),
                repo(),
                page(vec![pr(17, false)]),
                page(vec![]),
                Answer::json(&work_item),
                Answer::json(&work_item),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(outcome.json()["id"], 17, "the pull request prints anyway");
        assert!(outcome.json().get("work_items").is_none());
        assert!(
            outcome
                .stderr
                .contains("pull request 17 is open, but work item 42 did not link"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("hint: run the same command again"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(
            &[
                "ado", "pr", "create", "--repo", "web", "--source", "x", "--title", "Two",
            ],
            vec![repo(), page(vec![pr(17, false), pr(18, false)])],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(outcome.stderr.contains("17, 18"), "{}", outcome.stderr);
    }

    #[test]
    fn a_source_that_is_no_branch_is_exit_4_naming_where_the_branches_are() {
        let gone = Answer::status(
            400,
            r#"{"message":"TF401398: The pull request cannot be activated because the source and/or the target branch no longer exists, or the requested refs are not branches"}"#,
        );
        let argv = [
            "ado", "pr", "create", "--repo", "web", "--source", "gone", "--title", "T",
        ];
        let (outcome, _) = ado(&argv, vec![repo(), page(vec![]), gone]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
        assert!(
            outcome.stderr.contains("gone or main is not a branch of web\nhint: agent-cli ado repo get web --fields branches"),
            "{}",
            outcome.stderr
        );
    }
}
