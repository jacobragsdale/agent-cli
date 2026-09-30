//! What the log, span and event searches share: log statuses, one page's
//! bound, the notes on a slow window and on more matches, and the ids,
//! counts and durations Datadog writes more than one way.

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use serde_json::Value;

use crate::client::{Window, text};

/// A log's status, as Datadog's search syntax spells it.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LogStatus {
    Emergency,
    Alert,
    Critical,
    Error,
    Warn,
    Notice,
    Info,
    Debug,
    Ok,
}

/// `values` by their command-line names, for a search term.
pub(crate) fn names<T: clap::ValueEnum>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| Some(value.to_possible_value()?.get_name().to_owned()))
        .collect()
}

/// Longer messages are cut, with the count of what was left out.
pub(crate) const MESSAGE_MAX: usize = 300;
/// One page of the logs and spans search APIs.
pub(crate) const PAGE_MAX: usize = 1000;
/// Searches over a longer window are slow; counting is not.
const SLOW_WINDOW: i64 = 86_400;

/// A trace id Datadog wrote as a string or a number.
pub(crate) fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        other => text(other),
    }
}

/// `--limit` fits one page; anything more is a count's job.
pub(crate) fn check_page(limit: usize, what: &str) -> Result<()> {
    if limit > PAGE_MAX {
        return Err(
            Failure::usage(format!("--limit is at most {PAGE_MAX}, one page of {what}"))
                .hint(if what == "logs" {
                    "count every match with agent-cli dd log-count list --by service"
                } else {
                    "count every span with agent-cli dd service get SERVICE"
                })
                .into(),
        );
    }
    Ok(())
}

pub(crate) fn slow_window(ctx: &Ctx, window: Window) {
    if window.seconds() > SLOW_WINDOW {
        ctx.note(
            "[a window over 1d is slow to search; counting is fast: agent-cli dd log-count list]",
        );
    }
}

/// The logs and spans APIs return a cursor, not a total.
pub(crate) fn more_match(ctx: &Ctx, found: &Value, shown: usize, limit: usize, count: &str) {
    if shown >= limit && !found["meta"]["page"]["after"].is_null() {
        ctx.note(format!(
            "[{shown} shown, more match; narrow QUERY, or count them with agent-cli {count}]"
        ));
    }
}

/// A count Datadog wrote as an integer or a float.
pub(crate) fn count(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|count| count.max(0.0).round() as u64))
        .unwrap_or(0)
}

/// Nanoseconds, as Datadog measures a span, in milliseconds to 0.1.
pub(crate) fn millis(nanos: &Value) -> Option<f64> {
    nanos
        .as_f64()
        .map(|nanos| (nanos / 100_000.0).round() / 10.0)
}
