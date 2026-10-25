//! TLS for tokio-postgres over the workspace's rustls, checked against the
//! OS trust store as every other connection agent-cli makes, so a server
//! signed by a company CA is trusted once the CA is installed for the whole
//! machine. Azure's Flexible Server chains to a public root the store has.
//!
//! No channel binding: the client offers SCRAM without it, which every
//! server takes.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_postgres::tls::{ChannelBinding, TlsConnect};

/// How to speak TLS to one server. `None` when the connection is plain
/// (`encrypt = false`), where tokio-postgres never asks for it.
#[derive(Clone)]
pub(super) struct Tls {
    config: Option<Arc<ClientConfig>>,
    host: String,
}

impl Tls {
    /// `trust_cert` takes any certificate, as sql's mssql connections do.
    pub(super) fn new(host: &str, encrypt: bool, trust_cert: bool) -> anyhow::Result<Self> {
        let config = if encrypt {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let builder = ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()?
                .dangerous();
            let verifier: Arc<dyn ServerCertVerifier> = if trust_cert {
                Arc::new(Unverified)
            } else {
                Arc::new(rustls_platform_verifier::Verifier::new(provider)?)
            };
            Some(Arc::new(
                builder
                    .with_custom_certificate_verifier(verifier)
                    .with_no_client_auth(),
            ))
        } else {
            None
        };
        Ok(Self {
            config,
            host: host.to_owned(),
        })
    }
}

impl TlsConnect<TcpStream> for Tls {
    type Stream = TlsStream;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<TlsStream>> + Send>>;

    fn connect(self, stream: TcpStream) -> Self::Future {
        Box::pin(async move {
            let config = self
                .config
                .ok_or_else(|| io::Error::other("TLS is off for this connection"))?;
            let name = ServerName::try_from(self.host).map_err(io::Error::other)?;
            tokio_rustls::TlsConnector::from(config)
                .connect(name, stream)
                .await
                .map(TlsStream)
        })
    }
}

pub(super) struct TlsStream(tokio_rustls::client::TlsStream<TcpStream>);

impl tokio_postgres::tls::TlsStream for TlsStream {
    fn channel_binding(&self) -> ChannelBinding {
        ChannelBinding::none()
    }
}

impl AsyncRead for TlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(context, buf)
    }
}

impl AsyncWrite for TlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(context, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(context)
    }
}

/// `trust_cert = true`: any certificate.
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
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
