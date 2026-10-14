//! Every mode against an in-process rustls server whose certificates are made here (rcgen):
//! what is checked, what a connection gets, and how each failure reads.

use super::*;
use datarig_core::profile::dsn::Scheme;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, Issuer, KeyPair, date_time_ymd};
use rustls::server::WebPkiClientVerifier;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A certificate authority.
struct Ca(CertifiedIssuer<'static, KeyPair>);

impl Ca {
    fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        Ca(CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap())
    }

    fn issuer(&self) -> &Issuer<'static, KeyPair> {
        &self.0
    }

    /// A leaf certificate for `names` (DNS names or IP addresses) and its key.
    fn leaf(&self, names: &[&str], edit: impl FnOnce(&mut CertificateParams)) -> (String, String) {
        let mut params = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
        edit(&mut params);
        let key = KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, self.issuer()).unwrap();
        (cert.pem(), key.serialize_pem())
    }
}

/// A scratch directory for the files of one test.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-tls-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn file(&self, name: &str, contents: &str) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, contents).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How the test server is set up.
struct Server {
    cert: String,
    key: String,
    /// The client certificates it requires come from this CA.
    clients: Option<String>,
    tls12_only: bool,
}

impl Server {
    fn new((cert, key): (String, String)) -> Self {
        Server { cert, key, clients: None, tls12_only: false }
    }

    fn config(&self) -> rustls::ServerConfig {
        let provider = provider();
        let versions: &[&rustls::SupportedProtocolVersion] =
            if self.tls12_only { &[&rustls::version::TLS12] } else { rustls::DEFAULT_VERSIONS };
        let chain = CertificateDer::pem_slice_iter(self.cert.as_bytes()).collect::<Result<Vec<_>, _>>().unwrap();
        let key = PrivateKeyDer::from_pem_slice(self.key.as_bytes()).unwrap();
        let builder =
            rustls::ServerConfig::builder_with_provider(provider.clone()).with_protocol_versions(versions).unwrap();
        let builder = match &self.clients {
            None => builder.with_no_client_auth(),
            Some(ca) => {
                let mut roots = RootCertStore::empty();
                for c in CertificateDer::pem_slice_iter(ca.as_bytes()) {
                    roots.add(c.unwrap()).unwrap();
                }
                let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider).build().unwrap();
                builder.with_client_cert_verifier(verifier)
            }
        };
        builder.with_single_cert(chain, key).unwrap()
    }
}

/// Connect to `server` as `host` with `settings`, write a byte and read the server's answer
/// (a server that refuses a client certificate says so only then, after a TLS 1.3 handshake).
async fn talk(server: &Server, settings: &TlsSettings, host: &str) -> Result<Encryption, TlsProblem> {
    let connector = Connector::new(settings, host)?.expect("a mode with TLS");
    let (client, far) = tokio::io::duplex(1 << 16);
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server.config()));
    let serve = tokio::spawn(async move {
        let mut s = acceptor.accept(far).await.ok()?;
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.ok()?;
        s.write_all(&b).await.ok()?;
        s.flush().await.ok()
    });
    let mut stream = connector.connect(client).await?;
    let encryption = connector.encryption(&stream);
    let mut b = [7u8; 1];
    let echoed = async {
        stream.write_all(&b).await?;
        stream.flush().await?;
        stream.read_exact(&mut b).await
    };
    echoed.await.map_err(|e| connector.io_problem(&e))?;
    assert_eq!(b, [7]);
    let _ = serve.await;
    Ok(encryption)
}

fn settings(mode: &str, ca: Option<&str>, client: Option<(&str, &str)>) -> TlsSettings {
    TlsSettings::new(Scheme::Postgres, mode, [ca, client.map(|c| c.0), client.map(|c| c.1)], false).unwrap()
}

const TLS13: TlsVersion = TlsVersion::Tls13;

#[tokio::test]
async fn prefer_and_require_encrypt_without_checking_the_certificate() {
    let ca = Ca::new("nobody trusts this");
    let server = Server::new(ca.leaf(&["elsewhere.test"], |_| {}));
    for mode in ["prefer", "require"] {
        let got = talk(&server, &settings(mode, None, None), "db.test").await;
        assert_eq!(got, Ok(Encryption::Tls { version: TLS13, verified: Verified::No }), "{mode}");
    }
    // Even an expired certificate: nothing of it is checked.
    let server = Server::new(ca.leaf(&["db.test"], |p| {
        (p.not_before, p.not_after) = (date_time_ymd(2000, 1, 1), date_time_ymd(2001, 1, 1));
    }));
    assert!(talk(&server, &settings("require", None, None), "db.test").await.is_ok());
}

