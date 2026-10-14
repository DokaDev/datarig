use super::*;
use russh::keys::{Algorithm, PrivateKey};

fn key() -> PublicKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap().public_key().clone()
}

fn line(spec: &str, k: &PublicKey) -> String {
    let o = k.to_openssh().unwrap();
    let mut w = o.split_whitespace();
    format!("{spec} {} {}", w.next().unwrap(), w.next().unwrap())
}

/// A scratch directory of the test, removed when it goes out of scope.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-kh-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

fn files(tag: &str, user: &str, app: Option<&str>) -> (Scratch, KnownHosts) {
    let d = dir(tag);
    std::fs::write(d.join("user"), user).unwrap();
    if let Some(a) = app {
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub").join("app"), a).unwrap();
    }
    let kh = KnownHosts { user: Some(d.join("user")), app: d.join("sub").join("app") };
    (d, kh)
}

/// A hashed entry as `ssh-keygen -H` writes it.
fn hashed(spec: &str) -> String {
    let salt = [7u8; 20];
    let mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap().chain_update(spec.as_bytes()).finalize().into_bytes();
    let b = data_encoding::BASE64;
    format!("|1|{}|{}", b.encode(&salt), b.encode(&mac))
}

#[test]
fn keys_the_user_trusts_in_openssh_are_known_in_every_form() {
    let k = key();
    for spec in [
        "bastion.example.com".to_string(),
        "other,bastion.example.com".to_string(),
        "*.example.com".to_string(),
        "BASTION.example.com".to_string(),
        "bastion.example.co?".to_string(),
        hashed("bastion.example.com"),
    ] {
        let (_d, kh) = files("forms", &format!("# comment\n\n{}\n", line(&spec, &k)), None);
        assert_eq!(kh.check("bastion.example.com", 22, &k).unwrap(), Verdict::Known, "{spec}");
    }
    // A port other than 22 is `[name]:port`, hashed too.
    for spec in ["[bastion.example.com]:2222".to_string(), hashed("[bastion.example.com]:2222")] {
        let (_d, kh) = files("port", &line(&spec, &k), None);
        assert_eq!(kh.check("bastion.example.com", 2222, &k).unwrap(), Verdict::Known, "{spec}");
        assert_eq!(kh.check("bastion.example.com", 22, &k).unwrap(), Verdict::Unknown, "{spec}");
    }
    // A negation excludes.
    let (_d, kh) = files("neg", &line("*.example.com,!bastion.example.com", &k), None);
    assert_eq!(kh.check("bastion.example.com", 22, &k).unwrap(), Verdict::Unknown);
    assert_eq!(kh.check("db.example.com", 22, &k).unwrap(), Verdict::Known);
}

#[test]
fn another_key_for_the_host_is_a_change_never_unknown() {
    let (old, new) = (key(), key());
    let (_d, kh) = files("changed", &format!("{}\n", line("bastion", &old)), None);
    match kh.check("bastion", 22, &new).unwrap() {
        Verdict::Changed(stored) => {
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].line, 1);
            assert_eq!(stored[0].fingerprint, fingerprint(&old));
            assert_eq!(stored[0].algorithm, "ssh-ed25519");
            assert!(stored[0].fingerprint.starts_with("SHA256:"));
        }
        v => panic!("{v:?}"),
    }
    // A key of another type counts as a change too.
    let rsa = PrivateKey::random(&mut rand::rng(), Algorithm::Ecdsa { curve: russh::keys::EcdsaCurve::NistP256 })
        .unwrap()
        .public_key()
        .clone();
    assert!(matches!(kh.check("bastion", 22, &rsa).unwrap(), Verdict::Changed(_)));
}

#[test]
fn datarigs_file_decides_for_its_hosts_and_is_the_only_one_written() {
    let (old, new) = (key(), key());
    let user_text = format!("{}\n", line("bastion", &old));
    let (_d, kh) = files("trust", &user_text, None);
    assert!(matches!(kh.check("bastion", 22, &new).unwrap(), Verdict::Changed(_)));
    kh.trust("bastion", 22, &new).unwrap();
    assert_eq!(kh.check("bastion", 22, &new).unwrap(), Verdict::Known);
    assert!(matches!(kh.check("bastion", 22, &old).unwrap(), Verdict::Changed(_)), "the app's entry decides");
    assert_eq!(std::fs::read_to_string(kh.user.as_ref().unwrap()).unwrap(), user_text, "OpenSSH's file is untouched");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&kh.app).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // Trusting again replaces the host's line and keeps the others.
    let other = key();
    kh.trust("[db]:2222", 22, &other).unwrap();
    let newer = key();
    kh.trust("bastion", 22, &newer).unwrap();
    let text = std::fs::read_to_string(&kh.app).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.contains(&line("[db]:2222", &other)), "{text}");
    assert!(text.ends_with(&format!("{}\n", line("bastion", &newer))), "{text}");
    assert_eq!(kh.check("bastion", 22, &newer).unwrap(), Verdict::Known);
}

#[test]
fn a_revoked_key_is_refused_wherever_it_is_listed() {
    let k = key();
    let (_d, kh) = files("revoked", &format!("{}\n@revoked {}\n", line("bastion", &k), line("*", &k)), None);
    assert!(matches!(kh.check("bastion", 22, &k).unwrap(), Verdict::Revoked(s) if s.line == 2));
}

#[test]
fn lines_it_cannot_read_are_skipped_and_an_unreadable_file_is_an_error() {
    let k = key();
    let text = format!(
        "@cert-authority *.example.com {}\nbastion sk-unknown-type AAAA\nbroken\n{}\n",
        line("", &k).trim(),
        line("bastion", &k)
    );
    let (_d, kh) = files("skip", &text, None);
    assert_eq!(kh.check("bastion", 22, &k).unwrap(), Verdict::Known);
    // A file that is not there has no keys; one that cannot be read is not "no keys".
    let none_dir = dir("none");
    let none = KnownHosts { user: None, app: none_dir.join("missing") };
    assert_eq!(none.check("bastion", 22, &k).unwrap(), Verdict::Unknown);
    let d = dir("unreadable");
    let bad = KnownHosts { user: Some(d.to_path_buf()), app: d.join("app") };
    assert!(bad.check("bastion", 22, &k).is_err(), "a directory is not a readable file");
}

#[test]
fn stored_key_types_come_first_in_the_exchange() {
    let ed = key();
    let ec = PrivateKey::random(&mut rand::rng(), Algorithm::Ecdsa { curve: russh::keys::EcdsaCurve::NistP256 })
        .unwrap()
        .public_key()
        .clone();
    let (_d, kh) = files("algos", &format!("{}\n", line("bastion", &ec)), Some(&format!("{}\n", line("bastion", &ed))));
    assert_eq!(kh.algorithms("bastion", 22), vec![ed.algorithm(), ec.algorithm()]);
    assert!(kh.algorithms("elsewhere", 22).is_empty());
}
