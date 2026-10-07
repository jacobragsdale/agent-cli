//! How long a throttled answer asks to be left alone, read from whichever
//! header the service wrote it in. `http` waits it out within the deadline.

use std::time::Duration;

use time::format_description::BorrowedFormatItem;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::http::Response;

/// When a throttled service does not say how long to wait. Key Vault never does.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(30);
/// A header that works out at nothing is still a refusal; asking again in the
/// same breath is what makes it worse.
const MIN_RETRY_AFTER: Duration = Duration::from_secs(1);
/// So a date from a clock that disagrees with ours cannot ask for days.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);
/// `Retry-After` in its other form: an IMF-fixdate, always in GMT.
const HTTP_DATE: &[BorrowedFormatItem<'static>] = format_description!(
    "[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT"
);

/// How long to leave a throttled answer alone: `Retry-After`, Resource
/// Graph's `x-ms-user-quota-resets-after` clock, `X-RateLimit-Reset`
/// (Datadog's seconds to wait, an epoch second as some services write it,
/// or Confluence's ISO 8601 time), or the default. The caller caps it by the
/// deadline.
#[must_use]
pub(crate) fn throttle_wait(response: &Response, now: OffsetDateTime) -> Duration {
    if let Some(header) = response.header("Retry-After") {
        return retry_after(Some(header), now);
    }
    if let Some(seconds) = response
        .header("x-ms-user-quota-resets-after")
        .and_then(hms_seconds)
    {
        return retry_after(Some(&seconds.to_string()), now);
    }
    if let Some(reset) = response
        .header("X-RateLimit-Reset")
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|reset| reset.is_finite())
    {
        // No wait is a billion seconds long; a number that large is a clock.
        let seconds = if reset > 1e9 {
            reset - now.unix_timestamp() as f64
        } else {
            reset
        };
        return retry_after(Some(&seconds.to_string()), now);
    }
    // Confluence writes it as an ISO 8601 time: `2025-10-08T15:00:00Z`.
    if let Some(reset) = response
        .header("X-RateLimit-Reset")
        .and_then(|raw| OffsetDateTime::parse(raw.trim(), &Rfc3339).ok())
    {
        let seconds = (reset - now).as_seconds_f64();
        return retry_after(Some(&seconds.to_string()), now);
    }
    DEFAULT_RETRY_AFTER
}

/// `hh:mm:ss` (or `mm:ss`) as whole seconds.
fn hms_seconds(raw: &str) -> Option<u64> {
    let parts: Vec<u64> = raw
        .trim()
        .split(':')
        .map(|part| part.trim().parse::<u64>().ok())
        .collect::<Option<_>>()?;
    match parts[..] {
        [hours, minutes, seconds] => Some(hours * 3600 + minutes * 60 + seconds),
        [minutes, seconds] => Some(minutes * 60 + seconds),
        _ => None,
    }
}

/// `Retry-After` as seconds or as a date to count forward to, clamped to
/// [`MIN_RETRY_AFTER`]..[`MAX_RETRY_AFTER`].
#[must_use]
pub(crate) fn retry_after(header: Option<&str>, now: OffsetDateTime) -> Duration {
    let Some(raw) = header.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return DEFAULT_RETRY_AFTER;
    };
    // `nan` parses as a number and would take the clamp down with it.
    let seconds = match raw.parse::<f64>() {
        Ok(seconds) if seconds.is_finite() => seconds,
        _ => match PrimitiveDateTime::parse(raw, HTTP_DATE) {
            Ok(when) => (when.assume_utc() - now).as_seconds_f64(),
            Err(_) => return DEFAULT_RETRY_AFTER,
        },
    };
    Duration::from_secs_f64(
        seconds.clamp(MIN_RETRY_AFTER.as_secs_f64(), MAX_RETRY_AFTER.as_secs_f64()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_reads_seconds_a_date_and_nothing_at_all() {
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(retry_after(Some("7"), now), Duration::from_secs(7));
        assert_eq!(retry_after(Some("0"), now), MIN_RETRY_AFTER);
        assert_eq!(retry_after(Some("nan"), now), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after(None, now), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after(Some("99999"), now), MAX_RETRY_AFTER);
        // 1_700_000_000 is Tue, 14 Nov 2023 22:13:20 GMT.
        assert_eq!(
            retry_after(Some("Tue, 14 Nov 2023 22:13:50 GMT"), now),
            Duration::from_secs(30)
        );
        let quota = Response {
            headers: vec![("x-ms-user-quota-resets-after".into(), "00:00:04".into())],
            ..Response::default()
        };
        assert_eq!(throttle_wait(&quota, now), Duration::from_secs(4));
        let reset = |value: &str| Response {
            headers: vec![("X-RateLimit-Reset".into(), value.into())],
            ..Response::default()
        };
        assert_eq!(throttle_wait(&reset("12"), now), Duration::from_secs(12));
        assert_eq!(
            throttle_wait(&reset("1700000009"), now),
            Duration::from_secs(9),
            "an epoch second counts forward from now"
        );
        assert_eq!(throttle_wait(&reset("soon"), now), DEFAULT_RETRY_AFTER);
        assert_eq!(
            throttle_wait(&reset("2023-11-14T22:13:35Z"), now),
            Duration::from_secs(15),
            "an ISO 8601 time counts forward from now"
        );
    }
}
