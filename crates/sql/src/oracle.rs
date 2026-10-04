//! Oracle, over the `oracle` crate and the Instant Client it loads at run
//! time.
//!
//! ODPI-C `dlopen`s the client the first time a connection opens, so the
//! binary starts, and every other command works, on a machine without it.
//!
//! The driver blocks, so each call runs on a thread of its own that the
//! command waits on no later than the deadline ([`db::bounded`]). At the
//! deadline an OCI break stops the server's side, and the command answers
//! at once rather than waiting for a break that a PL/SQL sleep would ignore.
//!
//! Autocommit is off and every statement that may write is committed after
//! it runs: the same as autocommit for DML and DDL, and it lets a `select …
//! for update` read past its first fetch (an autocommit execute would end
//! the cursor, ORA-01002) before its commit lets the locks go.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use agent_cli_core::{Exit, Failure, Secret};
use anyhow::{Result, anyhow};
use oracle::io::SeekInChars;
use oracle::oci_attr::DefaultLobPrefetchSize;
use oracle::sql_type::{Blob, Clob, Lob, Nclob, OracleType, Timestamp};
use oracle::{Connection as Driver, InitParams, SqlValue};
use serde_json::Value;

use crate::config::{Connection, Kind};
use crate::db::{self, Fetch, Ran, ResultSet};
use crate::split;

/// How much of a LOB is worth holding: a CLOB can be four gigabytes, and the
/// output guard shows 12 KB of it anyway.
const LOB_LIMIT: usize = 1024 * 1024;

/// How much of each LOB rides along with its row, in the LOB's own unit.
/// Oracle reserves this per row of the fetch array, so it stays small.
const LOB_PREFETCH: u32 = 8 * 1024;

/// Rows per round trip.
const FETCH_ARRAY: u32 = 500;

/// Long enough for a container still waking up; Easy Connect carries it,
/// because ODPI-C has no connect timeout of its own.
const CONNECT_TIMEOUT_SECS: u64 = 10;

pub struct Session {
    driver: Arc<Driver>,
}

impl Session {
    pub fn open(
        spec: &Connection,
        password: &Secret,
        client_dir: Option<&Path>,
        deadline: Instant,
    ) -> Result<Self> {
        init(client_dir)?;
        let seconds = deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .clamp(1, CONNECT_TIMEOUT_SECS);
        // `::1` would read as a host and two ports; Easy Connect brackets it.
        let host = if spec.host.contains(':') && !spec.host.starts_with('[') {
            format!("[{}]", spec.host)
        } else {
            spec.host.clone()
        };
        let connect_string = format!(
            "//{host}:{}/{}?connect_timeout={seconds}",
            spec.port,
            spec.service.as_deref().unwrap_or_default()
        );
        let (user, password) = (spec.user.clone(), password.clone());
        let mut driver = db::bounded(deadline, move || {
            Driver::connect(&user, password.expose(), &connect_string)
        })?
        .map_err(|why| {
            // ORA-01017 is a refused login, which is setup (exit 3).
            let refused = complaint(&why).contains("ORA-01017");
            Failure::new(
                if refused { Exit::Setup } else { Exit::Failed },
                format!(
                    "cannot connect to {}:{}/{}: {}",
                    spec.host,
                    spec.port,
                    spec.service.as_deref().unwrap_or_default(),
                    complaint(&why)
                ),
            )
            .hint("check the connection in [sql]; `agent-cli doctor sql` tries every one")
        })?;
        driver.set_autocommit(false);
        // Only speed rides on this: a client that refuses it still reads
        // every LOB, a round trip or two slower.
        let _ = driver.set_oci_attr::<DefaultLobPrefetchSize>(&LOB_PREFETCH);
        Ok(Self {
            driver: Arc::new(driver),
        })
    }

