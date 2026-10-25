//! The one door to Redis: a connection for the length of one command (TCP,
//! TLS when configured, `AUTH`, `SELECT`), RESP2 both ways, and the op every
//! redis command performs through `Ctx`.
//!
//! The door sends only the read commands in [`READS`], so no handler can
//! write, whatever the credential allows.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use agent_cli_core::{Ctx, Exit, Failure, Op, Secret};
use anyhow::{Result, anyhow, bail};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::Value;

use crate::config::Instance;

/// Every command the door sends, by name, or name and subcommand. None of
/// them writes; the database test asks a server's `COMMAND INFO` to be sure.
pub const READS: &[&str] = &[
    "AUTH",
    "PING",
    "SELECT",
    "CLIENT SETNAME",
    "CLIENT ID",
    "CLIENT LIST",
    "INFO",
    "DBSIZE",
    "SCAN",
    "TYPE",
    "PTTL",
    "MEMORY USAGE",
    "OBJECT ENCODING",
    "OBJECT IDLETIME",
    "OBJECT FREQ",
    "STRLEN",
    "GETRANGE",
    "HLEN",
    "HSCAN",
    "LLEN",
    "LRANGE",
    "SCARD",
    "SSCAN",
    "ZCARD",
    "ZRANGE",
    "XLEN",
    "XREVRANGE",
    "SLOWLOG GET",
];

/// The Redis ACL that grants a user exactly what the door sends.
pub const ACL: &str =
    "+@read -@dangerous +info +slowlog|get +client|list +client|id +client|setname +ping +select";

/// The largest single value the door holds.
// ponytail: one cap per bulk string, not per command; a collection of
// many near-cap members could hold more, a byte budget per command if so.
const MAX_BULK: usize = 64 << 20;

/// A command's words, each anything that is bytes: `cmd!("SCAN", cursor, "COUNT", "1000")`.
macro_rules! cmd {
    ($($part:expr),+ $(,)?) => {
        vec![$(AsRef::<[u8]>::as_ref(&$part).to_vec()),+]
    };
}
pub(crate) use cmd;

/// One RESP2 reply. An error reply is a value here, so a pipeline can carry
/// the one command that failed (`OBJECT IDLETIME` under an LFU policy).
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Status(String),
    Error(String),
    Int(i64),
    Bulk(Vec<u8>),
    Array(Vec<Reply>),
    Nil,
}

