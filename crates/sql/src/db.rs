//! One database connection for the length of one command, the op every sql
//! command performs through `Ctx`, and how a value becomes JSON.
//!
//! Both drivers report in [`ResultSet`]s whose cells are already JSON, so the
//! catalog and the commands above never know which vendor answered.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Failure, Op};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Number, Value};

use crate::config::{Connection, Kind};
use crate::{mssql, oracle};

/// One result set, or one statement's count when it returned none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, JsonSchema)]
pub struct ResultSet {
    pub columns: Vec<String>,
    /// As the server names them: `nvarchar`, `decimal`, `NUMBER(10,2)`.
    pub types: Vec<String>,
    /// Positional, in `columns` order. NULL is null; integers past 2^53,
    /// decimals and money are strings that keep their scale; binary is
    /// `0x…`; dates and times are RFC 3339 (`2024-05-17T13:45:30.123`).
    pub rows: Vec<Vec<Value>>,
    /// Rows a statement changed when it returned no result set; 0 for a
    /// result set. SQL Server does not say what a batch that also returned
    /// rows changed.
    pub rows_affected: u64,
    /// The server had more rows than `--max-rows`.
    pub truncated: bool,
}

/// What one statement said.
#[derive(Debug, Default)]
pub struct Ran {
    pub sets: Vec<ResultSet>,
    /// Every row the server sent, kept or not.
    pub rows: u64,
    pub first_row: Option<Instant>,
}

/// How much of a result to keep. `stop` ends the read at the first row past
/// `keep`, which costs a SQL Server session (see [`mssql`]); otherwise the
/// rest is read and counted but not kept.
#[derive(Clone, Copy, Debug)]
pub struct Fetch {
    pub keep: usize,
    pub stop: bool,
}

impl Fetch {
    pub const ALL: Self = Self {
        keep: usize::MAX,
        stop: false,
    };
}

/// An open connection.
pub struct Session {
    pub kind: Kind,
    /// What opening it cost, for `query bench`.
    pub connect: Duration,
    driver: Driver,
}

// One per command, so tiberius' kilobyte of config costs nothing.
#[allow(clippy::large_enum_variant)]
enum Driver {
    Mssql(mssql::Session),
    Oracle(oracle::Session),
}

impl Session {
    pub fn open(
        spec: &Connection,
        password: &agent_cli_core::Secret,
        client_dir: Option<&Path>,
        deadline: Instant,
    ) -> Result<Self> {
        let started = Instant::now();
        let driver = match spec.kind {
            Kind::Mssql => Driver::Mssql(mssql::Session::open(spec, password, deadline)?),
            Kind::Oracle => {
                Driver::Oracle(oracle::Session::open(spec, password, client_dir, deadline)?)
            }
        };
        Ok(Self {
            kind: spec.kind,
            connect: started.elapsed(),
            driver,
        })
    }

    /// Runs one statement (a whole batch on SQL Server), stopping at
    /// `deadline` with exit 124.
    pub fn run(&mut self, sql: &str, fetch: Fetch, deadline: Instant) -> Result<Ran> {
        match &mut self.driver {
            Driver::Mssql(session) => session.run(sql, fetch, deadline),
            Driver::Oracle(session) => session.run(sql, fetch, deadline),
        }
    }
}

/// Work on one connection, as an op: a batch, a bench, a catalog read. The
/// connection (and any `password_cmd`) opens inside `perform`, so a refusal
/// or `--dry-run` happens before anything connects.
pub struct OnConnection<'a, F> {
    pub spec: &'a Connection,
    pub client_dir: Option<&'a Path>,
    pub deadline: Instant,
    /// What `--dry-run` prints.
    pub plan: Value,
    pub writes: bool,
    pub work: F,
}

impl<T, F: FnOnce(&mut Session) -> Result<T>> Op for OnConnection<'_, F> {
    type Output = T;

    fn plan(&self) -> Value {
        self.plan.clone()
    }

    fn writes(&self) -> bool {
        self.writes
    }

    fn perform(self, _: &Ctx) -> Result<T> {
        let password = self.spec.password(self.deadline)?;
        let mut session = Session::open(self.spec, &password, self.client_dir, self.deadline)?;
        (self.work)(&mut session)
    }
}

/// Exit 124 for a statement still running at the deadline.
pub fn timed_out() -> anyhow::Error {
    Failure::timed_out(
        "the statement was still running at the --timeout deadline and was cancelled",
    )
    .hint("run it again with a larger --timeout, or narrow it (a where clause, --max-rows)")
    .into()
}