    pub fn run(&mut self, sql: &str, fetch: Fetch, deadline: Instant) -> Result<Ran> {
        let driver = Arc::clone(&self.driver);
        let owned = sql.to_owned();
        let ran = db::bounded(deadline, move || execute(&driver, &owned, fetch));
        if ran.is_err() {
            // Best effort, and not waited for: the answer is already given.
            let driver = Arc::clone(&self.driver);
            std::thread::spawn(move || driver.break_execution());
        }
        ran?
    }
}

/// ODPI-C loads the client once per process, so the first connection
/// decides where from, and every later one lives with that, failure too.
pub fn init(dir: Option<&Path>) -> Result<()> {
    static CLIENT: OnceLock<Result<(), String>> = OnceLock::new();
    CLIENT.get_or_init(|| load(dir)).clone().map_err(|message| {
        Failure::setup(message)
            .hint(
                "set [sql] oracle_client_dir or AGENT_CLI_SQL_ORACLE_CLIENT_DIR to the \
                     Instant Client directory (on Linux it also needs libaio); \
                     `agent-cli config example sql` says more",
            )
            .into()
    })
}

fn load(dir: Option<&Path>) -> Result<(), String> {
    let mut params = InitParams::new();
    let looked = dir.map_or_else(String::new, |dir| format!(" in {}", dir.display()));
    let no_client = |why: oracle::Error| {
        // ODPI-C's own text names a documentation URL; the quoted part is
        // the loader's reason (a missing libaio, say), which is worth keeping.
        let text = why.to_string();
        let reason = text
            .split('"')
            .nth(1)
            .map_or_else(String::new, |reason| format!(" ({reason})"));
        format!("Oracle client library not found{looked}{reason}")
    };
    if let Some(dir) = dir {
        params.oracle_client_lib_dir(dir).map_err(no_client)?;
    }
    params.init().map_err(no_client)?;
    Ok(())
}

/// Runs one statement: a query with its rows, or a statement with its count.
fn execute(driver: &Driver, sql: &str, fetch: Fetch) -> Result<Ran> {
    let fail = |why: oracle::Error| failure(&why, sql);
    let mut ran = Ran::default();
    let mut statement = driver
        .statement(sql)
        .fetch_array_size(FETCH_ARRAY)
        .prefetch_rows(FETCH_ARRAY)
        // A locator keeps a `select *` over gigabyte CLOBs from being copied
        // whole.
        .lob_locator()
        .build()
        .map_err(fail)?;
    if statement.is_query() {
        let rows = statement.query(&[]).map_err(fail)?;
        let info = rows.column_info();
        ran.sets.push(ResultSet {
            columns: info.iter().map(|column| column.name().to_owned()).collect(),
            types: info
                .iter()
                .map(|column| column.oracle_type().to_string())
                .collect(),
            ..ResultSet::default()
        });
        for row in rows {
            let row = row.map_err(fail)?;
            ran.rows += 1;
            ran.first_row.get_or_insert_with(Instant::now);
            let set = &mut ran.sets[0];
            if set.rows.len() >= fetch.keep {
                set.truncated = true;
                if fetch.stop {
                    break;
                }
                continue;
            }
            let mut cells = Vec::with_capacity(row.sql_values().len());
            for (info, value) in row.column_info().iter().zip(row.sql_values()) {
                cells.push(cell(info.oracle_type(), value)?);
            }
            set.rows.push(cells);
        }
    } else {
        statement.execute(&[]).map_err(fail)?;
        // A stored program that does not compile is still created, INVALID,
        // with a warning; that is not success to anyone who asked for it.
        if driver.last_warning().is_some()
            && let Some(error) = compile_error(driver, sql)
        {
            return Err(error);
        }
        // OCI answers 1 for any PL/SQL block ("one block ran"); what a block
        // changed is its own business.
        let affected = if statement.is_plsql() {
            0
        } else {
            statement.row_count().unwrap_or(0)
        };
        ran.sets.push(ResultSet {
            rows_affected: affected,
            ..ResultSet::default()
        });
    }
    if split::writes(sql, Kind::Oracle) {
        driver.commit().map_err(fail)?;
    }
    Ok(ran)
}

