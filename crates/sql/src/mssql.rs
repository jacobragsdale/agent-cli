//! SQL Server, over tiberius.
//!
//! The only async code in agent-cli: a current-thread runtime made when a
//! SQL Server connection opens, and blocked on. Every call races the
//! deadline, and losing that race drops the socket, which is the only cancel
//! TDS gives tiberius.
//!
//! Stopping at `--max-rows` drops the socket too, which ends the session:
//! the server abandons the rest of the batch and rolls back an open
//! transaction. So a batch that may write is read to its end instead (rows
//! past the cap counted, not kept), and only a read is cut short. A read
//! batch changes no session state, so the next batch simply reconnects.

use std::panic::AssertUnwindSafe;
use std::sync::Once;
use std::time::{Duration, Instant};

use agent_cli_core::{Exit, Failure, Secret};
use anyhow::{Result, anyhow};
use futures_util::TryStreamExt as _;
use serde_json::Value;
use tiberius::error::Error as TiberiusError;
use tiberius::{AuthMethod, Client, ColumnData, ColumnType, EncryptionLevel, QueryItem};
use time::Date as CalendarDate;
use tokio::net::TcpStream;
use tokio::runtime::{Builder, Runtime};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt as _};

use crate::config::Connection;
use crate::db::{self, Fetch, Ran, ResultSet};

/// Long enough for a container still waking up, short enough that a wrong
/// host does not look like a hang.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type Driver = Client<Compat<TcpStream>>;

pub struct Session {
    config: tiberius::Config,
    /// An `Option` only so `Drop` can shut it down without waiting on a DNS
    /// lookup that the deadline gave up on.
    runtime: Option<Runtime>,
    /// `None` after a read was cut short: the next statement reconnects.
    client: Option<Driver>,
}

impl Session {
    pub fn open(spec: &Connection, password: &Secret, deadline: Instant) -> Result<Self> {
        quiet_driver_panics();
        let mut config = tiberius::Config::new();
        config.host(&spec.host);
        config.port(spec.port);
        if let Some(database) = &spec.database {
            config.database(database);
        }
        config.authentication(AuthMethod::sql_server(&spec.user, password.expose()));
        config.encryption(if spec.encrypt {
            EncryptionLevel::Required
        } else {
            EncryptionLevel::NotSupported
        });
        if spec.trust_cert {
            config.trust_cert();
        }
        config.application_name("agent-cli");
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let mut session = Self {
            config,
            runtime: Some(runtime),
            client: None,
        };
        session.connect(deadline)?;
        Ok(session)
    }

    pub fn run(&mut self, sql: &str, fetch: Fetch, deadline: Instant) -> Result<Ran> {
        if self.client.is_none() {
            self.connect(deadline)?;
        }
        let (Some(runtime), Some(client)) = (&self.runtime, &mut self.client) else {
            unreachable!("a connect leaves a client and the runtime is only taken by drop");
        };
        // The timer is made inside the runtime, which is the only place one
        // can be.
        let raced = std::panic::catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(async {
                tokio::time::timeout_at(deadline.into(), stream(client, sql, fetch)).await
            })
        }));
        match raced {
            Ok(Ok(Ok((ran, stopped)))) => {
                if stopped {
                    self.client = None;
                }
                Ok(ran)
            }
            // The command stops at the first failure, so the session is not
            // worth keeping whatever the failure was.
            Ok(Ok(Err(error))) => {
                self.client = None;
                Err(error)
            }
            Ok(Err(_elapsed)) => {
                self.client = None;
                Err(db::timed_out())
            }
            Err(panic) => {
                self.client = None;
                let why = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("it panicked");
                Err(anyhow!(
                    "a column the driver cannot read ({why}); cast it in the query to a type it \
                     can, such as nvarchar"
                ))
            }
        }
    }

    fn connect(&mut self, deadline: Instant) -> Result<()> {
        let limit = deadline.min(Instant::now() + CONNECT_TIMEOUT);
        let Some(runtime) = &self.runtime else {
            unreachable!("the runtime is only taken by drop");
        };
        let opened = runtime.block_on(async {
            tokio::time::timeout_at(limit.into(), open_driver(&self.config)).await
        });
        match opened {
            Ok(driver) => {
                self.client = Some(driver?);
                Ok(())
            }
            Err(_) if limit == deadline => Err(db::timed_out()),
            Err(_) => Err(cannot_connect(
                &self.config,
                &format!("no answer within {}s", CONNECT_TIMEOUT.as_secs()),
            )),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.client = None;
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// tiberius panics on a value it cannot read (`sql_variant`), which `run`
/// turns into an error; the default hook would print the panic on stderr
/// first. Other panics still print.
fn quiet_driver_panics() {
    static QUIET: Once = Once::new();
    QUIET.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info
                .location()
                .is_some_and(|at| at.file().contains("tiberius"))
            {
                previous(info);
            }
        }));
    });
}