impl Reply {
    pub fn int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Bulk(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
            _ => None,
        }
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bulk(bytes) => Some(bytes),
            Self::Status(text) => Some(text.as_bytes()),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<String> {
        self.bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    pub fn items(&self) -> &[Reply] {
        match self {
            Self::Array(items) => items,
            _ => &[],
        }
    }
}

/// An open connection to one instance.
pub struct Session {
    pub instance: String,
    /// host:port.
    address: String,
    tls: bool,
    reader: BufReader<Box<dyn Stream>>,
    /// The socket under any TLS, for its timeouts.
    tcp: TcpStream,
    deadline: Instant,
}

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

impl Session {
    /// Connects, signs in and selects the database, all before `deadline`.
    pub fn open(spec: &Instance, ctx: &Ctx, deadline: Instant) -> Result<Self> {
        let address = format!("{}:{}", spec.host, spec.port);
        let unreachable = |error: std::io::Error| -> anyhow::Error {
            if timed_out(&error) {
                return deadline_passed(&address);
            }
            Failure::new(
                Exit::Failed,
                format!("cannot connect to {} at {address}: {error}", spec.name),
            )
            .hint("check its host and port, and that this machine can reach it (VPN, firewall); agent-cli doctor redis")
            .into()
        };
        let mut tcp = None;
        let mut last = None;
        for target in resolve(&spec.host, spec.port, deadline)? {
            match TcpStream::connect_timeout(&target, left(deadline, &address)?) {
                Ok(stream) => {
                    tcp = Some(stream);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        let tcp = match (tcp, last) {
            (Some(tcp), _) => tcp,
            (None, Some(error)) => return Err(unreachable(error)),
            (None, None) => bail!("{} resolves to no address", spec.host),
        };
        tcp.set_nodelay(true)?;
        let stream: Box<dyn Stream> = if spec.tls {
            let mut tls = tls(spec, tcp.try_clone()?)?;
            set_timeouts(&tcp, deadline, &address)?;
            while tls.conn.is_handshaking() {
                tls.conn
                    .complete_io(&mut tls.sock)
                    .map_err(|error| refused_tls(spec, &address, error))?;
            }
            Box::new(tls)
        } else {
            Box::new(tcp.try_clone()?)
        };
        let mut session = Self {
            instance: spec.name.clone(),
            address: address.clone(),
            tls: spec.tls,
            reader: BufReader::new(stream),
            tcp,
            deadline,
        };
        session.sign_in(spec, ctx)?;
        let mut setup = vec![cmd!("CLIENT", "SETNAME", "agent-cli")];
        if spec.db != 0 {
            setup.push(cmd!("SELECT", spec.db.to_string()));
        }
        let replies = session.pipeline(&setup)?;
        if let Some(Reply::Error(message)) = replies.get(1) {
            return Err(session.refused(message));
        }
        Ok(session)
    }

    /// `AUTH`, once more when Redis refuses it and there is something new to
    /// send: a fresh az token (it may have expired between mint and use), or
    /// a `password_kv` that changed in Key Vault.
    fn sign_in(&mut self, spec: &Instance, ctx: &Ctx) -> Result<()> {
        let Some(signed) = spec.credentials(ctx, false)? else {
            return Ok(());
        };
        let Some(message) = self.auth(signed)? else {
            return Ok(());
        };
        if !(message.starts_with("WRONGPASS") || message.starts_with("NOAUTH")) {
            return Err(self.refused(&message));
        }
        let Some(signed) = spec.credentials(ctx, true)? else {
            return Err(self.refused(&message));
        };
        match self.auth(signed)? {
            Some(message) => Err(self.refused(&message)),
            None => Ok(()),
        }
    }

    /// One `AUTH`: Redis's error when it refuses.
    fn auth(&mut self, (user, password): (Option<String>, Secret)) -> Result<Option<String>> {
        let mut auth = cmd!("AUTH");
        auth.extend(user.map(String::into_bytes));
        auth.push(password.expose().as_bytes().to_vec());
        match self.pipeline(&[auth])?.remove(0) {
            Reply::Error(message) => Ok(Some(message)),
            _ => Ok(None),
        }
    }

    /// Sends `commands` in one write and reads one reply for each. Error
    /// replies come back as [`Reply::Error`]; the connection failing, or the
    /// deadline passing, is an `Err`.
    pub fn pipeline(&mut self, commands: &[Vec<Vec<u8>>]) -> Result<Vec<Reply>> {
        let mut wire = Vec::new();
        for command in commands {
            check_read(command)?;
            encode(command, &mut wire);
        }
        let (name, address, tls) = (self.instance.clone(), self.address.clone(), self.tls);
        set_timeouts(&self.tcp, self.deadline, &address)?;
        let failed = |error: std::io::Error| -> anyhow::Error {
            if timed_out(&error) {
                return deadline_passed(&address);
            }
            if error.kind() == ErrorKind::UnexpectedEof && !tls {
                return Failure::new(
                    Exit::Failed,
                    format!("{name} at {address} closed the connection"),
                )
                .hint(format!(
                    "if its port speaks TLS, set tls = true in {name:?}'s [[redis.instance]]"
                ))
                .into();
            }
            anyhow!("the connection to {name} at {address} failed: {error}")
        };
        let stream = self.reader.get_mut();
        stream.write_all(&wire).map_err(failed)?;
        stream.flush().map_err(failed)?;
        commands
            .iter()
            .map(|_| read_reply(&mut self.reader).map_err(failed))
            .collect()
    }

    /// One command whose error reply is the command's failure.
    pub fn call(&mut self, command: Vec<Vec<u8>>) -> Result<Reply> {
        match self.pipeline(&[command])?.remove(0) {
            Reply::Error(message) => Err(self.refused(&message)),
            reply => Ok(reply),
        }
    }

    /// Redis said no, in its words, with the exit code and next step its
    /// error class calls for.
    pub fn refused(&self, message: &str) -> anyhow::Error {
        let name = &self.instance;
        let class = message.split_whitespace().next().unwrap_or_default();
        let said = format!("{name}: Redis said: {message}");
        match class {
            "NOPERM" => Failure::setup(said)
                .hint(format!("give its user the reads agent-cli sends: the ACL `{ACL}`, or an Azure access policy like it"))
                .into(),
            "WRONGPASS" | "NOAUTH" => Failure::setup(said)
                .hint(format!("check the user and password of {name:?} in [redis]; agent-cli doctor redis"))
                .into(),
            "MOVED" | "ASK" | "CLUSTERDOWN" | "CROSSSLOT" => anyhow!(
                "{said} (a clustered Redis: agent-cli reads a single instance, not a cluster)"
            ),
            _ if message.contains("AUTH") && message.contains("without any password configured") => {
                Failure::setup(format!("{said} (the server has no password: leave it out of {name:?})")).into()
            }
            _ => anyhow!(said),
        }
    }
}

/// A refusal of any command not in [`READS`]: a bug in this crate, never a
/// thing a user can ask for.
fn check_read(command: &[Vec<u8>]) -> Result<()> {
    let word = |index: usize| {
        command
            .get(index)
            .map(|word| String::from_utf8_lossy(word).to_ascii_uppercase())
    };
    let name = word(0).unwrap_or_default();
    let with_sub = word(1).map(|sub| format!("{name} {sub}"));
    if READS.contains(&name.as_str()) || with_sub.is_some_and(|both| READS.contains(&both.as_str()))
    {
        return Ok(());
    }
    bail!("refusing to send {name} to Redis: agent-cli sends only reads")
}

fn encode(command: &[Vec<u8>], wire: &mut Vec<u8>) {
    wire.extend_from_slice(format!("*{}\r\n", command.len()).as_bytes());
    for word in command {
        wire.extend_from_slice(format!("${}\r\n", word.len()).as_bytes());
        wire.extend_from_slice(word);
        wire.extend_from_slice(b"\r\n");
    }
}

/// One reply, as RESP2 frames it.
pub fn read_reply(input: &mut impl BufRead) -> std::io::Result<Reply> {
    let invalid = |what: String| std::io::Error::new(ErrorKind::InvalidData, what);
    let mut line = Vec::new();
    input.read_until(b'\n', &mut line)?;
    let Some(body) = line.strip_suffix(b"\r\n") else {
        return Err(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "the server closed the connection",
        ));
    };
    let Some((&kind, rest)) = body.split_first() else {
        return Err(invalid("an empty reply line".to_owned()));
    };
    let rest = String::from_utf8_lossy(rest).into_owned();
    let number = |text: &str| {
        text.parse::<i64>()
            .map_err(|_| invalid(format!("{text:?} is not a length")))
    };
    Ok(match kind {
        b'+' => Reply::Status(rest),
        b'-' => Reply::Error(rest),
        b':' => Reply::Int(number(&rest)?),
        b'$' => match usize::try_from(number(&rest)?) {
            Err(_) => Reply::Nil,
            Ok(len) if len > MAX_BULK => {
                return Err(invalid(format!(
                    "a {} MB value is more than agent-cli holds",
                    len >> 20
                )));
            }
            Ok(len) => {
                let mut bytes = vec![0; len + 2];
                input.read_exact(&mut bytes)?;
                if !bytes.ends_with(b"\r\n") {
                    return Err(invalid("a value without its \\r\\n".to_owned()));
                }
                bytes.truncate(len);
                Reply::Bulk(bytes)
            }
        },
        b'*' => match usize::try_from(number(&rest)?) {
            Err(_) => Reply::Nil,
            Ok(len) => Reply::Array(
                (0..len)
                    .map(|_| read_reply(input))
                    .collect::<std::io::Result<_>>()?,
            ),
        },
        other => {
            return Err(invalid(format!(
                "an unknown reply type {:?}",
                other as char
            )));
        }
    })
}

/// Work on one instance, as an op. The connection (and any
/// `password_cmd`) opens inside `perform`.
pub struct OnInstance<'a, F> {
    pub spec: &'a Instance,
    pub deadline: Instant,
    /// What `--dry-run` would print; nothing here writes.
    pub plan: Value,
    pub work: F,
}

impl<T, F: FnOnce(&mut Session) -> Result<T>> Op for OnInstance<'_, F> {
    type Output = T;

