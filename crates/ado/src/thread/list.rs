use std::collections::HashMap;
use std::collections::hash_map::Entry;

use agent_cli_core::{Ctx, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, Kind, list, stamp, text};
use crate::file::{fetch, numbered};
use crate::ids::glob;
use crate::pr::fetch_pr;

use super::{fetch_threads, is_discussion, wire};

#[derive(clap::Args)]
pub struct ThreadListArgs {
    /// The pull request's id: 436, #436 or its web URL
    pr: String,
    /// Only threads in this state, or all (default: active and pending)
    #[arg(long, value_enum)]
    status: Option<Shown>,
    /// Only threads this person started: a name, a sign-in address or @me
    #[arg(long)]
    author: Option<String>,
    /// Only threads on files matching this glob (*.cs, src/Orders/*)
    #[arg(long)]
    file: Option<String>,
    /// Lines of code shown either side of each comment
    #[arg(long, default_value_t = 3)]
    around: usize,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A review thread and the code it is on.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ThreadRow {
    /// PR/THREAD: what thread comment and thread update take.
    id: String,
    /// active, pending, fixed, wontFix, closed or byDesign.
    status: Option<String>,
    /// The file get id of the lines it is on, at the pull request's head.
    at: Option<String>,
    file: Option<String>,
    line: Option<usize>,
    author: Option<String>,
    date: Option<String>,
    /// The first comment, Markdown.
    text: Option<String>,
    /// The last five replies.
    replies: Vec<Reply>,
    /// The lines it is on and around them, each after its number.
    code: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Reply {
    author: Option<String>,
    date: Option<String>,
    text: Option<String>,
}

/// `--status`: a thread status, or all of them.
#[derive(Clone, Copy, clap::ValueEnum)]
enum Shown {
    All,
    Active,
    Fixed,
    #[value(name = "wontFix")]
    WontFix,
    Closed,
    #[value(name = "byDesign")]
    ByDesign,
    Pending,
}

/// The most files one listing reads for `code`.
const FILES: usize = 20;
const REPLIES: usize = 5;

fn thread_list(ctx: &Ctx, args: ThreadListArgs) -> Result<Vec<ThreadRow>> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::PullRequest, &args.pr)?;
    let pr = fetch_pr(ctx, &ado, id)?;
    let threads = fetch_threads(ctx, &ado, &pr, id)?;
    let me = match &args.author {
        Some(who) if who.eq_ignore_ascii_case("@me") => Some(ado.me(ctx)?.id),
        _ => None,
    };
    let started_by = |thread: &Value| {
        let Some(who) = &args.author else { return true };
        let author = &thread["comments"][0]["author"];
        match &me {
            Some(me) => author["id"].as_str() == Some(me),
            None => ["displayName", "uniqueName"].iter().any(|key| {
                author[key]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(who.trim()))
            }),
        }
    };
    let in_status = |thread: &Value| {
        let status = thread["status"].as_str().unwrap_or_default();
        match args.status {
            None => matches!(status, "active" | "pending"),
            Some(Shown::All) => true,
            Some(wanted) => status.eq_ignore_ascii_case(&wire(wanted)),
        }
    };
    let mut picked: Vec<_> = threads
        .all
        .iter()
        .filter(|thread| is_discussion(thread) && in_status(thread) && started_by(thread))
        .map(|thread| (thread, threads.place(thread)))
        .filter(|(_, place)| match &args.file {
            Some(pattern) => place
                .as_ref()
                .is_some_and(|place| glob(pattern, &place.file)),
            None => true,
        })
        .collect();
    if picked.len() > args.limit {
        ctx.note(format!(
            "[first {} of {}; --limit N for more]",
            args.limit,
            picked.len()
        ));
        picked.truncate(args.limit);
    }
    let project = ado.code_project.clone();
    let mut files: HashMap<(String, String), Option<String>> = HashMap::new();
    let (mut rows, mut cut) = (Vec::new(), false);
    for (thread, place) in picked {
        let mut code = None;
        if let Some(place) = &place
            && let (Some(commit), Some(line)) = (&place.commit, place.line)
        {
            let key = (place.file.clone(), commit.clone());
            let full = files.len() >= FILES;
            match files.entry(key.clone()) {
                Entry::Vacant(slot) if !full => {
                    let reading = ("commit", commit.clone());
                    let read = fetch(
                        ctx,
                        &ado,
                        &project,
                        &threads.repo,
                        &place.file,
                        Some(&reading),
                    );
                    slot.insert(match read {
                        Ok(item) => text(&item["content"]),
                        // A file the thread outlived has no code to show.
                        Err(error) if status_of(&error) == Some(404) => None,
                        Err(error) => return Err(error),
                    });
                }
                Entry::Vacant(_) if !cut => {
                    cut = true;
                    ctx.note(format!(
                        "[code for the first {FILES} files; agent-cli ado file get AT for the rest]"
                    ));
                }
                _ => {}
            }
            if let Some(Some(content)) = files.get(&key) {
                let lines: Vec<&str> = content.lines().collect();
                let end = place.end.unwrap_or(line);
                code = Some(numbered(
                    &lines,
                    line.saturating_sub(args.around).max(1),
                    end + args.around,
                ));
            }
        }
        let comments = list(&thread["comments"]);
        let replies: Vec<Reply> = comments
            .iter()
            .skip(1)
            .filter(|comment| {
                comment["commentType"].as_str() != Some("system")
                    && !comment["isDeleted"].as_bool().unwrap_or_default()
            })
            .map(|comment| Reply {
                author: text(&comment["author"]["displayName"]),
                date: stamp(&comment["publishedDate"]),
                text: text(&comment["content"]),
            })
            .collect();
        let skip = replies.len().saturating_sub(REPLIES);
        rows.push(ThreadRow {
            id: format!("{id}/{}", thread["id"].as_i64().unwrap_or_default()),
            status: text(&thread["status"]),
            at: place.as_ref().and_then(|place| place.at.clone()),
            file: place.as_ref().map(|place| place.file.clone()),
            line: place.as_ref().and_then(|place| place.line),
            author: text(&comments.first().unwrap_or(&Value::Null)["author"]["displayName"]),
            date: stamp(&thread["lastUpdatedDate"]),
            text: text(&comments.first().unwrap_or(&Value::Null)["content"]),
            replies: replies.into_iter().skip(skip).collect(),
            code,
        });
    }
    Ok(rows)
}

