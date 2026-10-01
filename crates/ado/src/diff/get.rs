use agent_cli_core::{Ctx, command};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{API, Ado, Kind, list, query_value, segment, text};
use crate::file::fetch;
use crate::ids::{FileId, Range, file_id, glob, resolving};
use crate::pr::{fetch_pr, latest_iteration, pr_home};

use super::diff;

#[derive(clap::Args)]
pub struct DiffGetArgs {
    /// A pull request (436, #436 or its URL), or two refs of a repository: REPO@BASE..HEAD
    compare: String,
    /// Only files matching this glob or path (repeatable): *.cs, src/Orders/OrderClient.cs
    #[arg(long)]
    file: Vec<String>,
    /// Only which files changed and how, in one call
    #[arg(long)]
    names_only: bool,
    /// Unchanged lines shown around each change
    #[arg(long, default_value_t = 3)]
    unified: usize,
    /// Most files to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// One changed file.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileDiff {
    /// The file get id of the file after the change (before it, when deleted).
    at: String,
    path: String,
    /// add, edit, delete or rename.
    change: &'static str,
    /// The path before a rename.
    from: Option<String>,
    added: Option<usize>,
    removed: Option<usize>,
    /// Unified hunks, each with the file get id of the lines it changes (what pr comment --at takes).
    hunks: Vec<HunkRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HunkRow {
    at: String,
    diff: String,
}

/// The most diff lines shown for one file.
const MAX_LINES: usize = 400;

fn diff_get(ctx: &Ctx, args: DiffGetArgs) -> Result<Vec<FileDiff>> {
    let ado = Ado::load(ctx)?;
    let patterns = args
        .file
        .iter()
        .map(|pattern| {
            if pattern.contains(':') {
                // A file id, as thread list and diff get print it, names its path.
                FileId::parse(&ado, pattern, None, None).map(|id| id.path)
            } else {
                Ok(pattern.clone())
            }
        })
        .collect::<Result<Vec<String>>>()?;
    let (project, repo, answer) = if args.compare.contains("..") {
        let range = Range::parse(&ado, &args.compare)?;
        let on = range.project.as_deref().unwrap_or(&ado.code_project);
        let answer = resolving(&[&range.base, &range.head], |pick| {
            commits(ctx, &ado, on, &range.repo, &pick[0], &pick[1])
        })?;
        (range.project, range.repo, answer)
    } else {
        let id = ado.id(Kind::PullRequest, &args.compare)?;
        let pr = fetch_pr(ctx, &ado, id)?;
        let (repo_id, _) = pr_home(&pr)?;
        let latest = latest_iteration(ctx, &ado, &repo_id, id)?;
        let commit = |field: &str| {
            text(&latest[field]["commitId"])
                .map(|commit| ("commit", commit))
                .with_context(|| format!("pull request {id} has no pushes to compare"))
        };
        let (base, head) = (commit("commonRefCommit")?, commit("sourceRefCommit")?);
        let repo = text(&pr["repository"]["name"]).unwrap_or(repo_id);
        let answer = commits(ctx, &ado, &ado.code_project, &repo, &base, &head)?;
        (None, repo, answer)
    };
    let commit = |field: &str| {
        text(&answer[field]).with_context(|| format!("Azure DevOps did not say the {field}"))
    };
    let (base, head) = (commit("baseCommit")?, commit("targetCommit")?);
    if answer["allChangesIncluded"].as_bool() == Some(false) {
        ctx.note("[Azure DevOps listed only some of the changes]");
    }
    let mut changes: Vec<&Value> = list(&answer["changes"])
        .iter()
        .filter(|change| {
            let item = &change["item"];
            let path = item["path"].as_str().unwrap_or_default();
            item["isFolder"].as_bool() != Some(true)
                && item["gitObjectType"].as_str() != Some("tree")
                && (patterns.is_empty() || patterns.iter().any(|pattern| glob(pattern, path)))
        })
        .collect();
    if changes.len() > args.limit {
        ctx.note(format!(
            "[first {} of {} files; --limit N or --file GLOB]",
            args.limit,
            changes.len()
        ));
        changes.truncate(args.limit);
    }
    let on = project.as_deref().unwrap_or(&ado.code_project);
    let id = |commit: &str, path: &str, lines| {
        file_id(project.as_deref(), &repo, Some(commit), path, lines)
    };
    let mut rows = Vec::new();
    for change in changes {
        let path = text(&change["item"]["path"])
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_owned();
        let kind = change["changeType"].as_str().unwrap_or_default();
        let change_kind = ["delete", "rename", "add"]
            .into_iter()
            .find(|word| kind.contains(word))
            .unwrap_or("edit");
        let from = (change_kind == "rename")
            .then(|| text(&change["sourceServerItem"]).or_else(|| text(&change["originalPath"])))
            .flatten()
            .map(|from| from.trim_start_matches('/').to_owned());
        let mut row = FileDiff {
            at: match change_kind {
                "delete" => id(&base, &path, None),
                _ => id(&head, &path, None),
            },
            path: path.clone(),
            change: change_kind,
            from,
            added: None,
            removed: None,
            hunks: Vec::new(),
        };
        if !args.names_only {
            let read = |commit: &str, path: &str| -> Result<String> {
                let item = fetch(
                    ctx,
                    &ado,
                    on,
                    &repo,
                    path,
                    Some(&("commit", commit.to_owned())),
                )?;
                Ok(text(&item["content"]).unwrap_or_default())
            };
            let old = match change_kind {
                "add" => String::new(),
                _ => read(&base, row.from.as_deref().unwrap_or(&path))?,
            };
            let new = match change_kind {
                "delete" => String::new(),
                _ => read(&head, &path)?,
            };
            if old.contains('\0') || new.contains('\0') {
                ctx.note(format!("[{path} is binary]"));
            } else {
                let lines = diff(&old, &new, args.unified);
                (row.added, row.removed) = (Some(lines.added), Some(lines.removed));
                let mut shown = 0;
                for hunk in &lines.hunks {
                    shown += hunk.text.lines().count();
                    if shown > MAX_LINES {
                        ctx.note(format!(
                            "[{path}: the first {} of {} hunks; agent-cli ado file get {} for the file]",
                            row.hunks.len(),
                            lines.hunks.len(),
                            row.at
                        ));
                        break;
                    }
                    let at = match (hunk.added, hunk.removed) {
                        (Some(added), _) => id(&head, &path, Some(added)),
                        (None, removed) => id(&base, row.from.as_deref().unwrap_or(&path), removed),
                    };
                    row.hunks.push(HunkRow {
                        at,
                        diff: hunk.text.clone(),
                    });
                }
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

/// `diffs/commits` between two readings of refs: the files that differ, and
/// the commits compared. Two dots, so `diffCommonCommit=false`: what changed
/// from BASE to HEAD, not since their merge base.
fn commits(
    ctx: &Ctx,
    ado: &Ado,
    project: &str,
    repo: &str,
    base: &(&str, String),
    head: &(&str, String),
) -> Result<Value> {
    // ponytail: one page of up to 1000 changes; allChangesIncluded says
    // when there were more, and paging with $skip is unconfirmed.
    let query = format!(
        "baseVersion={}&baseVersionType={}&targetVersion={}&targetVersionType={}&diffCommonCommit=false&$top=1000",
        query_value(&base.1),
        base.0,
        query_value(&head.1),
        head.0
    );
    let path = format!("git/repositories/{}/diffs/commits", segment(repo));
    ado.get(ctx, &ado.api(Some(project), &path, &query, API))
}

command! {
    pub DIFF_GET = ["ado", "diff", "get"], Read,
    "Show what changed in a pull request or between two refs, as hunks per file",
    keywords: ["changes", "patch", "hunks", "files", "modified", "touched", "release", "between", "pr"],
    example: "ado diff get 436 --file '*.cs' --fields at,path,hunks",
    run: diff_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, pr, urls};

    fn content(text: &str) -> Answer {
        Answer::json(&json!({"content": text}))
    }

    #[test]
    fn a_pull_request_compares_its_merge_base_with_its_head_file_by_file() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "diff",
                "get",
                "17",
                "--file",
                "*.cs",
                "--unified",
                "1",
            ],
            vec![
                Answer::json(&pr(17, false)),
                page(vec![
                    json!({"id": 3, "sourceRefCommit": {"commitId": "head3"}, "commonRefCommit": {"commitId": "base3"}}),
                ]),
                Answer::json(
                    &json!({"baseCommit": "base3", "targetCommit": "head3", "allChangesIncluded": true, "changes": [
                        {"item": {"path": "/src", "isFolder": true, "gitObjectType": "tree"}, "changeType": "edit"},
                        {"item": {"path": "/src/x.cs", "gitObjectType": "blob"}, "changeType": "edit"},
                        {"item": {"path": "/README.md", "gitObjectType": "blob"}, "changeType": "edit"},
                        {"item": {"path": "/src/y.cs", "gitObjectType": "blob"}, "changeType": "edit, rename", "sourceServerItem": "/src/old.cs"},
                    ]}),
                ),
                content("a\nb\nc\nd\n"),
                content("a\nB\nc\nd\n"),
                content("same\n"),
                content("same\n"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"at": "web@head3:src/x.cs", "path": "src/x.cs", "change": "edit", "added": 1, "removed": 1,
                    "hunks": [{"at": "web@head3:src/x.cs:2", "diff": "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c"}]},
                {"at": "web@head3:src/y.cs", "path": "src/y.cs", "change": "rename", "from": "src/old.cs",
                    "added": 0, "removed": 0},
            ])
        );
        let sent = urls(&transport);
        assert_eq!(
            sent[2],
            format!(
                "{CODE}/git/repositories/web/diffs/commits?baseVersion=base3&baseVersionType=commit&targetVersion=head3&targetVersionType=commit&diffCommonCommit=false&$top=1000&api-version=7.1"
            )
        );
        assert!(sent[3].contains("path=%2Fsrc%2Fx.cs&versionDescriptor.version=base3&versionDescriptor.versionType=commit"));
        assert!(
            sent[5].contains("path=%2Fsrc%2Fold.cs&versionDescriptor.version=base3"),
            "a rename reads its old path"
        );
    }