fn cannot_connect(config: &tiberius::Config, why: &str) -> anyhow::Error {
    Failure::new(
        Exit::Failed,
        format!("cannot connect to {}: {why}", config.get_addr()),
    )
    .hint("check the connection in [sql]; `agent-cli doctor sql` tries every one")
    .into()
}

async fn open_driver(config: &tiberius::Config) -> Result<Driver> {
    let tcp = TcpStream::connect(config.get_addr())
        .await
        .map_err(|why| cannot_connect(config, &why.to_string()))?;
    // Row batches are small and frequent; Nagle would sit on them.
    tcp.set_nodelay(true)?;
    Client::connect(config.clone(), tcp.compat_write())
        .await
        .map_err(|why| match why {
            // The server's own sentence and number, not the driver's wrapping.
            TiberiusError::Server(token) => cannot_connect(
                config,
                &format!("{} (error {})", token.message(), token.code()),
            ),
            why @ TiberiusError::Tls(_) => cannot_connect(
                config,
                &format!("{why}; a server with a self-signed certificate wants trust_cert = true"),
            ),
            why => cannot_connect(config, &why.to_string()),
        })
}

/// Runs one batch with `simple_query`, which takes several statements and
/// hands back several result sets. True alongside when it stopped reading
/// before the server finished.
async fn stream(driver: &mut Driver, sql: &str, fetch: Fetch) -> Result<(Ran, bool)> {
    let mut ran = Ran::default();
    {
        let mut query = driver.simple_query(sql).await.map_err(failure)?;
        // The rows of a `for json` or `for xml` set so far, which are pieces
        // of one value and not rows of anything.
        let mut pieces: Option<Vec<String>> = None;
        while let Some(item) = query.try_next().await.map_err(failure)? {
            match item {
                QueryItem::Metadata(metadata) => {
                    if !whole(&mut ran, pieces.take(), fetch) {
                        return Ok((ran, true));
                    }
                    let columns = metadata.columns();
                    let set = ResultSet {
                        columns: columns
                            .iter()
                            .map(|column| column.name().to_owned())
                            .collect(),
                        types: columns
                            .iter()
                            .map(|column| type_name(column.column_type()).to_owned())
                            .collect(),
                        ..ResultSet::default()
                    };
                    pieces = in_pieces(&set.columns).then(Vec::new);
                    ran.sets.push(set);
                }
                QueryItem::Row(row) => match pieces.as_mut() {
                    Some(pieces) => pieces.extend(row.into_iter().filter_map(|data| match data {
                        ColumnData::String(Some(piece)) => Some(piece.into_owned()),
                        _ => None,
                    })),
                    None => {
                        let kept = keep(&mut ran, fetch, || {
                            row.cells()
                                .map(|(column, data)| cell(column.column_type(), data))
                                .collect()
                        });
                        if !kept {
                            return Ok((ran, true));
                        }
                    }
                },
            }
        }
        if !whole(&mut ran, pieces, fetch) {
            return Ok((ran, true));
        }
    }
    // A batch with no result set: an UPDATE, a DELETE, an EXEC. tiberius
    // drops the DONE token that carries the count, so the session is asked;
    // @@rowcount outlives the batch.
    if ran.sets.is_empty() {
        let affected = driver
            .simple_query("select @@rowcount")
            .await
            .map_err(failure)?
            .into_row()
            .await
            .map_err(failure)?
            .and_then(|row| row.get::<i32, _>(0))
            .unwrap_or_default();
        ran.sets.push(ResultSet {
            rows_affected: affected.max(0).unsigned_abs().into(),
            ..ResultSet::default()
        });
    }
    Ok((ran, false))
}

