//! `lake access get`: whether a principal can read (or write) a path, and if
//! not, the first place it breaks. Azure asks RBAC first: a data role at the
//! container, the account or above grants, and the ACLs are not consulted.
//! Else every folder from the container's root down needs execute, and the
//! path itself read (write). An ACL entry counts for the principal, any of
//! its groups (from Microsoft Graph), or other, under the mask; the owner
//! gets the owning user's bits.

use agent_cli_core::{Ctx, Failure, Request, command, host_under};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::arm::resource::target;
use crate::arm::role::assignments;
use crate::client::{GRAPH, STORAGE, az_token, bearer, parallel, text};
use crate::lake::{Lake, Ref, encoded};

/// The built-in data roles by definition id, and whether each writes.
// ponytail: built-in roles only. A custom role's dataActions are not read
// (VERIFY(work) where authorizationresources keeps them), so it counts as no
// data role and the ACLs decide.
const DATA_ROLES: [(&str, bool); 3] = [
    ("b7e6dc6d-f1e8-4753-8033-0f276bb0955b", true), // Storage Blob Data Owner
    ("ba92f5b4-2d11-453d-a403-e96b0029c9fe", true), // Storage Blob Data Contributor
    ("2a2b9908-6ea1-4ae2-8e65-a410df84e7d1", false), // Storage Blob Data Reader
];

#[derive(clap::Args)]
pub struct AccessGetArgs {
    /// The file or folder: an id from lake path list, or its abfss:// or https:// URL
    path: String,
    /// Who: a user, group, service principal or managed identity's object id (arm identity get prints it as principal_id), or @me for the az login
    #[arg(long)]
    principal: String,
    /// Whether it can write there, rather than read
    #[arg(long)]
    write: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessDetail {
    /// The path asked about.
    id: String,
    /// The principal's object id.
    principal: String,
    allowed: bool,
    /// What lets it in: a data role and where it is granted, or the ACLs.
    via: Option<String>,
    /// The first folder or file that refuses it.
    broken_at: Option<Broken>,
    /// From the container's root down, what each ACL grants the principal.
    steps: Vec<Step>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Broken {
    /// Its id, which `lake path get` takes.
    path: String,
    /// `--x` on a folder above, `r--` on a file, `r-x` on a folder read.
    needs: String,
    has: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Step {
    path: String,
    /// Its ACL as the service wrote it, object ids and all.
    acl: Option<String>,
    /// What it grants the principal.
    grants: String,
}

/// `r`, `w`, `x` as bits 4, 2, 1.
fn bits(raw: &str) -> u8 {
    raw.chars().fold(0, |held, flag| {
        held | match flag {
            'r' => 4,
            'w' => 2,
            'x' | 't' => 1,
            _ => 0,
        }
    })
}

fn shown(bits: u8) -> String {
    [(4, 'r'), (2, 'w'), (1, 'x')]
        .iter()
        .map(|(bit, flag)| if bits & bit != 0 { *flag } else { '-' })
        .collect()
}

/// What one ACL grants `principal` (a member of `groups`): the bits it
/// gets, and whether one entry alone holds all of `needs`, as POSIX asks.
fn evaluate(
    acl: &str,
    owner: &str,
    group: &str,
    principal: &str,
    groups: &[String],
    needs: u8,
) -> (u8, bool) {
    let mut entries: Vec<(&str, &str, u8)> = Vec::new();
    for entry in acl.split(',').map(str::trim) {
        let parts: Vec<&str> = entry.split(':').collect();
        // `default:` entries are what new children inherit, not this one's.
        if let [kind, qualifier, perms] = parts[..] {
            entries.push((kind, qualifier, bits(perms)));
        }
    }
    let find = |kind: &str, qualifier: &str| {
        entries
            .iter()
            .find(|(held, who, _)| *held == kind && who.eq_ignore_ascii_case(qualifier))
            .map(|(_, _, bits)| *bits)
    };
    let member = |id: &str| {
        id.eq_ignore_ascii_case(principal)
            || groups.iter().any(|held| held.eq_ignore_ascii_case(id))
    };
    if owner.eq_ignore_ascii_case(principal) {
        let held = find("user", "").unwrap_or(0);
        return (held, held & needs == needs);
    }
    let mask = find("mask", "").unwrap_or(7);
    if let Some(held) = find("user", principal) {
        let held = held & mask;
        return (held, held & needs == needs);
    }
    let matched: Vec<u8> = entries
        .iter()
        .filter(|(kind, who, _)| {
            *kind == "group"
                && if who.is_empty() {
                    member(group)
                } else {
                    member(who)
                }
        })
        .map(|(_, _, held)| held & mask)
        .collect();
    if !matched.is_empty() {
        let held = matched.iter().fold(0, |all, one| all | one);
        return (held, matched.iter().any(|one| one & needs == needs));
    }
    let held = find("other", "").unwrap_or(0);
    (held, held & needs == needs)
}

/// The object id in the az login's token.
fn me(ctx: &Ctx) -> Result<String> {
    let token = az_token(ctx, STORAGE, false)?;
    let payload = token
        .expose()
        .split('.')
        .nth(1)
        .and_then(base64url)
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    payload
        .and_then(|claims| text(&claims["oid"]))
        .ok_or_else(|| {
            Failure::setup("the az login's token names no object id")
                .hint("pass --principal OBJECT_ID")
                .into()
        })
}

fn base64url(raw: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut held, mut count) = (0_u32, 0);
    for byte in raw.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        held = held << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((held >> count) as u8);
            held &= (1 << count) - 1;
        }
    }
    Some(out)
}

