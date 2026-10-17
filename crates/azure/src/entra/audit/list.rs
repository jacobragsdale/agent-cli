//! `entra audit list`: `GET /auditLogs/directoryAudits`, filtered by Graph
//! on the window, the target, the caller, the activity, the category and
//! the result, newest first. Changed values are cut at 200 characters; a
//! credential change names key ids and display names, never a value
//! (VERIFY(work) on a live tenant).

use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{GRAPH, claims, guid, stamp, text};
use crate::entra::{Graph, Kind, Status, V1, find, odata, param};

/// How much of a changed value a row keeps.
const CUT: usize = 200;

#[derive(clap::Args)]
pub struct AuditListArgs {
    /// Only changes to this: an object id, or a user, group, app or service principal as its get takes it
    #[arg(long)]
    target: Option<String>,
    /// Only changes this caller made: a UPN, @me, or an app's client id
    #[arg(long)]
    caller: Option<String>,
    /// Only this activity, exactly: Add member to group, Update application – Certificates and secrets management …
    #[arg(long)]
    activity: Option<String>,
    /// Only this category: UserManagement, GroupManagement, ApplicationManagement, RoleManagement …
    #[arg(long)]
    category: Option<String>,
    /// Only changes that succeeded or failed
    #[arg(long, value_enum)]
    status: Option<Status>,
    /// From when (default 7 days before --until; P1 and P2 keep 30 days, Free 7)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AuditRow {
    when: Option<String>,
    /// The user's UPN or the app's name.
    caller: Option<String>,
    activity: Option<String>,
    category: Option<String>,
    /// success or failure.
    result: Option<String>,
    reason: Option<String>,
    targets: Vec<Target>,
    changes: Vec<Change>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Target {
    id: Option<String>,
    /// User, Group, Application, ServicePrincipal …
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Change {
    property: Option<String>,
    old: Option<String>,
    new: Option<String>,
}

fn cut(value: &Value) -> Option<String> {
    let held = text(value)?;
    Some(if held.chars().count() > CUT {
        format!("{}…", held.chars().take(CUT).collect::<String>())
    } else {
        held
    })
}

fn row(held: &Value) -> AuditRow {
    let by = &held["initiatedBy"];
    let targets = held["targetResources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    AuditRow {
        when: stamp(&held["activityDateTime"]),
        caller: text(&by["user"]["userPrincipalName"]).or_else(|| text(&by["app"]["displayName"])),
        activity: text(&held["activityDisplayName"]),
        category: text(&held["category"]),
        result: text(&held["result"]),
        reason: text(&held["resultReason"]),
        changes: targets
            .iter()
            .flat_map(|target| {
                target["modifiedProperties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .map(|change| Change {
                property: text(&change["displayName"]),
                old: cut(&change["oldValue"]),
                new: cut(&change["newValue"]),
            })
            .collect(),
        targets: targets
            .iter()
            .map(|target| Target {
                id: text(&target["id"]),
                kind: text(&target["type"]),
                name: text(&target["displayName"]).or_else(|| text(&target["userPrincipalName"])),
            })
            .collect(),
    }
}

fn audit_list(ctx: &Ctx, args: AuditListArgs) -> Result<Vec<AuditRow>> {
    let graph = Graph::new(ctx);
    let until = args.until.unwrap_or_else(When::now);
    let since = args
        .since
        .unwrap_or(When(until.0 - time::Duration::days(7)));
    if since.0 >= until.0 {
        return Err(Failure::usage("--since is not before --until").into());
    }
    let mut filter = vec![format!(
        "activityDateTime ge {} and activityDateTime le {}",
        since.utc(),
        until.utc()
    )];
    if let Some(target) = &args.target {
        let target = target.trim();
        let id = if guid(target) {
            target.to_ascii_lowercase()
        } else {
            let (_, held) = find(
                &graph,
                target,
                &[Kind::App, Kind::ServicePrincipal, Kind::User, Kind::Group],
                "",
            )?;
            text(&held["id"]).unwrap_or_default()
        };
        filter.push(format!("targetResources/any(t: t/id eq {})", odata(&id)));
    }
    if let Some(caller) = &args.caller {
        let caller = caller.trim();
        filter.push(if caller == "@me" {
            let upn = claims(ctx, GRAPH)?
                .and_then(|claims| text(&claims["upn"]))
                .ok_or_else(|| {
                    Failure::setup("the az login's token names no UPN").hint("pass --caller UPN")
                })?;
            format!("initiatedBy/user/userPrincipalName eq {}", odata(&upn))
        } else if guid(caller) {
            format!(
                "initiatedBy/app/appId eq {}",
                odata(&caller.to_ascii_lowercase())
            )
        } else {
            format!("initiatedBy/user/userPrincipalName eq {}", odata(caller))
        });
    }
    if let Some(activity) = &args.activity {
        filter.push(format!("activityDisplayName eq {}", odata(activity.trim())));
    }
    if let Some(category) = &args.category {
        filter.push(format!("category eq {}", odata(category.trim())));
    }
    match args.status {
        Some(Status::Succeeded) => filter.push("result eq 'success'".to_owned()),
        Some(Status::Failed) => filter.push("result eq 'failure'".to_owned()),
        None => {}
    }
    let url = format!(
        "{V1}/auditLogs/directoryAudits?$filter={}&$top={}",
        param(&filter.join(" and ")),
        (args.limit + 1).min(999)
    );
    let found = graph.pages(&url, args.limit)?;
    if found.more {
        ctx.note(format!(
            "[the newest {}; narrow --since, --target or --activity, or --limit N]",
            args.limit
        ));
    }
    Ok(found.rows.iter().map(row).collect())
}

command! {
    pub AUDIT_LIST = ["entra", "audit", "list"], Read,
    "List the Entra audit log: who added or removed members, credentials, roles",
    keywords: ["who", "added", "removed", "changed", "deleted", "history", "member", "credential", "directory", "activity"],
    example: "entra audit list --target sg-contoso-oncall --since 7d --fields when,caller,activity,targets",
    run: audit_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::entra::testing::{entra, page, urls};

    const APP: &str = "00000000-0000-4000-8000-000000005d02";

    #[test]
    fn a_target_filters_in_graph_and_changes_are_cut() {
        let long = "x".repeat(300);
        let (outcome, transport) = entra(
            &[
                "entra",
                "audit",
                "list",
                "--target",
                APP,
                "--caller",
                "dana@contoso.example",
                "--status",
                "succeeded",
            ],
            vec![page(vec![json!({
                "activityDateTime": "2026-09-18T15:20:41.1234567Z", "activityDisplayName": "Update application – Certificates and secrets management",
                "category": "ApplicationManagement", "result": "success", "resultReason": "",
                "initiatedBy": {"user": {"id": "u", "userPrincipalName": "dana@contoso.example"}, "app": null},
                "targetResources": [{"id": APP, "type": "Application", "displayName": "contoso-ado-deploy",
                    "modifiedProperties": [{"displayName": "KeyDescription", "oldValue": "[]", "newValue": long}]}]})])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["when"], "2026-09-18T15:20:41Z");
        assert_eq!(got[0]["caller"], "dana@contoso.example");
        assert_eq!(
            got[0]["targets"],
            json!([{"id": APP, "type": "Application", "name": "contoso-ado-deploy"}])
        );
        assert_eq!(
            got[0]["changes"][0]["new"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            201
        );
        let url = &urls(&transport)[0];
        assert!(
            url.contains(&format!("%20and%20targetResources%2Fany%28t%3A%20t%2Fid%20eq%20%27{APP}%27%29%20and%20initiatedBy%2Fuser%2FuserPrincipalName%20eq%20%27dana%40contoso.example%27%20and%20result%20eq%20%27success%27&$top=51")),
            "{url}"
        );
    }
}