    fn plan(&self) -> Value {
        self.plan.clone()
    }

    fn perform(self, ctx: &Ctx) -> Result<T> {
        let mut session = Session::open(self.spec, ctx, self.deadline)?;
        (self.work)(&mut session)
    }
}

/// Runs `work` on `spec` through `ctx.read`, under the command's deadline.
pub fn on<T>(
    ctx: &Ctx,
    spec: &Instance,
    work: impl FnOnce(&mut Session) -> Result<T>,
) -> Result<T> {
    ctx.enter_scope("redis instance", &spec.name, spec.read_only);
    ctx.read(OnInstance {
        spec,
        deadline: ctx.deadline(),
        plan: serde_json::json!({"instance": spec.name, "host": spec.host, "port": spec.port}),
        work,
    })
}

/// A name lookup bounded by the deadline: the resolver has no timeout of
/// its own, and a VPN-less WSL can wait on it for a long time.
fn resolve(host: &str, port: u16, deadline: Instant) -> Result<Vec<SocketAddr>> {
    let (done, answer) = mpsc::channel();
    let target = (host.to_owned(), port);
    std::thread::spawn(move || {
        let _ = done.send(target.to_socket_addrs().map(Iterator::collect::<Vec<_>>));
    });
    let address = format!("{host}:{port}");
    match answer.recv_timeout(left(deadline, &address)?) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(error)) => bail!("cannot resolve {host}: {error}"),
        Err(_) => Err(deadline_passed(&address)),
    }
}

