//! Postgres, over tokio-postgres, on a current-thread runtime made when the
//! connection opens and blocked on, as `mssql.rs` does.
//!
//! Each statement goes as a simple query, so every value comes back as the
//! server's own text and [`cell`] makes JSON of it by its column's type,
//! which a prepare reads first (once per statement text, so a bench pays it
//! once). Every statement commits as it runs unless the script begins a
//! transaction of its own.
//!
//! Postgres carries on with a statement whose client has gone until it next
//! writes to it, so at the deadline, and on a signal, a cancel request goes
//! to the server first (a second connection carrying the session's key),
//! then the socket is shut. Stopping at `--max-rows` shuts the socket as on
//! SQL Server: the server stops at its next write, and the next statement
//! reconnects. A late cancel then has no session left to hit.
//!
//! A connection the config makes read-only starts with
//! `default_transaction_read_only=on`, so a write the classifier missed is
//! refused by the server itself (SQLSTATE 25006).

mod cell;
mod tls;

use std::collections::HashMap;
use std::net::Shutdown;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use agent_cli_core::{Exit, Failure, OnStop, Secret, on_stop};
use anyhow::{Result, anyhow};
use futures_util::TryStreamExt as _;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::runtime::{Builder, Runtime};
use tokio_postgres::config::SslMode;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::types::Type;
use tokio_postgres::{CancelToken, Client, Config, SimpleQueryMessage};

use crate::config::Connection;
use crate::db::{self, Fetch, Ran, ResultSet};
use tls::Tls;

/// Long enough for a container still waking up, short enough that a wrong
/// host does not look like a hang.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a cancel request gets to reach the server.
const CANCEL_WAIT: Duration = Duration::from_secs(1);

/// Where the server is and how to speak TLS to it: what the session and a
/// cancel request both connect with.
#[derive(Clone)]
struct Target {
    host: String,
    port: u16,
    tls: Tls,
}

