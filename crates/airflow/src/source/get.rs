//! `airflow source get`: a DAG's file around a line (`GET dagSources/DAG`,
//! and `GET dags/DAG` for its path), and the same lines' id in Azure DevOps.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Airflow, At, Want, segment, text};

/// Lines shown either side of a `:LINE`.
const AROUND: usize = 20;
/// The most lines one answer holds.
const MOST: usize = 400;

#[derive(clap::Args)]
pub struct SourceGetArgs {
    /// The DAG and a line or range: DAG[:LINE[-LINE]] (etl_nightly:42, what task logs prints as at), or the DAG's Airflow UI URL
    source: String,
    /// The line or range, when the id leaves it out: LINE or A-B
    #[arg(long)]
    line: Option<String>,
    #[command(flatten)]
    at: At,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Source {
    /// DAG[:LINE[-LINE]]: what source get takes.
    id: String,
    dag: String,
    /// The DAG file, relative to its bundle.
    file: Option<String>,
    /// The DAG version this source is.
    version: Option<i64>,
    /// The lines shown: A-B of the file's total.
    lines: String,
    /// Those lines, each after its number.
    text: String,
    /// The same lines in Azure DevOps, the id ado file get takes (when the
    /// instance names its dags_repo).
    repo_file: Option<String>,
}

/// `LINE` or `A-B`, as the first and last line asked for.
fn span(raw: &str) -> Result<(usize, usize), String> {
    let number = |part: &str| part.trim().parse::<usize>().ok().filter(|line| *line > 0);
    let (first, last) = raw.split_once('-').unwrap_or((raw, raw));
    match (number(first), number(last)) {
        (Some(first), Some(last)) if first <= last => Ok((first, last)),
        _ => Err(format!(
            "{raw:?} is not a line (42) or a range of lines (30-50)"
        )),
    }
}

/// `:42` or `:30-50`, as an id ends.
fn suffix((first, last): (usize, usize)) -> String {
    if first == last {
        format!(":{first}")
    } else {
        format!(":{first}-{last}")
    }
}

fn source_get(ctx: &Ctx, args: SourceGetArgs) -> Result<Source> {
    let raw = args.source.trim();
    let url = raw.starts_with("https://") || raw.starts_with("http://");
    let (dag, held) = match raw.split_once(':') {
        Some((dag, lines)) if !url => (dag, Some(span(lines).map_err(Failure::usage)?)),
        _ => (raw, None),
    };
    let flag = args
        .line
        .as_deref()
        .map(span)
        .transpose()
        .map_err(Failure::usage)?;
    if let (Some(held), Some(flag)) = (held, flag)
        && held != flag
    {
        return Err(Failure::usage(format!(
            "{raw} names line {}, and --line says {}",
            &suffix(held)[1..],
            &suffix(flag)[1..]
        ))
        .into());
    }
    let asked = held.or(flag);
    let airflow = Airflow::load(ctx.config())?;
    let (client, id) =
        airflow.locate(ctx, args.at.instance.as_deref(), dag, Want::Dag, None, None)?;
    let meta = client.get(&id.dag_path())?;
    let source = client.get(&format!("dagSources/{}", segment(&id.dag)))?;
    let file = text(&meta["relative_fileloc"]);
    let all: Vec<&str> = source["content"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .collect();
    let total = all.len();
    let (first, last) = match asked {
        Some((first, _)) if first > total => {
            return Err(Failure::usage(format!(
                "{} has {total} lines, so line {first} is past its end",
                file.as_deref().unwrap_or(&id.dag)
            ))
            .hint(format!("agent-cli airflow source get {}", id.dag))
            .into());
        }
        Some((line, last)) if line == last => (
            line.saturating_sub(AROUND).max(1),
            (line + AROUND).min(total),
        ),
        Some((first, last)) => (first, last.min(total).min(first + MOST - 1)),
        None => (1, total.min(MOST)),
    };
    let asked_last = asked.map_or(total, |(_, last)| last.min(total));
    if last < asked_last {
        ctx.note(format!(
            "[lines {first}-{last} of {total}; the next: agent-cli airflow source get {}:{}-{}]",
            id.dag,
            last + 1,
            (last + MOST).min(asked_last)
        ));
    }
    let range = asked.map(suffix).unwrap_or_default();
    let width = last.to_string().len();
    let shown: Vec<String> = all[first - 1..last]
        .iter()
        .zip(first..)
        .map(|(line, number)| format!("{number:>width$}  {line}").trim_end().to_owned())
        .collect();
    let repo_file = client
        .instance
        .dags_repo
        .as_deref()
        .zip(file.as_deref())
        .map(|(repo, file)| {
            let (repo, folder) = repo.split_once(':').unwrap_or((repo, ""));
            match folder.trim_matches('/') {
                "" => format!("{}:{file}{range}", repo.trim()),
                folder => format!("{}:{folder}/{file}{range}", repo.trim()),
            }
        });
    Ok(Source {
        id: format!("{}{range}", id.dag),
        dag: id.dag,
        file: file.or_else(|| text(&meta["fileloc"])),
        version: source["version_number"].as_i64(),
        lines: format!("{first}-{last} of {total}"),
        text: shown.join("\n"),
        repo_file,
    })
}

command! {
    pub SOURCE_GET = ["airflow", "source", "get"], Read,
    "Show a DAG's Python file around a line, as Airflow parsed it",
    keywords: ["code", "python", "file", "line", "lines", "script", "definition", "read"],
    example: "airflow source get etl_nightly:42 --fields lines,text,repo_file",
    run: source_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{CONFIG, airflow, airflow_with, dag, paths};

    fn source(lines: usize) -> Answer {
        let content: Vec<String> = (1..=lines).map(|line| format!("line {line}")).collect();
        Answer::json(
            &json!({"content": content.join("\n"), "dag_id": "etl_nightly",
            "version_number": 7, "dag_display_name": "etl_nightly"}),
        )
    }

    fn dag_with_file(file: &str) -> Value {
        let mut dag = dag("etl_nightly", false);
        dag["relative_fileloc"] = json!(file);
        dag
    }

    #[test]
    fn a_line_shows_twenty_either_side_and_its_id_in_the_dags_repo() {
        let config = CONFIG.replace("k8s_scope", "dags_repo = \"airflow-dags:dags/\"\nk8s_scope");
        let (outcome, transport) = airflow_with(
            &config,
            &["airflow", "source", "get", "etl_nightly:42"],
            vec![
                Answer::json(&dag_with_file("orders/etl_nightly.py")),
                source(80),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], "etl_nightly:42");
        assert_eq!(got["file"], "orders/etl_nightly.py");
        assert_eq!(got["version"], 7);
        assert_eq!(got["lines"], "22-62 of 80");
        assert_eq!(
            got["repo_file"],
            "airflow-dags:dags/orders/etl_nightly.py:42"
        );
        let text = got["text"].as_str().unwrap();
        assert!(text.starts_with("22  line 22\n"), "{text}");
        assert!(text.contains("\n42  line 42\n"), "{text}");
        assert!(text.ends_with("\n62  line 62"), "{text}");
        assert_eq!(
            paths(&transport),
            ["dags/etl_nightly", "dagSources/etl_nightly"]
        );
    }

    #[test]
    fn no_line_shows_the_first_400_and_names_the_next_range() {
        let (outcome, _) = airflow(
            &[
                "airflow",
                "source",
                "get",
                "https://airflow.contoso.example/dags/etl_nightly/code",
            ],
            vec![Answer::json(&dag_with_file("etl_nightly.py")), source(500)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], "etl_nightly");
        assert_eq!(got["lines"], "1-400 of 500");
        assert_eq!(got.get("repo_file"), None, "no dags_repo, no repo_file");
        assert!(
            outcome
                .stderr
                .contains("the next: agent-cli airflow source get etl_nightly:401-500]"),
            "{}",
            outcome.stderr
        );
        let (outcome, _) = airflow(
            &["airflow", "source", "get", "etl_nightly", "--line", "70-90"],
            vec![Answer::json(&dag_with_file("etl_nightly.py")), source(80)],
        );
        assert_eq!(outcome.json()["lines"], "70-80 of 80");
        assert_eq!(outcome.json()["id"], "etl_nightly:70-90");
    }

    #[test]
    fn a_bad_or_disagreeing_line_is_exit_2() {
        for argv in [
            &["airflow", "source", "get", "etl_nightly:x"][..],
            &["airflow", "source", "get", "etl_nightly:9-3"],
            &["airflow", "source", "get", "etl_nightly:42", "--line", "43"],
        ] {
            let (outcome, transport) = airflow(argv, vec![]);
            assert_eq!(outcome.code, 2, "{argv:?}: {outcome:?}");
            assert!(transport.sent().is_empty());
        }
        let (outcome, _) = airflow(
            &["airflow", "source", "get", "etl_nightly:81"],
            vec![Answer::json(&dag_with_file("etl_nightly.py")), source(80)],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("etl_nightly.py has 80 lines"),
            "{}",
            outcome.stderr
        );
    }
}
