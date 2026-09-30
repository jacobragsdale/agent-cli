use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Dd, epoch, limited, strings, text};

/// Datadog's monitor states, as its search syntax spells them.
const STATES: &[&str] = &[
    "Alert", "Warn", "No Data", "OK", "Ignored", "Skipped", "Unknown",
];

/// A state as typed (`alert`, `no data`, `NoData`) in Datadog's spelling.
fn state(raw: &str) -> Result<&'static str> {
    let squashed: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    STATES
        .iter()
        .find(|state| state.replace(' ', "").to_ascii_lowercase() == squashed)
        .copied()
        .ok_or_else(|| {
            Failure::usage(format!(
                "{raw:?} is not a monitor state; --state takes {}",
                STATES.join(", ")
            ))
            .into()
        })
}

#[derive(clap::Args)]
pub struct MonitorListArgs {
    /// Monitor search syntax: 'service:api', 'type:metric', 'title:"error rate"'
    query: Option<String>,
    /// Alert, Warn, "No Data", OK (repeatable; any case)
    #[arg(long, value_delimiter = ',')]
    state: Vec<String>,
    /// Monitor tags: team:web (repeatable, all must hold)
    #[arg(long)]
    tag: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MonitorRow {
    /// What `dd monitor get` takes.
    id: i64,
    name: String,
    /// Alert, Warn, No Data, OK.
    state: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    priority: Option<i64>,
    /// When it last went to Alert.
    triggered: Option<String>,
    tags: Vec<String>,
}

fn monitor_list(ctx: &Ctx, args: MonitorListArgs) -> Result<Vec<MonitorRow>> {
    let states = args
        .state
        .iter()
        .map(|raw| {
            state(raw).map(|state| {
                if state.contains(' ') {
                    format!("\"{state}\"")
                } else {
                    state.to_owned()
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let dd = Dd::load(ctx)?;
    let mut terms: Vec<String> = crate::client::any_of("status", &states)
        .into_iter()
        .collect();
    terms.extend(args.tag.iter().map(|tag| format!("tag:\"{tag}\"")));
    let search = crate::client::search_query(args.query.as_deref(), &terms);
    let mut query = vec![
        ("per_page", args.limit.min(1000).to_string()),
        ("page", "0".to_owned()),
    ];
    if search != "*" {
        query.insert(0, ("query", search));
    }
    let found = dd.get(ctx, "/api/v1/monitor/search", &query)?;
    let rows: Vec<MonitorRow> = found["monitors"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|monitor| MonitorRow {
            id: monitor["id"].as_i64().unwrap_or_default(),
            name: text(&monitor["name"]).unwrap_or_default(),
            state: text(&monitor["status"]),
            kind: text(&monitor["type"]),
            priority: monitor["priority"].as_i64(),
            triggered: monitor["last_triggered_ts"].as_i64().and_then(epoch),
            tags: strings(&monitor["tags"]),
        })
        .collect();
    if let Some(total) = found["metadata"]["total_count"].as_u64()
        && total > rows.len() as u64
    {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub MONITOR_LIST = ["dd", "monitor", "list"], Read,
    "List Datadog monitors alerting or not, by state or tag, with when each triggered",
    keywords: ["alert", "alarm", "firing", "alerting", "paging", "team", "now"],
    example: "dd monitor list --state Alert,Warn --tag team:web --fields id,name,state,triggered",
    run: monitor_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::dd;

    #[test]
    fn monitor_list_searches_by_state_and_tag_in_datadog_syntax() {
        let (outcome, transport) = dd(
            &[
                "dd",
                "monitor",
                "list",
                "--state",
                "alert,no data",
                "--tag",
                "team:web",
                "--limit",
                "1",
            ],
            vec![Answer::json(&json!({
                "monitors": [{
                    "id": 4712, "name": "[prod] worker crash-looping", "status": "Alert", "type": "query alert",
                    "priority": 1, "tags": ["service:worker", "team:web"], "last_triggered_ts": 1_790_631_660,
                    "classification": "metric", "query": "max(last_10m):…"
                }],
                "metadata": {"total_count": 2, "page_count": 2, "page": 0, "per_page": 1},
                "counts": {"status": [{"name": "Alert", "count": 1}]}
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "id": 4712, "name": "[prod] worker crash-looping", "state": "Alert", "type": "query alert",
                "priority": 1, "triggered": "2026-09-28T21:41:00Z", "tags": ["service:worker", "team:web"]
            }])
        );
        assert!(
            outcome.stderr.contains("[1 of 2; --limit N]"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v1/monitor/search?query=status%3A%28Alert+OR+%22No+Data%22%29+tag%3A%22team%3Aweb%22&per_page=1&page=0"
        );
        let (outcome, _) = dd(&["dd", "monitor", "list", "--state", "firing"], vec![]);
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Alert, Warn, No Data, OK"),
            "{}",
            outcome.stderr
        );
    }
}
