//! The rest of what an agent looks up while debugging: incidents, hosts,
//! containers, SLOs and dashboard links.

use std::collections::BTreeMap;
use std::time::Duration;

use agent_cli_core::{Ctx, Failure, Span, When, command, utc, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Dd, Scope, Window, epoch, limited, pod_ref, strings, tag, text};

// ---------- incidents ----------

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

#[derive(Debug, Serialize, JsonSchema)]
pub struct IncidentRow {
    /// What `dd incident get` takes: the number Datadog shows.
    id: i64,
    title: String,
    state: Option<String>,
    severity: Option<String>,
    created: Option<String>,
    resolved: Option<String>,
}

fn incident_row(incident: &Value) -> IncidentRow {
    let attributes = &incident["attributes"];
    IncidentRow {
        id: attributes["public_id"].as_i64().unwrap_or_default(),
        title: text(&attributes["title"]).unwrap_or_default(),
        state: text(&attributes["state"]),
        severity: text(&attributes["severity"]),
        created: text(&attributes["created"]).map(|at| utc(&at)),
        resolved: text(&attributes["resolved"]).map(|at| utc(&at)),
    }
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

/// The incidents search: every hit is wrapped in its own `data`.
fn search_incidents(dd: &Dd, ctx: &Ctx, query: String, size: usize) -> Result<Vec<Value>> {
    let found = dd.get(
        ctx,
        "/api/v2/incidents/search",
        &[
            ("query", query),
            ("sort", "-created".to_owned()),
            ("page[size]", size.to_string()),
        ],
    )?;
    Ok(found["data"]["attributes"]["incidents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|hit| hit["data"].clone())
        .collect())
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

// ---------- dd host list ----------

#[derive(clap::Args)]
pub struct HostListArgs {
    /// Host name or tag to match: 'aks-nodepool1', 'env:prod'
    query: Option<String>,
    /// kube_cluster_name tag
    #[arg(long)]
    cluster: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HostRow {
    /// The host name.
    id: String,
    up: Option<bool>,
    last_reported: Option<String>,
    apps: Vec<String>,
    /// Its kube_cluster_name tag.
    cluster: Option<String>,
    muted: Option<bool>,
}

fn host_list(ctx: &Ctx, args: HostListArgs) -> Result<Vec<HostRow>> {
    let dd = Dd::load(ctx)?;
    let mut filter: Vec<String> = args.query.iter().cloned().collect();
    filter.extend(
        args.cluster
            .map(|cluster| format!("kube_cluster_name:{cluster}")),
    );
    let mut query = vec![("count", args.limit.clamp(1, 1000).to_string())];
    if !filter.is_empty() {
        query.insert(0, ("filter", filter.join(" ")));
    }
    let found = dd.get(ctx, "/api/v1/hosts", &query)?;
    let rows: Vec<HostRow> = found["host_list"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|host| {
            let tags: Vec<Value> = host["tags_by_source"]
                .as_object()
                .into_iter()
                .flat_map(|sources| sources.values())
                .filter_map(Value::as_array)
                .flatten()
                .cloned()
                .collect();
            HostRow {
                id: text(&host["host_name"])
                    .or_else(|| text(&host["name"]))
                    .unwrap_or_default(),
                up: host["up"].as_bool(),
                last_reported: host["last_reported_time"].as_i64().and_then(epoch),
                apps: strings(&host["apps"]),
                cluster: tag(&Value::Array(tags), "kube_cluster_name").map(str::to_owned),
                muted: host["is_muted"].as_bool(),
            }
        })
        .collect();
    if let Some(total) = found["total_matching"].as_u64()
        && total > rows.len() as u64
    {
        ctx.note(format!("[{} of {total}; --limit N]", rows.len()));
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub HOST_LIST = ["dd", "host", "list"], Read,
    "List hosts reporting to Datadog: up, last reported, apps and cluster",
    keywords: ["node", "machine", "vm", "agent reporting", "down", "infrastructure"],
    example: "dd host list --cluster prod --fields id,up,last_reported",
    run: host_list,
}

// ---------- dd container list ----------

#[derive(clap::Args)]
pub struct ContainerListArgs {
    #[command(flatten)]
    scope: Scope,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContainerRow {
    name: String,
    state: Option<String>,
    /// image:tag as Datadog saw it.
    image: Option<String>,
    /// The k8s pod id (cluster/namespace/pod) that `agent-cli k8s pod get` takes.
    pod: Option<String>,
    host: Option<String>,
    started: Option<String>,
}

fn container_list(ctx: &Ctx, args: ContainerListArgs) -> Result<Vec<ContainerRow>> {
    let dd = Dd::load(ctx)?;
    let tags = args.scope.tags(&dd, false)?;
    let mut query = vec![("page[size]", args.limit.clamp(1, 1000).to_string())];
    if !tags.is_empty() {
        query.insert(0, ("filter[tags]", tags.join(",")));
    }
    let found = dd.get(ctx, "/api/v2/containers", &query)?;
    let rows = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let attributes = &item["attributes"];
            let image = text(&attributes["image_name"]).map(|name| {
                match strings(&attributes["image_tags"]).first() {
                    Some(tag) => format!("{name}:{tag}"),
                    None => name,
                }
            });
            ContainerRow {
                name: text(&attributes["name"]).unwrap_or_default(),
                state: text(&attributes["state"]),
                image,
                pod: pod_ref(&attributes["tags"]),
                host: text(&attributes["host"]),
                started: text(&attributes["started_at"]).map(|at| naive_utc(&at)),
            }
        })
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

/// The containers API writes UTC with no offset; say so.
fn naive_utc(raw: &str) -> String {
    let has_offset =
        raw.ends_with('Z') || raw.get(19..).is_some_and(|rest| rest.contains(['+', '-']));
    utc(&if has_offset {
        raw.to_owned()
    } else {
        format!("{raw}Z")
    })
}

command! {
    pub CONTAINER_LIST = ["dd", "container", "list"], Read,
    "List containers Datadog sees, with state, image and the k8s pod id",
    keywords: ["docker", "image", "datadog containers"],
    example: "dd container list --namespace web --fields name,state,image,pod",
    run: container_list,
}

// ---------- SLOs ----------

#[derive(clap::Args)]
pub struct SloListArgs {
    /// Words in the SLO name
    query: Option<String>,
    /// SLO tags: team:web (repeatable, all must hold)
    #[arg(long)]
    tag: Vec<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

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

fn slo_list(ctx: &Ctx, args: SloListArgs) -> Result<Vec<SloRow>> {
    let dd = Dd::load(ctx)?;
    let mut query = vec![("limit", args.limit.clamp(1, 1000).to_string())];
    if let Some(words) = &args.query {
        query.push(("query", words.clone()));
    }
    if !args.tag.is_empty() {
        query.push(("tags_query", args.tag.join(" AND ")));
    }
    let found = dd.get(ctx, "/api/v1/slo", &query)?;
    let rows = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(slo_row)
        .collect();
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub SLO_LIST = ["dd", "slo", "list"], Read,
    "List Datadog SLOs with their target and timeframe",
    keywords: ["objective", "reliability", "target", "service level", "team"],
    example: "dd slo list --tag team:web --fields id,name,target",
    run: slo_list,
}

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

// ---------- dd dashboard list ----------

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

    use crate::testkit::dd;

    fn incident(public_id: i64, title: &str, state: &str) -> serde_json::Value {
        json!({"data": {
            "id": format!("00000000-0000-4000-8000-000000000{public_id}"),
            "type": "incidents",
            "attributes": {
                "public_id": public_id, "title": title, "state": state, "severity": "SEV-2",
                "customer_impacted": true,
                "created": "2026-09-28T21:40:12.000000+00:00", "detected": "2026-09-28T21:37:00+00:00",
                "resolved": null,
                "fields": {
                    "severity": {"type": "dropdown", "value": "SEV-2"},
                    "summary": {"type": "textbox", "value": "api 5xx during the v1.4.2 rollout"},
                    "teams": {"type": "autocomplete", "value": null}
                }
            }
        }})
    }

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

    #[test]
    fn host_and_container_rows_name_the_cluster_and_the_pod() {
        let (outcome, transport) = dd(
            &["dd", "host", "list", "--cluster", "prod"],
            vec![Answer::json(&json!({
                "host_list": [{
                    "id": 1_588_212_437, "name": "aks-nodepool1-12345678-vmss000001", "host_name": "aks-nodepool1-12345678-vmss000001",
                    "up": true, "last_reported_time": 1_790_683_140, "apps": ["agent", "kubernetes", "docker"],
                    "is_muted": false, "tags_by_source": {"Datadog": ["kube_cluster_name:prod", "env:prod"]}
                }],
                "total_matching": 1, "total_returned": 1
            }))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "id": "aks-nodepool1-12345678-vmss000001", "up": true, "last_reported": "2026-09-29T11:59:00Z",
                "apps": ["agent", "kubernetes", "docker"], "cluster": "prod", "muted": false
            }])
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v1/hosts?filter=kube_cluster_name%3Aprod&count=50"
        );

        let (outcome, transport) = dd(
            &["dd", "container", "list", "--deployment", "prod/web/worker"],
            vec![Answer::json(&json!({"data": [{
                "type": "container", "id": "c-1",
                "attributes": {
                    "name": "worker", "container_id": "4f5e6d7c8b9a", "state": "exited",
                    "host": "aks-nodepool1-12345678-vmss000001", "image_name": "contosoacr.azurecr.io/worker",
                    "image_tags": ["v1.4.2"], "started_at": "2026-09-29T11:47:10",
                    "tags": ["kube_cluster_name:prod", "kube_namespace:web", "pod_name:worker-5c4d3e9f1-q8zt1", "kube_deployment:worker"]
                }
            }]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{
                "name": "worker", "state": "exited", "image": "contosoacr.azurecr.io/worker:v1.4.2",
                "pod": "prod/web/worker-5c4d3e9f1-q8zt1", "host": "aks-nodepool1-12345678-vmss000001",
                "started": "2026-09-29T11:47:10Z"
            }])
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v2/containers?filter%5Btags%5D=kube_cluster_name%3Aprod%2Ckube_namespace%3Aweb%2Ckube_deployment%3Aworker&page%5Bsize%5D=50"
        );
    }

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