pub struct Session {
    config: Config,
    target: Target,
    /// `host:port/database`, for messages.
    address: String,
    /// The connection's name and `trust_cert`, for a TLS refusal's hint.
    conn: String,
    trust_cert: bool,
    /// An `Option` only so `Drop` can shut it down without waiting.
    runtime: Option<Runtime>,
    /// `None` after a read was cut short: the next statement reconnects.
    client: Option<Client>,
    /// The client's socket again, and its cancel key, for the deadline and
    /// a signal to stop it with while the client waits on it.
    socket: Arc<Mutex<Option<std::net::TcpStream>>>,
    cancel: Arc<Mutex<Option<CancelToken>>>,
    /// Each statement's column types, by its text.
    types: HashMap<String, Vec<Type>>,
    _stop: OnStop,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Session {
    pub fn open(
        spec: &Connection,
        password: &Secret,
        deadline: Instant,
        read_only: bool,
    ) -> Result<Self> {
        let database = spec.database.clone().unwrap_or_default();
        let mut config = Config::new();
        config
            .user(&spec.user)
            .password(password.expose())
            .dbname(&database)
            .application_name("agent-cli")
            .ssl_mode(if spec.encrypt {
                SslMode::Require
            } else {
                SslMode::Disable
            });
        // Only when asked: a PgBouncer in front refuses `options` unless
        // told to pass it, and then the refusal is the safe failure.
        if read_only {
            config.options("-c default_transaction_read_only=on");
        }
        let target = Target {
            host: spec.host.clone(),
            port: spec.port,
            tls: Tls::new(&spec.host, spec.encrypt, spec.trust_cert)?,
        };
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let socket: Arc<Mutex<Option<std::net::TcpStream>>> = Arc::default();
        let cancel: Arc<Mutex<Option<CancelToken>>> = Arc::default();
        let (shut, key, at) = (Arc::clone(&socket), Arc::clone(&cancel), target.clone());
        let stop = on_stop(format!("the session on {}", spec.name), move || {
            if let Some(key) = locked(&key).take() {
                send_cancel(&key, &at);
            }
            if let Some(socket) = locked(&shut).take() {
                let _ = socket.shutdown(Shutdown::Both);
            }
        });
        let mut session = Self {
            config,
            target,
            address: format!("{}:{}/{database}", spec.host, spec.port),
            conn: spec.name.clone(),
            trust_cert: spec.trust_cert,
            runtime: Some(runtime),
            client: None,
            socket,
            cancel,
            types: HashMap::new(),
            _stop: stop,
        };
        session.connect(deadline)?;
        Ok(session)
    }

    pub fn run(&mut self, sql: &str, fetch: Fetch, deadline: Instant) -> Result<Ran> {
        if self.client.is_none() {
            self.connect(deadline)?;
        }
        let (Some(runtime), Some(client)) = (&self.runtime, &self.client) else {
            unreachable!("a connect leaves a client and the runtime is only taken by drop");
        };
        let known = self.types.get(sql).cloned();
        let raced = runtime.block_on(async {
            tokio::time::timeout_at(deadline.into(), stream(client, sql, known, fetch)).await
        });
        match raced {
            Ok(Ok((ran, types, stopped))) => {
                self.types.entry(sql.to_owned()).or_insert(types);
                if stopped {
                    self.disconnect();
                }
                Ok(ran)
            }
            Ok(Err(error)) => Err(error),
            Err(_elapsed) => {
                if let Some(key) = locked(&self.cancel).take() {
                    send_cancel(&key, &self.target);
                }
                self.disconnect();
                Err(db::timed_out())
            }
        }
    }

    fn connect(&mut self, deadline: Instant) -> Result<()> {
        let limit = deadline.min(Instant::now() + CONNECT_TIMEOUT);
        let Some(runtime) = &self.runtime else {
            unreachable!("the runtime is only taken by drop");
        };
        let opened = runtime.block_on(async {
            tokio::time::timeout_at(limit.into(), open_client(&self.config, &self.target)).await
        });
        match opened {
            Ok(Ok((client, socket))) => {
                *locked(&self.cancel) = Some(client.cancel_token());
                *locked(&self.socket) = Some(socket);
                self.client = Some(client);
                Ok(())
            }
            Ok(Err(Why::Io(why))) => Err(self.cannot_connect(Exit::Failed, &why.to_string())),
            Ok(Err(Why::Postgres(why))) => Err(self.refused(&why)),
            // No statement ran yet: the deadline came during the sign-in.
            Err(_) if limit == deadline => {
                Err(self.cannot_connect(Exit::TimedOut, "no answer before the --timeout deadline"))
            }
            Err(_) => Err(self.cannot_connect(
                Exit::Failed,
                &format!("no answer within {}s", CONNECT_TIMEOUT.as_secs()),
            )),
        }
    }

    fn cannot_connect(&self, exit: Exit, why: &str) -> anyhow::Error {
        Failure::new(exit, format!("cannot connect to {}: {why}", self.address))
            .hint("check the connection in [sql]; `agent-cli doctor sql` tries every one")
            .into()
    }

    /// The server's refusal in its own words and SQLSTATE: a refused
    /// password is setup (exit 3), 28P01, or 28000 from a method that checks
    /// one elsewhere (`LDAP authentication failed for user …`).
    fn refused(&self, why: &tokio_postgres::Error) -> anyhow::Error {
        if let Some(db) = why.as_db_error() {
            let code = db.code().code();
            let login = code == "28P01"
                || (code == "28000" && db.message().contains("authentication failed"));
            return self.cannot_connect(
                if login { Exit::Setup } else { Exit::Failed },
                &format!("{} (SQLSTATE {code})", db.message()),
            );
        }
        let said = said(why);
        let hint = if said.contains("does not support TLS") {
            format!(
                "the server speaks no TLS: set encrypt = false in {:?}'s [[sql.connection]] if the \
                 network to it is one you trust",
                self.conn
            )
        } else if said.contains("certificate") && !self.trust_cert {
            format!(
                "install the CA that signed the server's certificate in the system store (on \
                 Ubuntu or WSL copy its .crt to /usr/local/share/ca-certificates/, then sudo \
                 update-ca-certificates), or for a self-signed server set trust_cert = true in \
                 {:?}'s [[sql.connection]]",
                self.conn
            )
        } else {
            return self.cannot_connect(Exit::Failed, &said);
        };
        Failure::new(
            Exit::Failed,
            format!("cannot connect to {}: {said}", self.address),
        )
        .hint(hint)
        .into()
    }

    /// Shuts the socket, which ends the session (and rolls back what it held
    /// open) once the server sees it.
    fn disconnect(&mut self) {
        self.client = None;
        locked(&self.cancel).take();
        if let Some(socket) = locked(&self.socket).take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.disconnect();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

enum Why {
    Io(std::io::Error),
    Postgres(tokio_postgres::Error),
}

/// A signed-in client, and a second handle on its socket. The connection
/// task runs on the runtime whenever the session blocks on it.
async fn open_client(
    config: &Config,
    target: &Target,
) -> std::result::Result<(Client, std::net::TcpStream), Why> {
    let tcp = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(Why::Io)?;
    // Rows come in many small messages; Nagle would sit on them.
    tcp.set_nodelay(true).map_err(Why::Io)?;
    let tcp = tcp.into_std().map_err(Why::Io)?;
    let socket = tcp.try_clone().map_err(Why::Io)?;
    let tcp = TcpStream::from_std(tcp).map_err(Why::Io)?;
    let (client, connection) = config
        .connect_raw(tcp, target.tls.clone())
        .await
        .map_err(Why::Postgres)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok((client, socket))
}

/// Asks the server to cancel what the session is running, waiting at most
/// [`CANCEL_WAIT`]. On a runtime of its own: a signal's thread has none.
fn send_cancel(key: &CancelToken, target: &Target) {
    let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
        return;
    };
    // The timer is made inside the runtime, which is the only place one can be.
    let _ = runtime.block_on(async {
        tokio::time::timeout(CANCEL_WAIT, async {
            let tcp = TcpStream::connect((target.host.as_str(), target.port)).await?;
            key.cancel_query_raw(tcp, target.tls.clone())
                .await
                .map_err(std::io::Error::other)
        })
        .await
    });
}

/// Runs one statement: its column types (`known`, or a prepare), then the
/// statement itself. True alongside when it stopped reading at the cap.
async fn stream(
    client: &Client,
    sql: &str,
    known: Option<Vec<Type>>,
    fetch: Fetch,
) -> Result<(Ran, Vec<Type>, bool)> {
    let types = match known {
        Some(types) => types,
        None => client
            .prepare(sql)
            .await
            .map_err(|why| failure(&why, sql))?
            .columns()
            .iter()
            .map(|column| column.type_().clone())
            .collect(),
    };
    let mut ran = Ran::default();
    let mut messages = std::pin::pin!(
        client
            .simple_query_raw(sql)
            .await
            .map_err(|why| failure(&why, sql))?
    );
    while let Some(message) = messages
        .try_next()
        .await
        .map_err(|why| failure(&why, sql))?
    {
        match message {
            SimpleQueryMessage::RowDescription(columns) => ran.sets.push(ResultSet {
                columns: columns
                    .iter()
                    .map(|column| column.name().to_owned())
                    .collect(),
                types: types.iter().map(cell::type_name).collect(),
                ..ResultSet::default()
            }),
            SimpleQueryMessage::Row(row) => {
                let more = ran.keep(fetch, || {
                    Ok((0..row.len())
                        .map(|at| {
                            let column = types.get(at).unwrap_or(&Type::TEXT);
                            cell::cell(column, row.try_get(at).ok().flatten())
                        })
                        .collect::<Vec<Value>>())
                })?;
                if !more {
                    return Ok((ran, types, true));
                }
            }
            // A statement with no rows: what it changed.
            SimpleQueryMessage::CommandComplete(count) if ran.sets.is_empty() => {
                ran.sets.push(ResultSet {
                    rows_affected: count,
                    ..ResultSet::default()
                });
            }
            _ => {}
        }
    }
    Ok((ran, types, false))
}

/// An error and what tokio-postgres says caused it, which its own
/// `Display` leaves out (`error performing TLS handshake: invalid peer
/// certificate: UnknownIssuer`).
fn said(why: &tokio_postgres::Error) -> String {
    use std::error::Error as _;
    match why.source() {
        Some(cause) => format!("{why}: {cause}"),
        None => why.to_string(),
    }
}

/// What the server said, on the line of the statement it points at, with
/// its detail, its hint and the innermost function it was in.
fn failure(why: &tokio_postgres::Error, sql: &str) -> anyhow::Error {
    let Some(db) = why.as_db_error() else {
        if why.is_closed() {
            return anyhow!("connection lost: the server closed the session");
        }
        return anyhow!(said(why));
    };
    let mut message = match db.position() {
        Some(ErrorPosition::Original(at)) => {
            format!("line {}: {}", line_of(sql, *at), db.message())
        }
        _ => db.message().to_owned(),
    };
    for more in [db.detail(), db.hint()].into_iter().flatten() {
        message.push_str("; ");
        message.push_str(more);
    }
    if let Some(inner) = db.where_().and_then(|trace| trace.lines().next()) {
        message.push_str(&format!(" ({inner})"));
    }
    if *db.code() == SqlState::UNDEFINED_TABLE {
        return anyhow::Error::new(db::UnknownObject {
            name: db::UnknownObject::quoted(db.message()),
            message,
        });
    }
    anyhow!(message)
}

/// The server counts a position in characters, from one.
fn line_of(sql: &str, at: u32) -> usize {
    let before = usize::try_from(at).unwrap_or(usize::MAX).saturating_sub(1);
    sql.chars().take(before).filter(|c| *c == '\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_counted_from_the_character_the_server_points_at() {
        let wide = "select '李李李' a,\n  nope b\nfrom t";
        let at = wide.chars().position(|c| c == 'n').unwrap() + 1;
        assert_eq!(line_of(wide, u32::try_from(at).unwrap()), 2);
        assert_eq!(line_of(wide, 1), 1);
        assert_eq!(line_of(wide, 10_000), 3, "past the end is the last line");
    }

    #[test]
    fn a_server_that_never_answers_is_a_connect_failure_at_the_deadline() {
        // Accepts the connection and never speaks: a paused server.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = format!(
            "[[sql.connection]]\nname = \"pg\"\nkind = \"postgres\"\nhost = \"127.0.0.1\"\nport = {port}\ndatabase = \"bench\"\nuser = \"bench\"\npassword = \"x\"\n"
        );
        let setup = agent_cli_core::Setup::fake(agent_cli_core::testing::FakeTransport::default())
            .with_config(&config);
        let started = Instant::now();
        let outcome = crate::testing::sql(
            &["sql", "query", "run", "select 1", "--timeout", "2"],
            setup,
        );
        drop(listener);
        assert_eq!(outcome.code, 124, "{outcome:?}");
        assert!(
            outcome.stderr.contains("cannot connect to 127.0.0.1")
                && outcome.stderr.contains("before the --timeout deadline"),
            "{}",
            outcome.stderr
        );
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_server_without_tls_is_refused_naming_encrypt_false() {
        // Answers the SSLRequest with N, as a Postgres with ssl = off does.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            stream.read_exact(&mut request).unwrap();
            stream.write_all(b"N").unwrap();
            request
        });
        let config = format!(
            "[[sql.connection]]\nname = \"pg\"\nkind = \"postgres\"\nhost = \"127.0.0.1\"\nport = {port}\ndatabase = \"bench\"\nuser = \"bench\"\npassword = \"x\"\n"
        );
        let setup = agent_cli_core::Setup::fake(agent_cli_core::testing::FakeTransport::default())
            .with_config(&config);
        let outcome = crate::testing::sql(
            &["sql", "query", "run", "select 1", "--timeout", "5"],
            setup,
        );
        // 80877103: the SSLRequest code, which a plain connection never sends.
        assert_eq!(&server.join().unwrap()[4..], &80_877_103u32.to_be_bytes());
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("server does not support TLS")
                && outcome.stderr.contains("set encrypt = false in \"pg\""),
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stderr.contains("\"x\""), "{}", outcome.stderr);
    }
}
