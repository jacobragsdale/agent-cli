//! One Oracle value as JSON, LOBs included.

use std::io::{Read, Seek, SeekFrom};

use anyhow::{Result, anyhow};
use oracle::SqlValue;
use oracle::io::SeekInChars;
use oracle::sql_type::{Blob, Clob, Lob, Nclob, OracleType, Timestamp};
use serde_json::Value;

use super::{LOB_PREFETCH, complaint};
use crate::db;

/// How much of a LOB is worth holding: a CLOB can be four gigabytes, and the
/// output guard shows 12 KB of it anyway.
const LOB_LIMIT: usize = 1024 * 1024;

/// One value, chosen by the column's declared type rather than by what the
/// driver fetched it as.
pub(super) fn cell(column_type: &OracleType, value: &SqlValue<'_>) -> Result<Value> {
    if value.is_null().unwrap_or(true) {
        return Ok(Value::Null);
    }
    Ok(match column_type {
        // An i64 holds 18 digits; wider, or with a scale, and the server's
        // text is the only lossless form.
        OracleType::Number(precision, 0) if (1..=18).contains(precision) => db::int(get(value)?),
        OracleType::Int64 => db::int(get(value)?),
        OracleType::Number(_, scale) => db::decimal(scaled(get(value)?, *scale)),
        OracleType::Float(_) => db::decimal(scaled(get(value)?, 0)),
        OracleType::BinaryFloat => db::float(db::widen(get(value)?)),
        OracleType::BinaryDouble => db::float(get(value)?),
        // 23ai's BOOLEAN.
        OracleType::Boolean => Value::Bool(get(value)?),
        OracleType::Date
        | OracleType::Timestamp(_)
        | OracleType::TimestampTZ(_)
        | OracleType::TimestampLTZ(_) => Value::String(rfc3339(&get(value)?)),
        OracleType::Raw(_) | OracleType::LongRaw => db::bytes(&get::<Vec<u8>>(value)?),
        OracleType::CLOB => {
            let mut clob = get::<Clob>(value)?;
            let size = size(&clob)?;
            text(lob(
                &mut clob,
                size,
                |clob| clob.seek_in_chars(SeekFrom::Current(0)),
                Some(|clob| clob.seek_in_chars(SeekFrom::Current(2))),
            )?)
        }
        OracleType::NCLOB => {
            let mut nclob = get::<Nclob>(value)?;
            let size = size(&nclob)?;
            text(lob(
                &mut nclob,
                size,
                |nclob| nclob.seek_in_chars(SeekFrom::Current(0)),
                Some(|nclob| nclob.seek_in_chars(SeekFrom::Current(2))),
            )?)
        }
        OracleType::BLOB => {
            let mut blob = get::<Blob>(value)?;
            let size = size(&blob)?;
            db::bytes(&lob(&mut blob, size, Seek::stream_position, None)?.0)
        }
        // CHAR, VARCHAR2, NCHAR, NVARCHAR2, LONG, the intervals, ROWID, XML:
        // the driver's own text of the value.
        _ => Value::String(get(value)?),
    })
}

/// A `NUMBER(p,s)` written with its `s` places, the way SQL Server writes a
/// `decimal`: the driver gives `100.5` for a `NUMBER(12,2)` holding 100.50,
/// and `.5` for a half.
fn scaled(text: String, scale: i8) -> String {
    let (sign, digits) = text
        .strip_prefix('-')
        .map_or(("", text.as_str()), |rest| ("-", rest));
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let whole = if whole.is_empty() { "0" } else { whole };
    let places = usize::try_from(scale).unwrap_or(0).max(fraction.len());
    if places == 0 {
        return format!("{sign}{whole}");
    }
    format!("{sign}{whole}.{fraction:0<places$}")
}

