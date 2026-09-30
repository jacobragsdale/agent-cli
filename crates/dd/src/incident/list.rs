use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::client::{Dd, limited};

use super::{IncidentRow, incident_row, search_incidents};

#[derive(clap::Args)]
pub struct IncidentListArgs {
    /// Incident search syntax, ANDed with the flags: 'customer_impacted:true'
    query: Option<String>,
    /// active, stable, resolved (repeatable; default active,stable)
    #[arg(long, value_delimiter = ',')]
    state: Vec<String>,
    /// SEV-1, SEV-2 … (repeatable; 1 means SEV-1)
    #[arg(long, value_delimiter = ',')]
    severity: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

/// `SEV-2` as typed (`2`, `sev-2`, `sev2`).
fn severity(raw: &str) -> String {
    let raw = raw.trim().to_ascii_uppercase();
    let digits = raw.trim_start_matches("SEV").trim_start_matches('-');
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        format!("SEV-{digits}")
    } else {
        raw
    }
}

fn incident_list(ctx: &Ctx, args: IncidentListArgs) -> Result<Vec<IncidentRow>> {
    let dd = Dd::load(ctx)?;
    let states: Vec<String> = if args.state.is_empty() {
        vec!["active".to_owned(), "stable".to_owned()]
    } else {
        args.state
            .iter()
            .map(|state| state.trim().to_ascii_lowercase())
            .collect()
    };
    let severities: Vec<String> = args.severity.iter().map(|raw| severity(raw)).collect();
    let mut terms: Vec<String> = args
        .query
        .iter()
        .map(|query| format!("({query})"))
        .collect();
    terms.extend(crate::client::any_of("state", &states));
    terms.extend(crate::client::any_of("severity", &severities));
    let incidents = search_incidents(&dd, ctx, terms.join(" AND "), args.limit.clamp(1, 100))?;
    let rows = incidents.iter().map(incident_row).collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub INCIDENT_LIST = ["dd", "incident", "list"], Read,
    "List Datadog incidents, active and stable by default",
    keywords: ["outage", "sev", "declared", "active", "ongoing", "postmortem"],
    example: "dd incident list --state active --fields id,title,severity",
    run: incident_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{dd, incident};

    #[test]
    fn incident_list_searches_active_and_stable_by_default() {
        let (outcome, transport) = dd(
            &["dd", "incident", "list", "--severity", "1,sev-2"],
            vec![Answer::json(
                &json!({"data": {"type": "incidents_search_results", "attributes": {
                    "total": 1, "incidents": [incident(982, "api errors after deploy", "stable")]
                }}}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "id": 982, "title": "api errors after deploy", "state": "stable", "severity": "SEV-2",
                "created": "2026-09-28T21:40:12Z"
            }]),
            "core drops nulls"
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/incidents/search?query=state%3A%28active+OR+stable%29+AND+severity%3A%28SEV-1+OR+SEV-2%29&sort=-created&page%5Bsize%5D=50"
        );
    }
}
