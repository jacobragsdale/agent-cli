use agent_cli_core::{Ctx, Failure, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, text};
use crate::ids::{Each, FileId, each, resolving, with_repo};

use super::{fetch, numbered, resolve, text_of, tree};

#[derive(clap::Args)]
pub struct FileGetArgs {
    /// The file: REPO[@REF]:PATH[:LINE[-LINE]] as code list, thread list and diff get print it, its web URL, or a path with --repo. Several print an array, in order
    #[arg(required = true)]
    file: Vec<String>,
    /// The repository, when FILE is a bare path
    #[arg(long)]
    repo: Option<String>,
    /// The branch, tag or commit (default: the repository's default branch)
    #[arg(long = "ref")]
    reference: Option<String>,
    /// The line, or lines A-B, to show (one line shows 20 either side)
    #[arg(long)]
    line: Option<String>,
}

/// Part of one file at one commit.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileText {
    /// The lines shown, as file get takes them.
    id: String,
    repo: String,
    path: String,
    /// The branch, tag or commit asked for; none is the default branch.
    #[serde(rename = "ref")]
    reference: Option<String>,
    /// The commit read.
    commit: Option<String>,
    /// The lines shown: A-B.
    lines: String,
    /// The file's length in lines.
    total: usize,
    /// The lines shown, each after its number.
    text: String,
}

/// Lines either side of a single line asked for.
const AROUND: usize = 20;
/// The most lines one call shows.
const MAX_LINES: usize = 400;

fn file_get(ctx: &Ctx, args: FileGetArgs) -> Result<Each<FileText>> {
    let ado = Ado::load(ctx)?;
    each(&args.file, |file| file_one(ctx, &ado, &args, file))
}

fn file_one(ctx: &Ctx, ado: &Ado, args: &FileGetArgs, file: &str) -> Result<FileText> {
    let mut id = FileId::parse(
        ado,
        &with_repo(file, args.repo.as_deref()),
        args.reference.as_deref(),
        args.line.as_deref(),
    )?;
    id.agree_repo(args.repo.as_deref())?;
    if id.path.is_empty() {
        return Err(Failure::usage(format!("{file} names no file"))
            .hint(format!("agent-cli ado file list {}", id.at(None)))
            .into());
    }
    let reference = id.reference.clone();
    let refs: Vec<&str> = reference.as_deref().into_iter().collect();
    let project = id.project(ado).to_owned();
    let read = |path: &str| {
        resolving(&refs, |reading| {
            fetch(ctx, ado, &project, &id.repo, path, reading.first())
        })
    };
    // Another domain hands over a path as its service printed it (a stack
    // frame's `/app/src/x.cs`); the one file it can only mean is read instead.
    let item = match read(&id.path) {
        Err(error) if status_of(&error) == Some(404) => {
            let found = resolving(&refs, |reading| {
                tree(ctx, ado, &project, &id.repo, reading.first())
            })
            .ok()
            .and_then(|files| resolve(&files, &id.path))
            .filter(|found| *found != id.path);
            let Some(found) = found else {
                return Err(error);
            };
            let item = read(&found)?;
            ctx.note(format!("[{} is {found}]", id.path));
            id.path = found;
            item
        }
        read => read?,
    };
    let content = text_of(&item, &id.at(None))?;
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let (first, last) = match id.lines {
        None => (1, total.min(MAX_LINES)),
        Some((line, same)) if line == same => (
            line.saturating_sub(AROUND).max(1),
            line.saturating_add(AROUND),
        ),
        Some((first, last)) => (first, last.min(first.saturating_add(MAX_LINES - 1))),
    };
    if first > total.max(1) {
        return Err(Failure::usage(format!(
            "{} has {total} lines, so line {first} is past its end",
            id.at(None)
        ))
        .hint(format!("agent-cli ado file get {}", id.at(None)))
        .into());
    }
    let last = last.min(total);
    let whole = first == 1 && last == total;
    let wanted_end = id.lines.map_or(total, |(_, end)| end.min(total));
    if last < wanted_end {
        ctx.note(format!(
            "[lines {first}-{last} of {total}; more: agent-cli ado file get {}]",
            id.at(Some((last + 1, (last + MAX_LINES).min(wanted_end))))
        ));
    }
    Ok(FileText {
        id: id.at((!whole).then_some((first, last))),
        repo: id.repo.clone(),
        path: id.path.clone(),
        reference: id.reference.clone(),
        commit: text(&item["commitId"]),
        lines: format!("{first}-{last}"),
        total,
        text: numbered(&lines, first, last),
    })
}

