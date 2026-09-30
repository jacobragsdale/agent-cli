use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Failure, command, utc};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Dd, text};

use super::{incident_row, search_incidents};

#[derive(clap::Args)]
pub struct IncidentGetArgs {
    /// The incident: its number (982), its UUID, or its https://app.<site>/incidents/982 URL
    id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IncidentDetail {
    id: i64,
    uuid: String,
    title: String,
    state: Option<String>,
    severity: Option<String>,
    customer_impacted: Option<bool>,
    created: Option<String>,
    detected: Option<String>,
    resolved: Option<String>,
    /// The incident's fields (summary, root cause, teams, services) that have a value.
    fields: BTreeMap<String, Value>,
    url: String,
}

fn incident_get(ctx: &Ctx, args: IncidentGetArgs) -> Result<IncidentDetail> {
    let dd = Dd::load(ctx)?;
    let raw = dd.web_id(&args.id, "incident", "incidents")?;
    let incident = if let Ok(number) = raw.parse::<i64>() {
        // ponytail: the incidents API is keyed by UUID; a number is found by
        // search. If Datadog stops matching public_id there, cache a
        // number-to-UUID map from incident list instead.
        search_incidents(&dd, ctx, format!("public_id:{number}"), 5)?
            .into_iter()
            .find(|hit| hit["attributes"]["public_id"].as_i64() == Some(number))
            .ok_or_else(|| {
                Failure::not_found(format!("no incident {number}")).hint(
                    "agent-cli dd incident list --state active,stable,resolved --fields id,title",
                )
            })?
    } else {
        dd.get(ctx, &format!("/api/v2/incidents/{raw}"), &[])?["data"].clone()
    };
    let row = incident_row(&incident);
    let attributes = &incident["attributes"];
    let fields = attributes["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, field)| {
            let value = field.get("value").unwrap_or(field);
            (!value.is_null() && value != &Value::Array(Vec::new()))
                .then(|| (name.clone(), value.clone()))
        })
        .collect();
    Ok(IncidentDetail {
        uuid: text(&incident["id"]).unwrap_or_default(),
        customer_impacted: attributes["customer_impacted"].as_bool(),
        detected: text(&attributes["detected"]).map(|at| utc(&at)),
        fields,
        url: dd.app(&format!("/incidents/{}", row.id)),
        id: row.id,
        title: row.title,
        state: row.state,
        severity: row.severity,
        created: row.created,
        resolved: row.resolved,
    })
}

command! {
    pub INCIDENT_GET = ["dd", "incident", "get"], Read,
    "Show a Datadog incident: state, severity, impact, timeline stamps and fields",
    keywords: ["root cause", "commander", "impact", "summary", "details"],
    example: "dd incident get 982 --fields title,state,fields",
    run: incident_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dd, incident};

    #[test]
    fn incident_get_finds_a_number_by_search_and_a_uuid_directly() {
        let search = json!({"data": {"attributes": {"incidents": [incident(982, "api errors after deploy", "stable")]}}});
        for id in ["982", "https://app.datadoghq.eu/incidents/982"] {
            let (outcome, transport) =
                dd(&["dd", "incident", "get", id], vec![Answer::json(&search)]);
            assert_eq!(outcome.code, 0, "{outcome:?}");
            let got = outcome.json();
            assert_eq!(got["id"], 982);
            assert_eq!(got["uuid"], "00000000-0000-4000-8000-000000000982");
            assert_eq!(
                got["fields"],
                json!({"severity": "SEV-2", "summary": "api 5xx during the v1.4.2 rollout"})
            );
            assert_eq!(got["detected"], "2026-09-28T21:37:00Z");
            assert_eq!(got["url"], "https://app.datadoghq.eu/incidents/982");
            assert!(transport.sent()[0].url.contains("query=public_id%3A982"));
        }
        let (outcome, transport) = dd(
            &[
                "dd",
                "incident",
                "get",
                "00000000-0000-4000-8000-000000000982",
            ],
            vec![Answer::json(&incident(
                982,
                "api errors after deploy",
                "stable",
            ))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/incidents/00000000-0000-4000-8000-000000000982"
        );
        let (outcome, _) = dd(&["dd", "incident", "get", "7"], vec![Answer::json(&search)]);
        assert_eq!(outcome.code, 4, "{outcome:?}");
    }
}