#[tokio::test]
async fn verify_full_checks_the_chain_and_the_name() {
    let dir = Dir::new("full");
    let ca = Ca::new("datarig test CA");
    let ca_file = dir.file("ca.pem", &ca.0.pem());
    let server = Server::new(ca.leaf(&["db.test", "127.0.0.1", "::1"], |_| {}));
    let full = settings("verify-full", Some(&ca_file), None);
    let ok = Ok(Encryption::Tls { version: TLS13, verified: Verified::Full });
    assert_eq!(talk(&server, &full, "db.test").await, ok);
    // IP addresses are checked against the certificate's IP addresses.
    assert_eq!(talk(&server, &full, "127.0.0.1").await, ok);
    assert_eq!(talk(&server, &full, "[::1]").await, ok);
    assert_eq!(talk(&server, &full, "other.test").await, Err(TlsProblem::NameMismatch { host: "other.test".into() }));
    assert_eq!(talk(&server, &full, "10.0.0.1").await, Err(TlsProblem::NameMismatch { host: "10.0.0.1".into() }));
    // Another CA's certificate.
    let stranger = Server::new(Ca::new("someone else").leaf(&["db.test"], |_| {}));
    assert_eq!(talk(&stranger, &full, "db.test").await, Err(TlsProblem::Untrusted));
    // Out of its dates.
    let expired = Server::new(ca.leaf(&["db.test"], |p| {
        (p.not_before, p.not_after) = (date_time_ymd(2000, 1, 1), date_time_ymd(2001, 1, 1));
    }));
    assert_eq!(talk(&expired, &full, "db.test").await, Err(TlsProblem::Expired));
    let early = Server::new(ca.leaf(&["db.test"], |p| {
        (p.not_before, p.not_after) = (date_time_ymd(2090, 1, 1), date_time_ymd(2091, 1, 1));
    }));
    assert_eq!(talk(&early, &full, "db.test").await, Err(TlsProblem::NotYetValid));
}

#[tokio::test]
async fn verify_ca_checks_the_chain_and_not_the_name() {
    let dir = Dir::new("ca");
    let ca = Ca::new("datarig test CA");
    let ca_file = dir.file("ca.pem", &ca.0.pem());
    let server = Server::new(ca.leaf(&["elsewhere.test"], |_| {}));
    let chain = settings("verify-ca", Some(&ca_file), None);
    let ok = Ok(Encryption::Tls { version: TLS13, verified: Verified::Chain });
    assert_eq!(talk(&server, &chain, "db.test").await, ok);
    // `require` with a CA file is `verify-ca`.
    assert_eq!(talk(&server, &settings("require", Some(&ca_file), None), "db.test").await, ok);
    let stranger = Server::new(Ca::new("someone else").leaf(&["db.test"], |_| {}));
    assert_eq!(talk(&stranger, &chain, "db.test").await, Err(TlsProblem::Untrusted));
}

#[tokio::test]
async fn verify_full_without_a_ca_file_trusts_only_the_systems_certificates() {
    // The platform's verifier: a certificate of a CA the system does not trust is refused (no
    // keychain item is read on macOS, so nothing asks for a password).
    let server = Server::new(Ca::new("datarig test CA").leaf(&["db.test"], |_| {}));
    assert_eq!(talk(&server, &settings("verify-full", None, None), "db.test").await, Err(TlsProblem::Untrusted));
}

#[tokio::test]
async fn a_client_certificate_is_sent_and_its_refusal_is_told() {
    let dir = Dir::new("client");
    let ca = Ca::new("datarig test CA");
    let mut server = Server::new(ca.leaf(&["db.test"], |_| {}));
    server.clients = Some(ca.0.pem());
    let (cert, key) = ca.leaf(&[], |_| {});
    let client = (dir.file("client.pem", &cert), dir.file("client.key", &key));
    let with = settings("require", None, Some((&client.0, &client.1)));
    assert!(talk(&server, &with, "db.test").await.is_ok());
    // None, or one of a CA the server does not know.
    assert_eq!(talk(&server, &settings("require", None, None), "db.test").await, Err(TlsProblem::ClientCertRejected));
    let (cert, key) = Ca::new("someone else").leaf(&[], |_| {});
    let other = (dir.file("other.pem", &cert), dir.file("other.key", &key));
    let with_other = settings("require", None, Some((&other.0, &other.1)));
    assert_eq!(talk(&server, &with_other, "db.test").await, Err(TlsProblem::ClientCertRejected));
    // Under TLS 1.2 the server refuses it in the handshake.
    server.tls12_only = true;
    assert_eq!(talk(&server, &settings("require", None, None), "db.test").await, Err(TlsProblem::ClientCertRejected));
}

