use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, text};
use crate::ids::{FileId, resolving, with_repo};

use super::{fetch, numbered, text_of};

#[derive(clap::Args)]
pub struct FileGetArgs {
    /// The file: REPO[@REF]:PATH[:LINE[-LINE]] as code list, thread list and diff get print it, its web URL, or a path with --repo
    file: String,
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

fn file_get(ctx: &Ctx, args: FileGetArgs) -> Result<FileText> {
    let ado = Ado::load(ctx)?;
    let id = FileId::parse(
        &ado,
        &with_repo(&args.file, args.repo.as_deref()),
        args.reference.as_deref(),
        args.line.as_deref(),
    )?;
    id.agree_repo(args.repo.as_deref())?;
    if id.path.is_empty() {
        return Err(Failure::usage(format!("{} names no file", args.file))
            .hint(format!("agent-cli ado file list {}", id.at(None)))
            .into());
    }
    let refs: Vec<&str> = id.reference.as_deref().into_iter().collect();
    let item = resolving(&refs, |reading| {
        fetch(
            ctx,
            &ado,
            id.project(&ado),
            &id.repo,
            &id.path,
            reading.first(),
        )
    })?;
    let content = text_of(&item, &id.at(None))?;
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let (first, last) = match id.lines {
        None => (1, total.min(MAX_LINES)),
        Some((line, same)) if line == same => (line.saturating_sub(AROUND).max(1), line + AROUND),
        Some((first, last)) => (first, last.min(first + MAX_LINES - 1)),
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

    use crate::testing::{CODE, ado, urls};

    fn file(lines: usize) -> Answer {
        let content: Vec<String> = (1..=lines).map(|n| format!("line {n}")).collect();
        Answer::json(
            &json!({"path": "/src/x.cs", "commitId": "c0ffee1", "content": content.join("\n")}),
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
        assert_eq!(got["commit"], "c0ffee1");
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
        let url = "https://dev.azure.com/contoso/Fabrikam/_git/web?path=/src/x.cs&version=GCc0ffee1&line=3&lineEnd=4";
        let (outcome, transport) = ado(&["ado", "file", "get", url], vec![file(5)]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "web@c0ffee1:src/x.cs:3-4");
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
}