/// Counts a row and keeps it while there is room. False when the read
/// should stop.
fn keep(ran: &mut Ran, fetch: Fetch, row: impl FnOnce() -> Vec<Value>) -> bool {
    ran.rows += 1;
    ran.first_row.get_or_insert_with(Instant::now);
    let Some(set) = ran.sets.last_mut() else {
        return true;
    };
    if set.rows.len() >= fetch.keep {
        set.truncated = true;
        return !fetch.stop;
    }
    set.rows.push(row());
    true
}

/// Whether a set is the one column SQL Server answers `for json` and `for
/// xml` in, cut into rows of 2,033 characters.
fn in_pieces(columns: &[String]) -> bool {
    matches!(columns, [only] if ["JSON", "XML"].iter().any(|kind| {
        only.strip_prefix(kind) == Some("_F52E2B61-18A1-11d1-B105-00805F49916B")
    }))
}

/// A `for json` or `for xml` set's pieces as the one row they are.
fn whole(ran: &mut Ran, pieces: Option<Vec<String>>, fetch: Fetch) -> bool {
    match pieces {
        Some(pieces) if !pieces.is_empty() => {
            keep(ran, fetch, || vec![Value::String(pieces.concat())])
        }
        _ => true,
    }
}

/// What the server would call the column, as near as the wire type says: a
/// nullable `smalldatetime` arrives as a `datetime`.
fn type_name(column_type: ColumnType) -> &'static str {
    match column_type {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money => "money",
        ColumnType::Money4 => "smallmoney",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln => "decimal",
        ColumnType::Numericn => "numeric",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigBinary => "binary",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
}

/// One value as JSON. The column type only matters for money, which the
/// protocol delivers as a float.
fn cell(column_type: ColumnType, data: &ColumnData<'static>) -> Value {
    let datetime = |text: String| Value::String(text);
    match data {
        ColumnData::Bit(Some(value)) => Value::Bool(*value),
        ColumnData::U8(Some(value)) => db::int(i64::from(*value)),
        ColumnData::I16(Some(value)) => db::int(i64::from(*value)),
        ColumnData::I32(Some(value)) => db::int(i64::from(*value)),
        ColumnData::I64(Some(value)) => db::int(*value),
        ColumnData::F32(Some(value)) => db::float(db::widen(*value)),
        ColumnData::F64(Some(value)) => match column_type {
            // ponytail: tiberius has already decoded money through an f64,
            // so past 2^39 (about 5.5e11) the last places are the float's
            // guess; cast the column to decimal(19,4) if that range matters.
            ColumnType::Money | ColumnType::Money4 => db::decimal(format!("{value:.4}")),
            _ => db::float(*value),
        },
        ColumnData::Numeric(Some(value)) => db::decimal(decimal(*value)),
        ColumnData::String(Some(value)) => Value::String(value.to_string()),
        ColumnData::Guid(Some(value)) => Value::String(value.to_string()),
        ColumnData::Xml(Some(value)) => Value::String(value.to_string()),
        ColumnData::Binary(Some(value)) => db::bytes(value),
        ColumnData::Date(Some(value)) => datetime(day(SQL_EPOCH, i64::from(value.days()))),
        ColumnData::Time(Some(value)) => datetime(clock(value.increments(), value.scale())),
        ColumnData::DateTime(Some(value)) => datetime(format!(
            "{}T{}",
            day(DATETIME_EPOCH, i64::from(value.days())),
            // The wire counts 1/300 s; SQL Server shows milliseconds rounded
            // to .000, .003 and .007.
            clock((u64::from(value.seconds_fragments()) * 10 + 1) / 3, 3)
        )),
        ColumnData::SmallDateTime(Some(value)) => datetime(format!(
            "{}T{}",
            day(DATETIME_EPOCH, i64::from(value.days())),
            clock(u64::from(value.seconds_fragments()) * 60, 0)
        )),
        ColumnData::DateTime2(Some(value)) => datetime(format!(
            "{}T{}",
            day(SQL_EPOCH, i64::from(value.date().days())),
            clock(value.time().increments(), value.time().scale())
        )),
        ColumnData::DateTimeOffset(Some(value)) => datetime(offset_datetime(*value)),
        // Every remaining arm is that variant's `None`.
        _ => Value::Null,
    }
}

