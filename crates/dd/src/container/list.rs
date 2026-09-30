use agent_cli_core::{Ctx, command, utc};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Dd, Scope, limited, pod_ref, strings, text};

#[derive(clap::Args)]
pub struct ContainerListArgs {
    #[command(flatten)]
    scope: Scope,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContainerRow {
    name: String,
    state: Option<String>,
    /// image:tag as Datadog saw it.
    image: Option<String>,
    /// The k8s pod id (cluster/namespace/pod) that `agent-cli k8s pod get` takes.
    pod: Option<String>,
    host: Option<String>,
    started: Option<String>,
}

fn container_list(ctx: &Ctx, args: ContainerListArgs) -> Result<Vec<ContainerRow>> {
    let dd = Dd::load(ctx)?;
    let tags = args.scope.tags(&dd, false)?;
    let mut query = vec![("page[size]", args.limit.clamp(1, 1000).to_string())];
    if !tags.is_empty() {
        query.insert(0, ("filter[tags]", tags.join(",")));
    }
    let found = dd.get(ctx, "/api/v2/containers", &query)?;
    let rows = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let attributes = &item["attributes"];
            let image = text(&attributes["image_name"]).map(|name| {
                match strings(&attributes["image_tags"]).first() {
                    Some(tag) => format!("{name}:{tag}"),
                    None => name,
                }
            });
            ContainerRow {
                name: text(&attributes["name"]).unwrap_or_default(),
                state: text(&attributes["state"]),
                image,
                pod: pod_ref(&attributes["tags"]),
                host: text(&attributes["host"]),
                started: text(&attributes["started_at"]).map(|at| naive_utc(&at)),
            }
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// The containers API writes UTC with no offset; say so.
fn naive_utc(raw: &str) -> String {
    let has_offset =
        raw.ends_with('Z') || raw.get(19..).is_some_and(|rest| rest.contains(['+', '-']));
    utc(&if has_offset {
        raw.to_owned()
    } else {
        format!("{raw}Z")
    })
}

command! {
    pub CONTAINER_LIST = ["dd", "container", "list"], Read,
    "List containers Datadog sees, with state, image and the k8s pod id",
    keywords: ["docker", "image", "datadog containers"],
    example: "dd container list --namespace web --fields name,state,image,pod",
    run: container_list,
}
