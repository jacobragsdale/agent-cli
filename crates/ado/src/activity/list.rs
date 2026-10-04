//! `ado activity list`: what one person did since a time, newest first,
//! merged from four places Azure DevOps keeps apart: work item updates
//! (WIQL `EVER ChangedBy`, then each item's updates), pull requests (created,
//! completed, voted on), commits in the code project's repositories, and
//! runs requested for them.

use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Ado, Person, list, query_value, segment, stamp, text};
use crate::history::get::{new_value, noise, updates};

// ponytail: updates are read for the 50 work items changed most recently,
// commits from the first 20 repositories and votes on 20 pull requests; a
// note names what was skipped. Narrowing --since is the way past them.
const WORK_ITEMS: usize = 50;
const REPOS: usize = 20;
const VOTED_PRS: usize = 20;

#[derive(clap::Args)]
pub struct ActivityListArgs {
    /// Whose activity: name, email or @me
    #[arg(long, default_value = "@me")]
    person: String,
    /// Only what happened after this
    #[arg(long, default_value = "1d")]
    since: When,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// One thing the person did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Activity {
    /// When, RFC 3339.
    at: String,
    /// workitem, comment, pr, commit or run.
    kind: &'static str,
    /// created, commented, moved to Active, changed Title, linked, approved,
    /// completed, committed, failed …
    action: String,
    /// What the kind's command takes: workitem get (a comment's work item),
    /// pr get, run get; a commit is REPO@SHA, as file get takes a ref.
    id: String,
    /// The work item's or pull request's title, the commit's first line, the
    /// run's pipeline.
    title: Option<String>,
}

/// `raw` when it falls in the window.
fn after(raw: &Value, since: When) -> Option<String> {
    let at = stamp(raw)?;
    let when: When = at.parse().ok()?;
    (when.0 >= since.0).then_some(at)
}

/// Whether an identity in an answer is `who`.
fn is(identity: &Value, who: &Person) -> bool {
    identity["id"]
        .as_str()
        .is_some_and(|id| id.eq_ignore_ascii_case(&who.id))
}

