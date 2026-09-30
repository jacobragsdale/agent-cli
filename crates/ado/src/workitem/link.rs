use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, Kind, RepoRef, list, query_value, short_branch, text};
use crate::work_items::add_artifact_link;

#[derive(clap::Args)]
pub struct LinkArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// The repository, by name
    #[arg(long)]
    repo: String,
    /// Default {id}-{title-slug}; made from the default branch if missing
    #[arg(long)]
    branch: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BranchLinked {
    work_item: i64,
    repo: String,
    branch: String,
    /// The branch did not exist and was made.
    branch_created: bool,
    /// The work item already carried the link; nothing was written for it.
    already_linked: bool,
}

/// The branch a work item's own work goes on when nobody names one:
/// `{id}-{slug}`, the slug being the title lowercased with every run of
/// characters that are not ASCII letters or digits made one `-`, at most
/// forty characters, so `#715 Fix the thing!` is `715-fix-the-thing`.
fn branch_name(id: i64, title: &str) -> String {
    let mut slug = String::new();
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        id.to_string()
    } else {
        format!("{id}-{slug}")
    }
}

/// The commit `branch` points at in `repo`, if it exists. The refs listing
/// filters by prefix, so `heads/main` also lists `main-old`; only the exact
/// name counts.
fn branch_head(ctx: &Ctx, ado: &Ado, repo: &RepoRef, branch: &str) -> Result<Option<String>> {
    let full = format!("refs/heads/{branch}");
    let url = ado.code(
        &format!("git/repositories/{}/refs", repo.id),
        &format!("filter={}", query_value(&format!("heads/{branch}"))),
    );
    let answer = ado.get(ctx, &url)?;
    Ok(list(&answer["value"])
        .iter()
        .find(|entry| entry["name"].as_str() == Some(full.as_str()))
        .and_then(|entry| text(&entry["objectId"])))
}

fn workitem_link(ctx: &Ctx, args: LinkArgs) -> Result<BranchLinked> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let repo = ado.repo(ctx, &args.repo)?;
    let branch = match &args.branch {
        Some(branch) => short_branch(branch.trim()),
        None => {
            let url = ado.api(
                None,
                &format!("wit/workitems/{}", id),
                "fields=System.Title",
                crate::client::API,
            );
            let item = ado.get(ctx, &url)?;
            branch_name(
                id,
                &text(&item["fields"]["System.Title"]).unwrap_or_default(),
            )
        }
    };
    let mut branch_created = false;
    if branch_head(ctx, &ado, &repo, &branch)?.is_none() {
        let from = short_branch(repo.default_branch.as_deref().ok_or_else(|| {
            Failure::usage(format!(
                "{} has no default branch to branch from",
                repo.name
            ))
        })?);
        let sha = branch_head(ctx, &ado, &repo, &from)?.with_context(|| {
            format!("there is no branch {from} in {} to branch from", repo.name)
        })?;
        let url = ado.code(&format!("git/repositories/{}/refs", repo.id), "");
        let made = ado.change(
            ctx,
            Effect::Write,
            Method::Post,
            &url,
            json!([{
                "name": format!("refs/heads/{branch}"),
                "oldObjectId": "0000000000000000000000000000000000000000",
                "newObjectId": sha,
            }]),
        )?;
        if made["value"][0]["updateStatus"].as_str() != Some("succeeded") {
            anyhow::bail!(
                "Azure DevOps did not create {branch}: {}",
                made["value"][0]["updateStatus"]
                    .as_str()
                    .unwrap_or("no answer")
            );
        }
        branch_created = true;
    }
    let project_id = repo
        .project_id
        .as_deref()
        .context("the repository came back without its project id")?;
    // The slashes inside a branch name are encoded like the separators.
    let url = format!(
        "vstfs:///Git/Ref/{project_id}%2F{}%2FGB{}",
        repo.id,
        branch.replace('/', "%2F")
    );
    let linked = add_artifact_link(ctx, &ado, id, &url, "Branch")?;
    Ok(BranchLinked {
        work_item: id,
        repo: repo.name,
        branch,
        branch_created,
        already_linked: !linked,
    })
}