/// What a `CREATE` of stored PL/SQL makes: its type as `ALL_ERRORS` spells
/// it, where in `sql` that keyword starts, and the word that names it.
fn created(sql: &str) -> Option<(&'static str, usize, &str)> {
    let mut words = code_start(sql).split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("create") {
        return None;
    }
    let mut word = words.next()?;
    if word.eq_ignore_ascii_case("or") {
        words.next()?; // replace
        word = words.next()?;
    }
    if word.eq_ignore_ascii_case("editionable") || word.eq_ignore_ascii_case("noneditionable") {
        word = words.next()?;
    }
    let (kind, body) = match word.to_ascii_lowercase().as_str() {
        "procedure" => ("PROCEDURE", ""),
        "function" => ("FUNCTION", ""),
        "trigger" => ("TRIGGER", ""),
        "package" => ("PACKAGE", "PACKAGE BODY"),
        "type" => ("TYPE", "TYPE BODY"),
        _ => return None,
    };
    // A slice of `sql`, so its address is its offset.
    let at = word.as_ptr() as usize - sql.as_ptr() as usize;
    let name = words.next()?;
    if !body.is_empty() && name.eq_ignore_ascii_case("body") {
        return Some((body, at, words.next()?));
    }
    Some((kind, at, name))
}

/// `sql` from its first word of code, past whitespace and comments.
fn code_start(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        rest = if let Some(comment) = rest.strip_prefix("--") {
            comment.split_once('\n').map_or("", |(_, after)| after)
        } else if let Some(comment) = rest.strip_prefix("/*") {
            comment.split_once("*/").map_or("", |(_, after)| after)
        } else {
            return rest;
        }
        .trim_start();
    }
}

/// The first error `ALL_ERRORS` holds for the program `sql` just created,
/// on the line of `sql` it is on.
fn compile_error(driver: &Driver, sql: &str) -> Option<anyhow::Error> {
    let (kind, at, word) = created(sql)?;
    // `bench.p(a number)`, `"Bench"."P"`: unquoted parts fold to upper case.
    let name = word.split('(').next().unwrap_or(word);
    let mut parts = name.split('.').map(|part| match part.strip_prefix('"') {
        Some(quoted) => quoted.trim_end_matches('"').to_owned(),
        None => part.to_uppercase(),
    });
    let (owner, name) = match (parts.next(), parts.next()) {
        (Some(owner), Some(name)) => (Some(owner), name),
        (Some(name), None) => (None, name),
        _ => return None,
    };
    // Not a bound NULL and `coalesce`: a NULL bind is NVARCHAR2 to OCI and
    // `sys_context` is not (ORA-12704).
    let owner = owner.map_or_else(
        || "sys_context('USERENV', 'CURRENT_SCHEMA')".to_owned(),
        |owner| format!("'{}'", owner.replace('\'', "''")),
    );
    let errors = driver
        .query_as::<(u32, String)>(
            &format!(
                "select line, text from all_errors \
                 where owner = {owner} and name = :1 and type = :2 and attribute = 'ERROR' \
                 order by sequence"
            ),
            &[&name, &kind],
        )
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let (line, text) = errors.first()?;
    // `ALL_ERRORS` counts from the line the type keyword is on.
    let before = u32::try_from(sql[..at].matches('\n').count()).unwrap_or(0);
    let more = match errors.len() {
        1 => String::new(),
        n => format!(" (and {} more)", n - 1),
    };
    // Oracle keeps the program, INVALID: say so, and what to do about it.
    Some(
        Failure::new(
            Exit::Failed,
            format!(
                "line {}: {}{more}; {} {name} now exists INVALID",
                line + before,
                text.trim_end(),
                kind.to_lowercase()
            ),
        )
        .hint("fix it and run the CREATE OR REPLACE again, or drop it")
        .into(),
    )
}

