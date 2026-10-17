//! What the contoso world answers of entra: who an object id is, who is in a
//! group and how, which app credentials lapse, what an identity was granted,
//! who holds a directory role, and the logs that say who did it. Compiled
//! only with the `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use serde_json::json;
use world::{agent_cli, follow, ok};

const PLATFORM: &str = "00000000-0000-4000-8000-00000000f001";
const ONCALL: &str = "00000000-0000-4000-8000-00000000f003";
const SAM: &str = "00000000-0000-4000-8000-00000000e102";
const DANA: &str = "00000000-0000-4000-8000-00000000e101";

/// arm's role list names its group; entra walks it to sam, nested, and the
/// audit log says dana put sam in the nested group.
#[test]
fn the_contributor_group_holds_sam_through_oncall_and_dana_put_him_there() {
    let roles = ok(&[
        "arm",
        "role",
        "list",
        "--fields",
        "principal_id,principal_name,role",
    ]);
    assert_eq!(
        roles[0],
        json!({"principal_id": PLATFORM, "principal_name": "sg-contoso-platform", "role": "Contributor"})
    );
    let members = ok(&[
        "entra",
        "member",
        "list",
        PLATFORM,
        "--fields",
        "id,type,via",
    ]);
    assert_eq!(
        members,
        json!([
            {"id": DANA, "type": "user"},
            {"id": "00000000-0000-4000-8000-0000000000e1", "type": "user"},
            {"id": ONCALL, "type": "group"},
            {"id": SAM, "type": "user", "via": ["sg-contoso-oncall"]},
        ])
    );
    let audit = ok(&["entra", "audit", "list", "--target", ONCALL]);
    assert_eq!(audit[0]["when"], "2026-09-25T10:02:13Z");
    assert_eq!(audit[0]["caller"], "dana@contoso.example");
    assert_eq!(audit[0]["activity"], "Add member to group");
    assert_eq!(audit[0]["targets"][0]["id"], SAM);
    let groups = ok(&[
        "entra",
        "group",
        "list",
        "--member",
        "sam@contoso.example",
        "--fields",
        "name,dynamic,direct",
    ]);
    assert_eq!(
        groups,
        json!([
            {"name": "sg-contoso-oncall", "dynamic": false, "direct": true},
            {"name": "sg-contoso-platform", "dynamic": false, "direct": false},
            {"name": "sg-contoso-sre", "dynamic": true, "direct": true},
        ])
    );
    let group = ok(&[
        "entra",
        "group",
        "get",
        "sg-contoso-platform",
        "--fields",
        "owners,members,transitive_members",
    ]);
    assert_eq!(
        (
            group["members"].clone(),
            group["transitive_members"].clone(),
            group["owners"][0]["id"].clone()
        ),
        (json!(3), json!(4), json!(DANA))
    );
}

/// The ledger's ACL names a GUID; principal list says it is the finance
/// close's service principal.
#[test]
fn the_ledgers_acl_names_the_finance_close_service_principal() {
    let ledger = ok(&[
        "lake",
        "path",
        "get",
        "contosolake/curated/finance/ledger",
        "--fields",
        "acl",
    ]);
    assert!(
        ledger
            .to_string()
            .contains("00000000-0000-4000-8000-00000000a2f2"),
        "{ledger}"
    );
    let named = ok(&[
        "entra",
        "principal",
        "list",
        "00000000-0000-4000-8000-00000000a2f2",
    ]);
    assert_eq!(named[0]["name"], "contoso-finance-close");
    assert_eq!(named[0]["type"], "serviceprincipal");
}

#[test]
fn two_app_registrations_need_attention_and_each_note_leads_somewhere() {
    let expiring = agent_cli(&[
        "entra",
        "app",
        "list",
        "--expires-within",
        "30d",
        "--fields",
        "name,next_expiry",
    ]);
    assert_eq!(expiring.code, 0, "{}", expiring.stderr);
    assert_eq!(
        expiring.json(),
        json!([
            {"name": "contoso-ado-deploy", "next_expiry": "2026-09-20T00:00:00Z"},
            {"name": "contoso-reports", "next_expiry": "2026-10-12T00:00:00Z"},
        ])
    );
    assert!(
        expiring
            .stderr
            .contains("[scanned 4 app registrations; 2 hold a credential"),
        "{}",
        expiring.stderr
    );
    // contoso-reports' only owner is disabled, so its note goes to the log.
    let walked = follow(&["entra", "app", "get", "contoso-reports"]);
    assert_eq!(walked.len(), 2, "app get, then the audit log");
    assert_eq!(walked[1].json(), json!([]));
    let dev = ok(&[
        "entra",
        "app",
        "get",
        "contoso-ado-dev",
        "--fields",
        "federated_credentials",
    ]);
    assert_eq!(
        dev["federated_credentials"][0]["subject"],
        "sc://contoso/Fabrikam/contoso-dev"
    );
}

#[test]
fn identities_users_roles_and_the_login_itself_answer() {
    let by_name = ok(&["entra", "serviceprincipal", "get", "id-contoso-airflow"]);
    let by_arm = ok(&[
        "entra",
        "serviceprincipal",
        "get",
        "/subscriptions/00000000-0000-4000-8000-000000005b01/resourceGroups/rg-contoso-prod/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-contoso-airflow",
    ]);
    assert_eq!(by_name, by_arm);
    assert_eq!(by_name["type"], "ManagedIdentity");
    assert!(by_name["groups"].is_null(), "in no group: {by_name}");
    let sam = ok(&[
        "entra",
        "user",
        "get",
        "sam@contoso.com",
        "--fields",
        "upn,manager,groups",
    ]);
    assert_eq!(sam["upn"], "sam@contoso.example");
    assert_eq!(sam["manager"]["id"], DANA);
    let me = ok(&["entra", "user", "get", "@me", "--fields", "upn,roles"]);
    assert_eq!(
        me,
        json!({"upn": "ops@contoso.example", "roles": ["Reports Reader"]})
    );
    let admins = ok(&[
        "entra",
        "role",
        "list",
        "--role",
        "Global Administrator",
        "--fields",
        "principal",
    ]);
    assert_eq!(
        admins,
        json!([{"principal": {"id": DANA, "type": "user", "name": "Dana Reyes"}}])
    );
    let doctor = ok(&["doctor", "entra", "--fields", "check,ok"]);
    assert!(
        doctor
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check["ok"] == true),
        "{doctor}"
    );
}
