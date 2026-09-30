use std::time::Duration;

use agent_cli_core::{Ctx, command, utc};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::client::{Dd, limited, text};

/// Every page is read, then kept this long: titles change rarely.
const DASHBOARDS_TTL: Duration = Duration::from_secs(600);
const DASHBOARD_PAGE: usize = 1000;
/// So an org with a runaway dashboard count still answers within the deadline.
const DASHBOARD_PAGES: usize = 20;

#[derive(clap::Args)]
pub struct DashboardListArgs {
    /// Words the title must contain, any case
    words: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DashboardRow {
    id: String,
    title: String,
    modified: Option<String>,
    url: String,
}

fn dashboard_list(ctx: &Ctx, args: DashboardListArgs) -> Result<Vec<DashboardRow>> {
    let dd = Dd::load(ctx)?;
    let key = format!("dd:{}:dashboards", dd.site.name);
    let all: Vec<DashboardRow> = match ctx.cache().get(&key) {
        Some(all) => all,
        None => {
            let mut all = Vec::new();
            for page in 0..DASHBOARD_PAGES {
                let found = dd.get(
                    ctx,
                    "/api/v1/dashboard",
                    &[
                        ("count", DASHBOARD_PAGE.to_string()),
                        ("start", (page * DASHBOARD_PAGE).to_string()),
                    ],
                )?;
                let listed = found["dashboards"].as_array().cloned().unwrap_or_default();
                all.extend(listed.iter().map(|dashboard| DashboardRow {
                    id: text(&dashboard["id"]).unwrap_or_default(),
                    title: text(&dashboard["title"]).unwrap_or_default(),
                    modified: text(&dashboard["modified_at"]).map(|at| utc(&at)),
                    url: dd.app(dashboard["url"].as_str().unwrap_or_default()),
                }));
                if listed.len() < DASHBOARD_PAGE {
                    break;
                }
            }
            ctx.cache().put(&key, &all, DASHBOARDS_TTL);
            all
        }
    };
    let words: Vec<String> = args.words.iter().map(|word| word.to_lowercase()).collect();
    let rows = all
        .into_iter()
        .filter(|row| {
            let title = row.title.to_lowercase();
            words.iter().all(|word| title.contains(word.as_str()))
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub DASHBOARD_LIST = ["dd", "dashboard", "list"], Read,
    "Find Datadog dashboards by words in the title, with their links",
    keywords: ["board", "link", "screen", "dashboards", "find dashboard"],
    example: "dd dashboard list kubernetes --fields id,title,url",
    run: dashboard_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::dd;

    #[test]
    fn dashboard_list_pages_every_dashboard_and_matches_title_words() {
        let page = |count: usize| {
            json!({"dashboards": (0..count).map(|i| json!({
                "id": format!("abc-def-{i:03}"), "title": if i == 7 { "Kubernetes pods overview".to_owned() } else { format!("board {i}") },
                "url": format!("/dashboard/abc-def-{i:03}/board-{i}"), "modified_at": "2026-09-20T10:00:00.123456+00:00"
            })).collect::<Vec<_>>()})
        };
        let (outcome, transport) = dd(
            &["dd", "dashboard", "list", "KUBERNETES", "pods"],
            vec![Answer::json(&page(1000)), Answer::json(&page(3))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "id": "abc-def-007", "title": "Kubernetes pods overview", "modified": "2026-09-20T10:00:00Z",
                "url": "https://app.datadoghq.eu/dashboard/abc-def-007/board-7"
            }])
        );
        assert!(transport.sent()[1].url.ends_with("count=1000&start=1000"));
    }
}