#[tokio::test]
async fn a_tls_1_2_server_is_told_as_such() {
    let mut server = Server::new(Ca::new("x").leaf(&["db.test"], |_| {}));
    server.tls12_only = true;
    let got = talk(&server, &settings("require", None, None), "db.test").await;
    assert_eq!(got, Ok(Encryption::Tls { version: TlsVersion::Tls12, verified: Verified::No }));
}

#[test]
fn files_that_cannot_be_used_are_errors_naming_the_file() {
    let dir = Dir::new("files");
    let ca = Ca::new("datarig test CA");
    let (cert, key) = ca.leaf(&[], |_| {});
    let (cert_file, key_file) = (dir.file("c.pem", &cert), dir.file("c.key", &key));
    let missing = dir.0.join("missing.pem").to_string_lossy().into_owned();
    let problem = |s: TlsSettings| Connector::new(&s, "db.test").expect_err("an error");
    let file_problem = |s: TlsSettings| match problem(s) {
        TlsProblem::File { file, problem, .. } => (file, problem),
        other => panic!("not a file problem: {other:?}"),
    };
    // Unreadable (unknown is not absent).
    let (file, p) = file_problem(settings("verify-full", Some(&missing), None));
    assert!(matches!((file, p), (TlsFile::Ca, FileProblem::Unreadable(_))));
    let (file, p) = file_problem(settings("require", None, Some((&missing, &key_file))));
    assert!(matches!((file, p), (TlsFile::Cert, FileProblem::Unreadable(_))));
    // A CA file without a certificate, a key file without a key.
    let empty = dir.file("empty.pem", "not a certificate\n");
    assert_eq!(file_problem(settings("verify-ca", Some(&empty), None)), (TlsFile::Ca, FileProblem::NoCertificate));
    assert_eq!(
        file_problem(settings("require", None, Some((&empty, &key_file)))),
        (TlsFile::Cert, FileProblem::NoCertificate)
    );
    assert_eq!(
        file_problem(settings("require", None, Some((&cert_file, &cert_file)))),
        (TlsFile::Key, FileProblem::NoKey)
    );
    // An encrypted key, in either form.
    for (name, text) in [
        ("p8.key", "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIB\n-----END ENCRYPTED PRIVATE KEY-----\n"),
        (
            "rsa.key",
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nMIIB\n-----END RSA PRIVATE KEY-----\n",
        ),
    ] {
        let encrypted = dir.file(name, text);
        assert_eq!(
            file_problem(settings("require", None, Some((&cert_file, &encrypted)))),
            (TlsFile::Key, FileProblem::Encrypted),
            "{name}"
        );
    }
    // A key that is not the certificate's.
    let (_, other_key) = ca.leaf(&[], |_| {});
    let other_key = dir.file("other.key", &other_key);
    let (file, p) = file_problem(settings("require", None, Some((&cert_file, &other_key))));
    assert!(matches!((file, p), (TlsFile::Key, FileProblem::Invalid(_))));
    // A name that names nothing a certificate could is refused where names are checked.
    let ca_file = dir.file("ca.pem", &ca.0.pem());
    let bad = Connector::new(&settings("verify-full", Some(&ca_file), None), "not a host").err();
    assert_eq!(bad, Some(TlsProblem::HostNotCheckable { host: "not a host".into() }));
    assert!(Connector::new(&settings("verify-ca", Some(&ca_file), None), "not a host").is_ok());
}

#[test]
fn disable_makes_no_connector() {
    assert!(Connector::new(&TlsSettings::plain(), "db.test").unwrap().is_none());
}

#[test]
fn channel_binding_hashes_the_certificate_with_its_signatures_hash() {
    let ca = Ca::new("x");
    let (pem, _) = ca.leaf(&["db.test"], |_| {});
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).unwrap();
    // rcgen signs with ECDSA P-256 / SHA-256 by default.
    assert_eq!(binding::end_point(&der), Some(ring::digest::digest(&ring::digest::SHA256, &der).as_ref().to_vec()));
    // Ed25519 has no hash of its own: no binding.
    let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let cert = CertificateParams::new(vec!["db.test".to_string()]).unwrap().self_signed(&key).unwrap();
    assert_eq!(binding::end_point(cert.der()), None);
    assert_eq!(binding::end_point(b"\x30\x03\x02\x01\x00"), None);
}

