//! `lake account list`: the storage accounts with a hierarchical namespace
//! the login reaches, from the inventory.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::limited;
use crate::graph::StorageAccount;
use crate::lake::Lake;

#[derive(clap::Args)]
pub struct AccountListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn account_list(ctx: &Ctx, args: AccountListArgs) -> Result<Vec<StorageAccount>> {
    Ok(limited(ctx, Lake::load(ctx)?.accounts, args.limit))
}

command! {
    pub ACCOUNT_LIST = ["lake", "account", "list"], Read,
    "List the Data Lake storage accounts the az login reaches",
    keywords: ["inventory", "hns", "hierarchical", "namespace", "firewall", "private", "gen2"],
    example: "lake account list --fields id,resource_group,public_network_access,endpoints",
    run: account_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::LAKE;
    use crate::testing::{self, azure, azure_with};

    #[test]
    fn the_accounts_in_reach_come_from_the_inventory_within_the_allowlist() {
        let accounts = || {
            testing::inventory(vec![
                testing::storage("contosolake"),
                testing::storage("contosolakedev"),
            ])
        };
        let (outcome, _) = azure(&[LAKE], &["lake", "account", "list"], vec![accounts()]);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got[0]["id"], "contosolake");
        assert_eq!(got[0]["default_action"], "Deny");
        assert_eq!(
            got[0]["arm"],
            json!(format!(
                "{}/providers/Microsoft.Storage/storageAccounts/contosolake",
                testing::RG
            ))
        );
        assert_eq!(got.as_array().unwrap().len(), 2);

        let (outcome, _) = azure_with(
            &[LAKE],
            &["lake", "account", "list"],
            vec![accounts()],
            "[azure]\nstorage_accounts = \"contosolakedev\"\n",
        );
        assert_eq!(outcome.json()[0]["id"], "contosolakedev");
        let (outcome, _) = azure_with(
            &[LAKE],
            &["lake", "account", "list"],
            vec![accounts()],
            "[azure]\nstorage_accounts = \"contosogone\"\n",
        );
        assert_eq!(
            outcome.code, 3,
            "an allowlist that reaches nothing: {outcome:?}"
        );
    }
}
