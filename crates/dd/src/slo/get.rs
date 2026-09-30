use agent_cli_core::{Ctx, Span, When, command, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Dd, Window};

use super::slo_row;

#[derive(clap::Args)]
pub struct SloGetArgs {
    /// The SLO: its id, or its https://app.<site>/slo?slo_id=… URL
    id: String,
    /// From when (default: the SLO's own timeframe before --until)
    #[arg(long)]
    since: Option<When>,
    /// Until when (default now)
    #[arg(long)]
    until: Option<When>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SloDetail {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    target: Option<f64>,
    timeframe: Option<String>,
    /// The measured percentage over the window.
    sli: Option<f64>,
    /// Percent of the error budget left.
    budget_remaining: Option<f64>,
    /// sli below target.
    breaching: Option<bool>,
    since: String,
    until: String,
    url: String,
}

fn slo_get(ctx: &Ctx, args: SloGetArgs) -> Result<SloDetail> {
    let dd = Dd::load(ctx)?;
    let id = dd.web_id(&args.id, "SLO", "slo")?;
    let slo = slo_row(&dd.get(ctx, &format!("/api/v1/slo/{id}"), &[])?["data"]);
    let own = slo
        .timeframe
        .as_deref()
        .filter(|timeframe| timeframe.parse::<Span>().is_ok())
        .unwrap_or("30d")
        .to_owned();
    let window = if args.since.is_none() {
        let until = args.until.unwrap_or_else(When::now).0;
        let span: Span = own.parse().map_err(anyhow::Error::msg)?;
        Window {
            since: until - span.0,
            until,
        }
    } else {
        Window::new(ctx, args.since, args.until, &own)?
    };
    let history = dd.get(
        ctx,
        &format!("/api/v1/slo/{id}/history"),
        &[
            ("from_ts", window.since.unix_timestamp().to_string()),
            ("to_ts", window.until.unix_timestamp().to_string()),
        ],
    )?;
    let overall = &history["data"]["overall"];
    let sli = overall["sli_value"].as_f64();
    let budget = &overall["error_budget_remaining"];
    let budget_remaining = budget[own.as_str()]
        .as_f64()
        .or_else(|| budget.as_object()?.values().find_map(Value::as_f64));
    Ok(SloDetail {
        url: dd.app(&format!("/slo?slo_id={}", slo.id)),
        breaching: sli.zip(slo.target).map(|(sli, target)| sli < target),
        id: slo.id,
        name: slo.name,
        kind: slo.kind,
        target: slo.target,
        timeframe: slo.timeframe,
        sli,
        budget_remaining,
        since: utc_time(window.since),
        until: utc_time(window.until),
    })
}

command! {
    pub SLO_GET = ["dd", "slo", "get"], Read,
    "Show an SLO's measured SLI and error budget left over its window",
    keywords: ["error budget", "burn", "compliance", "breaching", "availability"],
    example: "dd slo get 0c3fe5a1b2c34d5e8f9a0b1c2d3e4f50 --fields sli,target,budget_remaining",
    run: slo_get,
}