/// `date` and `datetime2` count from year one; `datetime` counts from 1900.
const SQL_EPOCH: CalendarDate = time::macros::date!(0001 - 01 - 01);
const DATETIME_EPOCH: CalendarDate = time::macros::date!(1900 - 01 - 01);

fn day(epoch: CalendarDate, days: i64) -> String {
    match epoch.checked_add(time::Duration::days(days)) {
        Some(date) => format!(
            "{:04}-{:02}-{:02}",
            date.year(),
            u8::from(date.month()),
            date.day()
        ),
        None => format!("<day {days}>"),
    }
}

/// `increments` is a count of 10^-`scale` seconds since midnight.
fn clock(increments: u64, scale: u8) -> String {
    let unit = 10u64.pow(u32::from(scale));
    let (seconds, fraction) = (increments / unit, increments % unit);
    let time = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    );
    if scale == 0 {
        time
    } else {
        format!("{time}.{fraction:0width$}", width = usize::from(scale))
    }
}

/// The wire carries a `datetimeoffset` as UTC plus the offset it was written
/// with; putting the offset back reads the value as stored.
fn offset_datetime(value: tiberius::time::DateTimeOffset) -> String {
    let time = value.datetime2().time();
    let scale = time.scale();
    let unit = i128::from(10u64.pow(u32::from(scale)));
    let per_day = 86_400 * unit;
    let local = i128::from(time.increments()) + i128::from(value.offset()) * 60 * unit;
    let days = i64::from(value.datetime2().date().days()) + (local.div_euclid(per_day) as i64);
    let increments = local.rem_euclid(per_day) as u64;
    let minutes = value.offset().abs();
    format!(
        "{}T{}{}{:02}:{:02}",
        day(SQL_EPOCH, days),
        clock(increments, scale),
        if value.offset() < 0 { '-' } else { '+' },
        minutes / 60,
        minutes % 60
    )
}

/// tiberius' own `Display` prints `-1234.-5678` for a negative value.
fn decimal(value: tiberius::numeric::Numeric) -> String {
    let scale = usize::from(value.scale());
    if scale == 0 {
        return value.value().to_string();
    }
    let unit = 10u128.pow(value.scale().into());
    let magnitude = value.value().unsigned_abs();
    format!(
        "{}{}.{:0scale$}",
        if value.value() < 0 { "-" } else { "" },
        magnitude / unit,
        magnitude % unit,
    )
}

