//! Metrics: finding a metric's name, and querying it as a summary an agent
//! can read (stats and a dozen points per series) instead of raw pointlists.

pub(crate) mod get;
pub(crate) mod list;

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::dd;

    #[test]
    fn a_query_datadog_refuses_is_a_usage_error_and_metric_list_filters_names() {
        let (outcome, _) = dd(
            &["dd", "metric", "get", "avg:nope{*"],
            vec![Answer::json(
                &json!({"status": "error", "error": "Error parsing query: unexpected end"}),
            )],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Error parsing query"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = dd(
            &[
                "dd",
                "metric",
                "list",
                "memory",
                "--cluster",
                "prod",
                "--since",
                "2026-09-29T11:00:00Z",
            ],
            vec![Answer::json(
                &json!({"from": "1790679600", "metrics": ["kubernetes.memory.usage", "kubernetes.cpu.usage.total", "kubernetes.memory.limits"]}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!(["kubernetes.memory.limits", "kubernetes.memory.usage"])
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://api.datadoghq.eu/api/v1/metrics?from=1790679600&tag_filter=kube_cluster_name%3Aprod"
        );
    }
}
