use std::collections::HashMap;

use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Ado, Kind, list, stamp, text};
use crate::markdown::html_to_markdown;
use crate::types::{self, Field};
use crate::work_items::person;

/// The most updates one page holds.
const PAGE: usize = 200;

/// Fields every revision touches, that only restate who and when, or that
/// mirror the board column: listing them would bury the changes someone made.
const NOISE: &[&str] = &[
    "System.IsDeleted",
    "System.BoardColumnDone",
    "System.Id",
    "System.Rev",
    "System.AuthorizedDate",
    "System.RevisedDate",
    "System.ChangedDate",
    "System.ChangedBy",
    "System.AuthorizedAs",
    "System.PersonId",
    "System.Watermark",
    "System.AreaId",
    "System.IterationId",
    "System.NodeName",
    "System.CreatedDate",
    "System.CreatedBy",
    "System.TeamProject",
    "System.CommentCount",
    "System.History",
    "Microsoft.VSTS.Common.StateChangeDate",
];

#[derive(clap::Args)]
pub struct HistoryGetArgs {
    /// The work item's id: 1207, #1207, AB#1207 or its web URL
    id: String,
    /// Only changes to this field, by reference or display name ("Story Points")
    #[arg(long)]
    field: Option<String>,
    /// Only changes after this (states always cover the whole history)
    #[arg(long)]
    since: Option<When>,
}

/// A work item's revisions: how long it spent in each state, and who changed
/// what.
#[derive(Debug, Serialize, JsonSchema)]
pub struct History {
    id: i64,
    title: Option<String>,
    /// Oldest first; the current state has no `until`.
    states: Vec<StateSpan>,
    /// Oldest first.
    changes: Vec<Change>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StateSpan {
    state: String,
    /// Who moved it there.
    by: Option<String>,
    since: Option<String>,
    until: Option<String>,
    /// Hours in the state, to a tenth; the current one counts until now.
    hours: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Change {
    rev: Option<i64>,
    by: Option<String>,
    date: Option<String>,
    fields: Vec<FieldChange>,
    /// The comment made with this revision, as Markdown.
    comment: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FieldChange {
    /// The display name, or the reference name of a field the type no longer has.
    field: String,
    /// Rich text as Markdown, a person as their name.
    old: Option<Value>,
    new: Option<Value>,
}

/// `fields[name].newValue` of one update.
pub(crate) fn new_value<'a>(update: &'a Value, name: &str) -> &'a Value {
    &update["fields"][name]["newValue"]
}

/// `WEF_{board}_Kanban.Column` and its kin are a board's private copy of
/// System.BoardColumn.
pub(crate) fn noise(reference: &str) -> bool {
    NOISE.contains(&reference)
        || reference.starts_with("WEF_")
        || (reference.starts_with("System.")
            && (reference.contains("Level") || reference.ends_with("Count")))
}

/// A field's value as an agent reads it: a person by name, rich text as
/// Markdown, a time in UTC.
fn shown(value: &Value, kind: Option<&str>) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::Object(_) => person(value).map(Value::String),
        Value::String(raw) if kind == Some("html") => Some(Value::String(html_to_markdown(raw))),
        Value::String(raw) => Some(Value::String(agent_cli_core::utc(raw))),
        other => Some(other.clone()),
    }
}

/// The reference name `--field` means: one of the type's fields by either
/// name, or a reference name as typed (a field the type has since dropped).
fn wanted(ado: &Ado, raw: &str, kind: Option<&str>, fields: &[Field]) -> Result<String> {
    let raw = raw.trim();
    if let Some(field) = fields
        .iter()
        .find(|f| f.reference.eq_ignore_ascii_case(raw) || f.name.eq_ignore_ascii_case(raw))
    {
        return Ok(field.reference.clone());
    }
    if raw.contains('.') {
        return Ok(raw.to_owned());
    }
    let kind = kind.unwrap_or("TYPE");
    Err(Failure::usage(format!("{kind} has no field {raw:?}"))
        .hint(format!(
            "agent-cli ado workitem-type get {kind:?}{}",
            ado.project_flag()
        ))
        .into())
}

