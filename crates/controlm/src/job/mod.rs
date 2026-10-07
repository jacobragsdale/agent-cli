//! `controlm job`: runs of jobs in the active environment (what Control-M's
//! Monitoring shows), the id they carry, and Control-M's status words.

pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod logs;
pub(crate) mod retry;
pub(crate) mod run;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use time::UtcOffset;

use crate::client::{At, Client, ControlM, order_date, stamp, text};

/// What a job's status reads when it failed.
pub(crate) const FAILED: &str = "Ended Not OK";

/// A job id and the instance it is on.
#[derive(Clone, Debug, clap::Args)]
pub struct JobIdArgs {
    /// The job run: SERVER:ORDER_ID, the id job list prints
    id: String,
    #[command(flatten)]
    at: At,
}

impl JobIdArgs {
    /// The client and the checked id; words that are not an id are exit 2
    /// naming the list that finds them.
    pub(crate) fn open<'a>(
        &self,
        controlm: &'a ControlM,
        ctx: &'a Ctx,
    ) -> Result<(Client<'a>, String)> {
        let id = job_id(&self.id)?;
        Ok((controlm.open(ctx, &self.at)?, id))
    }
}

/// `SERVER:ORDER_ID`, as the API takes it in a path.
// VERIFY(work): take the Control-M Web URL of a job here too ("the id is
// the ref"), once you know what one looks like.
pub(crate) fn job_id(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let plain = raw
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte));
    match raw.split_once(':') {
        Some((server, order)) if plain && !server.is_empty() && !order.is_empty() => {
            Ok(raw.to_owned())
        }
        _ => {
            let name = if plain && !raw.is_empty() {
                raw
            } else {
                "NAME"
            };
            Err(
                Failure::usage(format!("{raw:?} is not a job id (SERVER:ORDER_ID)"))
                    .hint(format!("agent-cli controlm job list --name {name}"))
                    .into(),
            )
        }
    }
}

/// What `--status` takes: words for Control-M's statuses.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum Status {
    Ok,
    Failed,
    Running,
    /// Any wait-*.
    Waiting,
    WaitCondition,
    WaitResource,
    WaitHost,
    WaitUser,
    WaitWorkload,
    Unknown,
}

impl Status {
    /// Control-M's own words for it.
    // VERIFY(work): Control-M's words (BMC's docs list these), and that
    // `status` takes a comma list as jobname does; if it takes one, `waiting`
    // needs one request per Wait status.
    pub(crate) fn words(self) -> &'static str {
        match self {
            Self::Ok => "Ended OK",
            Self::Failed => FAILED,
            Self::Running => "Executing",
            Self::Waiting => "Wait Condition,Wait Resource,Wait Host,Wait User,Wait Workload",
            Self::WaitCondition => "Wait Condition",
            Self::WaitResource => "Wait Resource",
            Self::WaitHost => "Wait Host",
            Self::WaitUser => "Wait User",
            Self::WaitWorkload => "Wait Workload",
            Self::Unknown => "Status Unknown",
        }
    }
}

/// One run of a job, as `run/jobs/status` and `run/job/ID/status` give it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JobRow {
    /// SERVER:ORDER_ID: what job get, logs and retry take.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) folder: Option<String>,
    /// SERVER/FOLDER/JOB, the definition it was ordered from: what job run
    /// and definition get take.
    pub(crate) definition: Option<String>,
    /// The Control-M/Server (data center) it runs on.
    pub(crate) server: Option<String>,
    /// Control-M's words: Ended OK, Ended Not OK, Executing, Wait Condition …
    pub(crate) status: Option<String>,
    pub(crate) held: bool,
    /// The plan day it was ordered into.
    pub(crate) order_date: Option<String>,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    /// How many times it has run (a rerun or a cyclic job counts each).
    pub(crate) executions: u64,
    /// The agent host it runs on.
    pub(crate) host: Option<String>,
    pub(crate) application: Option<String>,
    pub(crate) sub_application: Option<String>,
    /// Job:Command, Job:Script, Job:FileWatcher …
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
}

impl JobRow {
    pub(crate) fn from(status: &Value, offset: UtcOffset) -> Self {
        let server = text(&status["ctm"]);
        let folder = text(&status["folder"]);
        let name = text(&status["name"]).unwrap_or_default();
        let definition = match (&server, &folder) {
            (Some(server), Some(folder)) if !name.is_empty() => {
                Some(format!("{server}/{folder}/{name}"))
            }
            _ => None,
        };
        Self {
            id: text(&status["jobId"]).unwrap_or_default(),
            name,
            folder,
            definition,
            server,
            status: text(&status["status"]),
            held: status["held"].as_bool().unwrap_or(false),
            order_date: order_date(&status["orderDate"]),
            start: stamp(&status["startTime"], offset),
            end: stamp(&status["endTime"], offset),
            executions: status["numberOfRuns"].as_u64().unwrap_or_default(),
            host: text(&status["host"]),
            application: text(&status["application"]),
            sub_application: text(&status["subApplication"]),
            kind: text(&status["type"]),
        }
    }

    pub(crate) fn failed(&self) -> bool {
        self.status.as_deref() == Some(FAILED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_id_is_server_colon_order_id_and_a_name_hints_the_list() {
        assert_eq!(job_id(" ctm-prod:00a1b ").unwrap(), "ctm-prod:00a1b");
        let failure = job_id("load_orders").unwrap_err();
        let failure = failure.downcast_ref::<Failure>().unwrap();
        assert_eq!(
            failure.hint.as_deref(),
            Some("agent-cli controlm job list --name load_orders")
        );
        let failure = job_id("load orders/x").unwrap_err();
        assert!(
            failure
                .downcast_ref::<Failure>()
                .unwrap()
                .hint
                .as_deref()
                .unwrap()
                .ends_with("--name NAME")
        );
    }

    #[test]
    fn status_words_are_control_ms() {
        assert_eq!(Status::Failed.words(), "Ended Not OK");
        assert_eq!(Status::WaitResource.words(), "Wait Resource");
    }
}
