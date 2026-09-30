//! `acr registry list`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::acr::REGISTRIES;
use crate::client::{limited, narrow};
use crate::config::Azure;
use crate::graph::{Registry, inventory};

#[derive(clap::Args)]
pub struct RegistryListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn registry_list(ctx: &Ctx, args: RegistryListArgs) -> Result<Vec<Registry>> {
    let azure = Azure::load(ctx)?;
    let found = narrow(
        &inventory(ctx, &azure)?.registries,
        &azure.registries,
        &[],
        REGISTRIES,
        |r| &r.name,
    )?;
    Ok(limited(ctx, found, args.limit))
}

command! {
    pub REGISTRY_LIST = ["acr", "registry", "list"], Read,
    "List the container registries the az login reaches (within [azure] registries)",
    keywords: ["acr", "login", "server", "inventory", "docker"],
    example: "acr registry list --fields name,login_server",
    run: registry_list,
}
