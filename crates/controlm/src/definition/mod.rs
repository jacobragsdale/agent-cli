//! `controlm definition`: the jobs as defined in folders (what Control-M's
//! Planning shows), whether or not they run today, and the id
//! `SERVER/FOLDER/JOB` that `job run` orders.

pub(crate) mod get;
pub(crate) mod list;

use agent_cli_core::Failure;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::client::text;

/// `SERVER/FOLDER/JOB`. A folder inside a folder keeps its `/`s: the first
/// piece is the server and the last the job.
#[derive(Debug)]
pub(crate) struct DefRef {
    pub(crate) server: String,
    pub(crate) folder: String,
    pub(crate) job: String,
}

impl DefRef {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let pieces: Vec<&str> = raw.split('/').collect();
        match pieces.as_slice() {
            [server, folder @ .., job]
                if !folder.is_empty() && pieces.iter().all(|piece| !piece.is_empty()) =>
            {
                Ok(Self {
                    server: (*server).to_owned(),
                    folder: folder.join("/"),
                    job: (*job).to_owned(),
                })
            }
            _ => Err(Failure::usage(format!(
                "{raw:?} is not a job definition (SERVER/FOLDER/JOB)"
            ))
            .hint("agent-cli controlm definition list --name NAME")
            .into()),
        }
    }
}

/// One job as defined.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DefRow {
    /// SERVER/FOLDER/JOB: what definition get and job run take.
    pub(crate) id: String,
    pub(crate) server: String,
    pub(crate) folder: String,
    pub(crate) name: String,
    /// Command, Script, FileWatcher … (Control-M's Job:… without Job:)
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    pub(crate) description: Option<String>,
    /// The agent host, or host group, it runs on.
    pub(crate) host: Option<String>,
    pub(crate) run_as: Option<String>,
    pub(crate) application: Option<String>,
    pub(crate) sub_application: Option<String>,
    /// The command line, or the script's path, it runs.
    pub(crate) command: Option<String>,
}

/// A job found in a `deploy/jobs` answer, with its definition as Control-M
/// wrote it.
pub(crate) struct Found {
    pub(crate) row: DefRow,
    pub(crate) definition: Value,
}

/// Every job in a `deploy/jobs` answer: its top-level keys are folders, and
/// a folder holds jobs (a `Type` of `Job:…`) and folders (`…Folder`) by name
/// beside its own properties.
// VERIFY(work): this is the shape of BMC's JSON code reference (the same
// JSON `deploy` takes). Check against a real answer: where the server name
// is (`ControlmServer` on the folder), that jobs and sub-folders sit beside
// the folder's properties, and the key names of Command, FilePath/FileName,
// Host, RunAs, Application, SubApplication, Description.
pub(crate) fn found(answer: &Value, server: &str) -> Vec<Found> {
    let mut out = Vec::new();
    if let Some(folders) = answer.as_object() {
        walk(folders, server, "", &mut out);
    }
    out
}

fn walk(map: &Map<String, Value>, server: &str, folder: &str, out: &mut Vec<Found>) {
    for (name, value) in map {
        let Some(kind) = value["Type"].as_str() else {
            continue;
        };
        if let Some(kind) = kind.strip_prefix("Job:") {
            out.push(Found {
                row: row(server, folder, name, kind, value),
                definition: value.clone(),
            });
        } else if kind.ends_with("Folder")
            && let Some(inner) = value.as_object()
        {
            let server = value["ControlmServer"].as_str().unwrap_or(server);
            let path = if folder.is_empty() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            };
            walk(inner, server, &path, out);
        }
    }
}

fn row(server: &str, folder: &str, name: &str, kind: &str, job: &Value) -> DefRow {
    let script = match (text(&job["FilePath"]), text(&job["FileName"])) {
        (Some(path), Some(file)) => Some(format!("{}/{file}", path.trim_end_matches('/'))),
        (path, file) => path.or(file),
    };
    DefRow {
        id: format!("{server}/{folder}/{name}"),
        server: server.to_owned(),
        folder: folder.to_owned(),
        name: name.to_owned(),
        kind: Some(kind.to_owned()),
        description: text(&job["Description"]),
        host: text(&job["Host"]),
        run_as: text(&job["RunAs"]),
        application: text(&job["Application"]),
        sub_application: text(&job["SubApplication"]),
        command: text(&job["Command"]).or(script),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_definition_ref_is_server_folder_job_with_folders_inside_folders() {
        let one = DefRef::parse("ctm-prod/NightlyLoads/load_orders").unwrap();
        assert_eq!(
            (one.server.as_str(), one.folder.as_str(), one.job.as_str()),
            ("ctm-prod", "NightlyLoads", "load_orders")
        );
        assert_eq!(
            DefRef::parse("ctm-prod/Loads/Orders/*").unwrap().folder,
            "Loads/Orders"
        );
        for wrong in ["load_orders", "ctm-prod/load_orders", "ctm-prod//x", ""] {
            assert!(DefRef::parse(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn found_walks_folders_and_sub_folders_to_their_jobs() {
        let answer = json!({"NightlyLoads": {
            "Type": "Folder", "ControlmServer": "ctm-prod",
            "load_orders": {"Type": "Job:Command", "Command": "/opt/etl/load_orders.sh", "Host": "etl-01"},
            "Orders": {"Type": "SubFolder",
                "publish_orders": {"Type": "Job:Script", "FilePath": "/opt/etl/", "FileName": "publish.sh"}}
        }});
        let rows: Vec<DefRow> = found(&answer, "*")
            .into_iter()
            .map(|found| found.row)
            .collect();
        assert_eq!(rows[0].id, "ctm-prod/NightlyLoads/load_orders");
        assert_eq!(rows[0].command.as_deref(), Some("/opt/etl/load_orders.sh"));
        assert_eq!(rows[1].id, "ctm-prod/NightlyLoads/Orders/publish_orders");
        assert_eq!(rows[1].command.as_deref(), Some("/opt/etl/publish.sh"));
        assert_eq!(rows[1].kind.as_deref(), Some("Script"));
    }
}
