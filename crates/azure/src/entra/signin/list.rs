//! `entra signin list`: sign-ins with their AADSTS code and the credential
//! presented. v1.0 holds interactive user sign-ins only, so this reads
//! beta, the only version with `signInEventTypes`, whose filter picks the
//! kinds that follow the principal: a user's interactive and
//! non-interactive ones, a service principal's, or a managed identity's
//! (Graph refuses users' and apps' together). A window is always sent, as
//! Graph asks; one page, since the logs throttle at five calls in ten
//! seconds per app per tenant, and every az user is the same app.

use agent_cli_core::{Ctx, Failure, When, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{guid, stamp, text};
use crate::entra::{BETA, Graph, Kind, Status, find, odata, param};

/// The most sign-ins one page holds.
const PAGE: usize = 1000;
/// How long P1 and P2 keep sign-ins (Free: seven days).
const KEPT_DAYS: i64 = 30;

#[derive(clap::Args)]
pub struct SigninListArgs {
    /// Whose: a user (object id, UPN, @me), or a service principal or managed identity (object id, client id, name), whose sign-ins only Graph beta lists; else users'
    #[arg(long)]
    principal: Option<String>,
    /// Only sign-ins to this app: its client id or display name
    #[arg(long)]
    app: Option<String>,
    /// Only failed or succeeded sign-ins
    #[arg(long, value_enum)]
    status: Option<Status>,
    /// From when (default a day before --until; P1 and P2 keep 30 days, Free 7)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SigninRow {
    when: Option<String>,
    /// The user's UPN, or the service principal's name.
    principal: Option<String>,
    /// interactiveUser, nonInteractiveUser, servicePrincipal or managedIdentity.
    kind: Option<String>,
    /// The app that signed in (a user's client app).
    app: Option<String>,
    /// What it signed in to: Windows Azure Service Management API …
    resource: Option<String>,
    ip: Option<String>,
    /// The AADSTS number; 0 is success.
    error_code: Option<i64>,
    error: Option<String>,
    /// The credential a service principal presented: app get's key_id.
    key_id: Option<String>,
    /// Conditional access: success, failure or notApplied.
    ca_status: Option<String>,
    /// The conditional access policies that applied, when the token may read them.
    ca_policies: Vec<String>,
    correlation_id: Option<String>,
}

fn row(held: &Value) -> SigninRow {
    SigninRow {
        when: stamp(&held["createdDateTime"]),
        principal: text(&held["userPrincipalName"]).or_else(|| text(&held["servicePrincipalName"])),
        kind: held["signInEventTypes"]
            .as_array()
            .and_then(|kinds| kinds.first())
            .and_then(text),
        app: text(&held["appDisplayName"]),
        resource: text(&held["resourceDisplayName"]),
        ip: text(&held["ipAddress"]),
        error_code: held["status"]["errorCode"].as_i64(),
        error: text(&held["status"]["failureReason"]),
        key_id: text(&held["servicePrincipalCredentialKeyId"]),
        ca_status: text(&held["conditionalAccessStatus"]),
        // VERIFY(work): Graph omits the policies unless the app holds
        // Policy.Read.All, which az's does not; the status stays.
        ca_policies: held["appliedConditionalAccessPolicies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|policy| matches!(policy["result"].as_str(), Some("success" | "failure")))
            .filter_map(|policy| text(&policy["displayName"]))
            .collect(),
        correlation_id: text(&held["correlationId"]),
    }
}

fn signin_list(ctx: &Ctx, args: SigninListArgs) -> Result<Vec<SigninRow>> {
    let graph = Graph::new(ctx);
    let until = args.until.unwrap_or_else(When::now);
    let since = args
        .since
        .unwrap_or(When(until.0 - time::Duration::days(1)));
    if since.0 >= until.0 {
        return Err(Failure::usage("--since is not before --until").into());
    }
    let mut filter = vec![format!(
        "createdDateTime ge {} and createdDateTime le {}",
        since.utc(),
        until.utc()
    )];
    let users = "signInEventTypes/any(t: t eq 'interactiveUser' or t eq 'nonInteractiveUser')";
    match &args.principal {
        None => filter.push(users.to_owned()),
        Some(raw) => {
            let (kind, held) = find(&graph, raw, &[Kind::ServicePrincipal, Kind::User], "")?;
            let id = text(&held["id"]).unwrap_or_default();
            if kind == Kind::User {
                filter.push(format!("userId eq {}", odata(&id)));
                filter.push(users.to_owned());
            } else {
                let event =
                    if text(&held["servicePrincipalType"]).as_deref() == Some("ManagedIdentity") {
                        "managedIdentity"
                    } else {
                        "servicePrincipal"
                    };
                // VERIFY(work): servicePrincipalId is the service principal's object id.
                filter.push(format!("servicePrincipalId eq {}", odata(&id)));
                filter.push(format!("signInEventTypes/any(t: t eq '{event}')"));
            }
        }
    }
    if let Some(app) = &args.app {
        let app = app.trim();
        filter.push(if guid(app) {
            format!("appId eq {}", odata(&app.to_ascii_lowercase()))
        } else {
            format!("appDisplayName eq {}", odata(app))
        });
    }
    let failed = args.status == Some(Status::Failed);
    if args.status == Some(Status::Succeeded) {
        filter.push("status/errorCode eq 0".to_owned());
    }
    // Graph filters errorCode by eq only (VERIFY(work) whether ne works), so
    // failures are kept here from a full page.
    let top = if failed {
        PAGE
    } else {
        (args.limit + 1).min(PAGE)
    };
    // ponytail: beta, the only version with signInEventTypes; move to v1.0
    // once it lists service principals' and identities' sign-ins.
    let url = format!(
        "{BETA}/auditLogs/signIns?$filter={}&$top={top}",
        param(&filter.join(" and "))
    );
    let page = graph.get(&url)?;
    let read: Vec<&Value> = page["value"].as_array().into_iter().flatten().collect();
    let more = page["@odata.nextLink"].is_string() || read.len() > args.limit && !failed;
    let mut rows: Vec<SigninRow> = read
        .iter()
        .map(|held| row(held))
        .filter(|held| !failed || held.error_code.is_some_and(|code| code != 0))
        .collect();
    if since.0 < agent_cli_core::now() - time::Duration::days(KEPT_DAYS) {
        ctx.note(format!(
            "[sign-ins are kept {KEPT_DAYS} days on P1 and P2, 7 on Free; earlier ones are gone]"
        ));
    }
    if rows.len() > args.limit || more {
        rows.truncate(args.limit);
        ctx.note(format!(
            "[the newest {} of more read; narrow --since and --until, or --limit N]",
            rows.len()
        ));
    }
    Ok(rows)
}

command! {
    pub SIGNIN_LIST = ["entra", "signin", "list"], Read,
    "List Entra sign-ins of a user, service principal or managed identity",
    keywords: ["login", "logins", "signins", "aadsts", "failed", "authentication", "credential", "conditional", "access"],
    example: "entra signin list --principal contoso-ado-deploy --status failed --since 1d --fields when,error_code,error,key_id",
    run: signin_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::entra::testing::{entra, page, urls};

    const SP: &str = "00000000-0000-4000-8000-000000005d03";

    #[test]
    fn a_service_principals_failures_come_from_beta_with_the_key_it_presented() {
        let (outcome, transport) = entra(
            &[
                "entra",
                "signin",
                "list",
                "--principal",
                SP,
                "--status",
                "failed",
                "--since",
                "1d",
            ],
            vec![
                page(vec![
                    json!({"@odata.type": "#microsoft.graph.servicePrincipal", "id": SP,
                    "displayName": "contoso-ado-deploy", "servicePrincipalType": "Application"}),
                ]),
                page(vec![
                    json!({"createdDateTime": "2026-09-29T06:00:12Z", "servicePrincipalName": "contoso-ado-deploy",
                        "signInEventTypes": ["servicePrincipal"], "appDisplayName": "contoso-ado-deploy",
                        "resourceDisplayName": "Windows Azure Service Management API", "ipAddress": "203.0.113.40",
                        "status": {"errorCode": 7_000_222, "failureReason": "The provided client secret keys for app are expired."},
                        "servicePrincipalCredentialKeyId": "k-old", "conditionalAccessStatus": "notApplied",
                        "appliedConditionalAccessPolicies": [], "correlationId": "c1"}),
                    json!({"createdDateTime": "2026-09-29T05:00:00Z", "status": {"errorCode": 0}}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"when": "2026-09-29T06:00:12Z", "principal": "contoso-ado-deploy", "kind": "servicePrincipal",
                "app": "contoso-ado-deploy", "resource": "Windows Azure Service Management API", "ip": "203.0.113.40",
                "error_code": 7_000_222, "error": "The provided client secret keys for app are expired.",
                "key_id": "k-old", "ca_status": "notApplied", "correlation_id": "c1"}])
        );
        let url = &urls(&transport)[1];
        assert!(
            url.starts_with(
                "https://graph.microsoft.com/beta/auditLogs/signIns?$filter=createdDateTime%20ge%20"
            ),
            "{url}"
        );
        assert!(
            url.ends_with(&format!("%20and%20servicePrincipalId%20eq%20%27{SP}%27%20and%20signInEventTypes%2Fany%28t%3A%20t%20eq%20%27servicePrincipal%27%29&$top=1000")),
            "{url}"
        );
    }
}
