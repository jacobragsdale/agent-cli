use agent_cli_core::{Ctx, Failure, command, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::client::{API, Ado, list, segment, text};
use crate::ids::file_id;

#[derive(clap::Args)]
pub struct CodeListArgs {
    /// What to find; Code Search syntax passes through (ext:cs, class:Name, "a phrase")
    text: String,
    /// Only this repository (repeatable)
    #[arg(long)]
    repo: Vec<String>,
    /// Only this project (default: every project, or the code project with --repo)
    #[arg(long)]
    project: Option<String>,
    /// Only under this folder (src/Orders)
    #[arg(long)]
    path: Option<String>,
    /// Only this branch (default: each repository's default branch)
    #[arg(long)]
    branch: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// A file that matches, and where.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CodeRow {
    /// The file get id of its first match: REPO:PATH:LINE.
    id: String,
    repo: String,
    path: String,
    line: Option<usize>,
    /// The first matching line.
    text: Option<String>,
    /// How many matches the file holds.
    matches: usize,
}

fn code_list(ctx: &Ctx, args: CodeListArgs) -> Result<Vec<CodeRow>> {
    let ado = Ado::load(ctx)?;
    let mut filters = Map::new();
    // Code Search refuses a repository filter without its project.
    let project = args
        .project
        .clone()
        .or_else(|| (!args.repo.is_empty()).then(|| ado.code_project.clone()));
    if let Some(project) = project {
        filters.insert("Project".into(), json!([project]));
    }
    if !args.repo.is_empty() {
        filters.insert("Repository".into(), json!(args.repo));
    }
    if let Some(path) = &args.path {
        filters.insert(
            "Path".into(),
            json!([format!("/{}", path.trim_matches('/'))]),
        );
    }
    if let Some(branch) = &args.branch {
        filters.insert("Branch".into(), json!([branch]));
    }
    let mut body = json!({
        "searchText": args.text,
        "$skip": 0,
        "$top": args.limit.saturating_add(1),
        "includeSnippet": true,
    });
    if !filters.is_empty() {
        body["filters"] = Value::Object(filters);
    }
    let url = format!(
        "https://almsearch.dev.azure.com/{}/_apis/search/codesearchresults?api-version={API}",
        segment(&ado.org)
    );
    let answer = ado
        .query(ctx, &url, body)
        .map_err(|error| without_code_search(&ado, error))?;
    // Code Search that cannot search answers 200, no results and an infoCode.
    let unready = match answer["infoCode"].as_u64() {
        Some(1) => Some("it is reindexing the organization"),
        Some(2) => Some("it has not started indexing"),
        Some(6 | 7) => Some("it is onboarding the organization, or is not installed"),
        Some(9) => Some("it is indexing the branches"),
        _ => None,
    };
    if let Some(why) = unready.filter(|_| list(&answer["results"]).is_empty()) {
        return Err(Failure::setup(format!(
            "Code Search cannot search {} yet: {why} (infoCode {})",
            ado.org, answer["infoCode"]
        ))
        .hint(NOT_INSTALLED)
        .into());
    }
    let mut rows: Vec<CodeRow> = list(&answer["results"])
        .iter()
        .filter_map(|result| {
            let repo = text(&result["repository"]["name"])?;
            let path = text(&result["path"])?.trim_start_matches('/').to_owned();
            let project = text(&result["project"]["name"])
                .filter(|project| !project.eq_ignore_ascii_case(&ado.code_project));
            let hits = list(&result["matches"]["content"]);
            let first = hits
                .iter()
                .filter(|hit| hit["line"].as_u64().is_some_and(|line| line > 0))
                .min_by_key(|hit| hit["line"].as_u64());
            let line = first
                .and_then(|hit| hit["line"].as_u64())
                .and_then(|line| usize::try_from(line).ok());
            Some(CodeRow {
                id: file_id(
                    project.as_deref(),
                    &repo,
                    args.branch.as_deref(),
                    &path,
                    line.map(|line| (line, line)),
                ),
                text: first.and_then(|hit| text(&hit["codeSnippet"])),
                matches: hits.len(),
                repo,
                path,
                line,
            })
        })
        .collect();
    if rows.len() > args.limit {
        rows.truncate(args.limit);
        let total = answer["count"].as_u64().unwrap_or_default();
        ctx.note(format!(
            "[first {} of {total} files; --limit N, or narrow with --repo or --path]",
            args.limit
        ));
    }
    Ok(rows)
}

/// An organization without the Code Search extension has no search to
/// answer with: that is setup, not a missing file.
fn without_code_search(ado: &Ado, error: anyhow::Error) -> anyhow::Error {
    if status_of(&error) != Some(404) && !format!("{error:#}").contains("extension") {
        return error;
    }
    Failure::setup(format!(
        "{error:#} (Code Search may not be installed in {})",
        ado.org
    ))
    .hint(NOT_INSTALLED)
    .into()
}

const NOT_INSTALLED: &str = "an organization admin installs the Code Search extension from the Marketplace; until then, agent-cli ado file list REPO --recursive";

command! {
    pub CODE_LIST = ["ado", "code", "list"], Read,
    "Search code in every repository: where a symbol is defined and who calls it",
    keywords: ["grep", "find", "usages", "references", "callers", "definition", "symbol", "class"],
    example: "ado code list IOrderClient --fields id,text",
    run: code_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::ado;

    const SEARCH: &str =
        "https://almsearch.dev.azure.com/contoso/_apis/search/codesearchresults?api-version=7.1";

    #[test]
    fn each_matching_file_is_the_file_get_id_of_its_first_match() {
        let (outcome, transport) = ado(
            &[
                "ado",
                "code",
                "list",
                "IOrderClient",
                "--repo",
                "worker",
                "--path",
                "/src/",
                "--limit",
                "1",
            ],
            vec![Answer::json(&json!({"count": 2, "results": [
                {"fileName": "Retry.cs", "path": "/src/Jobs/Retry.cs",
                    "project": {"name": "Fabrikam"}, "repository": {"name": "worker"},
                    "matches": {"content": [
                        {"charOffset": 400, "length": 12, "line": 21, "codeSnippet": "public Retry(IOrderClient orders)"},
                        {"charOffset": 300, "length": 12, "line": 18, "codeSnippet": "private readonly IOrderClient _orders;"},
                    ]}},
                {"path": "/src/Other.cs", "project": {"name": "Data"}, "repository": {"name": "etl"},
                    "matches": {"content": [{"charOffset": 1}]}},
            ]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "worker:src/Jobs/Retry.cs:18", "repo": "worker", "path": "src/Jobs/Retry.cs", "line": 18,
                "text": "private readonly IOrderClient _orders;", "matches": 2}])
        );
        assert!(
            outcome.stderr.contains("[first 1 of 2 files;"),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert_eq!(sent[0].url, SEARCH);
        assert_eq!(
            sent[0].body.clone().unwrap(),
            json!({"searchText": "IOrderClient", "$skip": 0, "$top": 2, "includeSnippet": true,
                "filters": {"Project": ["Fabrikam"], "Repository": ["worker"], "Path": ["/src"]}})
        );
    }

    #[test]
    fn another_projects_file_carries_its_project_and_no_code_search_is_setup() {
        let (outcome, _) = ado(
            &["ado", "code", "list", "ext:py etl", "--branch", "dev"],
            vec![Answer::json(&json!({"count": 1, "results": [
                {"path": "/x.py", "project": {"name": "Data"}, "repository": {"name": "etl"}, "matches": {"content": []}},
            ]}))],
        );
        assert_eq!(outcome.json()[0]["id"], "Data/etl@dev:x.py");
        let (outcome, _) = ado(
            &["ado", "code", "list", "x"],
            vec![Answer::status(404, r#"{"message":"Not found"}"#)],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(outcome.stderr.contains("Code Search"), "{}", outcome.stderr);
    }

    #[test]
    fn a_code_search_that_cannot_search_yet_is_setup_not_an_empty_answer() {
        let (outcome, _) = ado(
            &["ado", "code", "list", "check_rows"],
            vec![Answer::json(
                &json!({"count": 0, "results": [], "infoCode": 6}),
            )],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(outcome.stderr.contains("infoCode 6"), "{}", outcome.stderr);
        assert!(
            outcome
                .stderr
                .contains("hint: an organization admin installs"),
            "{}",
            outcome.stderr
        );
    }
}
