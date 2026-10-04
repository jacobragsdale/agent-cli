//! `airflow source`: a DAG's file as Airflow parsed it, by line; and where a
//! DAG file's line is, which task logs and import-error get share.

pub(crate) mod get;

use serde_json::Value;

use crate::client::{Instance, text};

/// A DAG's file in its folder: Airflow 3's `relative_fileloc`, else Airflow
/// 2's absolute `fileloc` through [`in_folder`].
pub(crate) fn dag_file(dag: &Value) -> Option<String> {
    text(&dag["relative_fileloc"]).or_else(|| text(&dag["fileloc"]).map(|file| in_folder(&file)))
}

/// An absolute path after its `/dags/`, where Airflow 2's default
/// dags_folder ends; a relative one as it is.
// ponytail: a dags_folder named otherwise keeps its absolute path; read
// [core] dags_folder from the config endpoint when one shows up.
pub(crate) fn in_folder(path: &str) -> String {
    match path.split_once("/dags/") {
        Some((_, rest)) if path.starts_with('/') => rest.to_owned(),
        _ => path.to_owned(),
    }
}

/// The line of the last traceback frame in `file`: the path the DagBag
/// filled from, or the DAG's path in its bundle, which a frame's ends with.
/// Python prints the innermost frame last, and a chain's raised exception
/// after its causes.
pub(crate) fn failing_line<'a>(
    lines: impl DoubleEndedIterator<Item = &'a str>,
    file: &str,
) -> Option<u64> {
    let tail = format!("/{file}");
    lines.rev().find_map(|line| {
        let (path, rest) = line
            .trim_start()
            .strip_prefix("File \"")?
            .split_once("\", line ")?;
        let number = rest.split(',').next()?.trim().parse().ok()?;
        (path == file || path.ends_with(&tail)).then_some(number)
    })
}

/// A DAG file (its path in the bundle) as `REPO:PATH`, the id `ado file get`
/// takes, when the instance names its `dags_repo`.
pub(crate) fn repo_file(instance: &Instance, file: &str) -> Option<String> {
    let repo = instance.dags_repo.as_deref()?;
    let (repo, folder) = repo.split_once(':').unwrap_or((repo, ""));
    Some(match folder.trim_matches('/') {
        "" => format!("{}:{file}", repo.trim()),
        folder => format!("{}:{folder}/{file}", repo.trim()),
    })
}