fn hours(since: When, until: When) -> f64 {
    let seconds = (until.0 - since.0).whole_seconds();
    (seconds as f64 / 360.0).round() / 10.0
}

/// Every update to work item `id`, oldest first, page by page.
pub(crate) fn updates(ctx: &Ctx, ado: &Ado, id: i64) -> Result<Vec<Value>> {
    let mut updates = Vec::new();
    loop {
        let url = ado.work(
            &format!("wit/workItems/{id}/updates"),
            &format!("$top={PAGE}&$skip={}", updates.len()),
        );
        let page = ado.get(ctx, &url)?;
        let page = list(&page["value"]);
        updates.extend_from_slice(page);
        if page.len() < PAGE {
            return Ok(updates);
        }
    }
}

fn history_get(ctx: &Ctx, args: HistoryGetArgs) -> Result<History> {
    let ado = Ado::load(ctx)?;
    let id = ado.id(Kind::WorkItem, &args.id)?;
    let updates = updates(ctx, &ado, id)?;
    let latest = |name: &str| updates.iter().rev().find_map(|u| text(new_value(u, name)));
    // Its type's fields are its own project's, whichever one is configured.
    let ado = match latest("System.TeamProject") {
        Some(project) => ado.in_project(&project),
        None => ado,
    };
    let kind = latest("System.WorkItemType");
    let fields = match &kind {
        Some(kind) => types::fields(ctx, &ado, kind)?,
        None => Vec::new(),
    };
    let by_ref: HashMap<&str, &Field> = fields.iter().map(|f| (f.reference.as_str(), f)).collect();
    let only = match &args.field {
        Some(raw) => Some(wanted(&ado, raw, kind.as_deref(), &fields)?),
        None => None,
    };

    let mut history = History {
        id,
        title: latest("System.Title"),
        states: Vec::new(),
        changes: Vec::new(),
    };
    let mut entered: Option<When> = None;
    for update in &updates {
        let changed = new_value(update, "System.ChangedDate");
        let at = changed.as_str().and_then(|raw| raw.parse::<When>().ok());
        let date = stamp(changed);
        let by = person(&update["revisedBy"]);
        if let Some(state) = text(new_value(update, "System.State")) {
            if let Some(last) = history.states.last_mut() {
                last.until.clone_from(&date);
                last.hours = entered.zip(at).map(|(from, to)| hours(from, to));
            }
            entered = at;
            history.states.push(StateSpan {
                state,
                by: by.clone(),
                since: date.clone(),
                until: None,
                hours: None,
            });
        }
        if let (Some(since), Some(at)) = (args.since, at)
            && at < since
        {
            continue;
        }
        let mut changes: Vec<FieldChange> = update["fields"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(reference, _)| !noise(reference))
            .filter(|(reference, _)| {
                only.as_deref()
                    .is_none_or(|only| reference.eq_ignore_ascii_case(only))
            })
            .map(|(reference, change)| {
                let field = by_ref.get(reference.as_str());
                let kind = field.and_then(|f| f.kind.as_deref());
                FieldChange {
                    field: field.map_or_else(|| reference.clone(), |f| f.name.clone()),
                    old: shown(&change["oldValue"], kind),
                    new: shown(&change["newValue"], kind),
                }
            })
            .collect();
        changes.retain(|change| change.old.is_some() || change.new.is_some());
        let comment = text(new_value(update, "System.History"))
            .map(|html| html_to_markdown(&html))
            .filter(|markdown| !markdown.is_empty());
        if changes.is_empty() && (only.is_some() || comment.is_none()) {
            continue;
        }
        history.changes.push(Change {
            rev: update["rev"].as_i64(),
            by,
            date,
            fields: changes,
            comment,
        });
    }
    if let (Some(last), Some(from)) = (history.states.last_mut(), entered) {
        last.hours = Some(hours(from, When::now()));
    }
    Ok(history)
}

