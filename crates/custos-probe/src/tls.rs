//! Shared rustls plumbing for the HTTP(S) and TLS-certificate probes
//! (survey §26.11: transport hardening is centralized, not per-probe).
//!
//! Two configurations exist: the verifying one (system/webpki roots, for
//! plain HTTP(S) health checks) and an accept-any one used by the
//! TLS-certificate probe so that *expired or not-yet-valid* certificates
//! still complete the handshake and their validity dates can be read.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{ClientConfig, ClientConnection, RootCertStore, SignatureScheme, StreamOwned};
use std::net::TcpStream;
use std::sync::Arc;

/// A TLS stream over a blocking TCP socket.
pub(crate) type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// WebPKI trust anchors compiled into the binary.
pub(crate) fn root_store() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    roots
}

/// Builds a client config; `verify_tls = false` accepts any server
/// certificate (for expiry inspection of self-signed endpoints).
pub(crate) fn client_config(verify_tls: bool) -> Arc<ClientConfig> {
    if verify_tls {
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(root_store())
                .with_no_client_auth(),
        )
    } else {
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(provider.clone()))
                .with_safe_default_protocol_versions()
                .expect("rustls: TLS 1.2/1.3 are always safe")
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert(provider)))
                .with_no_client_auth(),
        )
    }
}

/// Verifier that accepts every certificate and signature. Its only job is
/// to let a handshake complete so `peer_certificates()` can be inspected.
#[derive(Debug)]
struct AcceptAnyServerCert(rustls::crypto::CryptoProvider);

impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Completes a TLS handshake over `stream` against `server_name`.
/// `verify_tls = false` skips chain/expiry validation (cert probe).
pub(crate) fn connect_tls(
    stream: TcpStream,
    server_name: &str,
    verify_tls: bool,
) -> Result<TlsStream, String> {
    let name = rustls::pki_types::ServerName::try_from(server_name.to_owned())
        .map_err(|_| format!("invalid TLS server name {server_name:?}"))?;
    let conn = ClientConnection::new(client_config(verify_tls), name)
        .map_err(|err| format!("tls setup failed: {err}"))?;
    let mut stream = StreamOwned::new(conn, stream);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|err| format!("tls handshake failed: {err}"))?;
    Ok(stream)
}
