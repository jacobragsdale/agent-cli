//! `lake container list`: every container in the accounts in reach (or one
//! account), each with its top-level folders, the lake's data domains.

use agent_cli_core::{Ctx, command, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{limited, parallel};
use crate::lake::table::hidden;
use crate::lake::{Lake, Ref};

/// The most top-level folders a row names.
const FOLDERS: usize = 50;

#[derive(clap::Args)]
pub struct ContainerListArgs {
    /// Only this account's (from lake account list), or any id in it; all in reach by default
    account: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContainerRow {
    /// `account/container`, which `lake path list` and `lake table list` take.
    id: String,
    account: String,
    modified: Option<String>,
    /// Its top-level folders (the data domains) as path ids, at most 50.
    folders: Vec<String>,
}

fn container_list(ctx: &Ctx, args: ContainerListArgs) -> Result<Vec<ContainerRow>> {
    let lake = Lake::load(ctx)?;
    let accounts: Vec<String> = match &args.account {
        Some(raw) => vec![lake.resolve(raw, false)?.account],
        None => lake.accounts.iter().map(|held| held.id.clone()).collect(),
    };
    let mut found = Vec::new();
    let mut failed = Vec::new();
    for (account, listed) in
        accounts
            .iter()
            .zip(parallel(&accounts, lake.azure.parallel(), |account| {
                lake.containers(account)
            }))
    {
        match listed {
            Ok(containers) => found.extend(containers.into_iter().map(|(name, modified)| {
                (
                    Ref {
                        account: account.clone(),
                        container: name,
                        path: String::new(),
                    },
                    modified,
                )
            })),
            Err(error) => failed.push(error),
        }
    }
    if !failed.is_empty() && failed.len() == accounts.len() {
        return Err(failed.remove(0));
    }
    for error in failed {
        ctx.note(format!("[not read: {error:#}]"));
    }
    let found = limited(ctx, found, args.limit);
    let roots = parallel(&found, lake.azure.parallel(), |(container, _)| {
        lake.list(container, false)
    });
    let mut rows = Vec::new();
    for ((container, modified), root) in found.into_iter().zip(roots) {
        let mut folders: Vec<String> = match root {
            Ok(listing) => listing
                .entries
                .iter()
                .filter(|entry| entry.directory && !hidden(&entry.path))
                .map(|entry| container.at(&entry.path).id())
                .collect(),
            Err(error) => {
                ctx.note(format!(
                    "[{}'s folders not read: {error:#}]",
                    container.id()
                ));
                Vec::new()
            }
        };
        if folders.len() > FOLDERS {
            ctx.note(format!(
                "[{} has {} top-level folders, {FOLDERS} shown; all: agent-cli lake path list {}]",
                container.id(),
                folders.len(),
                container.id()
            ));
            folders.truncate(FOLDERS);
        }
        rows.push(ContainerRow {
            id: container.id(),
            account: container.account,
            modified: modified.map(utc_time),
            folders,
        });
    }
    Ok(rows)
}

command! {
    pub CONTAINER_LIST = ["lake", "container", "list"], Read,
    "List Data Lake containers with their top-level folders (the data domains)",
    keywords: ["filesystem", "filesystems", "zones", "domain", "domains", "folders", "browse"],
    example: "lake container list --fields id,folders",
    run: container_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::LAKE;
    use crate::lake::testing::{dir, file, listing};
    use crate::testing::{self, azure};

    #[test]
    fn every_container_comes_with_its_data_domains_as_path_ids() {
        let (outcome, transport) = azure(
            &[LAKE],
            &["lake", "container", "list"],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                Answer::json(&json!({"filesystems": [
                    {"name": "curated", "lastModified": "Mon, 29 Sep 2026 06:30:00 GMT"},
                    {"name": "raw", "lastModified": "Sun, 28 Sep 2026 01:00:00 GMT"}]})),
                listing(vec![dir("finance"), dir("sales"), file("README.md", 10)]),
                listing(vec![dir("sales"), dir("_temporary")]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "contosolake/curated", "account": "contosolake", "modified": "2026-09-29T06:30:00Z",
                 "folders": ["contosolake/curated/finance", "contosolake/curated/sales"]},
                {"id": "contosolake/raw", "account": "contosolake", "modified": "2026-09-28T01:00:00Z",
                 "folders": ["contosolake/raw/sales"]},
            ])
        );
        let sent = transport.sent();
        assert_eq!(
            sent[1].url,
            "https://contosolake.dfs.core.windows.net/?resource=account"
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@https://storage.azure.com/")
        );
        assert!(
            sent[1]
                .headers
                .contains(&("x-ms-version".into(), "2023-11-03".into()))
        );
        assert_eq!(
            sent[2].url,
            "https://contosolake.dfs.core.windows.net/curated?resource=filesystem&recursive=false&maxResults=5000"
        );
    }

    #[test]
    fn an_account_out_of_reach_is_exit_2_naming_account_list() {
        let (outcome, transport) = azure(
            &[LAKE],
            &[
                "lake",
                "container",
                "list",
                "abfss://raw@contosoother.dfs.core.windows.net/",
            ],
            vec![testing::inventory(vec![testing::storage("contosolake")])],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("agent-cli lake account list"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent().len(),
            1,
            "nothing sent to the other account"
        );
    }
}
