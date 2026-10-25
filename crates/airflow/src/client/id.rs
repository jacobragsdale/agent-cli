//! The ids every command takes: a DAG, a run, a task instance, as their
//! pieces and their API paths.

use agent_cli_core::{Failure, same_origin};
use anyhow::Result;
use serde_json::Value;

use super::{Airflow, Instance, segment};

/// What a command's positional names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Want {
    Dag,
    Run,
    Task,
}

impl Want {
    pub(super) fn noun(self) -> &'static str {
        match self {
            Self::Dag => "DAG",
            Self::Run => "run",
            Self::Task => "task instance",
        }
    }

    pub(super) fn shape(self) -> &'static str {
        match self {
            Self::Dag => "DAG (etl_nightly)",
            Self::Run => {
                "DAG/RUN (etl_nightly/scheduled__2026-09-28T00:00:00+00:00, or DAG/latest)"
            }
            Self::Task => {
                "DAG/RUN/TASK[:MAP][/TRY] (etl_nightly/latest/load_orders, …/load_orders:3/2)"
            }
        }
    }

    pub(super) fn url_shape(self) -> &'static str {
        match self {
            Self::Dag => "/dags/DAG",
            Self::Run => "/dags/DAG/runs/RUN",
            Self::Task => "/dags/DAG/runs/RUN/tasks/TASK",
        }
    }
}

/// A DAG, run or task instance, as its pieces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Ref {
    pub(crate) dag: String,
    pub(crate) run: String,
    pub(crate) task: String,
    pub(crate) map: Option<i64>,
    /// The try the id names, when it names one.
    pub(crate) attempt: Option<i64>,
}

impl Ref {
    /// `raw` as `want`'s id, with `--dag` and `--run` standing in for its
    /// leading pieces. The DAG comes off the front and `TASK[:MAP][/TRY]` off
    /// the back, so a custom run id holding `/` still parses.
    pub(crate) fn parse(
        raw: &str,
        want: Want,
        dag: Option<&str>,
        run: Option<&str>,
    ) -> Result<Self, String> {
        let mut parts: Vec<&str> = raw.split('/').collect();
        if parts
            .iter()
            .any(|part| part.trim().is_empty() || matches!(*part, "." | ".."))
        {
            return Err(format!("{raw:?} is not a {} id", want.noun()));
        }
        let mut id = Self::default();
        let missing = |what: &str| format!("{raw:?} names no {what}; give {}", want.shape());
        match want {
            Want::Dag => {
                if parts.len() != 1 {
                    return Err(format!("{raw:?} is not a DAG id (it holds a /)"));
                }
                id.dag = parts[0].to_owned();
                disagree(raw, &id.dag, dag, "dag")?;
            }
            Want::Run => {
                if parts.len() == 1 {
                    id.dag = dag.ok_or_else(|| missing("DAG"))?.to_owned();
                    id.run = parts[0].to_owned();
                } else {
                    id.dag = parts[0].to_owned();
                    id.run = parts[1..].join("/");
                    disagree(raw, &id.dag, dag, "dag")?;
                }
            }
            Want::Task => {
                // The pieces the id must carry in front of the task.
                let front = 2 - usize::from(dag.is_some()) - usize::from(run.is_some());
                let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
                if parts.len() > front + 1 && parts.last().is_some_and(|last| digits(last)) {
                    id.attempt = parts.pop().and_then(|attempt| attempt.parse().ok());
                }
                let task = parts.pop().unwrap_or_default();
                match task.split_once(':') {
                    Some((task, map)) if digits(map) && !map.is_empty() => {
                        id.task = task.to_owned();
                        id.map = map.parse().ok();
                    }
                    Some(_) => return Err(format!("{raw:?}: a map index is :N, a number")),
                    None => id.task = task.to_owned(),
                }
                match (parts.as_slice(), dag, run) {
                    ([], Some(dag), Some(run)) => {
                        id.dag = dag.to_owned();
                        id.run = run.to_owned();
                    }
                    ([held], Some(dag), Some(run)) => {
                        disagree(raw, held, Some(run), "run")?;
                        id.dag = dag.to_owned();
                        id.run = run.to_owned();
                    }
                    ([held], Some(dag), None) => {
                        id.dag = dag.to_owned();
                        id.run = (*held).to_owned();
                    }
                    ([held], None, Some(run)) => {
                        id.dag = (*held).to_owned();
                        id.run = run.to_owned();
                    }
                    ([first, rest @ ..], dag, run) if !rest.is_empty() => {
                        id.dag = (*first).to_owned();
                        id.run = rest.join("/");
                        disagree(raw, &id.dag, dag, "dag")?;
                        disagree(raw, &id.run, run, "run")?;
                    }
                    _ => return Err(missing("DAG and run")),
                }
                if id.task.is_empty() {
                    return Err(missing("task"));
                }
            }
        }
        Ok(id)
    }

