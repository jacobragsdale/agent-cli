//! Monitors and downtimes: what is alerting and why, and muting it for a
//! bounded time. A mute is Destructive: it can be undone, a missed page
//! cannot, so it needs `--yes` and never lasts more than 7 days.

pub(crate) mod get;
pub(crate) mod list;

pub(crate) const MESSAGE_MAX: usize = 500;
