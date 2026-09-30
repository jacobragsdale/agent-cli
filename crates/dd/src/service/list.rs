use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{Dd, limited, strings};

#[derive(clap::Args)]
pub struct ServiceListArgs {
    /// env tag (default [datadog] env, else every env)
    #[arg(long)]
    env: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn service_list(ctx: &Ctx, args: ServiceListArgs) -> Result<Vec<String>> {
    let dd = Dd::load(ctx)?;
    let env = args
        .env
        .or_else(|| dd.env.clone())
        .unwrap_or_else(|| "*".to_owned());
    let found = dd.get(ctx, "/api/v2/apm/services", &[("filter[env]", env)])?;
    let mut names = strings(&found["data"]["attributes"]["services"]);
    names.sort();
    Ok(limited(ctx, names, args.limit))
}

command! {
    pub SERVICE_LIST = ["dd", "service", "list"], Read,
    "List the APM services that send traces to Datadog",
    keywords: ["apm services", "instrumented", "traced", "which services"],
    example: "dd service list --env prod",
    run: service_list,
}