    /// `DAG/RUN`: what the run commands take.
    pub(crate) fn run_id(&self) -> String {
        format!("{}/{}", self.dag, self.run)
    }

    pub(crate) fn dag_path(&self) -> String {
        format!("dags/{}", segment(&self.dag))
    }

    pub(crate) fn run_path(&self) -> String {
        format!("{}/dagRuns/{}", self.dag_path(), segment(&self.run))
    }

    /// `…/taskInstances/TASK[/MAP]`.
    pub(crate) fn ti_path(&self) -> String {
        let mut path = format!("{}/taskInstances/{}", self.run_path(), segment(&self.task));
        if let Some(map) = self.map {
            path.push_str(&format!("/{map}"));
        }
        path
    }
}

/// Exit 2 when a flag names a different piece than the ref holds.
pub(super) fn disagree(
    raw: &str,
    held: &str,
    flag: Option<&str>,
    what: &str,
) -> Result<(), String> {
    match flag {
        Some(flag) if !held.is_empty() && flag != held => Err(format!(
            "{raw} names {what} {held}, and --{what} says {flag}"
        )),
        _ => Ok(()),
    }
}

/// A task instance's id from the API's own record: `DAG/RUN/TASK[:MAP]`,
/// and `/TRY` when `with_try` and it has run, so it pastes into `task logs`
/// for that try.
pub(crate) fn ti_id(ti: &Value, with_try: bool) -> String {
    let mut id = format!(
        "{}/{}/{}",
        ti["dag_id"].as_str().unwrap_or_default(),
        ti["dag_run_id"].as_str().unwrap_or_default(),
        ti["task_id"].as_str().unwrap_or_default()
    );
    if let Some(map) = ti["map_index"].as_i64().filter(|map| *map >= 0) {
        id.push_str(&format!(":{map}"));
    }
    if let Some(attempt) = ti["try_number"]
        .as_i64()
        .filter(|attempt| with_try && *attempt > 0)
    {
        id.push_str(&format!("/{attempt}"));
    }
    id
}

impl Airflow {
    /// An Airflow UI URL under a configured `base_url` as the instance and
    /// the id it names, read and never fetched.
    pub(super) fn parse_url(&self, raw: &str, want: Want) -> Result<(&Instance, Ref)> {
        let url = raw.split('#').next().unwrap_or_default();
        let (url, query) = url.split_once('?').unwrap_or((url, ""));
        let Some(instance) = self
            .instances
            .iter()
            .find(|instance| same_origin(&instance.base_url, url))
        else {
            let configured: Vec<String> = self
                .instances
                .iter()
                .map(|instance| format!("{} ({})", instance.name, instance.base_url))
                .collect();
            return Err(Failure::usage(format!(
                "{raw} is not under a configured base_url; configured: {}",
                if configured.is_empty() {
                    "none".to_owned()
                } else {
                    configured.join(", ")
                }
            ))
            .hint("pass the id instead, or add the server as an [[airflow.instance]]")
            .into());
        };
        let rest = &url[instance.base_url.len() + 1..];
        let parts: Vec<String> = rest
            .split('/')
            .filter(|part| !part.is_empty())
            .map(decode)
            .collect();
        let after = |marker: &str| {
            parts
                .iter()
                .position(|part| part == marker)
                .and_then(|at| parts.get(at + 1))
                .cloned()
        };
        let mut id = Ref {
            dag: after("dags").unwrap_or_default(),
            run: after("runs").unwrap_or_default(),
            task: after("tasks").unwrap_or_default(),
            map: after("mapped").and_then(|map| map.parse().ok()),
            attempt: query
                .split('&')
                .find_map(|pair| pair.strip_prefix("try_number="))
                .and_then(|attempt| attempt.parse().ok()),
        };
        // A deeper URL than the command needs names its DAG or run too.
        if want != Want::Task {
            id.task.clear();
        }
        if want == Want::Dag {
            id.run.clear();
        }
        let missing = match want {
            Want::Dag => id.dag.is_empty(),
            Want::Run => id.dag.is_empty() || id.run.is_empty(),
            Want::Task => id.dag.is_empty() || id.run.is_empty() || id.task.is_empty(),
        };
        if missing {
            return Err(Failure::usage(format!(
                "{raw} does not name a {}; it needs {}",
                want.noun(),
                want.url_shape()
            ))
            .into());
        }
        Ok((instance, id))
    }
}

