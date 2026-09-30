use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{Dd, limited};

use super::{SloRow, slo_row};

#[derive(clap::Args)]
pub struct SloListArgs {
    /// Words in the SLO name
    query: Option<String>,
    /// SLO tags: team:web (repeatable, all must hold)
    #[arg(long)]
    tag: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn slo_list(ctx: &Ctx, args: SloListArgs) -> Result<Vec<SloRow>> {
    let dd = Dd::load(ctx)?;
    let mut query = vec![("limit", args.limit.clamp(1, 1000).to_string())];
    if let Some(words) = &args.query {
        query.push(("query", words.clone()));
    }
    if !args.tag.is_empty() {
        query.push(("tags_query", args.tag.join(" AND ")));
    }
    let found = dd.get(ctx, "/api/v1/slo", &query)?;
    let rows = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(slo_row)
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub SLO_LIST = ["dd", "slo", "list"], Read,
    "List Datadog SLOs with their target and timeframe",
    keywords: ["objective", "reliability", "target", "service level", "team"],
    example: "dd slo list --tag team:web --fields id,name,target",
    run: slo_list,
}
