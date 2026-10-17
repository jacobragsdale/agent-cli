//! `lake table diff`: schema drift. Two tables, partitions or files (today's
//! against yesterday's, raw against curated, one account against another)
//! by their columns and Parquet types, from the newest file's footer of a
//! folder or a file's own: one listing and one ranged read a side.

use agent_cli_core::{Ctx, Diff, DiffArgs, Failure, Rules, Side, command, diff};
use anyhow::Result;
use serde_json::json;

use crate::lake::Lake;
use crate::lake::parquet::footers;
use crate::lake::table::{files, partitioned_by};

#[derive(clap::Args)]
pub struct TableDiffArgs {
    /// One side: a table's or partition's folder, or a Parquet file (an id or its URL)
    left: String,
    /// The other side, as left
    right: String,
    #[command(flatten)]
    diff: DiffArgs,
}

/// Which file a folder's schema came from is said, not compared.
const RULES: Rules = Rules {
    keys: &[("columns", "name")],
    noise: &["file"],
    ..Rules::PLAIN
};

fn table_diff(ctx: &Ctx, args: TableDiffArgs) -> Result<Diff> {
    let lake = Lake::load(ctx)?;
    let side = |raw: &str| -> Result<Side> {
        let at = lake.resolve(raw, true)?;
        let (file, size, by) = if at.path.ends_with(".parquet") {
            let properties = lake.head(&at.head_url())?;
            let size = properties
                .header("content-length")
                .and_then(|held| held.trim().parse().ok())
                .unwrap_or_default();
            (at.clone(), size, Vec::new())
        } else {
            let found = files(&lake, &at)?;
            let Some(newest) = found.first() else {
                return Err(
                    Failure::not_found(format!("no Parquet files under {}", at.id()))
                        .hint(format!("agent-cli lake table list {}", at.id()))
                        .into(),
                );
            };
            (
                at.at(&newest.path),
                newest.size,
                partitioned_by(&at, &found),
            )
        };
        let footer = footers(&lake, &[(file.clone(), size)]).remove(0)?;
        let row = json!({"partitioned_by": by, "columns": footer.schema, "file": file.id()});
        Ok(Side::new(at.id(), row))
    };
    let (left, right) = (side(&args.left)?, side(&args.right)?);
    diff(ctx, &args.diff, &RULES, left, right)
}

command! {
    pub TABLE_DIFF = ["lake", "table", "diff"], Read,
    "Compare two Parquet tables, partitions or files: their schema (drift) and types",
    keywords: ["drift", "new", "parquet", "partitions"],
    example: "lake table diff contosolake/raw/sales/orders/date=2026-09-28 contosolake/raw/sales/orders/date=2026-09-29",
    run: table_diff,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::LAKE;
    use crate::lake::testing::{file, listing, tail, world};
    use crate::testing::{self, azure};

    #[test]
    fn two_tables_differ_by_the_columns_their_newest_footers_hold() {
        let orders = world("contosolake/raw/sales/orders/date=2026-09-29/part-0001.parquet");
        let ledger = world("contosolake/curated/finance/ledger/part-0000.parquet");
        let (outcome, _) = azure(
            &[LAKE],
            &[
                "lake",
                "table",
                "diff",
                "contosolake/raw/sales/orders",
                "contosolake/curated/finance/ledger",
            ],
            vec![
                testing::inventory(vec![testing::storage("contosolake")]),
                listing(vec![file(
                    "sales/orders/date=2026-09-29/part-0001.parquet",
                    orders.len(),
                )]),
                tail(&orders),
                listing(vec![file("finance/ledger/part-0000.parquet", ledger.len())]),
                tail(&ledger),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["same"], false);
        assert_eq!(
            got["changes"][0],
            json!({"path": "partitioned_by", "left": ["date"]})
        );
        assert_eq!(
            got["changes"][1],
            json!({"path": "columns.order_id", "left": {"type": "INT64"}})
        );
    }
}