/// A self-signed server certificate copied to the client as its CA file (PostgreSQL's own
/// documentation sets TLS up so; OpenSSL marks such a certificate as a CA): trusted as libpq
/// trusts it, its dates and name still checked.
#[tokio::test]
async fn a_self_signed_server_certificate_can_be_its_own_ca_file() {
    let dir = Dir::new("selfsigned");
    let self_signed = |edit: fn(&mut CertificateParams)| {
        let mut params = CertificateParams::new(vec!["db.test".to_string()]).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        edit(&mut params);
        let key = KeyPair::generate().unwrap();
        (params.self_signed(&key).unwrap().pem(), key.serialize_pem())
    };
    let (cert, key) = self_signed(|_| {});
    let server = Server::new((cert.clone(), key));
    let ca = dir.file("root.crt", &cert);
    let full = settings("verify-full", Some(&ca), None);
    assert_eq!(talk(&server, &full, "db.test").await, Ok(Encryption::Tls { version: TLS13, verified: Verified::Full }));
    assert_eq!(talk(&server, &full, "other.test").await, Err(TlsProblem::NameMismatch { host: "other.test".into() }));
    let chain = settings("verify-ca", Some(&ca), None);
    assert_eq!(
        talk(&server, &chain, "other.test").await,
        Ok(Encryption::Tls { version: TLS13, verified: Verified::Chain })
    );
    // `require` with that file is `verify-ca`, as in libpq.
    assert!(talk(&server, &settings("require", Some(&ca), None), "db.test").await.is_ok());
    // Its dates still count.
    let (cert, key) =
        self_signed(|p| (p.not_before, p.not_after) = (date_time_ymd(2000, 1, 1), date_time_ymd(2001, 1, 1)));
    let expired = Server::new((cert.clone(), key));
    let ca = dir.file("expired.crt", &cert);
    assert_eq!(talk(&expired, &settings("verify-ca", Some(&ca), None), "db.test").await, Err(TlsProblem::Expired));
    let (cert, key) =
        self_signed(|p| (p.not_before, p.not_after) = (date_time_ymd(2090, 1, 1), date_time_ymd(2091, 1, 1)));
    let early = Server::new((cert.clone(), key));
    let ca = dir.file("early.crt", &cert);
    assert_eq!(talk(&early, &settings("verify-full", Some(&ca), None), "db.test").await, Err(TlsProblem::NotYetValid));
}

#[tokio::test]
async fn a_certificate_for_another_purpose_is_told_as_such() {
    let dir = Dir::new("purpose");
    let ca = Ca::new("datarig test CA");
    let ca_file = dir.file("ca.pem", &ca.0.pem());
    let server = Server::new(ca.leaf(&["db.test"], |p| {
        p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    }));
    assert_eq!(
        talk(&server, &settings("verify-full", Some(&ca_file), None), "db.test").await,
        Err(TlsProblem::NotForServer)
    );
}

#[tokio::test]
async fn a_ca_bundle_with_an_unusable_certificate_still_works() {
    let dir = Dir::new("bundle");
    let ca = Ca::new("datarig test CA");
    let server = Server::new(ca.leaf(&["db.test"], |_| {}));
    // A certificate webpki cannot take as a trust anchor (not DER), then the real one.
    let junk = "-----BEGIN CERTIFICATE-----\nAAECAwQF\n-----END CERTIFICATE-----\n";
    let bundle = dir.file("bundle.pem", &format!("{junk}{}", ca.0.pem()));
    assert!(talk(&server, &settings("verify-full", Some(&bundle), None), "db.test").await.is_ok());
    // Nothing usable: an error naming the file.
    let only_junk = dir.file("junk.pem", junk);
    let err = Connector::new(&settings("verify-full", Some(&only_junk), None), "db.test").expect_err("unusable");
    assert!(matches!(err, TlsProblem::File { file: TlsFile::Ca, problem: FileProblem::Invalid(_), .. }), "{err:?}");
    // A CA file is read only where it is used: `prefer` and `require` without one never open it.
    let missing = dir.0.join("missing.pem").to_string_lossy().into_owned();
    assert!(Connector::new(&settings("prefer", Some(&missing), None), "db.test").is_ok());
}

#[test]
fn certificate_dates_are_read_in_both_time_forms() {
    let ca = Ca::new("x");
    // rcgen writes dates before 2050 as UTCTime, later ones as GeneralizedTime.
    let (pem, _) = ca
        .leaf(&["db.test"], |p| (p.not_before, p.not_after) = (date_time_ymd(2001, 2, 3), date_time_ymd(2091, 12, 31)));
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).unwrap();
    assert_eq!(der::validity(&der), Some((981_158_400, 3_849_897_600)));
    assert_eq!(der::validity(b"\x30\x00"), None);
}
