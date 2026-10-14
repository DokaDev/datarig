//! TLS for the database connections, with rustls (the `ring` provider, TLS 1.2 and 1.3): a
//! profile's [`TlsSettings`] become a [`Connector`] (its files read, its verifier chosen), which
//! does the handshake over a stream a driver opened (a socket, or an SSH tunnel's channel) and
//! says what the connection got ([`Encryption`]) or why it failed ([`TlsProblem`]).
//!
//! What is checked of the server's certificate follows the mode:
//!
//! * `prefer`, `require`: nothing (the handshake's own signatures still are);
//! * `verify-ca`: its chain, against the CA file only, not its name;
//! * `verify-full`: its chain and its name (the profile's host, also over a tunnel): against the
//!   CA file when there is one (only its certificates are trusted), else the system's trusted
//!   certificates through the platform's verifier (macOS: the Security framework's trust
//!   evaluation, which reads the trust settings and never a keychain item, so it never asks for
//!   a keychain password; Windows: CryptoAPI; Linux and others: the system's certificate files).

use datarig_core::fault::Fault;
use datarig_core::tls::{Encryption, FileProblem, TlsFile, TlsMode, TlsProblem, TlsSettings, TlsVersion, Verified};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{AlertDescription, CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::io;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};

pub use tokio_rustls::client::TlsStream;

mod binding;
mod der;
pub use binding::server_end_point;

/// The handshake of one connection, ready: the client's configuration and the name the server's
/// certificate is checked against.
#[derive(Clone)]
pub struct Connector {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
    host: String,
    mode: TlsMode,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector").field("host", &self.host).field("mode", &self.mode).finish()
    }
}

impl Connector {
    /// The connector of `settings` for a server at `host` (the profile's host, as its
    /// certificate names it: a DNS name or an IP address, IPv6 with or without brackets). Its
    /// files are read now. `None` for a mode without TLS.
    pub fn new(settings: &TlsSettings, host: &str) -> Result<Option<Self>, TlsProblem> {
        if settings.mode == TlsMode::Disable {
            return Ok(None);
        }
        let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host).to_string();
        let provider = provider();
        // The CA file is read only where it is used: the modes that check the chain.
        let roots = match settings.mode.verifies() {
            true => settings.ca_file.as_deref().map(ca_roots).transpose()?,
            false => None,
        };
        let verifier: Arc<dyn ServerCertVerifier> = match (settings.mode, roots) {
            (TlsMode::Prefer | TlsMode::Require, _) => Arc::new(Unverified(provider.clone())),
            (TlsMode::VerifyCa, Some((roots, anchors))) => {
                Arc::new(Anchored { inner: Arc::new(ChainOnly(webpki(roots, &provider)?)), anchors, name: false })
            }
            (TlsMode::VerifyFull, Some((roots, anchors))) => {
                Arc::new(Anchored { inner: webpki(roots, &provider)?, anchors, name: true })
            }
            (TlsMode::VerifyFull, None) => system_verifier(&provider)?,
            (TlsMode::VerifyCa, None) => return Err(TlsProblem::NeedsCaFile),
            (TlsMode::Disable, _) => return Ok(None),
        };
        let (name, sni) = server_name(&host);
        if name.is_none() && settings.mode == TlsMode::VerifyFull {
            return Err(TlsProblem::HostNotCheckable { host });
        }
        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| TlsProblem::Handshake(Fault::other(e.to_string())))?
            .dangerous()
            .with_custom_certificate_verifier(verifier);
        let mut config = match (&settings.cert_file, &settings.key_file) {
            (Some(cert), Some(key)) => {
                let chain = certificates(TlsFile::Cert, cert)?;
                let private = private_key(key)?;
                builder.with_client_auth_cert(chain, private).map_err(|e| TlsProblem::File {
                    file: TlsFile::Key,
                    path: key.clone(),
                    problem: FileProblem::Invalid(Fault::other(e.to_string())),
                })?
            }
            (Some(_), None) => return Err(TlsProblem::CertWithoutKey),
            (None, Some(_)) => return Err(TlsProblem::KeyWithoutCert),
            (None, None) => builder.with_no_client_auth(),
        };
        config.enable_sni = sni;
        // A name that names nothing a certificate could: never sent (no SNI) nor checked
        // (the modes that check it refused it above).
        let name = name.unwrap_or_else(|| ServerName::try_from("invalid").expect("a valid name"));
        Ok(Some(Self { config: Arc::new(config), name, host, mode: settings.mode }))
    }

    /// The mode it was made for.
    pub fn mode(&self) -> TlsMode {
        self.mode
    }

    /// Whether a server that does not offer TLS is an error (every mode but `prefer`).
    pub fn required(&self) -> bool {
        self.mode != TlsMode::Prefer
    }

    /// The handshake over `stream`.
    pub async fn connect<S>(&self, stream: S) -> Result<TlsStream<S>, TlsProblem>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let connector = tokio_rustls::TlsConnector::from(self.config.clone());
        connector.connect(self.name.clone(), stream).await.map_err(|e| self.io_problem(&e))
    }

    /// What a connection made by [`Self::connect`] got.
    pub fn encryption<S>(&self, stream: &TlsStream<S>) -> Encryption {
        let version = match stream.get_ref().1.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_2) => TlsVersion::Tls12,
            _ => TlsVersion::Tls13,
        };
        let verified = match self.mode {
            TlsMode::VerifyFull => Verified::Full,
            TlsMode::VerifyCa => Verified::Chain,
            _ => Verified::No,
        };
        Encryption::Tls { version, verified }
    }

    /// The problem behind an io error of the handshake, or of a TLS connection afterwards (a
    /// server that refuses the client certificate says so after a TLS 1.3 handshake, on the
    /// first read): rustls's own error inside it, else the io error itself.
    pub fn io_problem(&self, e: &io::Error) -> TlsProblem {
        problem_in_io(e, &self.host).unwrap_or_else(|| TlsProblem::Handshake(Fault::io(e)))
    }
}

