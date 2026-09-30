use std::time::Duration;

use agent_cli_core::{Ctx, Effect, Failure, Method, Span, command, now, utc_time};
use anyhow::Result;
use serde_json::json;

use crate::client::Dd;

use super::{DowntimeRow, downtime_row};

/// The longest mute: long enough for a weekend, short enough that a
/// forgotten one ends by itself.
const MUTE_MAX: Duration = Duration::from_secs(7 * 86_400);

#[derive(clap::Args)]
pub struct DowntimeCreateArgs {
    /// The monitor to mute
    #[arg(
        long,
        required_unless_present = "monitor_tag",
        conflicts_with = "monitor_tag"
    )]
    monitor: Option<i64>,
    /// Mute every monitor with these tags instead (repeatable)
    #[arg(long, value_delimiter = ',')]
    monitor_tag: Vec<String>,
    /// How long, at most 7d: 30m, 2h, 1d
    #[arg(long = "for", required = true)]
    length: Span,
    /// The groups to mute: '*' (every group), 'pod_name:api-1', 'env:prod'
    #[arg(long, default_value = "*")]
    scope: String,
    /// Why, as Datadog shows it
    #[arg(long)]
    message: Option<String>,
}

fn downtime_create(ctx: &Ctx, args: DowntimeCreateArgs) -> Result<DowntimeRow> {
    let length = args.length.0;
    if length.is_zero() || length > MUTE_MAX {
        return Err(Failure::usage(format!(
            "--for is {}s; a mute lasts more than 0 and at most 7d",
            length.as_secs()
        ))
        .hint("mute again when it ends, if it still needs it")
        .into());
    }
    let dd = Dd::load(ctx)?;
    let end = now() + length;
    let identifier = match args.monitor {
        Some(monitor) => json!({"monitor_id": monitor}),
        None => json!({"monitor_tags": args.monitor_tag}),
    };
    let mut attributes = json!({
        "monitor_identifier": identifier,
        "scope": args.scope,
        "schedule": {"end": utc_time(end)},
        "display_timezone": "UTC",
    });
    if let Some(message) = args.message {
        attributes["message"] = json!(message);
    }
    let created = dd.change(
        ctx,
        Effect::Destructive,
        Method::Post,
        "/api/v2/downtime",
        Some(json!({"data": {"type": "downtime", "attributes": attributes}})),
    )?;
    let row = downtime_row(&created["data"]);
    ctx.note(format!(
        "[unmute early with agent-cli dd downtime cancel {}]",
        row.id
    ));
    Ok(row)
}

command! {
    pub DOWNTIME_CREATE = ["dd", "downtime", "create"], Destructive,
    "Mute a Datadog monitor for a bounded time (--for, at most 7 days)",
    keywords: ["mute", "silence", "snooze", "suppress", "quiet", "pause", "during", "deploy", "minutes"],
    example: "dd downtime create --monitor 4711 --for 30m --message 'deploy v1.4.2' --dry-run",
    run: downtime_create,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, assert_dry_run};
    use serde_json::json;

    use super::*;
    use crate::testing::{dd, downtime, sent_bodies};

    #[test]
    fn downtime_create_needs_yes_and_a_for_of_at_most_seven_days() {
        let plan = assert_dry_run(
            &[crate::DOMAIN],
            &[
                "dd",
                "downtime",
                "create",
                "--monitor",
                "4711",
                "--for",
                "30m",
                "--message",
                "deploy v1.4.2",
            ],
            vec![],
        );
        assert_eq!(plan[0]["method"], "POST");
        assert_eq!(plan[0]["url"], "https://api.datadoghq.com/api/v2/downtime");
        let attributes = &plan[0]["body"]["data"]["attributes"];
        assert_eq!(
            attributes["monitor_identifier"],
            json!({"monitor_id": 4711})
        );
        assert_eq!(attributes["scope"], "*");
        assert!(
            attributes["schedule"]["end"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );

        for length in ["8d", "2w", "0s"] {
            let (outcome, transport) = dd(
                &[
                    "dd",
                    "downtime",
                    "create",
                    "--monitor",
                    "4711",
                    "--for",
                    length,
                    "--yes",
                ],
                vec![],
            );
            assert_eq!(outcome.code, 2, "{length}: {outcome:?}");
            assert!(outcome.stderr.contains("at most 7d"), "{}", outcome.stderr);
            assert!(transport.sent().is_empty());
        }
        let (outcome, transport) = dd(&["dd", "downtime", "create", "--monitor", "4711"], vec![]);
        assert_eq!(outcome.code, 2, "--for is required: {outcome:?}");
        assert!(transport.sent().is_empty());
        let (outcome, transport) = dd(
            &[
                "dd",
                "downtime",
                "create",
                "--monitor",
                "4711",
                "--for",
                "30m",
            ],
            vec![],
        );
        assert_eq!(outcome.code, 2, "destructive without --yes: {outcome:?}");
        assert!(outcome.stderr.contains("--yes"), "{}", outcome.stderr);
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn downtime_create_with_yes_mutes_and_says_how_to_unmute() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "downtime",
                "create",
                "--monitor-tag",
                "service:api",
                "--for",
                "1h",
                "--scope",
                "env:prod",
                "--yes",
            ],
            vec![Answer::json(
                &json!({"data": downtime("00000000-0000-4000-8000-00000000d003", 0, "2026-09-29T13:00:00+00:00")}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()["id"], "00000000-0000-4000-8000-00000000d003");
        assert!(
            outcome
                .stderr
                .contains("agent-cli dd downtime cancel 00000000-0000-4000-8000-00000000d003]"),
            "{}",
            outcome.stderr
        );
        let body = &sent_bodies(&transport)[0];
        assert_eq!(
            body["data"]["attributes"]["monitor_identifier"],
            json!({"monitor_tags": ["service:api"]})
        );
        assert_eq!(body["data"]["attributes"]["scope"], "env:prod");
        assert_eq!(transport.sent()[0].method, Method::Post);
    }
}
