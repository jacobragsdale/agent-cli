//! `kv vault list`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{limited, narrow};
use crate::config::Azure;
use crate::graph::{Vault, inventory};
use crate::kv::VAULTS;

#[derive(clap::Args)]
pub struct VaultListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn vault_list(ctx: &Ctx, args: VaultListArgs) -> Result<Vec<Vault>> {
    let azure = Azure::load(ctx)?;
    let vaults = narrow(
        &inventory(ctx, &azure)?.vaults,
        &azure.vaults,
        &[],
        VAULTS,
        |v| &v.name,
    )?;
    Ok(limited(ctx, vaults, args.limit))
}

command! {
    pub VAULT_LIST = ["kv", "vault", "list"], Read,
    "List the key vaults the az login reaches (within [azure] vaults)",
    keywords: ["keyvault", "inventory", "uri", "subscription"],
    example: "kv vault list --fields name,resource_group,uri",
    run: vault_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;

    use crate::KV;
    use crate::testing::{self, azure};

    #[test]
    fn a_throttle_longer_than_the_deadline_fails_now_with_124() {
        let started = std::time::Instant::now();
        let (outcome, _) = azure(
            &[KV],
            &["kv", "vault", "list", "--timeout", "5"],
            vec![Answer::status(429, "{}").with_header("Retry-After", "600")],
        );
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "no sleep into the same failure"
        );
    }

    #[test]
    fn the_inventory_is_cached_and_no_cache_reads_it_again_and_refreshes_it() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_owned();
        let setup = |answers: Vec<Answer>| {
            let transport = agent_cli_core::testing::FakeTransport::answering(answers);
            let mut setup =
                agent_cli_core::Setup::fake(transport.clone()).with_config(testing::SERIAL);
            setup.cache_dir = Some(dir.clone());
            (setup, transport)
        };
        let (first, transport) = setup(vec![testing::inventory(vec![testing::vault(
            "kv-contoso-dev",
        )])]);
        let outcome = agent_cli_core::testing::run(&[KV], &["kv", "vault", "list"], first);
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(transport.sent().len(), 1);
        let (second, transport) = setup(vec![]);
        let outcome = agent_cli_core::testing::run(&[KV], &["kv", "vault", "list"], second);
        assert_eq!(outcome.json()[0]["name"], "kv-contoso-dev");
        assert!(transport.sent().is_empty(), "the second run read the cache");
        let (third, transport) = setup(vec![testing::inventory(vec![])]);
        let outcome =
            agent_cli_core::testing::run(&[KV], &["kv", "vault", "list", "--no-cache"], third);
        assert_eq!(outcome.stdout.trim(), "[]");
        assert_eq!(
            transport.sent().len(),
            2,
            "the inventory, then the subscriptions check"
        );
        let (fourth, transport) = setup(vec![]);
        let outcome = agent_cli_core::testing::run(&[KV], &["kv", "vault", "list"], fourth);
        assert_eq!(
            outcome.stdout.trim(),
            "[]",
            "--no-cache refreshed the cache"
        );
        assert!(transport.sent().is_empty());
        let cached = std::fs::read_to_string(dir.join("cache.json")).unwrap();
        assert!(!cached.contains("token"), "{cached}");
    }
}