command! {
    pub WORKITEM_LINK = ["ado", "workitem", "link"], Write,
    "Link a work item to a branch, creating the branch when missing",
    keywords: ["branch", "start", "work", "on", "connect", "git"],
    example: "ado workitem link 42 --repo web --branch 42-fix-login",
    run: workitem_link,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, ado, dry_run, item, repos};

    use super::branch_name;

    fn refs(name: &str, sha: &str) -> Answer {
        Answer::json(&json!({"count": 1, "value": [{"name": name, "objectId": sha}]}))
    }

    #[test]
    fn link_makes_the_missing_branch_from_the_default_one_before_linking() {
        let title = Answer::json(
            &json!({"id": 42, "rev": 3, "fields": {"System.Title": "Fix the login!"}}),
        );
        let plans = dry_run(
            &["ado", "workitem", "link", "42", "--repo", "WEB"],
            vec![
                repos(),
                title,
                // The prefix filter answers with a different branch only.
                refs("refs/heads/42-fix-the-login-old", "aaa"),
                refs("refs/heads/main", "c0ffee"),
            ],
        );
        assert_eq!(plans[0]["method"], "POST");
        assert_eq!(
            plans[0]["url"],
            format!("{BASE}/Fabrikam/_apis/git/repositories/r-1/refs?api-version=7.1")
        );
        assert_eq!(
            plans[0]["body"],
            json!([{"name": "refs/heads/42-fix-the-login",
                "oldObjectId": "0000000000000000000000000000000000000000", "newObjectId": "c0ffee"}])
        );
    }

    #[test]
    fn link_to_an_existing_branch_patches_the_work_item_once_and_not_twice() {
        let mut current = item(42, 3, "Fix");
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "link",
                "42",
                "--repo",
                "web",
                "--branch",
                "refs/heads/feature/login",
            ],
            vec![
                repos(),
                refs("refs/heads/feature/login", "abc"),
                Answer::json(&current),
                Answer::json(&item(42, 4, "Fix")),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"work_item": 42, "repo": "web", "branch": "feature/login",
                "branch_created": false, "already_linked": false})
        );
        let patch = &transport.sent()[3];
        assert_eq!(
            patch.url,
            format!("{BASE}/_apis/wit/workitems/42?api-version=7.1")
        );
        assert_eq!(
            patch.body.as_ref().unwrap(),
            &json!([
                {"op": "test", "path": "/rev", "value": 3},
                {"op": "add", "path": "/relations/-", "value": {"rel": "ArtifactLink",
                    "url": "vstfs:///Git/Ref/p-1%2Fr-1%2FGBfeature%2Flogin",
                    "attributes": {"name": "Branch"}}}
            ])
        );

        current["relations"] = json!([{"rel": "ArtifactLink",
            "url": "vstfs:///Git/Ref/p-1%2fr-1%2fGBfeature%2flogin"}]);
        let (outcome, transport) = ado(
            &[
                "ado",
                "workitem",
                "link",
                "42",
                "--repo",
                "web",
                "--branch",
                "feature/login",
            ],
            vec![
                repos(),
                refs("refs/heads/feature/login", "abc"),
                Answer::json(&current),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["already_linked"], true);
        assert!(transport.sent().iter().all(|sent| sent.method.is_read()));

        let (outcome, _) = ado(
            &["ado", "workitem", "link", "42", "--repo", "api"],
            vec![repos(), repos()],
        );
        assert_eq!(
            outcome.code, 4,
            "an unknown repo is looked up afresh, then not found: {outcome:?}"
        );
        assert!(
            outcome.stderr.contains("hint: agent-cli ado repo list"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_branch_name_is_the_id_and_a_forty_character_slug() {
        assert_eq!(branch_name(715, "Fix the thing!"), "715-fix-the-thing");
        assert_eq!(
            branch_name(
                715,
                "  Réécrire — the (whole) sync/path, again & again & again "
            ),
            "715-r-crire-the-whole-sync-path-again-again"
        );
        assert_eq!(branch_name(715, "???"), "715");
    }
}
