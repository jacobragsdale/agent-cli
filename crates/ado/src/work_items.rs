//! What the workitem, pr and run commands share about work items: their
//! rows, read in batches by id, and the artifact links that tie one to a
//! pull request or a branch.

use std::collections::HashMap;

use agent_cli_core::{Ctx, Exit, Failure, Method};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{Ado, list, stamp, text};

/// The fields a list row carries. The batch endpoint returns only these, so
/// fifty rows cost one small request.
pub(crate) const ROW_FIELDS: [&str; 10] = [
    "System.WorkItemType",
    "System.Title",
    "System.State",
    "System.AssignedTo",
    "System.IterationPath",
    "System.AreaPath",
    "Microsoft.VSTS.Common.Priority",
    "System.Tags",
    "System.ChangedDate",
    "System.Id",
];

/// The largest id batch the work item endpoints accept.
const BATCH: usize = 200;

/// One work item as a list shows it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkItemRow {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    state: Option<String>,
    assignee: Option<String>,
    iteration: Option<String>,
    area: Option<String>,
    priority: Option<i64>,
    tags: Vec<String>,
    /// When it last changed, RFC 3339.
    changed: Option<String>,
    /// Its revision, for `workitem update --if-rev`.
    rev: Option<i64>,
}

/// A work item named by something else (a run, a pull request): enough to
/// say what it is, with the id `ado workitem get` takes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkItemRef {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    state: Option<String>,
}

impl From<&WorkItemRow> for WorkItemRef {
    fn from(row: &WorkItemRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind.clone(),
            title: row.title.clone(),
            state: row.state.clone(),
        }
    }
}