/// The TLS problem inside an io error of a TLS connection to `host`, if it has one.
pub fn problem_in_io(e: &io::Error, host: &str) -> Option<TlsProblem> {
    let inner = e.get_ref()?;
    let tls = inner.downcast_ref::<rustls::Error>()?;
    Some(problem(tls, host))
}

/// A rustls error of a connection to `host`, as a problem the app words.
pub fn problem(e: &rustls::Error, host: &str) -> TlsProblem {
    use AlertDescription as A;
    use CertificateError as C;
    use rustls::Error::{AlertReceived, InvalidCertificate};
    match e {
        InvalidCertificate(C::NotValidForName | C::NotValidForNameContext { .. }) => {
            TlsProblem::NameMismatch { host: host.to_string() }
        }
        InvalidCertificate(C::Expired | C::ExpiredContext { .. }) => TlsProblem::Expired,
        InvalidCertificate(C::NotValidYet | C::NotValidYetContext { .. }) => TlsProblem::NotYetValid,
        InvalidCertificate(C::Revoked) => TlsProblem::Revoked,
        InvalidCertificate(C::InvalidPurpose | C::InvalidPurposeContext { .. }) => TlsProblem::NotForServer,
        // The platform's verifier says why in words only (an expired or otherwise untrusted
        // certificate, `Other`): not trusted, the words in the log.
        InvalidCertificate(C::UnknownIssuer | C::BadSignature | C::Other(_)) => TlsProblem::Untrusted,
        // What a server answers about the client's certificate (a server never sends these
        // about its own).
        AlertReceived(
            A::BadCertificate
            | A::UnsupportedCertificate
            | A::CertificateRevoked
            | A::CertificateExpired
            | A::CertificateUnknown
            | A::UnknownCA
            | A::CertificateRequired,
        ) => TlsProblem::ClientCertRejected,
        _ => TlsProblem::Handshake(Fault::other(e.to_string())),
    }
}

/// The crypto provider: ring, with its default cipher suites and key exchanges.
fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The name the server's certificate is checked against, and whether to send it (SNI: DNS names
/// only). `None` for a host that is neither a DNS name nor an IP address.
fn server_name(host: &str) -> (Option<ServerName<'static>>, bool) {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return (Some(ServerName::IpAddress(ip.into())), false);
    }
    match ServerName::try_from(host.to_string()) {
        Ok(name) => (Some(name), true),
        Err(_) => (None, false),
    }
}

/// The path of a file the profile names (`~/` is the home directory).
fn expand(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(path),
    }
}

/// The bytes of a file of the settings.
fn read(file: TlsFile, path: &str) -> Result<Vec<u8>, TlsProblem> {
    std::fs::read(expand(path)).map_err(|e| TlsProblem::File {
        file,
        path: path.to_string(),
        problem: FileProblem::Unreadable(Fault::io(&e)),
    })
}

/// The certificates (PEM) of a file of the settings: at least one.
fn certificates(file: TlsFile, path: &str) -> Result<Vec<CertificateDer<'static>>, TlsProblem> {
    let bytes = read(file, path)?;
    let fail = |problem| TlsProblem::File { file, path: path.to_string(), problem };
    let certs = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| fail(FileProblem::Invalid(Fault::other(e.to_string()))))?;
    match certs.is_empty() {
        true => Err(fail(FileProblem::NoCertificate)),
        false => Ok(certs),
    }
}