command! {
    pub THREAD_LIST = ["ado", "thread", "list"], Read,
    "List a pull request's review threads with the code each comment is on",
    keywords: ["unresolved", "feedback", "address", "reviewer", "discussion", "open", "replies"],
    example: "ado thread list 436 --fields id,at,author,text,code",
    run: thread_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, pr, urls};

    fn comment(who: &str, text: &str) -> serde_json::Value {
        json!({"author": {"displayName": who, "uniqueName": format!("{}@contoso.com", who.to_lowercase().replace(' ', "."))},
            "content": text, "commentType": "text", "publishedDate": "2026-09-29T10:00:00Z"})
    }

    fn answers() -> Vec<Answer> {
        let content: Vec<String> = (1..=10).map(|n| format!("x {n}")).collect();
        vec![
            Answer::json(&pr(17, false)),
            page(vec![
                json!({"id": 1, "sourceRefCommit": {"commitId": "old1"}}),
                json!({"id": 2, "sourceRefCommit": {"commitId": "head2"}, "commonRefCommit": {"commitId": "base2"}}),
            ]),
            page(vec![
                json!({"id": 7, "status": "active", "lastUpdatedDate": "2026-09-29T11:00:00Z",
                    "threadContext": {"filePath": "/src/x.cs", "rightFileStart": {"line": 4, "offset": 1}, "rightFileEnd": {"line": 5, "offset": 9}},
                    "comments": [comment("Sam Lee", "Cap it?"), comment("Jane Doe", "Done"),
                        {"commentType": "system", "content": "status changed"}]}),
                json!({"id": 8, "status": "fixed", "threadContext": {"filePath": "/src/x.cs", "rightFileStart": {"line": 1}},
                    "comments": [comment("Sam Lee", "Nit")]}),
                json!({"id": 9, "status": "pending", "comments": [comment("Priya Patel", "Add a test")]}),
                json!({"id": 10, "status": "active", "comments": [{"commentType": "system", "content": "Jane voted 10"}]}),
            ]),
            Answer::json(
                &json!({"path": "/src/x.cs", "commitId": "head2", "content": content.join("\n")}),
            ),
        ]
    }

    #[test]
    fn open_threads_come_with_the_code_they_are_on_at_the_head_commit() {
        let (outcome, transport) =
            ado(&["ado", "thread", "list", "17", "--around", "1"], answers());
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "17/7", "status": "active", "at": "web@head2:src/x.cs:4-5", "file": "src/x.cs", "line": 4,
                    "author": "Sam Lee", "date": "2026-09-29T11:00:00Z", "text": "Cap it?",
                    "replies": [{"author": "Jane Doe", "date": "2026-09-29T10:00:00Z", "text": "Done"}],
                    "code": "3  x 3\n4  x 4\n5  x 5\n6  x 6"},
                {"id": "17/9", "status": "pending", "author": "Priya Patel", "text": "Add a test"},
            ])
        );
        let sent = urls(&transport);
        assert_eq!(
            sent[1],
            format!("{CODE}/git/repositories/r-1/pullRequests/17/iterations?api-version=7.1")
        );
        assert_eq!(
            sent[2],
            format!(
                "{CODE}/git/repositories/r-1/pullRequests/17/threads?$iteration=2&api-version=7.1"
            )
        );
        assert_eq!(
            sent[3],
            format!(
                "{CODE}/git/repositories/web/items?path=%2Fsrc%2Fx.cs&versionDescriptor.version=head2&versionDescriptor.versionType=commit&includeContent=true&$format=json&api-version=7.1"
            )
        );
    }

    #[test]
    fn status_author_and_file_narrow_the_threads() {
        let (outcome, _) = ado(
            &[
                "ado", "thread", "list", "17", "--status", "fixed", "--author", "sam lee",
                "--file", "*.cs", "--fields", "id,at",
            ],
            answers(),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "17/8", "at": "web@head2:src/x.cs:1"}])
        );
        let (outcome, _) = ado(
            &["ado", "thread", "list", "17", "--status", "resolved"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
