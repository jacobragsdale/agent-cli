//! `aisearch indexer run`: `POST /indexers/{n}/run` (202: it starts in the
//! background, and cannot be stopped once started), after `POST /reset`
//! with `--reset`.

use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::aisearch::indexer::named;
use crate::aisearch::{Role, holder, segment};
use crate::client::refused_with;
use crate::config::Azure;

#[derive(clap::Args)]
pub struct IndexerRunArgs {
    /// The indexer: SERVICE/INDEXER from indexer list, or a bare name
    indexer: String,
    /// The service that holds it; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// Reset it first: forget what it has read, so it reads everything again and re-runs every skill (which costs)
    #[arg(long)]
    reset: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Requested {
    id: String,
    reset: bool,
    /// When the run was asked for; indexer wait --since takes it.
    requested: String,
}

fn indexer_run(ctx: &Ctx, args: IndexerRunArgs) -> Result<Requested> {
    let azure = Azure::load(ctx)?;
    let (service, name) = named(&args.indexer, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexers", &name, service.as_deref())?;
    let id = format!("{}/{name}", search.name());
    // A second's margin: the service stamps the run, not this clock.
    let requested = agent_cli_core::utc_time(agent_cli_core::now() - time::Duration::seconds(1));
    let path = format!("/indexers/{}", segment(&name));
    if args.reset {
        search.change(
            Effect::Write,
            Method::Post,
            &format!("{path}/reset"),
            None,
            &[],
            Role::Manage,
        )?;
    }
    search
        .change(
            Effect::Write,
            Method::Post,
            &format!("{path}/run"),
            None,
            &[],
            Role::Manage,
        )
        .map_err(|error| match refused_with(&error) {
            Some(409) => Failure::conflict(format!("{id} did not start: {error:#}"))
                .hint(format!(
                    "a run is already going (one at a time): agent-cli aisearch indexer wait {id}"
                ))
                .into(),
            // Unconfirmed: Microsoft's own sample expects a 429 for a run
            // already going, which is also how the Free tier's 180-second
            // spacing and a runtime quota answer.
            Some(429) => match error.downcast::<Failure>() {
                Ok(failure) => failure
                    .hint(format!(
                        "a run may be going, or the tier allows no run now (every 180 s on Free): agent-cli aisearch indexer wait {id}"
                    ))
                    .into(),
                Err(error) => error,
            },
            _ => error,
        })?;
    ctx.note(format!(
        "[next: agent-cli aisearch indexer wait {id} --since {requested}]"
    ));
    Ok(Requested {
        id,
        reset: args.reset,
        requested,
    })
}

command! {
    pub INDEXER_RUN = ["aisearch", "indexer", "run"], Write,
    "Run an AI Search indexer now (--reset to re-read everything); it runs on its own",
    keywords: ["start", "trigger", "kick", "reindex", "refresh", "rerun", "sync"],
    example: "aisearch indexer run srch-contoso-prod/orders-sql",
    run: indexer_run,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run, next_command};

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    fn one() -> Answer {
        testing::inventory(vec![testing::search_service(
            "srch-contoso-prod",
            "aadOrApiKey",
        )])
    }

    #[test]
    fn run_posts_reset_first_when_asked_and_names_the_wait() {
        let plans = assert_dry_run(
            &[AISEARCH],
            &["aisearch", "indexer", "run", "orders-sql", "--reset"],
            vec![one()],
        );
        assert_eq!(plans.len(), 1, "the dry run stops at the first change");
        assert_eq!(
            plans[0]["url"],
            "https://srch-contoso-prod.search.windows.net/indexers/orders-sql/reset?api-version=2026-04-01"
        );

        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "indexer", "run", "srch-contoso-prod/orders-sql"],
            vec![one(), Answer::status(202, "")],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["reset"], false);
        assert_eq!(transport.sent()[1].method.wire(), "POST");
        let next = next_command(&outcome.stderr).unwrap();
        assert_eq!(
            &next[..5],
            [
                "aisearch",
                "indexer",
                "wait",
                "srch-contoso-prod/orders-sql",
                "--since"
            ]
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &["aisearch", "indexer", "run", "srch-contoso-prod/orders-sql"],
            vec![
                one(),
                Answer::status(
                    409,
                    r#"{"error":{"code":"","message":"Another indexer invocation is currently in progress; concurrent invocations are not allowed."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 5, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("agent-cli aisearch indexer wait srch-contoso-prod/orders-sql"),
            "{}",
            outcome.stderr
        );
    }
}
