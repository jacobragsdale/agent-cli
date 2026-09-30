//! The dd domain: Datadog logs, metrics, monitors, downtimes, APM, incidents,
//! SLOs and dashboards, over Datadog's public REST API.
//!
//! It covers the calls agents make most while debugging and operating, with
//! rows that bound their size and point back at Kubernetes (`pod` is the k8s
//! pod id). Datadog's own CLI, pup, covers the long tail; `doctor dd` says
//! so. pup is not wrapped: see docs/plans/datadog.md.

mod client;
mod container;
mod dashboard;
mod doctor;
mod downtime;
mod event;
mod host;
mod incident;
mod log;
mod log_count;
mod metric;
mod monitor;
mod search;
mod service;
mod slo;
mod span;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "dd",
    summary: "Datadog",
    commands: &[
        log::list::LOG_LIST,
        log_count::list::LOG_COUNT_LIST,
        metric::list::METRIC_LIST,
        metric::get::METRIC_GET,
        monitor::list::MONITOR_LIST,
        monitor::get::MONITOR_GET,
        downtime::list::DOWNTIME_LIST,
        downtime::create::DOWNTIME_CREATE,
        downtime::cancel::DOWNTIME_CANCEL,
        event::list::EVENT_LIST,
        service::list::SERVICE_LIST,
        service::get::SERVICE_GET,
        span::list::SPAN_LIST,
        incident::list::INCIDENT_LIST,
        incident::get::INCIDENT_GET,
        host::list::HOST_LIST,
        container::list::CONTAINER_LIST,
        slo::list::SLO_LIST,
        slo::get::SLO_GET,
        dashboard::list::DASHBOARD_LIST,
    ],
    synonyms: &[
        ("datadog", &["dd"]),
        ("apm", &["span", "service"]),
        ("trace", &["span"]),
        ("traces", &["span"]),
        ("tracing", &["span"]),
        ("request", &["span"]),
        ("requests", &["span"]),
        ("mute", &["downtime", "create"]),
        ("silence", &["downtime", "create"]),
        ("snooze", &["downtime", "create"]),
        ("unmute", &["downtime", "cancel"]),
        ("maintenance", &["downtime"]),
        ("alert", &["monitor"]),
        ("alerts", &["monitor"]),
        ("alerting", &["monitor"]),
        ("alarm", &["monitor"]),
        ("alarms", &["monitor"]),
        ("outage", &["incident"]),
        ("sev", &["incident"]),
        ("timeseries", &["metric"]),
        ("graph", &["metric"]),
        ("error rate", &["service", "error_rate"]),
        ("latency", &["service", "p95_ms"]),
        ("dashboards", &["dashboard"]),
    ],
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::assert_read_only_refuses;
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 20);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        assert_read_only_refuses(&[DOMAIN]);
    }
}