/// RFC 3339 with a `T`, a four-digit year, the column's fractional digits and
/// the offset when it has one: `2024-05-17T13:45:30.123456+02:00`. The
/// driver's own `Display` writes a space and `5` for the year 5.
fn rfc3339(value: &Timestamp) -> String {
    let year = match value.year() {
        year if year < 0 => format!("-{:04}", -year),
        year => format!("{year:04}"),
    };
    let mut text = format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}",
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    );
    let precision = u32::from(value.precision().min(9));
    if precision > 0 {
        let fraction = value.nanosecond() / 10u32.pow(9 - precision);
        text.push_str(&format!(".{fraction:0width$}", width = precision as usize));
    }
    if value.with_tz() {
        let (hours, minutes) = (value.tz_hour_offset(), value.tz_minute_offset());
        let sign = if hours < 0 || minutes < 0 { '-' } else { '+' };
        text.push_str(&format!("{sign}{:02}:{:02}", hours.abs(), minutes.abs()));
    }
    text
}

fn get<T: oracle::sql_type::FromSql>(value: &SqlValue<'_>) -> Result<T> {
    value.get().map_err(|why| anyhow!(complaint(&why)))
}

fn text((mut bytes, truncated): (Vec<u8>, bool)) -> Value {
    if truncated {
        // Cut on a character boundary, so the decode below does not invent a
        // replacement character at the end of every long CLOB.
        while std::str::from_utf8(&bytes).is_err() {
            bytes.pop();
        }
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push('…');
    }
    Value::String(text)
}

/// At most [`LOB_LIMIT`] bytes of a locator, and whether there were more.
/// `at` is how far in the reader is, in the locator's own unit: stopping at
/// the size saves the empty read that would find the end.
///
/// `past_pair`, a CLOB's, steps over the one character no read can get back:
/// when the prefetch ends on the first half of an emoji, every read that
/// touches that half fails with ORA-22831. So the first read stops short of
/// it, and when the next fails that way the character reads `�` rather than
/// the whole result being lost.
fn lob<R: Read>(
    locator: &mut R,
    size: u64,
    at: impl Fn(&mut R) -> std::io::Result<u64>,
    past_pair: Option<fn(&mut R) -> std::io::Result<u64>>,
) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    // A character is at most four bytes, so this holds every one before
    // the prefetch's last.
    let mut room = match past_pair {
        Some(_) => 4 * (LOB_PREFETCH as usize - 1),
        None => chunk.len(),
    };
    let mut stepped = false;
    while bytes.len() <= LOB_LIMIT && at(locator).map_err(broken)? < size {
        match locator.read(&mut chunk[..room]) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(why) if !stepped && code(&why) == Some(22831) && past_pair.is_some() => {
                bytes.extend_from_slice("\u{fffd}".as_bytes());
                past_pair
                    .map_or(Ok(0), |step| step(locator))
                    .map_err(broken)?;
                stepped = true;
            }
            Err(why) => return Err(broken(why)),
        }
        room = chunk.len();
    }
    let truncated = bytes.len() > LOB_LIMIT;
    bytes.truncate(LOB_LIMIT);
    Ok((bytes, truncated))
}

/// The ORA- number inside what a LOB read failed with.
fn code(why: &std::io::Error) -> Option<i32> {
    let inner = why.get_ref()?.downcast_ref::<oracle::Error>()?;
    inner.db_error().map(oracle::DbError::code)
}

fn broken(why: std::io::Error) -> anyhow::Error {
    match why
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<oracle::Error>())
    {
        Some(inner) => anyhow!(complaint(inner)),
        None => anyhow!(why),
    }
}

