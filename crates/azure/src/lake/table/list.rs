//! `lake table list`: the tables under a folder, or under every data domain
//! (each container's top-level folders), found by listing at most `--depth`
//! levels down and stopping at each table; then one recursive listing per
//! table for its files.

use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{limited, parallel};
use crate::lake::table::{bulk, files, hidden, is_table, newest, partitioned_by};
use crate::lake::{Lake, Ref};

#[derive(clap::Args)]
pub struct TableListArgs {
    /// Where to look: an account, container or folder id, or its URL; every data domain by default
    path: Option<String>,
    /// How many folder levels below it to look
    #[arg(long, default_value_t = 3)]
    depth: usize,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TableRow {
    /// The table's folder, which `lake table get` and `lake partition list` take.
    id: String,
    /// Its Hive partition columns, outermost first.
    partitioned_by: Vec<String>,
    /// Its Parquet files.
    files: usize,
    /// When the newest of them was written.
    modified: Option<String>,
    /// What Synapse's `OPENROWSET(BULK …)` takes for its files (`sql query run`).
    bulk: String,
}

/// The top-level folders of every container in `accounts`.
fn domains(ctx: &Ctx, lake: &Lake, accounts: &[String]) -> Result<Vec<Ref>> {
    let mut containers = Vec::new();
    for (account, listed) in
        accounts
            .iter()
            .zip(parallel(accounts, lake.azure.parallel(), |account| {
                lake.containers(account)
            }))
    {
        match listed {
            Ok(found) => containers.extend(found.into_iter().map(|(name, _)| Ref {
                account: account.clone(),
                container: name,
                path: String::new(),
            })),
            Err(error) => ctx.note(format!("[not read: {error:#}]")),
        }
    }
    let mut found = Vec::new();
    for (container, root) in
        containers
            .iter()
            .zip(parallel(&containers, lake.azure.parallel(), |container| {
                lake.list(container, false)
            }))
    {
        match root {
            Ok(listing) => found.extend(
                listing
                    .entries
                    .iter()
                    .filter(|entry| entry.directory && !hidden(&entry.path))
                    .map(|entry| container.at(&entry.path)),
            ),
            Err(error) => ctx.note(format!(
                "[{}'s folders not read: {error:#}]",
                container.id()
            )),
        }
    }
    Ok(found)
}

fn table_list(ctx: &Ctx, args: TableListArgs) -> Result<Vec<TableRow>> {
    let lake = Lake::load(ctx)?;
    let mut frontier = match &args.path {
        Some(raw) => {
            let start = lake.resolve(raw, false)?;
            if start.container.is_empty() {
                domains(ctx, &lake, &[start.account])?
            } else {
                vec![start]
            }
        }
        None => {
            let accounts: Vec<String> = lake.accounts.iter().map(|held| held.id.clone()).collect();
            domains(ctx, &lake, &accounts)?
        }
    };
    let mut tables: Vec<Ref> = Vec::new();
    for level in 0..=args.depth {
        if frontier.is_empty() || tables.len() > args.limit {
            break;
        }
        let listed = parallel(&frontier, lake.azure.parallel(), |folder| {
            lake.list(folder, false)
        });
        let mut next = Vec::new();
        let mut failed = Vec::new();
        for (folder, listing) in frontier.iter().zip(listed) {
            let listing = match listing {
                Ok(listing) => listing,
                Err(error) => {
                    failed.push(error);
                    continue;
                }
            };
            if is_table(&listing.entries) {
                tables.push(folder.clone());
            } else if level < args.depth {
                next.extend(
                    listing
                        .entries
                        .iter()
                        .filter(|entry| {
                            entry.directory
                                && !hidden(entry.path.rsplit('/').next().unwrap_or_default())
                        })
                        .map(|entry| folder.at(&entry.path)),
                );
            }
        }
        if level == 0 && !failed.is_empty() && failed.len() == frontier.len() {
            return Err(failed.remove(0));
        }
        for error in failed {
            ctx.note(format!("[not read: {error:#}]"));
        }
        frontier = next;
    }
    let tables = limited(ctx, tables, args.limit);
    let read = parallel(&tables, lake.azure.parallel(), |table| files(&lake, table));
    let mut rows = Vec::new();
    for (table, found) in tables.iter().zip(read) {
        let found = found?;
        let partitioned_by = partitioned_by(table, &found);
        rows.push(TableRow {
            id: table.id(),
            bulk: bulk(table, &partitioned_by),
            partitioned_by,
            files: found.len(),
            modified: newest(&found),
        });
    }
    Ok(rows)
}

command! {
    pub TABLE_LIST = ["lake", "table", "list"], Read,
    "List the Parquet tables under a Data Lake folder or in every data domain",
    keywords: ["datasets", "dataset", "partitioned", "hive", "find", "catalog", "domain"],
    example: "lake table list contosolake/raw --fields id,partitioned_by,files,modified",
    run: table_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::LAKE;
    use crate::lake::testing::{dir, file, file_at, listing};
    use crate::testing::{self, azure};

    #[test]
    fn tables_are_found_below_a_folder_and_each_is_listed_once_for_its_files() {
        let (outcome, transport) = azure(
            &[LAKE],
            &["lake", "table", "list", "contosolake/raw/sales"],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![
                    dir("sales/orders"),
                    dir("sales/returns"),
                    dir("sales/_staging"),
                ]),
                listing(vec![
                    dir("sales/orders/date=2026-09-28"),
                    dir("sales/orders/date=2026-09-29"),
                ]),
                listing(vec![dir("sales/returns/2026")]),
                listing(vec![file("sales/returns/2026/part-0.parquet", 900)]),
                listing(vec![
                    dir("sales/orders/date=2026-09-28"),
                    file_at(
                        "sales/orders/date=2026-09-28/part-0001.parquet",
                        100,
                        "Sun, 28 Sep 2026 06:01:00 GMT",
                    ),
                    dir("sales/orders/date=2026-09-29"),
                    file("sales/orders/date=2026-09-29/part-0001.parquet", 100),
                    file("sales/orders/date=2026-09-29/_SUCCESS", 0),
                ]),
                listing(vec![file("sales/returns/2026/part-0.parquet", 900)]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "contosolake/raw/sales/orders", "partitioned_by": ["date"], "files": 2, "modified": "2026-09-29T06:02:11Z",
                 "bulk": "https://contosolake.dfs.core.windows.net/raw/sales/orders/date=*/*.parquet"},
                {"id": "contosolake/raw/sales/returns/2026", "files": 1, "modified": "2026-09-29T06:02:11Z",
                 "bulk": "https://contosolake.dfs.core.windows.net/raw/sales/returns/2026/*.parquet"},
            ])
        );
        assert!(
            transport.sent()[5].url.contains("recursive=true"),
            "{}",
            transport.sent()[5].url
        );
    }
}