/// True when `error` is the deadline running out.
pub fn is_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<Failure>()
            .is_some_and(|failure| failure.exit == agent_cli_core::Exit::TimedOut)
    })
}

/// Runs `work` on a thread of its own and waits for it until `deadline`.
/// For a driver that can block where no cancel reaches (Oracle asleep in
/// PL/SQL, or on a network that stopped): the deadline holds anyway, and the
/// thread is left to the process exit.
pub fn bounded<T: Send + 'static>(
    deadline: Instant,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T> {
    let (done, result) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(work());
    });
    match result.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(value) => Ok(value),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(timed_out()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
            "the database driver stopped without an answer"
        )),
    }
}

/// Integers a JavaScript reader holds exactly; past 2^53 it would round.
const EXACT: u64 = 1 << 53;

/// A number, or its digits as a string past 2^53.
pub fn int(value: i64) -> Value {
    if value.unsigned_abs() <= EXACT {
        Value::from(value)
    } else {
        Value::String(value.to_string())
    }
}

/// A decimal's exact text as a string, scale and all, so no reader rounds it
/// or drops its trailing zeros. The one exception, kept from sql-bench: a
/// whole number within 2^53, written canonically (`42`, not `+42` or
/// `042`), is a number, because it is one (Oracle's `count(*)`, a `NUMBER`
/// id).
pub fn decimal(text: String) -> Value {
    match text.parse::<i64>() {
        Ok(whole) if whole.unsigned_abs() <= EXACT && whole.to_string() == text => {
            Value::from(whole)
        }
        _ => Value::String(text),
    }
}

/// JSON has no NaN or infinity; null is the one thing left.
pub fn float(value: f64) -> Value {
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

pub fn bytes(bytes: &[u8]) -> Value {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    Value::String(text)
}

/// An `f32` as the `f64` that prints the same: `real` 0.1 is 0.1, not
/// 0.10000000149011612.
pub fn widen(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::from(value))
}

/// A cell as text, for the catalog: strings as they are, numbers written out.
pub fn text(row: &[Value], index: usize) -> String {
    match row.get(index) {
        Some(Value::String(text)) => text.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => other.to_string(),
    }
}

/// A cell that may be NULL, as text.
pub fn maybe(row: &[Value], index: usize) -> Option<String> {
    match row.get(index) {
        None | Some(Value::Null) => None,
        Some(_) => Some(text(row, index)),
    }
}

/// A cell as a whole number, 0 when it is not one.
pub fn whole(row: &[Value], index: usize) -> i64 {
    text(row, index).parse().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_value_becomes_the_json_a_reader_cannot_misread() {
        assert_eq!(int(42), json!(42));
        assert_eq!(int(-(1 << 53)), json!(-9_007_199_254_740_992_i64));
        assert_eq!(int(i64::MAX), json!("9223372036854775807"));
        assert_eq!(decimal("10.2500".into()), json!("10.2500"), "scale kept");
        assert_eq!(
            decimal("42".into()),
            json!(42),
            "a whole decimal is a number"
        );
        assert_eq!(decimal("-7".into()), json!(-7));
        for text in ["+5", "007", "-0", "9007199254740993", "1e3"] {
            assert_eq!(decimal(text.into()), json!(text), "{text} stays text");
        }
        assert_eq!(float(1.25), json!(1.25));
        assert_eq!(float(f64::NAN), Value::Null);
        assert_eq!(float(f64::INFINITY), Value::Null);
        assert_eq!(float(widen(0.1)), json!(0.1));
        assert_eq!(bytes(&[0, 15, 255]), json!("0x000fff"));
        assert_eq!(bytes(&[]), json!("0x"));
    }

    #[test]
    fn the_catalog_reads_cells_as_text_and_numbers() {
        let row = vec![json!("dbo"), json!(200), Value::Null, json!("12")];
        assert_eq!(text(&row, 0), "dbo");
        assert_eq!(text(&row, 1), "200");
        assert_eq!((text(&row, 2), maybe(&row, 2)), (String::new(), None));
        assert_eq!(
            (whole(&row, 1), whole(&row, 3), whole(&row, 9)),
            (200, 12, 0)
        );
    }

    #[test]
    fn bounded_work_gives_up_at_the_deadline() {
        let started = Instant::now();
        let error = bounded(started + Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(5));
        })
        .unwrap_err();
        assert!(is_timeout(&error));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            bounded(Instant::now() + Duration::from_secs(5), || 7).unwrap(),
            7
        );
    }
}