/// A URL path segment with every `%XX` decoded.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ui_url_decodes_to_the_same_pieces() {
        assert_eq!(
            decode("scheduled__2026-09-28T00%3A00%3A00%2B00%3A00"),
            "scheduled__2026-09-28T00:00:00+00:00"
        );
        assert_eq!(decode("100%"), "100%");
    }

    #[test]
    fn ids_take_the_dag_off_the_front_and_the_task_off_the_back() {
        let task =
            |raw: &str, dag: Option<&str>, run: Option<&str>| Ref::parse(raw, Want::Task, dag, run);
        let want = |dag: &str, run: &str, task: &str, map: Option<i64>, attempt: Option<i64>| {
            Ok(Ref {
                dag: dag.into(),
                run: run.into(),
                task: task.into(),
                map,
                attempt,
            })
        };
        let run = "scheduled__2026-09-28T00:00:00+00:00";
        assert_eq!(
            task(&format!("etl/{run}/load_orders:3/2"), None, None),
            want("etl", run, "load_orders", Some(3), Some(2))
        );
        assert_eq!(
            task("etl/custom/with/slash/load/2", None, None),
            want("etl", "custom/with/slash", "load", None, Some(2)),
            "a run id holding / still parses"
        );
        assert_eq!(
            task("etl/r1/123", None, None),
            want("etl", "r1", "123", None, None),
            "three pieces are always DAG/RUN/TASK"
        );
        assert_eq!(
            task("load", Some("etl"), Some("r1")),
            want("etl", "r1", "load", None, None)
        );
        assert_eq!(
            task("load/2", Some("etl"), Some("r1")),
            want("etl", "r1", "load", None, Some(2))
        );
        assert_eq!(
            task("r1/load", Some("etl"), None),
            want("etl", "r1", "load", None, None)
        );
        assert_eq!(
            task("etl/r1/load", Some("etl"), Some("r1")),
            want("etl", "r1", "load", None, None),
            "a flag that agrees is fine"
        );
        for (raw, dag, run) in [
            ("etl/r1/load", Some("other"), None),
            ("etl/r1/load", None, Some("r2")),
            ("load", None, None),
            ("r1/load", None, None),
            ("etl/r1/load:x", None, None),
            ("etl//load", None, None),
            ("etl/../load", None, None),
        ] {
            assert!(task(raw, dag, run).is_err(), "{raw} {dag:?} {run:?}");
        }
        assert_eq!(
            Ref::parse("etl/latest", Want::Run, None, None)
                .unwrap()
                .run_id(),
            "etl/latest"
        );
        assert_eq!(
            Ref::parse("r1", Want::Run, Some("etl"), None)
                .unwrap()
                .run_id(),
            "etl/r1"
        );
        assert!(Ref::parse("r1", Want::Run, None, None).is_err());
        assert!(Ref::parse("etl/r1", Want::Run, Some("other"), None).is_err());
        assert!(Ref::parse("etl/r1", Want::Dag, None, None).is_err());
        let id = Ref::parse(&format!("etl/{run}/load:3"), Want::Task, None, None).unwrap();
        assert_eq!(
            id.ti_path(),
            "dags/etl/dagRuns/scheduled__2026-09-28T00%3A00%3A00%2B00%3A00/taskInstances/load/3"
        );
    }
}