/// Every group `principal` is in, transitively, from Microsoft Graph.
// VERIFY(work): that the az login's Graph token may call getMemberObjects.
fn groups(ctx: &Ctx, principal: &str) -> Result<Vec<String>> {
    let url = format!(
        "https://graph.microsoft.com/v1.0/directoryObjects/{}/getMemberObjects",
        encoded(principal, false)
    );
    if !host_under(&url, "graph.microsoft.com") {
        bail!("{url} is not Microsoft Graph; the Graph token is not sent there");
    }
    let mint = bearer(ctx, GRAPH);
    let answer = ctx
        .read(Request::query(&url, json!({"securityEnabledOnly": false})).auth(&mint))?
        .json()?;
    Ok(answer["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(text)
        .collect())
}

fn guid(raw: &str) -> bool {
    raw.len() == 36
        && raw.char_indices().all(|(at, held)| {
            if [8, 13, 18, 23].contains(&at) {
                held == '-'
            } else {
                held.is_ascii_hexdigit()
            }
        })
}

fn access_get(ctx: &Ctx, args: AccessGetArgs) -> Result<AccessDetail> {
    let lake = Lake::load(ctx)?;
    let at = lake.resolve(&args.path, true)?;
    let account = lake.account(&at.account)?.clone();
    let principal = match args.principal.trim() {
        "@me" => me(ctx)?,
        other if guid(other) => other.to_ascii_lowercase(),
        other => {
            return Err(Failure::usage(format!("--principal takes an object id, not {other:?}"))
                .hint("`agent-cli arm identity get NAME` prints a managed identity's principal_id")
                .into());
        }
    };
    let groups = groups(ctx, &principal).unwrap_or_else(|error| {
        ctx.note(format!("[the principal's groups were not read, so ACL entries for its groups do not count: {error:#}]"));
        Vec::new()
    });
    let denied = |detail: AccessDetail| {
        ctx.note(format!(
            "[next: agent-cli arm role list --principal {principal} --scope {}]",
            account.arm
        ));
        detail
    };

    let scope = format!(
        "{}/blobServices/default/containers/{}",
        account.arm, at.container
    );
    let container = target(&scope).with_context(|| format!("{scope} is not an ARM id"))?;
    let mut ids = vec![principal.as_str()];
    ids.extend(groups.iter().map(String::as_str));
    let roles = assignments(ctx, &lake.azure, &ids, Some(&container), usize::MAX)?;
    let granting = roles.iter().find(|held| {
        DATA_ROLES
            .iter()
            .any(|(id, writes)| held.role_id.eq_ignore_ascii_case(id) && (*writes || !args.write))
    });
    if let Some(role) = granting {
        return Ok(AccessDetail {
            id: at.id(),
            principal,
            allowed: true,
            via: Some(format!(
                "{} at {}",
                role.role.as_deref().unwrap_or(&role.role_id),
                role.scope
            )),
            broken_at: None,
            steps: Vec::new(),
        });
    }

    let mut chain = vec![at.at("")];
    let mut path = String::new();
    for segment in at.path.split('/').filter(|segment| !segment.is_empty()) {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);
        chain.push(at.at(&path));
    }
    let directory = at.path.is_empty()
        || lake.head(&at.head_url())?.header("x-ms-resource-type") == Some("directory");
    let acls = parallel(&chain, lake.azure.parallel(), |step: &Ref| {
        lake.head(&format!("{}?action=getAccessControl", step.head_url()))
    });
    let mut steps = Vec::new();
    for (index, (step, answer)) in chain.iter().zip(acls).enumerate() {
        let answer = answer?;
        let leaf = index == chain.len() - 1;
        let needs = match (leaf, args.write, directory) {
            (false, _, _) => "--x",
            (true, false, true) => "r-x",
            (true, false, false) => "r--",
            (true, true, true) => "-wx",
            (true, true, false) => "-w-",
        };
        let acl = answer.header("x-ms-acl").map(str::to_owned);
        let (has, ok) = evaluate(
            acl.as_deref().unwrap_or_default(),
            answer.header("x-ms-owner").unwrap_or_default(),
            answer.header("x-ms-group").unwrap_or_default(),
            &principal,
            &groups,
            bits(needs),
        );
        steps.push(Step {
            path: step.id(),
            acl,
            grants: shown(has),
        });
        if !ok {
            return Ok(denied(AccessDetail {
                id: at.id(),
                principal: principal.clone(),
                allowed: false,
                via: None,
                broken_at: Some(Broken {
                    path: step.id(),
                    needs: needs.to_owned(),
                    has: shown(has),
                }),
                steps,
            }));
        }
    }
    Ok(AccessDetail {
        id: at.id(),
        principal,
        allowed: true,
        via: Some("ACLs: execute on each folder above it, and what it needs on itself".to_owned()),
        broken_at: None,
        steps,
    })
}

