use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::kubectl::{At, items, limited, name_has};

use super::{StatefulSetRow, row};

#[derive(clap::Args)]
pub struct StatefulsetListArgs {
    /// Part of the statefulset name
    name: Option<String>,
    #[command(flatten)]
    at: At,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn statefulset_list(ctx: &Ctx, args: StatefulsetListArgs) -> Result<Vec<StatefulSetRow>> {
    let target = args.at.listing(ctx)?;
    let listed = target.json(ctx, &["get", "statefulsets", "-o", "json"])?;
    let rows = items(&listed)
        .filter(|item| name_has(item, args.name.as_deref()))
        .filter_map(|item| row(&target, item))
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub STATEFULSET_LIST = ["k8s", "statefulset", "list"], Read,
    "List statefulsets: ready pods, images, volume claim templates",
    keywords: ["sts", "stateful", "workloads"],
    example: "k8s statefulset list --cluster prod --namespace web --fields id,ready,claim_templates",
    run: statefulset_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::run;

    #[test]
    fn statefulset_list_shows_ready_pods_and_claim_templates() {
        let outcome = run(&[
            "k8s",
            "statefulset",
            "list",
            "--fields",
            "id,name,ready,images,claim_templates",
        ]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "qa/dev/redis", "name": "redis", "ready": "1/1",
                "images": ["redis:7"], "claim_templates": ["data"]}])
        );
        let outcome = run(&["k8s", "statefulset", "list", "nope"]);
        assert_eq!(outcome.json(), json!([]));
    }
}