pub(crate) fn row(item: &Value) -> WorkItemRow {
    let fields = &item["fields"];
    let field = |name: &str| text(&fields[name]);
    WorkItemRow {
        id: item["id"]
            .as_i64()
            .or_else(|| fields["System.Id"].as_i64())
            .unwrap_or_default(),
        kind: field("System.WorkItemType"),
        title: field("System.Title"),
        state: field("System.State"),
        assignee: person(&fields["System.AssignedTo"]),
        iteration: field("System.IterationPath"),
        area: field("System.AreaPath"),
        priority: fields["Microsoft.VSTS.Common.Priority"].as_i64(),
        tags: field("System.Tags")
            .map(|tags| {
                tags.split(';')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        changed: stamp(&fields["System.ChangedDate"]),
        rev: item["rev"].as_i64(),
    }
}

/// An identity field's display name; older answers hold a plain string.
pub(crate) fn person(value: &Value) -> Option<String> {
    text(&value["displayName"])
        .or_else(|| text(&value["uniqueName"]))
        .or_else(|| text(value))
}

/// The rows for `ids`, in the order given.
pub(crate) fn rows(ctx: &Ctx, ado: &Ado, ids: &[i64]) -> Result<Vec<WorkItemRow>> {
    Ok(read(ctx, ado, ids, &ROW_FIELDS)?.iter().map(row).collect())
}

/// The work items `ids` with `fields` (and each one's `rev`), in the order
/// given: the batch endpoint does not promise the WIQL's order.
pub(crate) fn read(ctx: &Ctx, ado: &Ado, ids: &[i64], fields: &[&str]) -> Result<Vec<Value>> {
    let mut items = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(BATCH) {
        let answer = ado.query(
            ctx,
            &ado.work("wit/workitemsbatch", ""),
            json!({"ids": chunk, "fields": fields, "errorPolicy": "omit"}),
        )?;
        items.extend(
            list(&answer["value"])
                .iter()
                .filter(|item| !item.is_null())
                .cloned(),
        );
    }
    let rank: HashMap<i64, usize> = ids.iter().enumerate().map(|(at, id)| (*id, at)).collect();
    items.sort_by_key(|item| {
        item["id"]
            .as_i64()
            .and_then(|id| rank.get(&id).copied())
            .unwrap_or(usize::MAX)
    });
    Ok(items)
}

/// Where each process keeps a work item's size: Story Points (Agile), Effort
/// (Scrum), Size (CMMI).
pub(crate) const POINTS: [&str; 3] = [
    "Microsoft.VSTS.Scheduling.StoryPoints",
    "Microsoft.VSTS.Scheduling.Effort",
    "Microsoft.VSTS.Scheduling.Size",
];

/// A batch-read work item's points, whichever process field holds them.
pub(crate) fn points(item: &Value) -> Option<f64> {
    POINTS
        .iter()
        .find_map(|field| item["fields"][field].as_f64())
}

/// What a `vstfs:///` artifact link points at, when it is something this
/// command shows: `vstfs:///Git/PullRequestId/{project}%2F{repo}%2F{id}` or
/// `vstfs:///Git/Ref/{project}%2F{repo}%2FGB{branch}`.
#[derive(Debug, PartialEq)]
pub(crate) enum Artifact {
    PullRequest { repo_id: String, id: i64 },
    Branch { repo_id: String, name: String },
}

pub(crate) fn artifact(url: &str) -> Option<Artifact> {
    let rest = url.strip_prefix("vstfs:///Git/")?;
    let (kind, id) = rest.split_once('/')?;
    // The separators inside the id are percent-encoded, in either case.
    let parts: Vec<&str> = id
        .split('/')
        .flat_map(|part| part.split("%2F"))
        .flat_map(|part| part.split("%2f"))
        .collect();
    match (kind, parts.as_slice()) {
        ("PullRequestId", [_project, repo_id, id]) => Some(Artifact::PullRequest {
            repo_id: (*repo_id).to_owned(),
            id: id.parse().ok()?,
        }),
        // A branch name carries its own slashes, encoded like the separators.
        ("Ref", [_project, repo_id, rest @ ..]) => {
            let name = rest.join("/");
            let name = name.strip_prefix("GB")?;
            (!name.is_empty()).then(|| Artifact::Branch {
                repo_id: (*repo_id).to_owned(),
                name: name.to_owned(),
            })
        }
        _ => None,
    }
}

/// The work item's own links: `(rel, url)`.
pub(crate) fn relations(item: &Value) -> impl Iterator<Item = (&str, &str)> {
    list(&item["relations"])
        .iter()
        .filter_map(|relation| Some((relation["rel"].as_str()?, relation["url"].as_str()?)))
}

/// A refused write that means the work item changed since it was read, as
/// exit 5 with the way back. Azure DevOps reports a failed `test /rev` as a
/// 4xx whose message talks about the revision or the test.
pub(crate) fn moved_on(error: anyhow::Error, id: i64) -> anyhow::Error {
    let said = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Failure>())
        .filter(|failure| {
            let message = failure.message.to_ascii_lowercase();
            failure.exit == Exit::Conflict
                || (failure.exit == Exit::Usage
                    && ["/rev", "revision", "test operation", "changed by another"]
                        .iter()
                        .any(|needle| message.contains(needle)))
        })
        .map(|failure| failure.message.clone());
    match said {
        Some(message) => Failure::conflict(format!("work item {id} changed since it was read: {message}"))
            .hint(format!(
                "re-read it (agent-cli ado workitem get {id} --fields rev,state,assignee), then run it again with the new --if-rev"
            ))
            .into(),
        None => error,
    }
}

/// Appends one artifact link to work item `id` behind a test of the revision
/// it was read at, unless it already holds a link to the same thing (however
/// Azure DevOps encoded it). Returns whether it wrote one.
pub(crate) fn add_artifact_link(
    ctx: &Ctx,
    ado: &Ado,
    id: i64,
    url: &str,
    name: &str,
) -> Result<bool> {
    let item_url = ado.api(
        None,
        &format!("wit/workitems/{id}"),
        "$expand=relations",
        crate::client::API,
    );
    let item = ado.get(ctx, &item_url)?;
    let rev = item["rev"]
        .as_i64()
        .context("the work item came back without a revision to test")?;
    let wanted = artifact(url);
    if relations(&item).any(|(rel, held)| rel == "ArtifactLink" && artifact(held) == wanted) {
        return Ok(false);
    }
    let document = vec![
        json!({"op": "test", "path": "/rev", "value": rev}),
        json!({"op": "add", "path": "/relations/-", "value": {
            "rel": "ArtifactLink", "url": url, "attributes": {"name": name},
        }}),
    ];
    let url = ado.api(None, &format!("wit/workitems/{id}"), "", crate::client::API);
    ado.patch_work_item(ctx, Method::Patch, &url, document)
        .map_err(|error| moved_on(error, id))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::testing::{ado, batch, item, wiql};

    #[test]
    fn more_than_two_hundred_ids_are_read_in_batches_the_endpoint_accepts() {
        let ids: Vec<i64> = (1..=250).collect();
        let first: Vec<Value> = ids[..200].iter().map(|id| item(*id, 1, "t")).collect();
        let second: Vec<Value> = ids[200..].iter().map(|id| item(*id, 1, "t")).collect();
        let (outcome, transport) = ado(
            &[
                "ado", "workitem", "list", "--limit", "250", "--fields", "id",
            ],
            vec![wiql(&ids), batch(first), batch(second)],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
        assert_eq!(outcome.json()[249], json!({"id": 250}));
        let sent = transport.sent();
        let count = |at: usize| {
            sent[at].body.as_ref().unwrap()["ids"]
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!((count(1), count(2)), (200, 50));
    }
}
