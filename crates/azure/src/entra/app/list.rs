//! `entra app list`: app registrations with their credentials. Graph cannot
//! filter on `passwordCredentials`, so `--expires-within` and `--expired`
//! read every app, 999 a page, within the deadline, and sort by the soonest
//! end; an expired credential counts as within, as in `kv secret list`.

use agent_cli_core::{Ctx, Span, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{limited, parse_stamp, stamp, text};
use crate::entra::{Credential, Graph, Kind, V1, credentials, find, odata, param};

const SELECT: &str = "id,appId,displayName,createdDateTime,passwordCredentials,keyCredentials";

#[derive(clap::Args)]
pub struct AppListArgs {
    /// Only app registrations whose name starts with this
    text: Option<String>,
    /// Only the apps this user or service principal owns: an id, UPN, name, or @me (the az login)
    #[arg(long)]
    owner: Option<String>,
    /// Only apps with a secret or certificate ending within this long, or already expired
    #[arg(long)]
    expires_within: Option<Span>,
    /// Only apps with a secret or certificate that has expired
    #[arg(long)]
    expired: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AppRow {
    /// The object id, which app get takes.
    id: String,
    /// The client id: what a service connection, pipeline or app signs in as.
    app_id: Option<String>,
    name: Option<String>,
    created: Option<String>,
    /// When its first secret or certificate ends (or ended).
    next_expiry: Option<String>,
    credentials: Vec<Credential>,
}

fn row(app: &Value) -> Option<AppRow> {
    let credentials = credentials(app);
    Some(AppRow {
        id: text(&app["id"])?,
        app_id: text(&app["appId"]),
        name: text(&app["displayName"]),
        created: stamp(&app["createdDateTime"]),
        next_expiry: credentials.iter().find_map(|held| held.ends.clone()),
        credentials,
    })
}

fn app_list(ctx: &Ctx, args: AppListArgs) -> Result<Vec<AppRow>> {
    let graph = Graph::new(ctx);
    let scan = args.expired || args.expires_within.is_some();
    let (apps, complete) = if let Some(owner) = &args.owner {
        let (kind, held) = find(&graph, owner, &[Kind::User, Kind::ServicePrincipal], "")?;
        // VERIFY(work): that ownedObjects takes application properties in $select.
        let owned = graph.pages(
            &format!(
                "{V1}/{}/{}/ownedObjects?$select={SELECT}",
                kind.collection(),
                text(&held["id"]).unwrap_or_default()
            ),
            usize::MAX,
        )?;
        let apps: Vec<Value> = owned
            .rows
            .into_iter()
            .filter(|held| Kind::of(held) == Some(Kind::App))
            .collect();
        (apps, owned.complete)
    } else {
        let top = if scan { 999 } else { (args.limit + 1).min(999) };
        let mut url = format!("{V1}/applications?$select={SELECT}&$top={top}");
        if let Some(prefix) = &args.text {
            url.push_str(&format!(
                "&$filter={}",
                param(&format!("startswith(displayName,{})", odata(prefix.trim())))
            ));
        }
        let found = graph.pages(&url, if scan { usize::MAX } else { args.limit })?;
        if !scan && found.more {
            ctx.note(format!(
                "[first {}; --limit N, or narrow by the name's start]",
                args.limit
            ));
        }
        (found.rows, found.complete)
    };
    let scanned = apps.len();
    let prefix = args.text.as_deref().map(|held| held.trim().to_lowercase());
    let mut rows: Vec<AppRow> = apps
        .iter()
        .filter_map(row)
        .filter(|app| {
            prefix.as_deref().is_none_or(|prefix| {
                app.name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase().starts_with(prefix))
            })
        })
        .collect();
    if !scan {
        return Ok(limited(ctx, rows, args.limit));
    }
    let horizon = args
        .expires_within
        .map(|span| agent_cli_core::now() + span.0);
    rows.retain(|app| {
        app.credentials.iter().any(|held| {
            held.expired
                || horizon.is_some_and(|horizon| {
                    held.ends
                        .as_deref()
                        .and_then(parse_stamp)
                        .is_some_and(|ends| ends <= horizon)
                })
        })
    });
    rows.sort_by(|a, b| a.next_expiry.cmp(&b.next_expiry));
    let within = match &args.expires_within {
        Some(_) => "ending within the window or expired",
        None => "expired",
    };
    let cut = if complete {
        ""
    } else {
        "; the deadline cut the scan: a larger --timeout reads the rest"
    };
    ctx.note(format!(
        "[scanned {scanned} app registrations; {} hold a credential {within}{cut}]",
        rows.len()
    ));
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub APP_LIST = ["entra", "app", "list"], Read,
    "List Entra app registrations and when their credentials expire (no values)",
    keywords: ["registration", "client", "certificate", "expire", "expiring", "expired", "credential", "owned"],
    example: "entra app list --expires-within 30d --fields name,app_id,next_expiry",
    run: app_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::utc_time;
    use serde_json::{Value, json};

    use crate::entra::testing::{V1, entra, page, urls};

    fn app(name: &str, days: i64) -> Value {
        let ends = utc_time(agent_cli_core::now() + time::Duration::days(days));
        json!({"id": format!("o-{name}"), "appId": format!("a-{name}"), "displayName": name,
            "createdDateTime": "2025-01-01T00:00:00Z",
            "passwordCredentials": [{"keyId": format!("k-{name}"), "displayName": "ci", "hint": "abc",
                "secretText": null, "startDateTime": "2025-01-01T00:00:00Z", "endDateTime": ends}],
            "keyCredentials": []})
    }

    #[test]
    fn a_scan_reads_every_page_and_keeps_the_apps_whose_credentials_end_soon() {
        let mut first = page(vec![app("contoso-later", 200), app("contoso-soon", 10)]);
        first.body = first.body.replace(
            "\"value\"",
            &format!("\"@odata.nextLink\": \"{V1}/applications?$skiptoken=x\", \"value\""),
        );
        let (outcome, transport) = entra(
            &["entra", "app", "list", "--expires-within", "30d"],
            vec![first, page(vec![app("contoso-gone", -9)])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        let names: Vec<&str> = got
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["contoso-gone", "contoso-soon"]);
        assert_eq!(got[0]["credentials"][0]["expired"], true);
        assert!(
            !outcome.stdout.contains("abc"),
            "no hint: {}",
            outcome.stdout
        );
        assert!(
            outcome.stderr.contains(
                "[scanned 3 app registrations; 2 hold a credential ending within the window or expired]"
            ),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            urls(&transport),
            [
                format!(
                    "{V1}/applications?$select=id,appId,displayName,createdDateTime,passwordCredentials,keyCredentials&$top=999"
                ),
                format!("{V1}/applications?$skiptoken=x"),
            ]
        );
    }

    #[test]
    fn a_next_link_off_graph_is_refused() {
        let mut first = page(vec![]);
        first.body = format!(
            "{{\"@odata.nextLink\": \"https://graph.contoso.example/v1.0/applications\", \"value\": [{}]}}",
            app("contoso-x", 1)
        );
        let (outcome, transport) = entra(&["entra", "app", "list", "--expired"], vec![first]);
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("is not Microsoft Graph"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 1);
    }
}