/// What one update did, in words, and whether it was a comment.
fn action(update: &Value) -> Option<(&'static str, String)> {
    if !new_value(update, "System.History").is_null() {
        return Some(("comment", "commented".to_owned()));
    }
    if !new_value(update, "System.CreatedDate").is_null() {
        return Some(("workitem", "created".to_owned()));
    }
    if let Some(state) = text(new_value(update, "System.State")) {
        return Some(("workitem", format!("moved to {state}")));
    }
    let changed: Vec<&str> = update["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(reference, _)| reference.as_str())
        .filter(|reference| !noise(reference))
        .map(|reference| reference.rsplit('.').next().unwrap_or(reference))
        .collect();
    if !changed.is_empty() {
        return Some(("workitem", format!("changed {}", changed.join(", "))));
    }
    let relations = &update["relations"];
    if list(&relations["added"])
        .iter()
        .any(|relation| relation["rel"] == "AttachedFile")
    {
        return Some(("workitem", "attached".to_owned()));
    }
    if !list(&relations["added"]).is_empty() {
        return Some(("workitem", "linked".to_owned()));
    }
    if !list(&relations["removed"]).is_empty() {
        return Some(("workitem", "unlinked".to_owned()));
    }
    None
}

fn work_items(ctx: &Ctx, ado: &Ado, who: &Person, since: When) -> Result<Vec<Activity>> {
    let name = who.email.as_deref().unwrap_or(&who.name);
    let query = format!(
        "SELECT [System.Id] FROM WorkItems WHERE {} \
         AND [System.ChangedDate] >= '{}' AND EVER [System.ChangedBy] = '{}' \
         ORDER BY [System.ChangedDate] DESC",
        ado.projects_condition(false),
        since.utc(),
        name.replace('\'', "''")
    );
    let url = ado.work(
        "wit/wiql",
        &format!("$top={}&timePrecision=true", WORK_ITEMS + 1),
    );
    let found = ado.query(ctx, &url, json!({ "query": query }))?;
    let ids: Vec<i64> = list(&found["workItems"])
        .iter()
        .filter_map(|item| item["id"].as_i64())
        .collect();
    if ids.len() > WORK_ITEMS {
        ctx.note(format!(
            "[work item changes read from the {WORK_ITEMS} changed most recently; more changed: narrow --since]"
        ));
    }
    let mut rows: Vec<Activity> = Vec::new();
    for id in ids.into_iter().take(WORK_ITEMS) {
        let updates = updates(ctx, ado, id)?;
        let title = updates
            .iter()
            .rev()
            .find_map(|update| text(new_value(update, "System.Title")));
        let start = rows.len();
        for update in &updates {
            if !is(&update["revisedBy"], who) {
                continue;
            }
            // A relation-only update has no ChangedDate; its revisedDate is
            // when it was made.
            let changed = new_value(update, "System.ChangedDate");
            let when = if changed.is_null() {
                &update["revisedDate"]
            } else {
                changed
            };
            let (Some(at), Some((kind, action))) = (after(when, since), action(update)) else {
                continue;
            };
            // Adding five children is one thing done, not five.
            if let Some(last) = rows[start..].last_mut()
                && last.kind == kind
                && last.action == action
            {
                last.at = at;
                continue;
            }
            rows.push(Activity {
                at,
                kind,
                action,
                id: id.to_string(),
                title: title.clone(),
            });
        }
    }
    Ok(rows)
}

fn pull_requests(
    ctx: &Ctx,
    ado: &Ado,
    who: &Person,
    since: When,
    limit: usize,
) -> Result<Vec<Activity>> {
    let search = |criteria: String| -> Result<Vec<Value>> {
        let url = ado.code(
            "git/pullrequests",
            &format!("{criteria}&$top={}", limit.saturating_add(1)),
        );
        Ok(list(&ado.get(ctx, &url)?["value"]).to_vec())
    };
    let since_utc = query_value(&since.utc());
    let row = |pr: &Value, at: String, action: &str| Activity {
        at,
        kind: "pr",
        action: action.to_owned(),
        id: pr["pullRequestId"].as_i64().unwrap_or_default().to_string(),
        title: text(&pr["title"]),
    };
    let mut rows = Vec::new();
    for pr in search(format!(
        "searchCriteria.creatorId={}&searchCriteria.status=all&searchCriteria.minTime={since_utc}",
        who.id
    ))? {
        rows.extend(after(&pr["creationDate"], since).map(|at| row(&pr, at, "created")));
    }
    // The list does not say who completed a pull request, so completing is
    // counted for its author.
    for pr in search(format!(
        "searchCriteria.creatorId={}&searchCriteria.status=completed&searchCriteria.queryTimeRangeType=closed&searchCriteria.minTime={since_utc}",
        who.id
    ))? {
        rows.extend(after(&pr["closedDate"], since).map(|at| row(&pr, at, "completed")));
    }
    // A vote carries no time; its system thread does. Only pull requests
    // still open, or closed in the window, can hold a vote cast in it.
    let mut voted: Vec<Value> = Vec::new();
    for criteria in [
        "searchCriteria.status=active".to_owned(),
        format!(
            "searchCriteria.status=all&searchCriteria.queryTimeRangeType=closed&searchCriteria.minTime={since_utc}"
        ),
    ] {
        voted.extend(
            search(format!("searchCriteria.reviewerId={}&{criteria}", who.id))?
                .into_iter()
                .filter(|pr| {
                    list(&pr["reviewers"])
                        .iter()
                        .any(|r| is(r, who) && r["vote"].as_i64().unwrap_or_default() != 0)
                }),
        );
    }
    if voted.len() > VOTED_PRS {
        ctx.note(format!(
            "[votes read on {VOTED_PRS} of {} pull requests; narrow --since]",
            voted.len()
        ));
    }
    for pr in voted.iter().take(VOTED_PRS) {
        let repository = &pr["repository"];
        let project = text(&repository["project"]["name"]).unwrap_or(ado.code_project.clone());
        let url = ado.api(
            Some(&project),
            &format!(
                "git/repositories/{}/pullRequests/{}/threads",
                segment(repository["id"].as_str().unwrap_or_default()),
                pr["pullRequestId"]
            ),
            "",
            crate::client::API,
        );
        for thread in list(&ado.get(ctx, &url)?["value"]) {
            let comment = list(&thread["comments"]).first().unwrap_or(&Value::Null);
            if thread["properties"]["CodeReviewThreadType"]["$value"] != "VoteUpdate"
                || !is(&comment["author"], who)
            {
                continue;
            }
            let vote = &thread["properties"]["CodeReviewVoteResult"]["$value"];
            let vote = vote
                .as_i64()
                .or_else(|| vote.as_str().and_then(|v| v.parse().ok()));
            let action = match vote {
                Some(10) => "approved",
                Some(5) => "approved with suggestions",
                Some(-5) => "waiting for author",
                Some(-10) => "rejected",
                _ => "reset vote",
            };
            rows.extend(after(&thread["publishedDate"], since).map(|at| row(pr, at, action)));
        }
    }
    Ok(rows)
}

fn commits(ctx: &Ctx, ado: &Ado, who: &Person, since: When, limit: usize) -> Result<Vec<Activity>> {
    // Fresh: a cached list can name a repository deleted since.
    let repos = ado.repos(ctx, true)?;
    if repos.len() > REPOS {
        let skipped: Vec<&str> = repos[REPOS..].iter().map(|r| r.name.as_str()).collect();
        ctx.note(format!(
            "[commits read from {REPOS} of {} repositories; skipped: {}]",
            repos.len(),
            skipped.join(", ")
        ));
    }
    let mut rows = Vec::new();
    let mut unread = Vec::new();
    for repo in repos.iter().take(REPOS) {
        // The commits API matches the author's name as written.
        let url = ado.code(
            &format!("git/repositories/{}/commits", segment(&repo.id)),
            &format!(
                "searchCriteria.author={}&searchCriteria.fromDate={}&searchCriteria.$top={}",
                query_value(&who.name),
                query_value(&since.utc()),
                limit.saturating_add(1)
            ),
        );
        // A disabled repository answers 404, as a deleted one would.
        let answer = match ado.get(ctx, &url) {
            Err(error) if agent_cli_core::status_of(&error) == Some(404) => {
                unread.push(repo.name.as_str());
                continue;
            }
            answer => answer?,
        };
        for commit in list(&answer["value"]) {
            let (Some(at), Some(sha)) = (
                after(&commit["committer"]["date"], since),
                text(&commit["commitId"]),
            ) else {
                continue;
            };
            rows.push(Activity {
                at,
                kind: "commit",
                action: "committed".to_owned(),
                id: format!("{}@{sha}", repo.name),
                title: text(&commit["comment"])
                    .and_then(|message| message.lines().next().map(str::to_owned)),
            });
        }
    }
    if !unread.is_empty() {
        ctx.note(format!(
            "[commits not read from {} (disabled or gone)]",
            unread.join(", ")
        ));
    }
    Ok(rows)
}

fn runs(ctx: &Ctx, ado: &Ado, who: &Person, since: When, limit: usize) -> Result<Vec<Activity>> {
    // The builds API takes the person by name, as run list sends them.
    let url = ado.code(
        "build/builds",
        &format!(
            "queryOrder=queueTimeDescending&$top={}&minTime={}&requestedFor={}",
            limit.saturating_add(1),
            query_value(&since.utc()),
            query_value(&who.name)
        ),
    );
    Ok(list(&ado.get(ctx, &url)?["value"])
        .iter()
        .filter_map(|run| {
            Some(Activity {
                at: after(&run["queueTime"], since)?,
                kind: "run",
                action: text(&run["result"]).or_else(|| text(&run["status"]))?,
                id: run["id"].as_i64()?.to_string(),
                title: text(&run["definition"]["name"]),
            })
        })
        .collect())
}

fn activity_list(ctx: &Ctx, args: ActivityListArgs) -> Result<Vec<Activity>> {
    let ado = Ado::load(ctx)?;
    let who = ado.person(ctx, &args.person)?;
    let mut rows = work_items(ctx, &ado, &who, args.since)?;
    rows.extend(pull_requests(ctx, &ado, &who, args.since, args.limit)?);
    rows.extend(commits(ctx, &ado, &who, args.since, args.limit)?);
    rows.extend(runs(ctx, &ado, &who, args.since, args.limit)?);
    // Every `at` is the same RFC 3339 form, so text order is time order.
    rows.sort_by(|a, b| b.at.cmp(&a.at));
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[latest {}; --limit N, or narrow with --since]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub ACTIVITY_LIST = ["ado", "activity", "list"], Read,
    "List what someone did: work items changed, comments, PRs, votes, commits, runs",
    keywords: ["standup", "did", "done", "yesterday", "today", "recent", "feed", "timeline", "worked", "contributions", "summary", "report", "person"],
    example: "ado activity list --person @me --since 1d --fields at,kind,action,id,title",
    run: activity_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{CODE, ado, build, me, page, person, pr, repos, urls, wiql};

    fn update(by: &str, fields: Value, relations: Value, revised: &str) -> Value {
        json!({"revisedBy": {"id": by}, "revisedDate": revised, "fields": fields, "relations": relations})
    }

    fn changed(at: &str, mut fields: Value) -> Value {
        fields["System.ChangedDate"] = json!({"newValue": at});
        fields
    }

    #[test]
    fn a_persons_work_items_comments_prs_votes_commits_and_runs_merge_newest_first() {
        let child = json!({"added": [{"rel": "System.LinkTypes.Hierarchy-Forward"}]});
        let updates = page(vec![
            update(
                "u-2",
                changed(
                    "2026-09-27T09:00:00Z",
                    json!({"System.CreatedDate": {"newValue": "x"}, "System.Title": {"newValue": "Checkout"}}),
                ),
                json!(null),
                "2026-09-29T09:00:00Z",
            ),
            update(
                "u-2",
                changed(
                    "2026-09-29T09:00:00.4Z",
                    json!({"System.State": {"oldValue": "New", "newValue": "Active"}}),
                ),
                json!(null),
                "2026-09-29T11:00:00Z",
            ),
            update(
                "u-9",
                changed(
                    "2026-09-29T09:30:00Z",
                    json!({"Microsoft.VSTS.Common.Priority": {"newValue": 1}}),
                ),
                json!(null),
                "x",
            ),
            update("u-2", json!(null), child.clone(), "2026-09-29T10:00:00Z"),
            update("u-2", json!(null), child, "2026-09-29T10:00:01Z"),
            update(
                "U-2",
                changed(
                    "2026-09-29T11:00:00Z",
                    json!({"System.History": {"newValue": "<p>done</p>"}}),
                ),
                json!(null),
                "9999-01-01T00:00:00Z",
            ),
        ]);
        let threads = page(vec![
            json!({"publishedDate": "2026-09-29T12:00:00Z",
                "properties": {"CodeReviewThreadType": {"$value": "VoteUpdate"}, "CodeReviewVoteResult": {"$value": "10"}},
                "comments": [{"author": {"id": "u-2"}}]}),
            json!({"publishedDate": "2026-09-29T12:30:00Z",
                "properties": {"CodeReviewThreadType": {"$value": "VoteUpdate"}, "CodeReviewVoteResult": {"$value": "-10"}},
                "comments": [{"author": {"id": "u-3"}}]}),
            json!({"publishedDate": "2026-09-29T12:40:00Z", "comments": [{"author": {"id": "u-2"}}]}),
        ]);
        let commits = page(vec![json!({"commitId": "abc123",
            "committer": {"date": "2026-09-29T13:00:00Z"}, "comment": "Fix login\n\nWhy"})]);
        let (outcome, transport) = ado(
            &[
                "ado",
                "activity",
                "list",
                "--person",
                "sam",
                "--since",
                "2026-09-28",
            ],
            vec![
                person("u-2", "Sam Lee", "sam@contoso.com"),
                wiql(&[42]),
                updates,
                page(vec![pr(7, false)]),
                page(vec![]),
                page(vec![pr(8, false)]),
                page(vec![]),
                threads,
                repos(),
                commits,
                page(vec![build(5, "completed", Some("failed"))]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let row = |at: &str, kind: &str, action: &str, id: &str, title: &str| json!({"at": at, "kind": kind, "action": action, "id": id, "title": title});
        assert_eq!(
            outcome.json(),
            json!([
                row(
                    "2026-09-29T13:00:00Z",
                    "commit",
                    "committed",
                    "web@abc123",
                    "Fix login"
                ),
                row("2026-09-29T12:00:00Z", "pr", "approved", "8", "Change 8"),
                row(
                    "2026-09-29T11:00:00Z",
                    "comment",
                    "commented",
                    "42",
                    "Checkout"
                ),
                row(
                    "2026-09-29T10:00:01Z",
                    "workitem",
                    "linked",
                    "42",
                    "Checkout"
                ),
                row("2026-09-29T10:00:00Z", "run", "failed", "5", "web-ci"),
                row(
                    "2026-09-29T09:00:00Z",
                    "workitem",
                    "moved to Active",
                    "42",
                    "Checkout"
                ),
                row("2026-09-28T10:00:00Z", "pr", "created", "7", "Change 7"),
            ])
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!("{CODE}/wit/wiql?$top=51&timePrecision=true&api-version=7.1")
        );
        let query = sent[1].body.as_ref().unwrap()["query"].as_str().unwrap();
        assert!(
            query.contains(
                "[System.ChangedDate] >= '2026-09-28T00:00:00Z' AND EVER [System.ChangedBy] = 'sam@contoso.com'"
            ),
            "{query}"
        );
        let since = "searchCriteria.minTime=2026-09-28T00%3A00%3A00Z";
        assert_eq!(
            &urls(&transport)[3..],
            [
                format!(
                    "{CODE}/git/pullrequests?searchCriteria.creatorId=u-2&searchCriteria.status=all&{since}&$top=51&api-version=7.1"
                ),
                format!(
                    "{CODE}/git/pullrequests?searchCriteria.creatorId=u-2&searchCriteria.status=completed&searchCriteria.queryTimeRangeType=closed&{since}&$top=51&api-version=7.1"
                ),
                format!(
                    "{CODE}/git/pullrequests?searchCriteria.reviewerId=u-2&searchCriteria.status=active&$top=51&api-version=7.1"
                ),
                format!(
                    "{CODE}/git/pullrequests?searchCriteria.reviewerId=u-2&searchCriteria.status=all&searchCriteria.queryTimeRangeType=closed&{since}&$top=51&api-version=7.1"
                ),
                format!("{CODE}/git/repositories/r-1/pullRequests/8/threads?api-version=7.1"),
                format!("{CODE}/git/repositories?api-version=7.1"),
                format!(
                    "{CODE}/git/repositories/r-1/commits?searchCriteria.author=Sam+Lee&searchCriteria.fromDate=2026-09-28T00%3A00%3A00Z&searchCriteria.$top=51&api-version=7.1"
                ),
                format!(
                    "{CODE}/build/builds?queryOrder=queueTimeDescending&$top=51&minTime=2026-09-28T00%3A00%3A00Z&requestedFor=Sam+Lee&api-version=7.1"
                ),
            ]
        );
    }

    #[test]
    fn past_fifty_work_items_and_twenty_repositories_a_note_names_what_was_skipped() {
        let ids: Vec<i64> = (1..=51).collect();
        let mut answers: Vec<Answer> = vec![me(), wiql(&ids)];
        answers.extend((0..54).map(|_| page(vec![])));
        let many: Vec<Value> = (1..=22)
            .map(|n| json!({"id": format!("r-{n}"), "name": format!("repo{n}")}))
            .collect();
        answers.push(page(many));
        answers.extend((0..21).map(|_| page(vec![])));
        let (outcome, transport) = ado(&["ado", "activity", "list"], answers);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.stdout.trim(), "[]");
        assert!(
            outcome
                .stderr
                .contains("[work item changes read from the 50 changed most recently;"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("[commits read from 20 of 22 repositories; skipped: repo21, repo22]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.remaining(), 0);
        let query = transport.sent()[1].body.clone().unwrap();
        assert!(
            query["query"]
                .as_str()
                .unwrap()
                .contains("EVER [System.ChangedBy] = 'Jane Doe'")
        );
    }

    #[test]
    fn a_repository_whose_commits_answer_404_is_named_and_skipped() {
        let disabled = Answer::status(
            404,
            r#"{"message":"TF401019: The Git repository with name or identifier r-1 does not exist."}"#,
        );
        let (outcome, _) = ado(
            &[
                "ado",
                "activity",
                "list",
                "--person",
                "sam",
                "--since",
                "2026-09-28",
            ],
            vec![
                person("u-2", "Sam Lee", "sam@contoso.com"),
                wiql(&[]),
                page(vec![]),
                page(vec![]),
                page(vec![]),
                page(vec![]),
                repos(),
                disabled,
                page(vec![build(5, "completed", Some("failed"))]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("[commits not read from web (disabled or gone)]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(outcome.json()[0]["kind"], "run");
    }
}