fn size(locator: &impl Lob) -> Result<u64> {
    locator.size().map_err(|why| anyhow!(complaint(&why)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_rfc_3339_with_a_t_and_its_offset() {
        // A DATE column's timestamps come with precision 0.
        let date = Timestamp::new(2024, 5, 17, 0, 0, 0, 0)
            .unwrap()
            .and_prec(0)
            .unwrap();
        assert_eq!(rfc3339(&date), "2024-05-17T00:00:00");
        let precise = Timestamp::new(2024, 5, 17, 13, 45, 30, 123_456_000)
            .unwrap()
            .and_prec(6)
            .unwrap();
        assert_eq!(rfc3339(&precise), "2024-05-17T13:45:30.123456");
        let zoned = precise.and_tz_hm_offset(2, 0).unwrap();
        assert_eq!(rfc3339(&zoned), "2024-05-17T13:45:30.123456+02:00");
        let west = date.and_tz_hm_offset(-5, -30).unwrap();
        assert_eq!(rfc3339(&west), "2024-05-17T00:00:00-05:30");
        let ancient = Timestamp::new(5, 3, 4, 0, 0, 0, 0)
            .unwrap()
            .and_prec(0)
            .unwrap();
        assert_eq!(rfc3339(&ancient), "0005-03-04T00:00:00");
    }

    #[test]
    fn a_number_keeps_the_scale_its_column_declares() {
        assert_eq!(scaled("100.5".into(), 2), "100.50");
        assert_eq!(scaled("100".into(), 2), "100.00");
        assert_eq!(scaled(".5".into(), 0), "0.5");
        assert_eq!(scaled("-.5".into(), 2), "-0.50");
        assert_eq!(scaled("12345.6789".into(), 0), "12345.6789");
        assert_eq!(scaled("7".into(), 0), "7");
        assert_eq!(scaled("1.23".into(), 1), "1.23", "never drops digits");
        assert_eq!(scaled("1200".into(), -2), "1200");
    }

    fn read(data: &[u8]) -> (Vec<u8>, bool) {
        let size = data.len() as u64;
        lob(
            &mut std::io::Cursor::new(data),
            size,
            Seek::stream_position,
            None,
        )
        .unwrap()
    }

    #[test]
    fn a_lob_stops_at_the_ceiling_and_a_cut_clob_ends_between_characters() {
        assert_eq!(read(b"short"), (b"short".to_vec(), false));
        let long = vec![0xff; LOB_LIMIT * 2];
        let (bytes, truncated) = read(&long);
        assert_eq!((bytes.len(), truncated), (LOB_LIMIT, true));

        let mut source = vec![b'x'; LOB_LIMIT - 1];
        source.extend_from_slice("é".as_bytes());
        source.extend_from_slice(&[b'y'; 10]);
        let Value::String(text) = text(read(&source)) else {
            panic!("a CLOB is text");
        };
        assert!(text.ends_with('…'));
        assert_eq!(text.len(), LOB_LIMIT - 1 + '…'.len_utf8());
    }

    /// A CLOB whose prefetch ended on the first half of a pair: a read that
    /// starts on it fails with ORA-22831.
    struct Straddle {
        data: Vec<u8>,
        at: usize,
        half: usize,
    }

    impl Read for Straddle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.at == self.half {
                #[allow(deprecated)]
                let why = oracle::Error::OciError(oracle::DbError::new(
                    22831,
                    0,
                    "ORA-22831: Offset or offset+amount does not land on character boundary",
                    "",
                    "",
                ));
                return Err(std::io::Error::other(why));
            }
            let end = if self.at < self.half {
                self.half
            } else {
                self.data.len()
            };
            let read = buf.len().min(end - self.at);
            buf[..read].copy_from_slice(&self.data[self.at..self.at + read]);
            self.at += read;
            Ok(read)
        }
    }

    #[test]
    fn an_emoji_the_prefetch_cut_in_half_is_one_replacement_and_not_a_lost_result() {
        let mut clob = Straddle {
            data: b"aaaaXXtail".to_vec(),
            at: 0,
            half: 4,
        };
        let (bytes, truncated) = lob(
            &mut clob,
            10,
            |clob| Ok(clob.at as u64),
            Some(|clob| {
                clob.at += 2;
                Ok(clob.at as u64)
            }),
        )
        .unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "aaaa\u{fffd}tail");
        assert!(!truncated);
    }
}