/// What went wrong, with the line of the batch when the server says.
fn failure(why: TiberiusError) -> anyhow::Error {
    match why {
        // A procedure's line is of its own text, which is not in the batch.
        TiberiusError::Server(token) if !token.procedure().is_empty() => anyhow!(
            "{}, line {}: {}",
            token.procedure(),
            token.line(),
            token.message()
        ),
        TiberiusError::Server(token) if token.code() == 208 => {
            anyhow::Error::new(crate::db::UnknownObject {
                message: format!("line {}: {}", token.line(), token.message()),
                name: crate::db::UnknownObject::quoted(token.message()),
            })
        }
        TiberiusError::Server(token) => anyhow!("line {}: {}", token.line(), token.message()),
        TiberiusError::Io { message, .. } => anyhow!(
            "connection lost: {}",
            message.trim_start_matches("An error occured during the attempt of performing I/O: ")
        ),
        // `left` or `substring` can leave half an emoji in an nvarchar, and
        // the driver then reads none of the result.
        TiberiusError::Utf16 => anyhow!(
            "a value is not whole UTF-16 (half of a surrogate pair, as left or substring can \
             leave of an emoji) and the driver reads none of the result past it; cast that \
             column to varbinary to see its bytes"
        ),
        other => anyhow!("{other}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tiberius::numeric::Numeric;
    use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, Time};

    use super::*;

    #[test]
    fn dates_and_times_are_rfc_3339_with_every_digit_of_their_scale() {
        assert_eq!(
            cell(
                ColumnType::Daten,
                &ColumnData::Date(Some(Date::new(739_022)))
            ),
            json!("2024-05-17")
        );
        assert_eq!(
            cell(
                ColumnType::Datetimen,
                &ColumnData::DateTime(Some(DateTime::new(45_427, 14_859_000)))
            ),
            json!("2024-05-17T13:45:30.000")
        );
        let value = DateTime2::new(Date::new(739_022), Time::new(495_301_234_567, 7));
        assert_eq!(
            cell(ColumnType::Datetime2, &ColumnData::DateTime2(Some(value))),
            json!("2024-05-17T13:45:30.1234567")
        );
        // 13:45:30.1234567 +02:00, which the wire carries as 11:45:30 UTC.
        let utc = DateTime2::new(Date::new(739_022), Time::new(423_301_234_567, 7));
        assert_eq!(
            cell(
                ColumnType::DatetimeOffsetn,
                &ColumnData::DateTimeOffset(Some(DateTimeOffset::new(utc, 120)))
            ),
            json!("2024-05-17T13:45:30.1234567+02:00")
        );
        let midnight = DateTime2::new(Date::new(739_022), Time::new(0, 0));
        assert_eq!(
            cell(
                ColumnType::DatetimeOffsetn,
                &ColumnData::DateTimeOffset(Some(DateTimeOffset::new(midnight, -300)))
            ),
            json!("2024-05-16T19:00:00-05:00"),
            "an offset that carries the value back over midnight"
        );
    }

    #[test]
    fn decimals_and_money_keep_their_scale_and_sign() {
        assert_eq!(
            decimal(Numeric::new_with_scale(-123_456_789, 4)),
            "-12345.6789"
        );
        assert_eq!(decimal(Numeric::new_with_scale(-5, 1)), "-0.5");
        assert_eq!(
            cell(
                ColumnType::Decimaln,
                &ColumnData::Numeric(Some(Numeric::new_with_scale(102_500, 4)))
            ),
            json!("10.2500")
        );
        assert_eq!(
            cell(
                ColumnType::Decimaln,
                &ColumnData::Numeric(Some(Numeric::new_with_scale(42, 0)))
            ),
            json!(42)
        );
        assert_eq!(
            cell(ColumnType::Money, &ColumnData::F64(Some(1234.5678))),
            json!("1234.5678")
        );
        assert_eq!(
            cell(ColumnType::Float8, &ColumnData::F64(Some(1234.5678))),
            json!(1234.5678)
        );
        assert_eq!(
            cell(ColumnType::Float4, &ColumnData::F32(Some(0.1))),
            json!(0.1)
        );
    }

    #[test]
    fn integers_booleans_binary_and_nulls() {
        assert_eq!(
            cell(ColumnType::Int8, &ColumnData::I64(Some(i64::MAX))),
            json!("9223372036854775807")
        );
        assert_eq!(
            cell(ColumnType::Int4, &ColumnData::I32(Some(-7))),
            json!(-7)
        );
        assert_eq!(
            cell(ColumnType::Bitn, &ColumnData::Bit(Some(true))),
            json!(true)
        );
        assert_eq!(
            cell(
                ColumnType::BigVarBin,
                &ColumnData::Binary(Some(vec![1, 2, 255].into()))
            ),
            json!("0x0102ff")
        );
        assert_eq!(cell(ColumnType::Intn, &ColumnData::I32(None)), Value::Null);
        assert_eq!(
            cell(ColumnType::Datetime2, &ColumnData::DateTime2(None)),
            Value::Null
        );
    }

    #[test]
    fn a_session_the_server_ended_says_so_once() {
        let why = TiberiusError::Io {
            kind: std::io::ErrorKind::UnexpectedEof,
            message:
                "An error occured during the attempt of performing I/O: unexpected end of file"
                    .to_owned(),
        };
        assert_eq!(
            failure(why).to_string(),
            "connection lost: unexpected end of file"
        );
    }

    #[test]
    fn a_for_json_set_is_recognised_by_its_column_name() {
        assert!(in_pieces(&[
            "JSON_F52E2B61-18A1-11d1-B105-00805F49916B".to_owned()
        ]));
        assert!(!in_pieces(&["JSON".to_owned()]));
    }
}
