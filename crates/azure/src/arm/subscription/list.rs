//! `arm subscription list`: ARM's `/subscriptions`, within `[azure]
//! subscriptions`.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::arm::{SUBSCRIPTIONS, Subscription, subscriptions};
use crate::client::{limited, narrow};
use crate::config::Azure;

#[derive(clap::Args)]
pub struct SubscriptionListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn subscription_list(ctx: &Ctx, args: SubscriptionListArgs) -> Result<Vec<Subscription>> {
    let azure = Azure::load(ctx)?;
    let found = narrow(
        &subscriptions(ctx, &azure)?,
        &azure.subscriptions,
        &[],
        SUBSCRIPTIONS,
        Subscription::guid,
    )?;
    Ok(limited(ctx, found, args.limit))
}

command! {
    pub SUBSCRIPTION_LIST = ["arm", "subscription", "list"], Read,
    "List the Azure subscriptions the az login reaches (within [azure] subscriptions)",
    keywords: ["account", "tenant", "accounts"],
    example: "arm subscription list --fields id,name,state",
    run: subscription_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{azure, azure_with};

    fn page(guids: &[(&str, &str)], next: Option<&str>) -> Answer {
        Answer::json(&json!({
            "value": guids.iter().map(|(guid, name)| json!({
                "id": format!("/subscriptions/{guid}"), "subscriptionId": guid,
                "displayName": name, "state": "Enabled",
                "tenantId": "00000000-0000-4000-8000-00000000aa01",
            })).collect::<Vec<_>>(),
            "nextLink": next,
        }))
    }

    #[test]
    fn subscriptions_page_through_arm_and_keep_the_allowlists_order() {
        let (outcome, transport) = azure_with(
            &[crate::ARM],
            &["arm", "subscription", "list"],
            vec![
                page(
                    &[("00000000-0000-4000-8000-000000000001", "contoso-dev")],
                    Some(
                        "https://management.azure.com/subscriptions?api-version=2022-12-01&$skiptoken=2",
                    ),
                ),
                page(
                    &[("00000000-0000-4000-8000-000000000002", "contoso-prod")],
                    None,
                ),
            ],
            "[azure]\nsubscriptions = [\"00000000-0000-4000-8000-000000000002\", \"00000000-0000-4000-8000-000000000001\"]\n",
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([
                {"id": "/subscriptions/00000000-0000-4000-8000-000000000002", "name": "contoso-prod",
                 "state": "Enabled", "tenant": "00000000-0000-4000-8000-00000000aa01"},
                {"id": "/subscriptions/00000000-0000-4000-8000-000000000001", "name": "contoso-dev",
                 "state": "Enabled", "tenant": "00000000-0000-4000-8000-00000000aa01"},
            ])
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            "https://management.azure.com/subscriptions?api-version=2022-12-01"
        );
        assert_eq!(
            sent[1].authorization.as_deref(),
            Some("Bearer token@https://management.azure.com/")
        );
    }

    #[test]
    fn a_next_link_off_arm_gets_no_token() {
        let (outcome, transport) = azure(
            &[crate::ARM],
            &["arm", "subscription", "list"],
            vec![page(
                &[("00000000-0000-4000-8000-000000000001", "contoso-dev")],
                Some("https://management.azure.com.evil.example/subscriptions"),
            )],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(outcome.stderr.contains("is not ARM"), "{}", outcome.stderr);
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn an_allowlist_that_reaches_nothing_is_a_setup_error() {
        let (outcome, _) = azure_with(
            &[crate::ARM],
            &["arm", "subscription", "list"],
            vec![page(
                &[("00000000-0000-4000-8000-000000000001", "contoso-dev")],
                None,
            )],
            "[azure]\nsubscriptions = \"00000000-0000-4000-8000-0000000000ff\"\n",
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(outcome.stderr.contains("doctor arm"), "{}", outcome.stderr);
    }
}