fn left(deadline: Instant, address: &str) -> Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(deadline_passed(address));
    }
    Ok(left)
}

fn set_timeouts(tcp: &TcpStream, deadline: Instant, address: &str) -> Result<()> {
    let left = left(deadline, address)?;
    tcp.set_read_timeout(Some(left))?;
    tcp.set_write_timeout(Some(left))?;
    Ok(())
}

fn timed_out(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

fn deadline_passed(address: &str) -> anyhow::Error {
    Failure::timed_out(format!(
        "{address} did not answer before the --timeout deadline"
    ))
    .hint("check the host, port and firewall (agent-cli doctor redis), or run it again with a larger --timeout")
    .into()
}

fn refused_tls(spec: &Instance, address: &str, error: std::io::Error) -> anyhow::Error {
    if timed_out(&error) {
        return deadline_passed(address);
    }
    let text = error.to_string();
    let failure = Failure::setup(format!("TLS to {} at {address} failed: {text}", spec.name));
    if text.contains("certificate") && !spec.trust_cert {
        return failure
            .hint(format!(
                "for a self-signed certificate set trust_cert = true in {:?}'s [[redis.instance]]",
                spec.name
            ))
            .into();
    }
    failure
        .hint("check that the port speaks TLS, or set tls = false")
        .into()
}

fn tls(spec: &Instance, tcp: TcpStream) -> Result<StreamOwned<ClientConnection, TcpStream>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous();
    let config = if spec.trust_cert {
        builder.with_custom_certificate_verifier(Arc::new(Unverified))
    } else {
        builder.with_custom_certificate_verifier(Arc::new(rustls_platform_verifier::Verifier::new(
            provider,
        )?))
    }
    .with_no_client_auth();
    let name = ServerName::try_from(spec.host.clone())?;
    let connection = ClientConnection::new(Arc::new(config), name)?;
    Ok(StreamOwned::new(connection, tcp))
}

