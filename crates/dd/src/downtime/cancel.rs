use agent_cli_core::{Ctx, Effect, Failure, Method, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::Dd;

#[derive(clap::Args)]
pub struct DowntimeCancelArgs {
    /// The downtime id, from dd downtime list
    id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Canceled {
    id: String,
    canceled: bool,
}

fn downtime_cancel(ctx: &Ctx, args: DowntimeCancelArgs) -> Result<Canceled> {
    let id = args.id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(Failure::usage(format!("{id:?} is not a downtime id"))
            .hint("list them: agent-cli dd downtime list --fields id,monitor,end")
            .into());
    }
    let dd = Dd::load(ctx)?;
    dd.change(
        ctx,
        Effect::Write,
        Method::Delete,
        &format!("/api/v2/downtime/{id}"),
        None,
    )?;
    Ok(Canceled {
        id: id.to_owned(),
        canceled: true,
    })
}

command! {
    pub DOWNTIME_CANCEL = ["dd", "downtime", "cancel"], Write,
    "Unmute a Datadog monitor now, ending its downtime before it runs out",
    keywords: ["unsilence", "resume", "early", "stop muting", "monitor"],
    example: "dd downtime cancel 00000000-0000-4000-8000-00000000d001 --dry-run",
    run: downtime_cancel,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use super::*;
    use crate::testing::dd;

    #[test]
    fn downtime_cancel_deletes_and_its_dry_run_sends_nothing() {
        let plan = assert_dry_run(
            &[crate::DOMAIN],
            &[
                "dd",
                "downtime",
                "cancel",
                "00000000-0000-4000-8000-00000000d001",
            ],
            vec![],
        );
        assert_eq!(
            plan[0]["url"],
            "https://api.datadoghq.com/api/v2/downtime/00000000-0000-4000-8000-00000000d001"
        );
        let (outcome, transport) = dd(
            &[
                "dd",
                "downtime",
                "cancel",
                "00000000-0000-4000-8000-00000000d001",
            ],
            vec![Answer::status(204, "")],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": "00000000-0000-4000-8000-00000000d001", "canceled": true})
        );
        assert_eq!(transport.sent()[0].method, Method::Delete);
        let (outcome, _) = dd(&["dd", "downtime", "cancel", "../monitor/1"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
    }
}
