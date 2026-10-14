use super::*;

const PG: Scheme = Scheme::Postgres;
const MY: Scheme = Scheme::MySql;

fn mode(scheme: Scheme, text: &str) -> Result<TlsMode, TlsProblem> {
    TlsSettings::new(scheme, text, [None, None, None], false).map(|s| s.mode)
}

#[test]
fn each_driver_spells_the_modes_as_its_own_client_does() {
    let pg: Vec<_> = TlsMode::ALL.iter().map(|m| m.name(PG)).collect();
    assert_eq!(pg, ["disable", "prefer", "require", "verify-ca", "verify-full"]);
    let my: Vec<_> = TlsMode::ALL.iter().map(|m| m.name(MY)).collect();
    assert_eq!(my, ["DISABLED", "PREFERRED", "REQUIRED", "VERIFY_CA", "VERIFY_IDENTITY"]);
    for m in TlsMode::ALL {
        assert_eq!(TlsMode::parse(PG, m.name(PG)), Some(m));
        assert_eq!(TlsMode::parse(MY, m.name(MY)), Some(m));
        assert_eq!(TlsMode::parse(MY, &m.name(MY).to_lowercase()), Some(m));
        assert_eq!(TlsMode::ALL[m.index()], m);
    }
    // libpq's names are lower case only; neither driver reads the other's.
    assert_eq!(TlsMode::parse(PG, "Require"), None);
    assert_eq!(TlsMode::parse(PG, "REQUIRED"), None);
    assert_eq!(TlsMode::parse(MY, "verify-full"), None);
}

#[test]
fn a_profile_without_a_mode_connects_as_before() {
    // PostgreSQL profiles without `sslmode` always connected unencrypted.
    assert_eq!(mode(PG, ""), Ok(TlsMode::Disable));
    // MySQL had no TLS; its default is now the mysql client's.
    assert_eq!(mode(MY, ""), Ok(TlsMode::Prefer));
    // The PostgreSQL value the form wrote into MySQL profiles was never read: no mode.
    for legacy in ["disable", "prefer"] {
        assert_eq!(mode(MY, legacy), Ok(TlsMode::Prefer), "{legacy}");
    }
    // A stricter libpq spelling (written by hand: the keys are libpq's) means its MySQL mode,
    // never a weaker one.
    assert_eq!(mode(MY, "require"), Ok(TlsMode::Require));
    assert_eq!(mode(MY, "verify-full"), Ok(TlsMode::VerifyFull));
    assert_eq!(mode(MY, "verify-ca"), Err(TlsProblem::NeedsCaFile));
    // A MySQL profile that pins the server's key keeps its unencrypted, key-protected login.
    let pinned = |m: &str| TlsSettings::new(MY, m, [None, None, None], true).map(|s| s.mode);
    assert_eq!(pinned(""), Ok(TlsMode::Disable));
    assert_eq!(pinned("prefer"), Ok(TlsMode::Disable));
    assert_eq!(pinned("REQUIRED"), Ok(TlsMode::Require));
}

#[test]
fn an_unknown_mode_is_an_error_not_a_default() {
    assert_eq!(mode(PG, "bogus"), Err(TlsProblem::UnknownMode("bogus".into())));
    assert_eq!(mode(PG, "allow"), Err(TlsProblem::UnknownMode("allow".into())));
    assert_eq!(mode(MY, "VERIFY"), Err(TlsProblem::UnknownMode("VERIFY".into())));
}

#[test]
fn require_with_a_ca_file_checks_the_chain_and_verify_ca_needs_one() {
    let ca = Some("/etc/db/ca.pem");
    let s = TlsSettings::new(PG, "require", [ca, None, None], false).unwrap();
    assert_eq!((s.mode, s.ca_file.as_deref()), (TlsMode::VerifyCa, ca));
    let s = TlsSettings::new(MY, "REQUIRED", [ca, None, None], false).unwrap();
    assert_eq!(s.mode, TlsMode::VerifyCa);
    // Without a CA file, `require` stays unverified, `verify-ca` cannot work.
    assert_eq!(mode(PG, "require"), Ok(TlsMode::Require));
    assert_eq!(mode(PG, "verify-ca"), Err(TlsProblem::NeedsCaFile));
    assert_eq!(mode(MY, "VERIFY_CA"), Err(TlsProblem::NeedsCaFile));
    // `verify-full` falls back to the system's trusted certificates.
    assert_eq!(mode(PG, "verify-full"), Ok(TlsMode::VerifyFull));
    // `prefer` stays unverified even with a CA file (as libpq).
    let s = TlsSettings::new(PG, "prefer", [ca, None, None], false).unwrap();
    assert_eq!(s.mode, TlsMode::Prefer);
}

#[test]
fn a_client_certificate_and_its_key_go_together() {
    let (cert, key) = (Some("/c/client.pem"), Some("~/c/client.key"));
    let s = TlsSettings::new(PG, "verify-full", [None, cert, key], false).unwrap();
    assert_eq!((s.cert_file.as_deref(), s.key_file.as_deref()), (cert, key));
    assert_eq!(TlsSettings::new(PG, "require", [None, cert, None], false), Err(TlsProblem::CertWithoutKey));
    assert_eq!(TlsSettings::new(PG, "require", [None, None, key], false), Err(TlsProblem::KeyWithoutCert));
    // Blank paths are no path.
    let s = TlsSettings::new(PG, "require", [Some("  "), Some(""), None], false).unwrap();
    assert_eq!((s.ca_file, s.cert_file), (None, None));
}

#[test]
fn the_system_ca_file_means_the_systems_certificates_and_the_name_checked() {
    for m in ["prefer", "require", "verify-ca", "verify-full"] {
        let s = TlsSettings::new(PG, m, [Some("system"), None, None], false).unwrap();
        assert_eq!((s.mode, s.ca_file), (TlsMode::VerifyFull, None), "{m}");
    }
    assert_eq!(TlsSettings::new(PG, "disable", [Some("system"), None, None], false), Ok(TlsSettings::plain()));
}

#[test]
fn with_tls_off_the_files_are_not_used_nor_checked() {
    let s = TlsSettings::new(PG, "disable", [Some("ca.pem"), Some("c.pem"), None], false);
    assert_eq!(s, Ok(TlsSettings::plain()));
}

#[test]
fn relative_paths_are_refused() {
    for (i, file) in [TlsFile::Ca, TlsFile::Cert, TlsFile::Key].into_iter().enumerate() {
        let mut files = [None, None, None];
        files[i] = Some("certs/x.pem");
        assert_eq!(
            TlsSettings::new(PG, "require", files, false),
            Err(TlsProblem::PathRelative { file, path: "certs/x.pem".into() })
        );
    }
}