command! {
    pub FILE_GET = ["ado", "file", "get"], Read,
    "Show a file in a repository at a branch, tag or commit, around a line",
    keywords: ["read", "source", "cat", "contents", "line", "blob", "open"],
    example: "ado file get api@main:src/Program.cs:42",
    run: file_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, page, urls};

    fn file(lines: usize) -> Answer {
        let content: Vec<String> = (1..=lines).map(|n| format!("line {n}")).collect();
        Answer::json(
            &json!({"path": "/src/x.cs", "commitId": "c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe", "content": content.join("\n")}),
        )
    }

    #[test]
    fn a_line_shows_twenty_either_side_at_the_ref_asked_for() {
        let (outcome, transport) = ado(
            &["ado", "file", "get", "web@main:src/x.cs:42"],
            vec![file(100)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], "web@main:src/x.cs:22-62");
        assert_eq!(got["ref"], "main");
        assert_eq!(got["commit"], "c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe");
        assert_eq!(got["lines"], "22-62");
        assert_eq!(got["total"], 100);
        let text = got["text"].as_str().unwrap();
        assert!(text.starts_with("22  line 22\n"), "{text}");
        assert!(text.ends_with("62  line 62"), "{text}");
        assert_eq!(
            urls(&transport),
            [format!(
                "{CODE}/git/repositories/web/items?path=%2Fsrc%2Fx.cs&versionDescriptor.version=main&versionDescriptor.versionType=branch&includeContent=true&$format=json&api-version=7.1"
            )]
        );
    }

    #[test]
    fn a_bare_ref_that_is_no_branch_is_read_as_a_tag() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "file",
                "get",
                "web:src/x.cs",
                "--ref",
                "v1.2",
                "--line",
                "2-3",
            ],
            vec![
                Answer::status(
                    404,
                    r#"{"message":"TF401175: The version descriptor <Branch: v1.2 > could not be resolved"}"#,
                ),
                file(5),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["text"], "2  line 2\n3  line 3");
        assert!(urls(&transport)[1].contains("versionDescriptor.versionType=tag"));
    }

    #[test]
    fn a_long_file_shows_its_first_400_lines_and_says_how_to_read_on() {
        let (outcome, _) = ado(&["ado", "file", "get", "web:src/x.cs"], vec![file(500)]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "web:src/x.cs:1-400");
        assert!(
            outcome
                .stderr
                .contains("more: agent-cli ado file get web:src/x.cs:401-500]"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_web_url_reads_like_the_id_and_a_disagreeing_flag_is_exit_2() {
        let url = "https://dev.azure.com/contoso/Fabrikam/_git/web?path=/src/x.cs&version=GCc0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe&line=3&lineEnd=4";
        let (outcome, transport) = ado(&["ado", "file", "get", url], vec![file(5)]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["id"],
            "web@c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe:src/x.cs:3-4"
        );
        assert!(urls(&transport)[0].contains("versionDescriptor.versionType=commit"));

        let (outcome, transport) = ado(
            &["ado", "file", "get", "web@main:src/x.cs", "--ref", "dev"],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(transport.sent().is_empty());
        let (outcome, _) = ado(
            &[
                "ado",
                "file",
                "get",
                "https://dev.azure.com/other/P/_git/web?path=/x",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }

    #[test]
    fn a_binary_file_is_refused_with_its_size() {
        let (outcome, _) = ado(
            &["ado", "file", "get", "web:logo.png"],
            vec![Answer::json(
                &json!({"path": "/logo.png", "content": "PNG\u{0}\u{1}"}),
            )],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("binary file (5 bytes)"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_path_not_in_the_repository_is_read_as_the_one_file_it_ends_with() {
        let missing = || {
            Answer::status(
                404,
                r#"{"message":"TF401174: The item '/app/src/x.cs' could not be found."}"#,
            )
        };
        let tree = || {
            page(vec![
                json!({"path": "/src", "isFolder": true}),
                json!({"path": "/src/x.cs"}),
                json!({"path": "/tools/x.cs"}),
            ])
        };
        let (outcome, transport) = ado(
            &[
                "ado",
                "file",
                "get",
                "Fabrikam/web@c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe:/app/src/x.cs:2",
            ],
            vec![missing(), tree(), file(3)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json()["id"],
            "web@c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe:src/x.cs"
        );
        assert!(
            outcome.stderr.contains("[app/src/x.cs is src/x.cs]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{CODE}/git/repositories/web/items?scopePath=%2F&versionDescriptor.version=c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe&versionDescriptor.versionType=commit&recursionLevel=Full&api-version=7.1"
            )
        );

        let (outcome, _) = ado(
            &[
                "ado",
                "file",
                "get",
                "web@c0ffee1c0ffee1c0ffee1c0ffee1c0ffee1c0ffe:/app/x.cs",
            ],
            vec![missing(), tree()],
        );
        assert_eq!(outcome.code, 4, "two files end in x.cs: {outcome:?}");
    }

    #[test]
    fn several_files_print_an_array_in_order_and_a_refused_one_fails_with_the_rest() {
        let (outcome, transport) = ado(
            &["ado", "file", "get", "web:src/x.cs:2", "web:src/y.cs:1-2"],
            vec![file(3), file(3)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["id"], "web:src/x.cs");
        assert_eq!(got[1]["id"], "web:src/y.cs:1-2");
        assert!(urls(&transport)[1].contains("path=%2Fsrc%2Fy.cs"));

        let (outcome, _) = ado(
            &["ado", "file", "get", "web:logo.png", "web:src/x.cs:2"],
            vec![
                Answer::json(&json!({"path": "/logo.png", "content": "PNG\u{0}"})),
                file(3),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert_eq!(outcome.json()[0]["id"], "web:src/x.cs");
        assert!(
            outcome
                .stderr
                .contains("web:logo.png: web:logo.png is a binary file"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_file_past_the_5_mib_azure_devops_puts_in_json_is_read_whole() {
        let line = "x".repeat(99) + "\n";
        let cut = line.repeat(5 * 1024 * 1024 / 100 + 1)[..5 * 1024 * 1024].to_owned();
        let whole = line.repeat(60_000);
        let (outcome, transport) = ado(
            &["ado", "file", "get", "web:big.txt:59999-60000"],
            vec![
                Answer::json(&json!({"path": "/big.txt", "content": cut})),
                Answer::ok(whole),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["total"], 60_000);
        assert!(urls(&transport)[1].contains("$format=octetStream"));
    }
}
