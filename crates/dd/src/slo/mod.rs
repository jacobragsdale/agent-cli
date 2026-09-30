//! `dd slo`: service level objectives and their error budget.

pub(crate) mod get;
pub(crate) mod list;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{strings, text};

#[derive(Debug, Serialize, JsonSchema)]
pub struct SloRow {
    /// What `dd slo get` takes.
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Percent.
    target: Option<f64>,
    timeframe: Option<String>,
    tags: Vec<String>,
}

fn slo_row(slo: &Value) -> SloRow {
    let threshold = slo["thresholds"]
        .as_array()
        .and_then(|thresholds| thresholds.first())
        .cloned()
        .unwrap_or(Value::Null);
    SloRow {
        id: text(&slo["id"]).unwrap_or_default(),
        name: text(&slo["name"]).unwrap_or_default(),
        kind: text(&slo["type"]),
        target: slo["target_threshold"]
            .as_f64()
            .or_else(|| threshold["target"].as_f64()),
        timeframe: text(&slo["timeframe"]).or_else(|| text(&threshold["timeframe"])),
        tags: strings(&slo["tags"]),
    }
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::dd;

    #[test]
    fn slo_get_reads_the_slo_then_its_history_over_its_own_timeframe() {
        let slo = json!({
            "id": "0c3fe5a1b2c34d5e8f9a0b1c2d3e4f50", "name": "api availability", "type": "metric",
            "thresholds": [{"timeframe": "7d", "target": 99.9, "warning": 99.95}],
            "timeframe": "7d", "target_threshold": 99.9, "tags": ["service:api", "team:web"]
        });
        let (outcome, transport) = dd(
            &[
                "dd",
                "slo",
                "get",
                "https://app.datadoghq.eu/slo/manage?slo_id=0c3fe5a1b2c34d5e8f9a0b1c2d3e4f50&timeframe=7d",
                "--until",
                "2026-09-29T12:00:00Z",
            ],
            vec![
                Answer::json(&json!({"data": slo, "error": null})),
                Answer::json(
                    &json!({"data": {"overall": {"sli_value": 99.82, "error_budget_remaining": {"7d": -80.0}}}, "errors": null}),
                ),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let got = outcome.json();
        assert_eq!(got["sli"], 99.82);
        assert_eq!(got["budget_remaining"], -80.0);
        assert_eq!(got["breaching"], true);
        assert_eq!(got["since"], "2026-09-22T12:00:00Z");
        assert_eq!(
            transport.sent()[1].url,
            "https://api.datadoghq.eu/api/v1/slo/0c3fe5a1b2c34d5e8f9a0b1c2d3e4f50/history?from_ts=1790078400&to_ts=1790683200"
        );

        let (outcome, _) = dd(
            &["dd", "slo", "list", "--tag", "team:web"],
            vec![Answer::json(&json!({"data": [slo]}))],
        );
        assert_eq!(outcome.json()[0]["target"], 99.9);
    }
}
