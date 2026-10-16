//! `arm activity list`: the activity log (`Microsoft.Insights/eventtypes/
//! management/values`), every subscription in scope read in parallel, the
//! filter built as `az monitor activity-log list` builds it, newest first.

use agent_cli_core::{Ctx, Failure, Request, When, command, percent_encode};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::arm::resource::any;
use crate::arm::{SUBSCRIPTIONS, Subscription, subscriptions};
use crate::client::{ARM, arm_only, bearer, limited, narrow, parallel, program, stamp, text};
use crate::config::Azure;
use crate::net::list;

const API: &str = "2015-04-01";
const SELECT: &str = "eventTimestamp,caller,operationName,status,resourceId,properties";

#[derive(clap::Args)]
pub struct ActivityListArgs {
    /// Only what was done to this resource: an ARM id, portal link or name
    #[arg(long)]
    resource: Option<String>,
    /// Only what was done in this resource group
    #[arg(long)]
    group: Option<String>,
    /// Only what this caller did: a UPN, an object id, or @me (the az login)
    #[arg(long)]
    caller: Option<String>,
    /// Only events in this state
    #[arg(long, value_enum)]
    status: Option<Status>,
    /// From when (default a day before --until; the log keeps 90 days)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// An event's state, as the log's filter spells it.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Status {
    Succeeded,
    Failed,
    Started,
    Accepted,
    Canceled,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ActivityRow {
    when: Option<String>,
    /// A user's UPN, or a service principal's or managed identity's id.
    caller: Option<String>,
    /// Microsoft.Authorization/roleAssignments/delete
    operation: Option<String>,
    /// Started, Accepted, Succeeded, Failed or Canceled.
    status: Option<String>,
    /// The ARM id it was done to, which `arm resource get` takes.
    resource: Option<String>,
    /// Why it failed, as `CODE: message` (RequestDisallowedByPolicy: …).
    error: Option<String>,
}

/// An OData string literal.
fn odata(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

/// A query parameter's value, a space as `%20`.
fn encoded(raw: &str) -> String {
    let mut out = String::new();
    percent_encode(raw, &mut out);
    out.replace('+', "%20")
}

/// Who the az login is: its user's UPN, what the log names a user by.
// VERIFY(work): a service principal login's user.name is its app id, while
// the log may name it by object id.
fn me(ctx: &Ctx) -> Result<String> {
    let mut az = program("az");
    az.args(["account", "show", "--query", "user.name", "-o", "tsv"]);
    let output = ctx.read(az)?;
    let name = output.stdout.trim();
    if !output.status.success() || name.is_empty() {
        return Err(Failure::setup("az account show names no signed-in user")
            .hint("az login, or pass --caller UPN")
            .into());
    }
    Ok(name.to_owned())
}

/// A failed event's reason, from the JSON its `statusMessage` holds.
fn error(event: &Value) -> Option<String> {
    let raw = text(&event["properties"]["statusMessage"])?;
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Some(raw);
    };
    let inner = if parsed["error"].is_object() {
        &parsed["error"]
    } else {
        &parsed
    };
    let field = |key: &str, upper: &str| text(&inner[key]).or_else(|| text(&inner[upper]));
    match (field("code", "Code"), field("message", "Message")) {
        (Some(code), Some(message)) => Some(format!("{code}: {message}")),
        (code, message) => code.or(message).or(Some(raw)),
    }
}

fn activity_list(ctx: &Ctx, args: ActivityListArgs) -> Result<Vec<ActivityRow>> {
    let azure = Azure::load(ctx)?;
    let target = match &args.resource {
        Some(raw) => Some(any(ctx, &azure, raw, args.group.as_deref())?),
        None => None,
    };
    let until = args.until.unwrap_or_else(When::now);
    let since = args
        .since
        .unwrap_or(When(until.0 - time::Duration::days(1)));
    let mut filter = format!(
        "eventTimestamp ge {} and eventTimestamp le {}",
        odata(&since.utc()),
        odata(&until.utc())
    );
    // VERIFY(work): that resourceUri eq matches the events on a resource's
    // extension resources (its role assignments) too; --group does.
    if let Some(target) = &target {
        filter.push_str(&format!(" and resourceUri eq {}", odata(&target.id)));
    } else if let Some(group) = &args.group {
        filter.push_str(&format!(
            " and resourceGroupName eq {}",
            odata(group.trim())
        ));
    }
    if let Some(caller) = &args.caller {
        let caller = match caller.trim() {
            "@me" => me(ctx)?,
            other => other.to_owned(),
        };
        filter.push_str(&format!(" and caller eq {}", odata(&caller)));
    }
    if let Some(status) = args.status {
        filter.push_str(&format!(" and status eq {}", odata(&format!("{status:?}"))));
    }
    let guids: Vec<String> = match target.as_ref().and_then(|t| t.subscription.clone()) {
        Some(guid) => vec![guid],
        None => narrow(
            &subscriptions(ctx, &azure)?,
            &azure.subscriptions,
            &[],
            SUBSCRIPTIONS,
            Subscription::guid,
        )?
        .iter()
        .map(|held| held.guid().to_owned())
        .collect(),
    };
    let read = |guid: &String| -> Result<Vec<Value>> {
        let mint = bearer(ctx, ARM);
        let mut url = format!(
            "https://management.azure.com/subscriptions/{guid}/providers/Microsoft.Insights/eventtypes/management/values?api-version={API}&$filter={}&$select={}",
            encoded(&filter),
            encoded(SELECT)
        );
        let mut events = Vec::new();
        loop {
            let answer = ctx
                .read(Request::get(arm_only(&url)?).auth(&mint))?
                .json()?;
            events.extend(list(&answer["value"]).iter().cloned());
            match text(&answer["nextLink"]) {
                Some(next) if events.len() <= args.limit && next != url => url = next,
                _ => return Ok(events),
            }
        }
    };
    let mut events = Vec::new();
    for found in parallel(&guids, azure.parallel(), read) {
        events.extend(found?);
    }
    let mut rows: Vec<ActivityRow> = events
        .iter()
        .map(|event| {
            let status = text(&event["status"]["value"]);
            ActivityRow {
                when: stamp(&event["eventTimestamp"]),
                caller: text(&event["caller"]),
                operation: text(&event["operationName"]["value"]),
                error: status
                    .as_deref()
                    .filter(|status| status.eq_ignore_ascii_case("failed"))
                    .and_then(|_| error(event)),
                status,
                resource: text(&event["resourceId"]),
            }
        })
        .collect();
    // RFC 3339 in UTC whole seconds sorts as text.
    rows.sort_by(|a, b| b.when.cmp(&a.when));
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub ACTIVITY_LIST = ["arm", "activity", "list"], Read,
    "List the Azure activity log: who changed or deleted what, and why it failed",
    keywords: ["audit", "who", "deleted", "changed", "caller", "operation", "failed", "log", "events", "history"],
    example: "arm activity list --group rg-contoso-prod --since 2d --fields when,caller,operation,status",
    run: activity_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::azure;

    const SUB: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn events_from_every_page_come_newest_first_with_why_they_failed() {
        let event = |at: &str, status: &str, message: Option<&str>| {
            json!({"eventTimestamp": at, "caller": "dana@contoso.example",
                "operationName": {"value": "Microsoft.Storage/storageAccounts/write"},
                "status": {"value": status},
                "resourceId": format!("/subscriptions/{SUB}/resourceGroups/rg-contoso/providers/Microsoft.Storage/storageAccounts/stcontoso"),
                "properties": {"statusMessage": message}})
        };
        let denied = r#"{"error":{"code":"RequestDisallowedByPolicy","message":"Resource 'stcontoso' was disallowed by policy."}}"#;
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "activity",
                "list",
                "--group",
                "rg-contoso",
                "--status",
                "failed",
                "--since",
                "2026-09-28",
                "--until",
                "2026-09-29",
            ],
            vec![
                Answer::json(
                    &json!({"value": [{"subscriptionId": SUB, "displayName": "contoso", "state": "Enabled", "tenantId": "t"}]}),
                ),
                Answer::json(
                    &json!({"value": [event("2026-09-28T09:00:00.1234567Z", "Failed", Some(denied))],
                    "nextLink": format!("https://management.azure.com/subscriptions/{SUB}/providers/Microsoft.Insights/eventtypes/management/values?skipToken=2")}),
                ),
                Answer::json(
                    &json!({"value": [event("2026-09-28T10:00:00Z", "Failed", Some("not json"))]}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["when"], "2026-09-28T10:00:00Z");
        assert_eq!(got[0]["error"], "not json");
        assert_eq!(
            got[1]["error"],
            "RequestDisallowedByPolicy: Resource 'stcontoso' was disallowed by policy."
        );
        assert_eq!(
            transport.sent()[1].url,
            format!(
                "https://management.azure.com/subscriptions/{SUB}/providers/Microsoft.Insights/eventtypes/management/values?api-version=2015-04-01&$filter=eventTimestamp%20ge%20%272026-09-28T00%3A00%3A00Z%27%20and%20eventTimestamp%20le%20%272026-09-29T00%3A00%3A00Z%27%20and%20resourceGroupName%20eq%20%27rg-contoso%27%20and%20status%20eq%20%27Failed%27&$select=eventTimestamp%2Ccaller%2CoperationName%2Cstatus%2CresourceId%2Cproperties"
            )
        );
    }

    #[test]
    fn a_next_link_off_arm_gets_no_token() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "activity",
                "list",
                "--resource",
                &format!("/subscriptions/{SUB}/resourceGroups/rg-contoso"),
            ],
            vec![Answer::json(
                &json!({"value": [], "nextLink": "https://example.com/steal"}),
            )],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn at_me_is_the_az_login() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &[
                "arm",
                "activity",
                "list",
                "--resource",
                &format!("/subscriptions/{SUB}"),
                "--caller",
                "@me",
            ],
            vec![Answer::json(&json!({"value": []}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert!(
            transport.sent()[0]
                .url
                .contains("%20and%20caller%20eq%20%27ops%40contoso.example%27"),
            "{}",
            transport.sent()[0].url
        );
    }
}
