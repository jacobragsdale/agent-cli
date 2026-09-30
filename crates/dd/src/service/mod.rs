//! APM: which services send traces, how healthy one is (error rate and
//! latency from span aggregates Datadog computes), and the spans themselves,
//! with durations in milliseconds rather than Datadog's nanoseconds.

pub(crate) mod get;
pub(crate) mod list;
