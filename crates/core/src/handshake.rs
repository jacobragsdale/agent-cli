//! A TLS handshake that reports what the server presented rather than
//! refusing it: the chain, whether this machine trusts it and why not, the
//! protocol and ALPN. `http certificate` reads servers' certificates with it,
//! and doctor sees through it whether a proxy intercepts TLS.
//!
//! The handshake goes through the transport, as a request does, so tests and
//! the recorded world answer it too.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use serde_json::{Value, json};

use crate::ctx::{Ctx, Meter, Op, locked};
use crate::error::{Exit, Failure};
use crate::proxy::Via;

/// One handshake to make. Send it with [`Ctx::read`].
#[derive(Clone, Debug)]
pub struct Handshake {
    pub host: String,
    pub port: u16,
    /// PEM certificates to judge the chain by instead of the system store.
    pub roots: Option<Vec<u8>>,
    /// The proxy core chose, set as the handshake goes out.
    pub via: Option<Via>,
}

impl Handshake {
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            roots: None,
            via: None,
        }
    }

    /// `host:port`, IPv6 in brackets.
    #[must_use]
    pub fn target(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// What a server presented.
#[derive(Clone, Debug, Default)]
pub struct Peer {
    /// The chain as sent, leaf first, each certificate DER.
    pub chain: Vec<Vec<u8>>,
    /// Why this machine does not trust it (rustls's words), or `None`.
    pub refusal: Option<String>,
    /// `TLSv1_3`, `TLSv1_2`.
    pub version: Option<String>,
    pub alpn: Option<String>,
    /// The TCP connection (through the proxy, when there is one).
    pub connect_ms: u64,
    pub handshake_ms: u64,
}

impl Op for Handshake {
    type Output = Peer;

    fn plan(&self) -> Value {
        json!({"tls": self.target()})
    }

    fn perform(mut self, ctx: &Ctx) -> Result<Peer> {
        self.via = ctx.via(&format!("https://{}/", self.target()))?;
        Meter::count(&ctx.meter.requests, 1);
        ctx.transport().handshake(&self, ctx.remaining()?)
    }
}

/// The handshake itself, for the real transport.
pub(crate) fn shake(target: &Handshake, timeout: Duration) -> Result<Peer> {
    let started = Instant::now();
    let mut stream = connect(target, timeout)?;
    let connect_ms = elapsed_ms(started);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let inner: Arc<dyn ServerCertVerifier> =
        match &target.roots {
            Some(pem) => {
                let mut store = RootCertStore::empty();
                for certificate in CertificateDer::pem_slice_iter(pem) {
                    store
                        .add(certificate.map_err(|why| {
                            Failure::usage(format!("--cacert is not PEM ({why})"))
                        })?)
                        .map_err(|why| {
                            Failure::usage(format!(
                                "--cacert holds a certificate rustls refuses ({why})"
                            ))
                        })?;
                }
                if store.is_empty() {
                    return Err(Failure::usage("--cacert holds no PEM certificate").into());
                }
                rustls::client::WebPkiServerVerifier::builder_with_provider(
                    Arc::new(store),
                    provider.clone(),
                )
                .build()
                .context("the --cacert roots")?
            }
            None => Arc::new(
                rustls_platform_verifier::Verifier::new(provider.clone())
                    .context("the system's certificate store")?,
            ),
        };
    let recorder = Arc::new(Recorder {
        inner,
        seen: Mutex::new((Vec::new(), None)),
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let name = ServerName::try_from(target.host.clone())
        .map_err(|_| Failure::usage(format!("{:?} is not a host name or address", target.host)))?;
    let mut connection = ClientConnection::new(Arc::new(config), name)?;
    let left = timeout.saturating_sub(started.elapsed());
    stream.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
    stream.set_write_timeout(Some(left.max(Duration::from_millis(1))))?;
    while connection.is_handshaking() {
        if let Err(error) = connection.complete_io(&mut stream) {
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) {
                return Err(Failure::timed_out(format!(
                    "{} did not finish its TLS handshake before the deadline",
                    target.target()
                ))
                .into());
            }
            // A server that wants a client certificate, or speaks no TLS,
            // ends here; what it presented first still says who it is.
            let (chain, _) = locked(&recorder.seen).clone();
            if chain.is_empty() {
                return Err(anyhow::Error::new(error)
                    .context(format!("the TLS handshake with {} failed", target.target())));
            }
            break;
        }
    }
    let handshake_ms = elapsed_ms(started).saturating_sub(connect_ms);
    connection.send_close_notify();
    let _ = connection.complete_io(&mut stream);
    let (chain, refusal) = locked(&recorder.seen).clone();
    Ok(Peer {
        chain,
        refusal,
        version: connection
            .protocol_version()
            .map(|version| format!("{version:?}")),
        alpn: connection
            .alpn_protocol()
            .map(|alpn| String::from_utf8_lossy(alpn).into_owned()),
        connect_ms,
        handshake_ms,
    })
}

/// A TCP connection to the server, through the proxy's `CONNECT` when one
/// applies.
fn connect(target: &Handshake, timeout: Duration) -> Result<TcpStream> {
    let open = |address: &str| -> Result<TcpStream> {
        let addresses: Vec<_> = address
            .to_socket_addrs()
            .with_context(|| format!("{address} does not resolve"))?
            .collect();
        let mut last = None;
        for at in addresses {
            match TcpStream::connect_timeout(&at, timeout.min(Duration::from_secs(10))) {
                Ok(stream) => return Ok(stream),
                Err(error) => last = Some(error),
            }
        }
        Err(match last {
            Some(error) => anyhow::Error::new(error).context(format!("no connection to {address}")),
            None => anyhow::anyhow!("{address} resolves to no address"),
        })
    };
    let Some(via) = &target.via else {
        return open(&target.target());
    };
    if !via.url.starts_with("http://") {
        bail!(
            "the proxy {} ({}) is not an http:// proxy, the one kind a certificate probe goes through",
            via.url,
            via.from
        );
    }
    let mut stream =
        open(&via.address()).with_context(|| format!("the proxy {} ({})", via.url, via.from))?;
    stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
    let mut ask = format!("CONNECT {0} HTTP/1.1\r\nHost: {0}\r\n", target.target());
    if let Some(user) = &via.user {
        let password = via.password.as_ref().map_or("", |p| p.expose());
        ask.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            crate::http::base64(format!("{user}:{password}").as_bytes())
        ));
    }
    ask.push_str("\r\n");
    stream.write_all(ask.as_bytes())?;
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    // Byte by byte, so nothing of the TLS that follows is read here.
    while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    if status == 200 {
        return Ok(stream);
    }
    let schemes: Vec<String> = head
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("proxy-authenticate")
                .then(|| value.trim().to_owned())
        })
        .collect();
    let failure = Failure::new(
        if status == 407 {
            Exit::Setup
        } else {
            Exit::Failed
        },
        format!(
            "the proxy {} ({}) answered CONNECT {} with {}",
            via.url,
            via.from,
            target.target(),
            head.lines().next().unwrap_or("nothing")
        ),
    );
    Err(if status == 407 {
        failure.hint(crate::proxy::proxy_refused(via, &schemes))
    } else {
        failure
    }
    .into())
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Keeps what the server presented and the real verifier's verdict on it,
/// and lets the handshake finish either way.
#[derive(Debug)]
struct Recorder {
    inner: Arc<dyn ServerCertVerifier>,
    seen: Mutex<(Vec<Vec<u8>>, Option<String>)>,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verdict = self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .err()
            .map(|error| error.to_string());
        let mut chain = vec![end_entity.as_ref().to_vec()];
        chain.extend(intermediates.iter().map(|der| der.as_ref().to_vec()));
        *locked(&self.seen) = (chain, verdict);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}
