use agent_cli_core::{Ctx, When, command};
use anyhow::Result;
use serde_json::Value;

use crate::client::{Ado, full_ref, list, query_value, segment};

use super::{PrRow, pr_row, vote_word};

#[derive(Clone, Copy, clap::ValueEnum)]
enum Status {
    Active,
    Completed,
    Abandoned,
    All,
}

#[derive(clap::Args)]
pub struct PrListArgs {
    /// The repository, by name
    #[arg(long)]
    repo: Option<String>,
    /// Which pull requests
    #[arg(long, value_enum, default_value = "active")]
    status: Status,
    /// Who opened it: name, email or @me
    #[arg(long)]
    author: Option<String>,
    /// A reviewer: name, email or @me
    #[arg(long)]
    reviewer: Option<String>,
    /// The --reviewer's own vote (@me's without one); none is not yet voted (repeatable)
    #[arg(long, value_delimiter = ',', value_parser = ["approved", "suggestions", "waiting", "rejected", "none"])]
    vote: Vec<String>,
    /// The branch it merges into
    #[arg(long)]
    target: Option<String>,
    /// The branch it merges from
    #[arg(long)]
    source: Option<String>,
    /// True for drafts only, false for none
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    draft: Option<bool>,
    /// Opened after this
    #[arg(long)]
    since: Option<When>,
    /// Opened before this
    #[arg(long)]
    until: Option<When>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A page of the search; more is read only while fewer than `--limit + 1`
/// rows have matched (drafts and votes are filtered here, the rest by Azure
/// DevOps).
const PR_PAGE: usize = 100;

/// The vote the reviewer with identity `id` has cast on `pr`, in words: none
/// when they have not, or review only through a group.
fn vote_of(pr: &Value, id: &str) -> &'static str {
    let vote = list(&pr["reviewers"])
        .iter()
        .find(|reviewer| {
            reviewer["id"]
                .as_str()
                .is_some_and(|held| held.eq_ignore_ascii_case(id))
        })
        .and_then(|reviewer| reviewer["vote"].as_i64());
    vote_word(vote.unwrap_or_default())
}

fn pr_list(ctx: &Ctx, args: PrListArgs) -> Result<Vec<PrRow>> {
    let ado = Ado::load(ctx)?;
    let status = match args.status {
        Status::Active => "active",
        Status::Completed => "completed",
        Status::Abandoned => "abandoned",
        Status::All => "all",
    };
    let mut criteria = format!("searchCriteria.status={status}");
    if let Some(who) = &args.author {
        criteria.push_str(&format!(
            "&searchCriteria.creatorId={}",
            ado.identity(ctx, who)?
        ));
    }
    // A vote is someone's: the reviewer's, or yours when none is named.
    let reviewer = match (&args.reviewer, args.vote.is_empty()) {
        (Some(who), _) => Some(ado.identity(ctx, who)?),
        (None, false) => Some(ado.identity(ctx, "@me")?),
        (None, true) => None,
    };
    if let Some(id) = &reviewer {
        criteria.push_str(&format!("&searchCriteria.reviewerId={id}"));
    }
    for (key, branch) in [
        ("targetRefName", &args.target),
        ("sourceRefName", &args.source),
    ] {
        if let Some(branch) = branch {
            criteria.push_str(&format!(
                "&searchCriteria.{key}={}",
                query_value(&full_ref(branch))
            ));
        }
    }
    for (key, when) in [("minTime", args.since), ("maxTime", args.until)] {
        if let Some(when) = when {
            criteria.push_str(&format!(
                "&searchCriteria.{key}={}",
                query_value(&when.utc())
            ));
        }
    }
    let path = match &args.repo {
        Some(repo) => format!("git/repositories/{}/pullrequests", segment(repo)),
        None => "git/pullrequests".to_owned(),
    };
    let page = (args.limit + 1).min(PR_PAGE);
    let mut rows = Vec::new();
    let mut skip = 0;
    loop {
        let url = ado.code(&path, &format!("{criteria}&$top={page}&$skip={skip}"));
        let answer = ado.get(ctx, &url)?;
        let found = list(&answer["value"]);
        rows.extend(
            found
                .iter()
                .filter(|pr| {
                    args.draft
                        .is_none_or(|draft| pr["isDraft"].as_bool().unwrap_or_default() == draft)
                })
                .filter(|pr| {
                    args.vote.is_empty()
                        || reviewer
                            .as_deref()
                            .is_some_and(|id| args.vote.iter().any(|vote| vote == vote_of(pr, id)))
                })
                .map(|pr| pr_row(&ado, pr)),
        );
        if rows.len() > args.limit || found.len() < page {
            break;
        }
        skip += page;
    }
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[first {}; more match: --limit N, or narrow with --repo --author --target]",
            args.limit
        ));
    }
    Ok(rows)
}