/// One value, chosen by the column's declared type rather than by what the
/// driver fetched it as.
fn cell(column_type: &OracleType, value: &SqlValue<'_>) -> Result<Value> {
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

/// What the server said, on the line of the statement where it stopped
/// parsing. An error with no such place (a block that raised) has offset 0,
/// and PL/SQL's own `ORA-06512: at line 3` is in the message instead.
fn failure(why: &oracle::Error, sql: &str) -> anyhow::Error {
    let line = why
        .db_error()
        .map(|db| db.offset() as usize)
        .filter(|offset| *offset > 0)
        .map(|offset| line_of(sql, offset));
    let message = match line {
        Some(line) => format!("line {line}: {}", complaint(why)),
        None => complaint(why),
    };
    // ORA-00942, table or view does not exist.
    if why.db_error().is_some_and(|db| db.code() == 942) {
        return anyhow::Error::new(crate::db::UnknownObject {
            name: crate::db::UnknownObject::quoted(&complaint(why)),
            message,
        });
    }
    anyhow!(message)
}

/// ODPI-C counts the offset in bytes.
fn line_of(sql: &str, offset: usize) -> usize {
    let before = sql.as_bytes().get(..offset).unwrap_or(sql.as_bytes());
    before.iter().filter(|byte| **byte == b'\n').count() + 1
}

/// `ORA-00933: …` rather than the crate's `OCI Error: ORA-00933: …`.
fn complaint(why: &oracle::Error) -> String {
    match why.db_error() {
        // A string the driver fetches XMLTYPE into holds 4,000 bytes.
        Some(db) if db.code() == 19011 => format!(
            "{} An XMLTYPE past 4,000 bytes reads as xmlserialize(document … as clob).",
            unhelped(db.message())
        ),
        Some(db) => unhelped(db.message()),
        None => unreadable(why.to_string()),
    }
}

/// A column the driver cannot fetch fails the whole query, so what to
/// select instead is the half of the message worth reading.
fn unreadable(message: String) -> String {
    let (what, instead) = if message.ends_with("Oracle type JSON") {
        ("JSON", "json_serialize(… returning clob)")
    } else if message.ends_with("Oracle type number 2033") {
        ("VECTOR", "vector_serialize(… returning clob)")
    } else if message.starts_with("unknown Oracle type number") {
        ("REF or other", "reftohex(…) for a REF, or any cast to text")
    } else {
        return message;
    };
    format!("a {what} column the driver cannot read ({message}): select {instead} instead")
}

/// 23ai ends every message with a `Help: https://…` line.
fn unhelped(message: &str) -> String {
    message
        .lines()
        .filter(|line| !line.starts_with("Help: http"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
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

    #[test]
    fn a_create_says_what_it_made_and_where_the_keyword_is() {
        let sql = "create or replace\n  package body bench.pk as end;";
        assert_eq!(created(sql), Some(("PACKAGE BODY", 20, "bench.pk")));
        assert_eq!(
            created("-- mine\ncreate procedure p is begin nope; end;"),
            Some(("PROCEDURE", 15, "p"))
        );
        assert_eq!(created("create package"), None);
        assert_eq!(created("select 1 from dual"), None);
    }

    #[test]
    fn the_line_is_counted_from_the_byte_offset_the_server_reports() {
        let wide = "select '李李李李李李' a,\n  nope b,\n  1 c\nfrom dual";
        assert_eq!(line_of(wide, wide.find("nope").unwrap()), 2);
        assert_eq!(line_of(wide, 10_000), 4, "past the end is the last line");
    }

    #[test]
    fn a_driver_message_loses_its_help_link_and_says_what_to_select_instead() {
        assert_eq!(
            unhelped("ORA-01476: divisor is equal to zero\nHelp: https://docs.oracle.com/x"),
            "ORA-01476: divisor is equal to zero"
        );
        assert!(unreadable("unsupported Oracle type JSON".to_owned()).contains("json_serialize"));
        assert_eq!(unreadable("ORA-1".to_owned()), "ORA-1");
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