/// `trust_cert = true`: any certificate, as sql's `trust_cert` does.
#[derive(Debug)]
struct Unverified;

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_parse_as_resp2_frames_them() {
        let wire = b"+OK\r\n-ERR no\r\n:42\r\n$3\r\na\r\n\r\n$-1\r\n*2\r\n$1\r\nx\r\n*-1\r\n*0\r\n";
        let mut input = &wire[..];
        let replies: Vec<Reply> = (0..7).map(|_| read_reply(&mut input).unwrap()).collect();
        assert_eq!(
            replies,
            [
                Reply::Status("OK".into()),
                Reply::Error("ERR no".into()),
                Reply::Int(42),
                Reply::Bulk(b"a\r\n".to_vec()),
                Reply::Nil,
                Reply::Array(vec![Reply::Bulk(b"x".to_vec()), Reply::Nil]),
                Reply::Array(vec![]),
            ]
        );
        assert_eq!(
            read_reply(&mut &b"+OK"[..]).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
        assert!(read_reply(&mut &b"$99999999999\r\n"[..]).is_err());
    }

    #[test]
    fn commands_encode_as_arrays_of_bulk_strings() {
        let (mut wire, command) = (
            Vec::new(),
            cmd!("GETRANGE", b"k\r\n".as_slice(), "0", 7.to_string()),
        );
        encode(&command, &mut wire);
        assert_eq!(
            wire,
            b"*4\r\n$8\r\nGETRANGE\r\n$3\r\nk\r\n\r\n$1\r\n0\r\n$1\r\n7\r\n"
        );
    }

    /// Against scripts/redis-up.sh's qa, with `AGENT_CLI_TEST_REDIS=1`.
    #[test]
    fn every_read_is_a_read_by_the_servers_own_word() {
        if std::env::var("AGENT_CLI_TEST_REDIS").ok().as_deref() != Some("1") {
            return;
        }
        let stream = TcpStream::connect("127.0.0.1:16379").unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut ask = |command: Vec<Vec<u8>>| {
            let mut wire = Vec::new();
            encode(&command, &mut wire);
            writer.write_all(&wire).unwrap();
            read_reply(&mut reader).unwrap()
        };
        assert_eq!(ask(cmd!("AUTH", "seed-pass")), Reply::Status("OK".into()));
        for read in READS {
            let info = ask(cmd!(
                "COMMAND",
                "INFO",
                read.to_ascii_lowercase().replace(' ', "|")
            ));
            let entry = info.items()[0].items();
            assert!(!entry.is_empty(), "{read} is no command");
            let flags: Vec<String> = entry[2].items().iter().filter_map(Reply::text).collect();
            assert!(
                !flags.iter().any(|flag| flag == "write"),
                "{read}: {flags:?}"
            );
        }
    }

    #[test]
    fn the_door_sends_reads_and_refuses_everything_else() {
        for read in [
            cmd!("scan", "0"),
            cmd!("CLIENT", "list"),
            cmd!("memory", "usage", "k"),
        ] {
            check_read(&read).unwrap();
        }
        for write in [
            cmd!("FLUSHALL"),
            cmd!("DEL", "k"),
            cmd!("CLIENT", "KILL", "ID", "1"),
            cmd!("CONFIG", "SET", "x", "y"),
            cmd!("KEYS", "*"),
            cmd!("SLOWLOG", "RESET"),
            cmd!("MEMORY", "PURGE"),
        ] {
            let message = check_read(&write).unwrap_err().to_string();
            assert!(message.contains("sends only reads"), "{message}");
        }
    }

    /// Opens `orders-qa` (on `fake`'s port, `password_kv`) with these vault
    /// answers: whether it signed in, and what the server was asked.
    fn open_with_kv(script: Vec<(&str, Reply)>, vault: &[&str]) -> (Result<()>, Vec<String>) {
        use agent_cli_core::testing::{Answer, FakeTransport, ctx};
        let fake = crate::testing::serve(script);
        let config = format!(
            "{}password_kv = \"kv-contoso/orders-qa\"\n",
            crate::testing::config(&[("orders-qa", fake.port)])
        );
        let answers = vault
            .iter()
            .map(|value| Answer::json(&serde_json::json!({ "value": value })));
        let ctx = ctx(
            agent_cli_core::Setup::fake(FakeTransport::answering(answers)).with_config(&config),
        );
        let redis = crate::config::Redis::load(ctx.config()).unwrap();
        let opened = on(&ctx, redis.instance(None).unwrap(), |_| Ok(()));
        (opened, fake.sent())
    }

    #[test]
    fn a_password_kv_refused_with_wrongpass_is_read_again_and_sent_once_more() {
        let wrong = || Reply::Error("WRONGPASS invalid username-password pair".into());
        let script = vec![
            ("AUTH old-pass", wrong()),
            ("AUTH new-pass", Reply::Status("OK".into())),
        ];
        let (opened, sent) = open_with_kv(script, &["old-pass", "new-pass"]);
        opened.unwrap();
        assert_eq!(
            sent,
            ["AUTH old-pass", "AUTH new-pass", "CLIENT SETNAME agent-cli"]
        );

        let (opened, sent) =
            open_with_kv(vec![("AUTH old-pass", wrong())], &["old-pass", "old-pass"]);
        let failure = opened.unwrap_err().downcast::<Failure>().unwrap();
        assert_eq!(
            failure.exit,
            Exit::Setup,
            "unchanged, the refusal stands: {failure:?}"
        );
        assert_eq!(sent, ["AUTH old-pass"]);
    }

    fn probe(ctx: &Ctx, _: ProbeArgs) -> Result<bool> {
        let redis = crate::config::Redis::load(ctx.config())?;
        on(ctx, redis.instance(None)?, |_| Ok(()))?;
        ctx.read_only()
    }

    #[derive(clap::Args)]
    struct ProbeArgs {}

    agent_cli_core::command! {
        PROBE = ["redis", "instance", "probe"], Read,
        "Say whether the picked instance refuses changes",
        keywords: [],
        example: "redis instance probe",
        run: probe,
    }

    #[test]
    fn working_on_a_read_only_instance_enters_it_as_a_read_only_scope() {
        let fake = crate::testing::serve(Vec::new());
        let domain = agent_cli_core::Domain {
            commands: &[PROBE],
            ..crate::DOMAIN
        };
        let config = |extra: &str| {
            format!(
                "{}{extra}",
                crate::testing::config(&[("orders-prod", fake.port)])
            )
        };
        let run = |extra: &str| {
            let setup =
                agent_cli_core::Setup::fake(agent_cli_core::testing::FakeTransport::default())
                    .with_config(&config(extra));
            agent_cli_core::testing::run(
                std::slice::from_ref(&domain),
                &["redis", "instance", "probe"],
                setup,
            )
            .stdout
        };
        assert_eq!(run("read_only = true\n").trim(), "true");
        assert_eq!(run("").trim(), "false");
    }
}