/// The CA file's certificates, as trust anchors.
/// The CA file's certificates as trust anchors, and the certificates themselves. One that cannot
/// be a trust anchor (an old or odd certificate in a large bundle) is left out; a file none of
/// whose certificates can be one is an error.
fn ca_roots(path: &str) -> Result<(RootCertStore, Vec<CertificateDer<'static>>), TlsProblem> {
    let certs = certificates(TlsFile::Ca, path)?;
    let mut roots = RootCertStore::empty();
    let (added, ignored) = roots.add_parsable_certificates(certs.iter().cloned());
    if added == 0 {
        let why = format!("none of its {ignored} certificates can be a trust anchor");
        return Err(TlsProblem::File {
            file: TlsFile::Ca,
            path: path.to_string(),
            problem: FileProblem::Invalid(Fault::other(why)),
        });
    }
    Ok((roots, certs))
}

/// The client's private key (PEM: PKCS#8, PKCS#1 RSA or SEC1 EC). An encrypted one is refused:
/// there is no passphrase to open it with.
fn private_key(path: &str) -> Result<PrivateKeyDer<'static>, TlsProblem> {
    let bytes = read(TlsFile::Key, path)?;
    let fail = |problem| TlsProblem::File { file: TlsFile::Key, path: path.to_string(), problem };
    let text = String::from_utf8_lossy(&bytes);
    if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") || text.contains("Proc-Type: 4,ENCRYPTED") {
        return Err(fail(FileProblem::Encrypted));
    }
    PrivateKeyDer::from_pem_slice(&bytes).map_err(|e| match e {
        rustls::pki_types::pem::Error::NoItemsFound => fail(FileProblem::NoKey),
        e => fail(FileProblem::Invalid(Fault::other(e.to_string()))),
    })
}

/// A verifier of the chain and the name against `roots`.
fn webpki(roots: RootCertStore, provider: &Arc<CryptoProvider>) -> Result<Arc<WebPkiServerVerifier>, TlsProblem> {
    WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|e| TlsProblem::Handshake(Fault::other(e.to_string())))
}

/// The platform's verifier (the system's trusted certificates), made once per process: on Linux
/// it reads every certificate of the system's store.
fn system_verifier(provider: &Arc<CryptoProvider>) -> Result<Arc<dyn ServerCertVerifier>, TlsProblem> {
    static SYSTEM: Mutex<Option<Arc<rustls_platform_verifier::Verifier>>> = Mutex::new(None);
    let mut cached = SYSTEM.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = cached.as_ref() {
        return Ok(v.clone());
    }
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map(Arc::new)
        .map_err(|e| TlsProblem::SystemRoots(Fault::other(e.to_string())))?;
    *cached = Some(verifier.clone());
    Ok(verifier)
}

/// `prefer` and `require`: the server's certificate is not checked; the signatures of the
/// handshake are (a server that cannot sign with its certificate's key fails).
#[derive(Debug)]
struct Unverified(Arc<CryptoProvider>);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `verify-ca` and `verify-full` against a CA file whose certificates include the server's own (a
/// self-signed server certificate copied to the client, as PostgreSQL's documentation sets one
/// up, often marked as a CA): trusted as it is, as libpq trusts it, once its dates and (with
/// `name`) its name are checked; anything else as `inner` checks it. The handshake's signatures
/// are always `inner`'s to check.
#[derive(Debug)]
struct Anchored {
    inner: Arc<dyn ServerCertVerifier>,
    anchors: Vec<CertificateDer<'static>>,
    name: bool,
}

impl ServerCertVerifier for Anchored {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verdict = self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now);
        if verdict.is_ok() || !self.anchors.iter().any(|a| a.as_ref() == end_entity.as_ref()) {
            return verdict;
        }
        let (not_before, not_after) =
            der::validity(end_entity).ok_or(rustls::Error::InvalidCertificate(CertificateError::BadEncoding))?;
        let now = now.as_secs() as i64;
        if now < not_before {
            return Err(rustls::Error::InvalidCertificate(CertificateError::NotValidYet));
        }
        if now > not_after {
            return Err(rustls::Error::InvalidCertificate(CertificateError::Expired));
        }
        if self.name {
            let cert = webpki::EndEntityCert::try_from(end_entity)
                .map_err(|_| rustls::Error::InvalidCertificate(CertificateError::BadEncoding))?;
            cert.verify_is_valid_for_subject_name(server_name)
                .map_err(|_| rustls::Error::InvalidCertificate(CertificateError::NotValidForName))?;
        }
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

/// `verify-ca`: the chain against the CA file, and not the name. The chain is checked before the
/// name, so a certificate whose only fault is its name has a good chain.
#[derive(Debug)]
struct ChainOnly(Arc<WebPkiServerVerifier>);

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.0.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests;