command! {
    pub ACCESS_GET = ["lake", "access", "get"], Read,
    "Say whether a principal can read or write a Data Lake path, and where it breaks",
    keywords: ["403", "forbidden", "denied", "permission", "acl", "rbac", "authorization", "identity", "AuthorizationPermissionMismatch"],
    example: "lake access get contosolake/curated/finance/ledger --principal 00000000-0000-4000-8000-00000000a1f2",
    run: access_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::LAKE;
    use crate::testing::{self, azure};

    const ME: &str = "00000000-0000-4000-8000-00000000a1f2";
    const GROUP: &str = "00000000-0000-4000-8000-00000000f001";

    fn acl(owner: &str, acl: &str) -> Answer {
        Answer::ok("")
            .with_header("x-ms-owner", owner)
            .with_header("x-ms-group", "$superuser")
            .with_header("x-ms-acl", acl)
    }

    #[test]
    fn posix_rules_decide_an_acl_owner_named_user_groups_under_the_mask_then_other() {
        let groups = vec![GROUP.to_owned()];
        let check = |acl: &str, owner: &str, needs: &str| {
            let (has, ok) = evaluate(acl, owner, "$superuser", ME, &groups, bits(needs));
            (shown(has), ok)
        };
        let base = "user::rwx,group::r-x,other::---";
        assert_eq!(check(base, ME, "rwx"), ("rwx".into(), true));
        assert_eq!(check(base, "$superuser", "--x"), ("---".into(), false));
        assert_eq!(
            check(
                &format!("{base},user:{ME}:rwx,mask::r-x"),
                "$superuser",
                "-w-"
            ),
            ("r-x".into(), false)
        );
        assert_eq!(
            check(
                &format!("{base},group:{GROUP}:--x,mask::rwx"),
                "$superuser",
                "--x"
            ),
            ("--x".into(), true)
        );
        assert_eq!(
            check(
                "user::rwx,group::---,other::r-x,default:user:x:rwx",
                "$superuser",
                "r-x"
            ),
            ("r-x".into(), true)
        );
    }

    #[test]
    fn a_token_names_its_object_id() {
        let claims = agent_cli_core::base64(format!(r#"{{"oid":"{ME}"}}"#).as_bytes());
        let claims = claims
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_");
        assert_eq!(
            base64url(&claims).unwrap(),
            format!(r#"{{"oid":"{ME}"}}"#).into_bytes()
        );
    }

    #[test]
    fn no_data_role_and_a_folder_without_execute_breaks_there_and_names_role_list() {
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "access",
                "get",
                "contosolake/curated/finance/ledger",
                "--principal",
                ME,
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::json(&json!({"value": [GROUP]})),
                testing::inventory(vec![
                    json!({"id": "a1", "principalId": GROUP, "role": "Reader",
                    "roleId": "acdd72a7-3385-48ef-bd42-f606fba81ae7", "scope": "/subscriptions/00000000-0000-0000-0000-000000000001"}),
                ]),
                Answer::ok("").with_header("x-ms-resource-type", "directory"),
                acl("$superuser", "user::rwx,group::r-x,other::--x"),
                acl(
                    "$superuser",
                    &format!("user::rwx,group::r-x,group:{GROUP}:r-x,mask::r-x,other::---"),
                ),
                acl("$superuser", "user::rwx,group::r-x,mask::r-x,other::---"),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["allowed"], false);
        assert_eq!(
            got["broken_at"],
            json!({"path": "contosolake/curated/finance/ledger", "needs": "r-x", "has": "---"})
        );
        assert_eq!(got["steps"][1]["grants"], "r-x");
        assert!(
            outcome.stderr.contains(&format!("[next: agent-cli arm role list --principal {ME} --scope {}/providers/Microsoft.Storage/storageAccounts/contosolake]", testing::RG)),
            "{}",
            outcome.stderr
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            format!("https://graph.microsoft.com/v1.0/directoryObjects/{ME}/getMemberObjects")
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@https://graph.microsoft.com/")
        );
        let query = sent[2].body.as_ref().unwrap()["query"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            query.contains(&format!("where principalId in~ ('{ME}', '{GROUP}')")),
            "{query}"
        );
        assert!(
            query.contains("/blobServices/default/containers/curated'"),
            "{query}"
        );
        assert_eq!(
            sent[4].url,
            "https://contosolake.dfs.core.windows.net/curated/?action=getAccessControl"
        );
        assert_eq!(
            sent[6].url,
            "https://contosolake.dfs.core.windows.net/curated/finance/ledger?action=getAccessControl"
        );
    }

    #[test]
    fn a_data_role_grants_without_reading_an_acl() {
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "access",
                "get",
                "contosolake/raw/sales/orders",
                "--principal",
                ME,
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::status(
                    403,
                    r#"{"error":{"code":"Authorization_RequestDenied","message":"Insufficient privileges"}}"#,
                ),
                testing::inventory(vec![
                    json!({"id": "a2", "principalId": ME, "role": "Storage Blob Data Reader",
                    "roleId": "2a2b9908-6ea1-4ae2-8e65-a410df84e7d1", "scope": format!("{}/providers/Microsoft.Storage/storageAccounts/contosolake", testing::RG)}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["allowed"], true);
        assert!(
            got["via"]
                .as_str()
                .unwrap()
                .starts_with("Storage Blob Data Reader at /subscriptions/"),
            "{got}"
        );
        assert!(
            outcome.stderr.contains("groups were not read"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 3);
        let (outcome, _) = azure(
            &[LAKE],
            &[
                "lake",
                "access",
                "get",
                "contosolake/raw/sales/orders",
                "--principal",
                "etl-identity",
            ],
            vec![testing::inventory(vec![testing::storage("contosolake")])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
