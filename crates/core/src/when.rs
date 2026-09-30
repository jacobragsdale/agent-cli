//! Times and durations: the one parser for `--since`/`--until`, the one for
//! durations such as dd's `--for`, and the one formatter for every timestamp
//! the CLI prints.
//!
//! Every printed time is RFC 3339 in UTC, in whole seconds, ending in `Z`, and
//! every time flag takes that form back as it is, so a time an agent copies
//! out of one answer pastes into the next call. Relative forms (`2h`,
//! `now-15m`) and the overview's `Now:` line spare it date arithmetic, the
//! likeliest source of a silent error.

use std::str::FromStr;
use std::time::Duration;

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime, UtcOffset};

/// What a time flag accepts, for help and errors.
pub(crate) const TIME_FORMS: &str = "15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339";
/// What a duration flag accepts.
pub(crate) const SPAN_FORMS: &str = "500ms, 30s, 15m, 2h, 7d, 1w";

/// A point in time from the command line, resolved to UTC when it is parsed:
/// `15m`, `2h`, `1d`, `1w` (that long ago), the same after `now-`, `now`,
/// RFC 3339 with any offset, or a date (`2026-09-29`, midnight UTC).
///
/// Declare a flag as `since: Option<When>`; its kind is `time`, and
/// `check_registry` holds the name to `--since` or `--until`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct When(pub OffsetDateTime);

impl When {
    #[must_use]
    pub fn now() -> Self {
        Self(now())
    }

    /// RFC 3339, UTC, whole seconds: `2026-09-29T14:00:00Z`.
    #[must_use]
    pub fn utc(self) -> String {
        utc_time(self.0)
    }

    #[must_use]
    pub fn unix(self) -> i64 {
        self.0.unix_timestamp()
    }
}

impl FromStr for When {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let wrong = || format!("expected a time: {TIME_FORMS}; not {raw:?}");
        let ago = |text: &str| relative(text, false).map(|span| Self(now() - span));
        if raw.eq_ignore_ascii_case("now") {
            return Ok(Self::now());
        }
        if let Some(rest) = raw.strip_prefix("now-") {
            return ago(rest).ok_or_else(wrong);
        }
        if let Some(when) = ago(raw) {
            return Ok(when);
        }
        if let Ok(instant) = OffsetDateTime::parse(raw, &Rfc3339) {
            return Ok(Self(instant.to_offset(UtcOffset::UTC)));
        }
        Date::parse(raw, format_description!("[year]-[month]-[day]"))
            .map(|day| Self(day.midnight().assume_utc()))
            .map_err(|_| wrong())
    }
}

/// A length of time: `500ms`, `30s`, `15m`, `2h`, `7d`, `1w`. Its kind is
/// `duration`, which a time flag's name (`--since`) may not take.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Span(pub Duration);

impl FromStr for Span {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        relative(raw.trim(), true)
            .map(|span| Self(span.unsigned_abs()))
            .ok_or_else(|| format!("expected a duration: {SPAN_FORMS}; not {raw:?}"))
    }
}

/// `<digits><unit>` as a span; `ms` only when `millis` allows it.
fn relative(raw: &str, millis: bool) -> Option<time::Duration> {
    let split = raw.find(|c: char| !c.is_ascii_digit())?;
    let (count, unit) = raw.split_at(split);
    let count: i64 = count.parse().ok()?;
    let seconds = match unit {
        "ms" if millis => return Some(time::Duration::milliseconds(count)),
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return None,
    };
    Some(time::Duration::seconds(count.checked_mul(seconds)?))
}

/// The clock every command reads. Under the `fixtures` feature,
/// `AGENT_CLI_NOW` (RFC 3339) freezes it, so a recorded world answers the
/// same relative question the same way on any day.
#[must_use]
pub fn now() -> OffsetDateTime {
    #[cfg(feature = "fixtures")]
    if let Some(frozen) = std::env::var("AGENT_CLI_NOW")
        .ok()
        .and_then(|raw| OffsetDateTime::parse(raw.trim(), &Rfc3339).ok())
    {
        return frozen.to_offset(UtcOffset::UTC);
    }
    OffsetDateTime::now_utc()
}

/// An instant as every command prints one: RFC 3339, UTC, whole seconds.
#[must_use]
pub fn utc_time(instant: OffsetDateTime) -> String {
    instant
        .to_offset(UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .unwrap_or_default()
}

/// A service's timestamp as every command prints one: any RFC 3339 form
/// (`+02:00`, seven fractional digits) becomes `2026-09-29T14:00:00Z`.
/// Anything that is not RFC 3339 comes back as it was, rather than dropped.
#[must_use]
pub fn utc(raw: &str) -> String {
    OffsetDateTime::parse(raw.trim(), &Rfc3339).map_or_else(|_| raw.to_owned(), utc_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn when(raw: &str) -> OffsetDateTime {
        raw.parse::<When>().unwrap().0
    }

    #[test]
    fn a_time_is_relative_absolute_or_a_date_and_always_utc() {
        let before = now();
        let hour_ago = when("1h");
        assert!(hour_ago <= before - time::Duration::hours(1) + time::Duration::seconds(5));
        assert!(hour_ago >= before - time::Duration::hours(1) - time::Duration::seconds(5));
        assert_eq!(
            when("now-15m").unix_timestamp() / 10,
            when("15m").unix_timestamp() / 10
        );
        assert!(when("2w") < when("13d"));
        assert_eq!(
            utc_time(when("2026-09-29T16:00:00+02:00")),
            "2026-09-29T14:00:00Z"
        );
        assert_eq!(utc_time(when("2026-09-29")), "2026-09-29T00:00:00Z");
        for bad in [
            "",
            "yesterday",
            "15",
            "3y",
            "now-",
            "now+1h",
            "2026-13-01",
            "-1d",
        ] {
            let error = bad.parse::<When>().unwrap_err();
            assert!(error.starts_with("expected a time: 15m"), "{bad}: {error}");
        }
    }

    #[test]
    fn a_duration_takes_milliseconds_to_weeks() {
        let span = |raw: &str| raw.parse::<Span>().map(|span| span.0);
        assert_eq!(span("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(span("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(span("30m"), Ok(Duration::from_secs(1800)));
        assert_eq!(span("2h"), Ok(Duration::from_secs(7200)));
        assert_eq!(span("7d"), Ok(Duration::from_secs(7 * 86_400)));
        assert_eq!(span("1w"), Ok(Duration::from_secs(7 * 86_400)));
        for bad in ["", "d", "7", "7y", "1.5h", "-1d"] {
            assert!(span(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_service_stamp_prints_in_utc_whole_seconds_and_anything_else_as_it_was() {
        assert_eq!(utc("2026-09-29T10:00:05.1234567Z"), "2026-09-29T10:00:05Z");
        assert_eq!(utc("2026-09-11T15:00:00-05:00"), "2026-09-11T20:00:00Z");
        assert_eq!(utc("9/29/2026"), "9/29/2026");
    }
}