command! {
    pub PR_LIST = ["ado", "pr", "list"], Read,
    "List pull requests by repo, author, reviewer, vote (approved …), branch, status",
    keywords: ["open", "active", "mine", "reviewer", "pending", "waiting", "drafts", "queue", "unreviewed", "approved"],
    example: "ado pr list --vote none --fields id,title,author,repo",
    run: pr_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, CODE, ado, me, page, pr, urls};

    #[test]
    fn pr_list_filters_server_side_and_pages_until_enough_drafts_match() {
        let (outcome, transport) = ado(
            &[
                "ado", "pr", "list", "--author", "@me", "--target", "main", "--draft", "--limit",
                "1",
            ],
            vec![
                me(),
                page(vec![pr(1, false), pr(2, false)]),
                page(vec![pr(3, true)]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], 3);
        assert_eq!(rows[0]["is_draft"], true);
        assert_eq!(rows[0]["source"], "42-fix-login");
        assert_eq!(
            rows[0]["reviewers"],
            json!([{"name": "Sam Lee", "vote": "approved", "required": true},
                   {"name": "Web Team", "vote": "waiting"}])
        );
        assert_eq!(
            rows[0]["url"],
            format!("{BASE}/Fabrikam/_git/web/pullrequest/3")
        );
        let sent = urls(&transport);
        let criteria = "searchCriteria.status=active&searchCriteria.creatorId=u-1&searchCriteria.targetRefName=refs%2Fheads%2Fmain";
        assert_eq!(
            sent[1],
            format!("{CODE}/git/pullrequests?{criteria}&$top=2&$skip=0&api-version=7.1")
        );
        assert_eq!(
            sent[2],
            format!("{CODE}/git/pullrequests?{criteria}&$top=2&$skip=2&api-version=7.1")
        );

        let (outcome, transport) = ado(
            &[
                "ado", "pr", "list", "--repo", "web", "--status", "all", "--limit", "1",
            ],
            vec![page(vec![pr(1, false), pr(2, false)])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            outcome.stderr.starts_with("[first 1; more match"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/git/repositories/web/pullrequests?searchCriteria.status=all&$top=2&$skip=0&api-version=7.1"
            )]
        );

        let (outcome, transport) = ado(
            &[
                "ado",
                "pr",
                "list",
                "--since",
                "2026-09-01",
                "--until",
                "2026-09-02",
            ],
            vec![page(vec![])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            urls(&transport)[0].contains(
                "&searchCriteria.minTime=2026-09-01T00%3A00%3A00Z&searchCriteria.maxTime=2026-09-02T00%3A00%3A00Z&"
            ),
            "{:?}",
            urls(&transport)
        );
    }

    #[test]
    fn a_vote_filter_is_the_reviewers_own_vote_and_yours_without_one() {
        let mut approved = pr(2, false);
        approved["reviewers"] = json!([{"id": "U-1", "displayName": "Jane Doe", "vote": 10}]);
        let mut waiting = pr(3, false);
        waiting["reviewers"] = json!([{"id": "u-1", "displayName": "Jane Doe", "vote": -5}]);
        let (outcome, transport) = ado(
            &["ado", "pr", "list", "--vote", "none", "--fields", "id"],
            vec![
                me(),
                page(vec![pr(1, false), approved.clone(), waiting.clone()]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!([{"id": 1}]), "only a group reviews 1");
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/git/pullrequests?searchCriteria.status=active&searchCriteria.reviewerId=u-1&$top=51&$skip=0&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &[
                "ado",
                "pr",
                "list",
                "--vote",
                "waiting,approved",
                "--fields",
                "id",
            ],
            vec![me(), page(vec![pr(1, false), approved, waiting])],
        );
        assert_eq!(outcome.json(), json!([{"id": 2}, {"id": 3}]));

        let sam = Answer::json(&json!({"count": 1, "value": [{"id": "u-2",
            "providerDisplayName": "Sam Lee", "properties": {"Mail": {"$value": "sam@contoso.com"}}}]}));
        let (outcome, transport) = ado(
            &[
                "ado",
                "pr",
                "list",
                "--reviewer",
                "sam@contoso.com",
                "--vote",
                "approved",
                "--fields",
                "id",
            ],
            vec![sam, page(vec![pr(1, false)])],
        );
        assert_eq!(outcome.json(), json!([{"id": 1}]), "Sam approved 1");
        assert!(
            urls(&transport)[1].contains("&searchCriteria.reviewerId=u-2&"),
            "{:?}",
            urls(&transport)
        );

        let (outcome, transport) = ado(&["ado", "pr", "list", "--vote", "lgtm"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
    }
}
