//! Oracle, over the `oracle` crate and the Instant Client it loads at run
//! time.
//!
//! ODPI-C `dlopen`s the client the first time a connection opens, so the
//! binary starts, and every other command works, on a machine without it.
//!
//! The deadline is the connection's call timeout, renewed before each round
//! trip, so the client itself breaks a call still running at the deadline
//! and resets the session. Each call also runs on a thread of its own that
//! the command waits on no longer than [`RESET_WAIT`] past the deadline
//! ([`db::bounded`]), and a signal breaks the call and waits for that up to
//! [`BREAK_WAIT`] ([`agent_cli_core::on_stop`]): a server waiting on a lock
//! or asleep in PL/SQL may not hear a break until the wait ends.
//!
//! Autocommit is off and every statement that may write is committed after
//! it runs: the same as autocommit for DML and DDL, and it lets a `select …
//! for update` read past its first fetch (an autocommit execute would end
//! the cursor, ORA-01002) before its commit lets the locks go.

mod cell;

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use agent_cli_core::{Exit, Failure, OnStop, Secret, on_stop};
use anyhow::{Result, anyhow};
use oracle::oci_attr::DefaultLobPrefetchSize;
use oracle::{Connection as Driver, InitParams};

use crate::config::{Connection, Kind};
use crate::db::{self, Fetch, Ran, ResultSet};
use crate::split;
use cell::cell;

/// How much of each LOB rides along with its row, in the LOB's own unit.
/// Oracle reserves this per row of the fetch array, so it stays small.
const LOB_PREFETCH: u32 = 8 * 1024;

/// Rows per round trip.
const FETCH_ARRAY: u32 = 500;

/// Long enough for a container still waking up; Easy Connect carries it,
/// because ODPI-C has no connect timeout of its own.
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// How long past the deadline the call timeout's break and reset get: a
/// round trip.
const RESET_WAIT: Duration = Duration::from_millis(200);

/// How long a signal's break gets to reach the server and come back.
const BREAK_WAIT: Duration = Duration::from_secs(1);

pub struct Session {
    driver: Arc<Driver>,
    /// Held while a call runs, so a break can tell when it has landed.
    busy: Arc<Mutex<()>>,
    _stop: OnStop,
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
        let driver = Arc::new(driver);
        let busy = Arc::new(Mutex::new(()));
        let (breaker, idle) = (Arc::clone(&driver), Arc::clone(&busy));
        let stop = on_stop(format!("the session on {}", spec.name), move || {
            interrupt(&breaker, &idle);
            // Logs off when the break landed; otherwise the exit drops it.
            let _ = breaker.close();
        });
        Ok(Self {
            driver,
            busy,
            _stop: stop,
        })
    }

    pub fn run(&mut self, sql: &str, fetch: Fetch, deadline: Instant) -> Result<Ran> {
        let (driver, busy) = (Arc::clone(&self.driver), Arc::clone(&self.busy));
        let owned = sql.to_owned();
        let ran = db::bounded(deadline + RESET_WAIT, move || {
            let _busy = busy.lock().unwrap_or_else(PoisonError::into_inner);
            execute(&driver, &owned, fetch, deadline)
        });
        match ran {
            // The server did not answer the call timeout's break; one more,
            // not waited for, as the deadline has passed.
            Err(timed_out) => {
                let driver = Arc::clone(&self.driver);
                std::thread::spawn(move || driver.break_execution());
                Err(timed_out)
            }
            // DPI-1067 from the call timeout (its timer can fire a little
            // early), or the ORA-01013 a break ends a call with.
            Ok(Err(error))
                if Instant::now() >= deadline || error.to_string().contains("DPI-1067") =>
            {
                Err(db::timed_out())
            }
            Ok(ran) => ran,
        }
    }
}

/// Breaks the call running on `driver`, if one is, and waits at most
/// [`BREAK_WAIT`] for it to end.
// ponytail: a server that cannot hear a break while it waits (DISABLE_OOB
// in its sqlnet.ora, or a network that drops TCP urgent data) sits out a
// lock wait or a PL/SQL sleep after the process is gone; what it does next
// fails on the closed socket and is rolled back, but a COMMIT later in the
// same PL/SQL block is not. `alter system cancel sql` from a second session
// would stop it, for a login with ALTER SYSTEM.
fn interrupt(driver: &Driver, busy: &Mutex<()>) {
    if busy.try_lock().is_ok() {
        return;
    }
    let _ = driver.break_execution();
    let until = Instant::now() + BREAK_WAIT;
    while Instant::now() < until && busy.try_lock().is_err() {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Exit 124 past the deadline; before it, the call timeout set to what is
/// left, so the client breaks the next round trip there.
fn budget(driver: &Driver, deadline: Instant) -> Result<()> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(db::timed_out());
    }
    // Whole milliseconds, rounded up, so it never fires before the deadline.
    let millis = Duration::from_millis(u64::try_from(left.as_millis()).unwrap_or(u64::MAX) + 1);
    driver
        .set_call_timeout(Some(millis))
        .map_err(|why| anyhow!(complaint(&why)))
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
fn execute(driver: &Driver, sql: &str, fetch: Fetch, deadline: Instant) -> Result<Ran> {
    let fail = |why: oracle::Error| failure(&why, sql);
    let mut ran = Ran::default();
    budget(driver, deadline)?;
    // A read that ends at the cap asks the server for no more rows than
    // that: on a costly plan each one is work.
    let array = if fetch.stop {
        u32::try_from(fetch.keep.saturating_add(1))
            .map_or(FETCH_ARRAY, |rows| rows.min(FETCH_ARRAY))
    } else {
        FETCH_ARRAY
    };
    let mut statement = driver
        .statement(sql)
        .fetch_array_size(array)
        .prefetch_rows(array)
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
            let more = ran.keep(fetch, || {
                row.column_info()
                    .iter()
                    .zip(row.sql_values())
                    .map(|(info, value)| cell(info.oracle_type(), value))
                    .collect()
            })?;
            if !more {
                break;
            }
            // The next fetch, or this row's LOB reads, is a round trip.
            budget(driver, deadline)?;
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
        budget(driver, deadline)?;
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
}