command! {
    pub HISTORY_GET = ["ado", "history", "get"], Read,
    "Show a work item's history: days spent in each state, who set which field",
    keywords: ["revisions", "audit", "who", "trail", "timeline", "updates", "previous", "long", "spent", "duration", "age", "stuck"],
    example: "ado history get 42 --field state --fields states",
    run: history_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::{Value, json};

    use crate::testing::{BASE, ado, urls};

    fn update(rev: i64, by: &str, date: &str, fields: Value) -> Value {
        let mut fields = fields;
        fields["System.ChangedDate"] = json!({"newValue": date});
        fields["System.Rev"] = json!({"oldValue": rev - 1, "newValue": rev});
        json!({"id": rev, "rev": rev, "revisedBy": {"displayName": by}, "fields": fields})
    }

    fn type_fields() -> Vec<Answer> {
        vec![
            Answer::json(&json!({"value": [
                {"referenceName": "System.Title", "name": "Title"},
                {"referenceName": "System.State", "name": "State"},
                {"referenceName": "System.AssignedTo", "name": "Assigned To"},
                {"referenceName": "System.Description", "name": "Description"},
                {"referenceName": "Microsoft.VSTS.Scheduling.StoryPoints", "name": "Story Points"}
            ]})),
            Answer::json(&json!({"value": [
                {"referenceName": "System.Description", "type": "html"},
                {"referenceName": "Microsoft.VSTS.Scheduling.StoryPoints", "type": "double"}
            ]})),
        ]
    }

    fn updates() -> Answer {
        Answer::json(&json!({"count": 4, "value": [
            update(1, "Sam Lee", "2026-09-20T09:00:00.12Z", json!({
                "System.WorkItemType": {"newValue": "User Story"},
                "System.Title": {"newValue": "Checkout"},
                "System.State": {"newValue": "New"},
                "System.AreaLevel1": {"newValue": "Fabrikam"},
                "System.IsDeleted": {"newValue": false},
                "WEF_0A1B_Kanban.Column": {"newValue": "New"},
                "System.CreatedBy": {"newValue": {"displayName": "Sam Lee"}}
            })),
            update(2, "Jane Doe", "2026-09-21T09:00:00Z", json!({
                "System.State": {"oldValue": "New", "newValue": "Active"},
                "System.AssignedTo": {"newValue": {"displayName": "Jane Doe", "uniqueName": "jane@contoso.com"}},
                "System.History": {"newValue": "<p>Taking <b>this</b></p>"}
            })),
            {"id": 3, "rev": 2, "revisedBy": {"displayName": "Jane Doe"},
             "relations": {"added": [{"rel": "System.LinkTypes.Related", "url": "x"}]}},
            update(3, "Jane Doe", "2026-09-22T21:00:00Z", json!({
                "System.Description": {"oldValue": "<p>old</p>", "newValue": "<p>Pay with <b>cards</b></p>"},
                "Microsoft.VSTS.Scheduling.StoryPoints": {"oldValue": 3.0, "newValue": 5.0},
                "System.Title": {"oldValue": "Checkout", "newValue": "Checkout with cards"}
            })),
            update(4, "Sam Lee", "2026-09-23T09:00:00Z", json!({
                "System.State": {"oldValue": "Active", "newValue": "Resolved"}
            }))
        ]}))
    }

    #[test]
    fn history_gives_time_in_each_state_and_each_change_as_markdown() {
        let mut answers = vec![updates()];
        answers.extend(type_fields());
        let (outcome, transport) = ado(&["ado", "history", "get", "42"], answers);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["id"], 42);
        assert_eq!(got["title"], "Checkout with cards");
        assert_eq!(
            got["states"][0],
            json!({"state": "New", "by": "Sam Lee", "since": "2026-09-20T09:00:00Z",
                "until": "2026-09-21T09:00:00Z", "hours": 24.0})
        );
        assert_eq!(got["states"][1]["hours"], 48.0);
        assert_eq!(got["states"][2]["state"], "Resolved");
        assert!(got["states"][2].get("until").is_none());
        assert!(got["states"][2]["hours"].as_f64().unwrap() > 0.0);
        assert_eq!(
            got["changes"][0]["fields"],
            json!([{"field": "System.WorkItemType", "new": "User Story"},
                {"field": "Title", "new": "Checkout"}, {"field": "State", "new": "New"}])
        );
        assert_eq!(
            got["changes"][1],
            json!({"rev": 2, "by": "Jane Doe", "date": "2026-09-21T09:00:00Z",
                "fields": [{"field": "State", "old": "New", "new": "Active"},
                    {"field": "Assigned To", "new": "Jane Doe"}],
                "comment": "Taking **this**"})
        );
        assert_eq!(
            got["changes"][2]["fields"],
            json!([
                {"field": "Description", "old": "old", "new": "Pay with **cards**"},
                {"field": "Story Points", "old": 3.0, "new": 5.0},
                {"field": "Title", "old": "Checkout", "new": "Checkout with cards"}
            ])
        );
        assert_eq!(got["changes"].as_array().unwrap().len(), 4);
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{BASE}/Fabrikam/_apis/wit/workItems/42/updates?$top=200&$skip=0&api-version=7.1"
                ),
                format!(
                    "{BASE}/Fabrikam/_apis/wit/workitemtypes/User%20Story/fields?$expand=allowedValues&api-version=7.1"
                ),
                format!("{BASE}/Fabrikam/_apis/wit/fields?api-version=7.1"),
            ]
        );
    }

    #[test]
    fn field_and_since_keep_only_the_changes_asked_for() {
        let mut answers = vec![updates()];
        answers.extend(type_fields());
        let (outcome, _) = ado(
            &[
                "ado",
                "history",
                "get",
                "42",
                "--field",
                "story points",
                "--since",
                "2026-09-22",
            ],
            answers,
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["states"].as_array().unwrap().len(), 3);
        assert_eq!(
            got["changes"],
            json!([{"rev": 3, "by": "Jane Doe", "date": "2026-09-22T21:00:00Z",
                "fields": [{"field": "Story Points", "old": 3.0, "new": 5.0}]}])
        );

        let mut answers = vec![updates()];
        answers.extend(type_fields());
        let (outcome, _) = ado(
            &["ado", "history", "get", "42", "--field", "Effort"],
            answers,
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli ado workitem-type get \"User Story\""),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn an_item_in_another_project_is_read_with_that_projects_type() {
        let mut moved = updates();
        let mut body: Value = serde_json::from_str(&moved.body).unwrap();
        body["value"][0]["fields"]["System.TeamProject"] = json!({"newValue": "Contoso Mobile"});
        moved.body = body.to_string();
        let mut answers = vec![moved];
        answers.extend(type_fields());
        let (outcome, transport) = ado(
            &["ado", "history", "get", "42", "--field", "Effort"],
            answers,
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains(
                "agent-cli ado workitem-type get \"User Story\" --project 'Contoso Mobile'"
            ),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport)[1],
            format!(
                "{BASE}/Contoso%20Mobile/_apis/wit/workitemtypes/User%20Story/fields?$expand=allowedValues&api-version=7.1"
            )
        );
    }

    #[test]
    fn a_long_history_is_read_page_by_page() {
        let first: Vec<Value> = (1..=200)
            .map(|rev| {
                super::tests::update(
                    rev,
                    "Sam Lee",
                    "2026-09-20T09:00:00Z",
                    json!({"System.Title": {"newValue": format!("t{rev}")}}),
                )
            })
            .collect();
        let (outcome, transport) = ado(
            &["ado", "history", "get", "42", "--fields", "title"],
            vec![
                Answer::json(&json!({"count": 200, "value": first})),
                Answer::json(&json!({"count": 1, "value": [
                    update(201, "Sam Lee", "2026-09-21T09:00:00Z", json!({"System.Title": {"newValue": "last"}}))
                ]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!({"title": "last"}));
        assert!(urls(&transport)[1].contains("$top=200&$skip=200"));
    }
}
