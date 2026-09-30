use agent_cli_core::{Ctx, When, command};
use anyhow::Result;

use crate::client::{Dd, Scope, limited, strings};

#[derive(clap::Args)]
pub struct MetricListArgs {
    /// Part of the metric name: kubernetes.memory, latency
    pattern: Option<String>,
    /// Reporting since when (default 1h ago)
    #[arg(long)]
    since: Option<When>,
    #[command(flatten)]
    scope: Scope,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn metric_list(ctx: &Ctx, args: MetricListArgs) -> Result<Vec<String>> {
    let dd = Dd::load(ctx)?;
    let since = args.since.map_or_else(
        || agent_cli_core::now() - time::Duration::hours(1),
        |since| since.0,
    );
    let mut query = vec![("from", since.unix_timestamp().to_string())];
    let tags = args.scope.tags(&dd, false)?;
    if !tags.is_empty() {
        query.push(("tag_filter", tags.join(" AND ")));
    }
    let found = dd.get(ctx, "/api/v1/metrics", &query)?;
    let mut names: Vec<String> = strings(&found["metrics"])
        .into_iter()
        .filter(|name| {
            args.pattern
                .as_deref()
                .is_none_or(|pattern| name.contains(pattern))
        })
        .collect();
    names.sort();
    Ok(limited(ctx, names, args.limit))
}

command! {
    pub METRIC_LIST = ["dd", "metric", "list"], Read,
    "Find Datadog metric names reporting recently, by part of the name",
    keywords: ["names", "available", "find metric", "which metrics", "exist"],
    example: "dd metric list kubernetes.memory --namespace web",
    run: metric_list,
}
