use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{API, Ado, list, query_value, segment, stamp, text};
use crate::ids::{FileId, agree, resolving};

#[derive(clap::Args)]
pub struct CommitListArgs {
    /// The repository, or a file or folder in it: REPO[@REF][:PATH], as file get takes it (a line is ignored)
    repo: String,
    /// The file or folder in the repository, when REPO names none
    #[arg(long)]
    path: Option<String>,
    /// The branch, tag or commit to read history back from (default: the default branch)
    #[arg(long = "ref")]
    reference: Option<String>,
    /// Committed after this
    #[arg(long)]
    since: Option<When>,
    /// Committed before this
    #[arg(long)]
    until: Option<When>,
    /// Who wrote it: name, email or @me
    #[arg(long)]
    author: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// One commit, newest first.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CommitRow {
    /// The full SHA: a ref for file get and diff get.
    commit: String,
    author: Option<String>,
    /// When it was committed; for a merge, when it merged.
    date: Option<String>,
    /// Its message's first line.
    message: Option<String>,
    /// The pull request that merged it.
    pr: Option<MergedBy>,
    /// Its change, as diff get takes it: REPO@PARENT..SHA.
    diff: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MergedBy {
    /// What `ado pr get` takes.
    id: i64,
    title: Option<String>,
}

fn commit_list(ctx: &Ctx, args: CommitListArgs) -> Result<Vec<CommitRow>> {
    let ado = Ado::load(ctx)?;
    let mut id = FileId::parse(&ado, &args.repo, args.reference.as_deref(), None)?;
    if let Some(flag) = &args.path {
        let mut held = Some(std::mem::take(&mut id.path)).filter(|path| !path.is_empty());
        agree(
            "--path",
            &mut held,
            flag.trim().trim_start_matches('/').to_owned(),
        )?;
        id.path = held.unwrap_or_default();
    }
    let project = id.project(&ado).to_owned();
    let repo = segment(&id.repo);
    let mut query = format!("searchCriteria.$top={}", args.limit.saturating_add(1));
    if !id.path.is_empty() {
        query.push_str(&format!(
            "&searchCriteria.itemPath={}",
            query_value(&format!("/{}", id.path))
        ));
    }
    for (key, when) in [("fromDate", args.since), ("toDate", args.until)] {
        if let Some(when) = when {
            query.push_str(&format!(
                "&searchCriteria.{key}={}",
                query_value(&when.utc())
            ));
        }
    }
    if let Some(who) = args.author.as_deref().map(str::trim) {
        // The commits API matches the author's name or address as written.
        let who = if who.eq_ignore_ascii_case("@me") {
            ado.me(ctx)?.name
        } else {
            who.to_owned()
        };
        query.push_str(&format!("&searchCriteria.author={}", query_value(&who)));
    }
    let refs: Vec<&str> = id.reference.as_deref().into_iter().collect();
    let answer = resolving(&refs, |reading| {
        let mut query = query.clone();
        if let Some((kind, version)) = reading.first() {
            query.push_str(&format!(
                "&searchCriteria.itemVersion.version={}&searchCriteria.itemVersion.versionType={kind}",
                query_value(version)
            ));
        }
        let url = ado.api(
            Some(&project),
            &format!("git/repositories/{repo}/commits"),
            &query,
            API,
        );
        ado.get(ctx, &url)
    })?;
    let mut commits: Vec<&serde_json::Value> = list(&answer["value"]).iter().collect();
    if commits.len() > args.limit {
        commits.truncate(args.limit);
        ctx.note(format!(
            "[latest {}; --limit N, or narrow with --since --until --author]",
            args.limit
        ));
    }
    let shas: Vec<String> = commits
        .iter()
        .filter_map(|c| text(&c["commitId"]))
        .collect();
    // One query for the page: the pull request whose merge each commit is.
    let merged = if shas.is_empty() {
        serde_json::Value::Null
    } else {
        let url = ado.api(
            Some(&project),
            &format!("git/repositories/{repo}/pullrequestquery"),
            "",
            API,
        );
        let body = json!({"queries": [{"type": "lastMergeCommit", "items": shas}]});
        ado.query(ctx, &url, body)?
    };
    let range_repo = match &id.project {
        Some(project) => format!("{project}/{}", id.repo),
        None => id.repo.clone(),
    };
    Ok(commits
        .into_iter()
        .filter_map(|commit| {
            let sha = text(&commit["commitId"])?;
            let pr = list(&merged["results"])
                .iter()
                .flat_map(|result| list(&result[sha.as_str()]))
                .find_map(|pr| {
                    Some(MergedBy {
                        id: pr["pullRequestId"].as_i64()?,
                        title: text(&pr["title"]),
                    })
                });
            Some(CommitRow {
                author: text(&commit["author"]["name"]),
                date: stamp(&commit["committer"]["date"]),
                message: text(&commit["comment"])
                    .and_then(|message| message.lines().next().map(str::to_owned)),
                pr,
                diff: list(&commit["parents"])
                    .first()
                    .and_then(text)
                    .map(|parent| format!("{range_repo}@{parent}..{sha}")),
                commit: sha,
            })
        })
        .collect())
}

command! {
    pub COMMIT_LIST = ["ado", "commit", "list"], Read,
    "List the commits on a repository, file or folder, with the pull request of each",
    keywords: ["history", "log", "blame", "changed", "who", "when", "sha", "git", "merged", "introduced"],
    example: "ado commit list api:src/Orders/OrderClient.cs --fields commit,date,message,pr",
    run: commit_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    #[test]
    fn a_files_commits_carry_their_pull_request_and_their_diff() {
        let commit = |sha: &str, parent: &str, message: &str| {
            json!({"commitId": sha, "parents": [parent], "comment": message,
                "author": {"name": "Jane Doe", "email": "jane@contoso.com", "date": "2026-09-28T19:00:00Z"},
                "committer": {"name": "Jane Doe", "date": "2026-09-28T20:14:00.1210000Z"}})
        };
        let (outcome, transport) = ado(
            &[
                "ado",
                "commit",
                "list",
                "web:src/x.cs:19",
                "--until",
                "2026-09-29",
                "--limit",
                "1",
            ],
            vec![
                page(vec![
                    commit("c2", "c1", "Merged PR 17: Fix login\n\nLonger text"),
                    commit("c1", "c0", "Add login"),
                ]),
                Answer::json(
                    &json!({"results": [{"c2": [{"pullRequestId": 17, "title": "Fix login"}]}]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"commit": "c2", "author": "Jane Doe", "date": "2026-09-28T20:14:00Z",
                "message": "Merged PR 17: Fix login", "pr": {"id": 17, "title": "Fix login"},
                "diff": "web@c1..c2"}])
        );
        assert!(outcome.stderr.contains("[latest 1;"), "{}", outcome.stderr);
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            format!(
                "{CODE}/git/repositories/web/commits?searchCriteria.$top=2&searchCriteria.itemPath=%2Fsrc%2Fx.cs&searchCriteria.toDate=2026-09-29T00%3A00%3A00Z&api-version=7.1"
            )
        );
        assert!(sent[1].method.is_read(), "a query that only reads");
        assert_eq!(
            sent[1].body.as_ref().unwrap(),
            &json!({"queries": [{"type": "lastMergeCommit", "items": ["c2"]}]})
        );
    }

    #[test]
    fn path_names_the_file_of_a_bare_repository_and_must_agree_with_one_in_the_id() {
        let (outcome, transport) = ado(
            &["ado", "commit", "list", "web", "--path", "/src/x.cs"],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(urls(&transport)[0].contains("searchCriteria.itemPath=%2Fsrc%2Fx.cs"));
        let (outcome, transport) = ado(
            &[
                "ado",
                "commit",
                "list",
                "web:src/y.cs",
                "--path",
                "src/x.cs",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(outcome.stderr.contains("--path"), "{}", outcome.stderr);
        assert!(urls(&transport).is_empty());
    }

    #[test]
    fn a_bare_ref_is_a_branch_else_a_tag_and_no_commits_ask_for_no_pull_requests() {
        let (outcome, transport) = ado(
            &["ado", "commit", "list", "web", "--ref", "v1.2"],
            vec![
                Answer::status(404, r#"{"message":"TF401175: could not be resolved"}"#),
                page(vec![]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([]));
        let sent = urls(&transport);
        assert_eq!(sent.len(), 2);
        assert!(sent[1].contains(
            "searchCriteria.itemVersion.version=v1.2&searchCriteria.itemVersion.versionType=tag"
        ));
        assert!(!sent[1].contains("itemPath"));
    }
}