    #[test]
    fn a_range_of_tags_is_tried_as_branches_first_and_names_only_is_one_call() {
        let (outcome, transport) = ado(
            &["ado", "diff", "get", "web@v1.0..v1.1", "--names-only"],
            vec![
                Answer::status(
                    404,
                    r#"{"message":"TF401175: The version descriptor <Branch: v1.0 > could not be resolved"}"#,
                ),
                Answer::json(
                    &json!({"baseCommit": "c10", "targetCommit": "c11", "changes": [
                        {"item": {"path": "/gone.txt"}, "changeType": "delete"},
                        {"item": {"path": "/new.txt"}, "changeType": "add"},
                    ]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"at": "web@c10:gone.txt", "path": "gone.txt", "change": "delete"},
                {"at": "web@c11:new.txt", "path": "new.txt", "change": "add"},
            ])
        );
        let sent = urls(&transport);
        assert!(sent[0].contains(
            "baseVersion=v1.0&baseVersionType=branch&targetVersion=v1.1&targetVersionType=branch"
        ));
        assert!(sent[1].contains(
            "baseVersion=v1.0&baseVersionType=tag&targetVersion=v1.1&targetVersionType=tag"
        ));

        let (outcome, _) = ado(&["ado", "diff", "get", "web@v1.0"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_deleted_files_hunk_points_at_its_lines_before() {
        let (outcome, _) = ado(
            &[
                "ado",
                "diff",
                "get",
                "web@main..dev",
                "--file",
                "web:gone.txt",
            ],
            vec![
                Answer::json(
                    &json!({"baseCommit": "b1", "targetCommit": "t1", "changes": [
                        {"item": {"path": "/gone.txt"}, "changeType": "delete"},
                        {"item": {"path": "/kept.txt"}, "changeType": "edit"},
                    ]}),
                ),
                content("x\ny\n"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()[0]["hunks"],
            json!([{"at": "web@b1:gone.txt:1-2", "diff": "@@ -1,2 +0,0 @@\n-x\n-y"}])
        );
    }
}
