//! Key files made at test time by the tools users make them with (`ssh-keygen`, `openssl`),
//! never committed. A tool that is missing skips its formats with a visible line, except on
//! CI, where it fails.

use super::*;
use std::process::Command;

/// A scratch directory of the test, removed with everything in it (generated private keys
/// too) when it goes out of scope, also when the test fails.
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
    let d = std::env::temp_dir().join(format!("datarig-keys-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

/// Run a key tool; `false` when it is not installed (not allowed on CI).
fn tool(program: &str, args: &[&str]) -> bool {
    match Command::new(program).args(args).output() {
        Ok(out) => {
            assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            true
        }
        Err(e) => {
            assert!(std::env::var_os("CI").is_none(), "{program} is needed on CI: {e}");
            eprintln!("SKIPPED formats made by {program}: {e}");
            false
        }
    }
}

/// `chmod 600`, as a user does after downloading a `.pem`.
fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let _ = path;
}

fn algorithm(k: &LoadedKey) -> String {
    k.key.algorithm().to_string()
}

#[test]
fn pem_and_openssh_keys_load_encrypted_or_not() {
    let d = dir("formats");
    let p = |n: &str| d.join(n);
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let mut made: Vec<(&str, &str, Option<&str>)> = Vec::new();
    // ssh-keygen: OpenSSH format and PEM (`-m PEM`: PKCS#1 for RSA, SEC1 for ECDSA).
    if tool("ssh-keygen", &["-q", "-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "", "-f", &s(&p("rsa_pem"))]) {
        tool(
            "ssh-keygen",
            &["-q", "-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "pass-one", "-f", &s(&p("rsa_pem_enc"))],
        );
        tool("ssh-keygen", &["-q", "-t", "ed25519", "-N", "", "-f", &s(&p("ed"))]);
        tool("ssh-keygen", &["-q", "-t", "ed25519", "-N", "pass-two", "-f", &s(&p("ed_enc"))]);
        tool("ssh-keygen", &["-q", "-t", "ecdsa", "-N", "", "-f", &s(&p("ecdsa"))]);
        tool("ssh-keygen", &["-q", "-t", "rsa", "-b", "2048", "-N", "", "-f", &s(&p("rsa_openssh"))]);
        made.extend([
            ("rsa_pem", "ssh-rsa", None),
            ("rsa_pem_enc", "ssh-rsa", Some("pass-one")),
            ("ed", "ssh-ed25519", None),
            ("ed_enc", "ssh-ed25519", Some("pass-two")),
            ("ecdsa", "ecdsa-sha2-nistp256", None),
            ("rsa_openssh", "ssh-rsa", None),
        ]);
    }
    // openssl: what AWS-style `.pem` files are (PKCS#1 RSA, PKCS#8, SEC1 EC, encrypted PKCS#8).
    if tool("openssl", &["genrsa", "-traditional", "-out", &s(&p("aws.pem")), "2048"]) {
        tool("openssl", &["genrsa", "-out", &s(&p("pkcs8.pem")), "2048"]);
        tool("openssl", &["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", &s(&p("ec.pem"))]);
        tool(
            "openssl",
            &[
                "pkcs8",
                "-topk8",
                "-in",
                &s(&p("pkcs8.pem")),
                "-v2",
                "aes-256-cbc",
                "-passout",
                "pass:pass-three",
                "-out",
                &s(&p("pkcs8_enc.pem")),
            ],
        );
        made.extend([
            ("aws.pem", "ssh-rsa", None),
            ("pkcs8.pem", "ssh-rsa", None),
            ("ec.pem", "ecdsa-sha2-nistp256", None),
            ("pkcs8_enc.pem", "ssh-rsa", Some("pass-three")),
        ]);
    }
    for (name, algo, pass) in made {
        let path = p(name);
        private(&path);
        let first = std::fs::read_to_string(&path).unwrap().lines().next().unwrap().to_string();
        let k = load(&path, pass).unwrap_or_else(|e| panic!("{name} ({first}): {e:?}"));
        assert_eq!(algorithm(&k), algo, "{name}");
        match pass {
            Some(_) => {
                assert_eq!(load(&path, None).unwrap_err(), KeyError::Encrypted, "{name}");
                assert_eq!(load(&path, Some("nope")).unwrap_err(), KeyError::WrongPassphrase, "{name}");
            }
            // A passphrase given for a key that has none is not needed.
            None => assert_eq!(algorithm(&load(&path, Some("unused")).unwrap()), algo, "{name}"),
        }
        assert!(k.cert.is_none());
    }
}

#[test]
fn files_that_are_not_usable_keys_say_why() {
    let d = dir("bad");
    let missing = d.join("missing.pem");
    assert!(matches!(load(&missing, None), Err(KeyError::Unreadable(_))));
    let putty = d.join("key.ppk");
    std::fs::write(&putty, "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\n").unwrap();
    private(&putty);
    assert_eq!(load(&putty, None).unwrap_err(), KeyError::Putty);
    let junk = d.join("junk.pem");
    std::fs::write(&junk, "-----BEGIN RSA PRIVATE KEY-----\nnot base64\n-----END RSA PRIVATE KEY-----\n").unwrap();
    private(&junk);
    assert!(matches!(load(&junk, None), Err(KeyError::Unsupported(_))));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let open = d.join("open.pem");
        std::fs::write(&open, "anything").unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load(&open, None).unwrap_err(), KeyError::OpenToOthers { mode: 0o644 });
    }
}

#[test]
fn a_certificate_next_to_the_key_is_used() {
    let d = dir("cert");
    let s = |n: &str| d.join(n).to_str().unwrap().to_string();
    if !tool("ssh-keygen", &["-q", "-t", "ed25519", "-N", "", "-f", &s("ca")]) {
        return;
    }
    tool("ssh-keygen", &["-q", "-t", "ed25519", "-N", "", "-f", &s("user")]);
    tool("ssh-keygen", &["-q", "-s", &s("ca"), "-I", "it", "-n", "tunnel", &s("user.pub")]);
    private(&d.join("user"));
    let k = load(&d.join("user"), None).unwrap();
    assert!(k.cert.is_some(), "user-cert.pub is read");
    assert_eq!(cert_path(Path::new("/k/id.pem")), PathBuf::from("/k/id.pem-cert.pub"));
}

/// A legacy PEM key encrypted with AES-256-CBC, AES-192-CBC or 3DES is named as such from its
/// header, with or without a passphrase (no passphrase is asked for); AES-128-CBC goes on.
#[test]
fn a_legacy_cipher_datarig_cannot_open_is_named_before_any_passphrase() {
    let d = dir("legacy-cipher");
    for cipher in ["AES-256-CBC", "AES-192-CBC", "DES-EDE3-CBC"] {
        let path = d.join(format!("{cipher}.pem"));
        let text = format!(
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: {cipher},0011223344556677\n\nAAAA\n-----END RSA PRIVATE KEY-----\n"
        );
        std::fs::write(&path, text).unwrap();
        private(&path);
        for pass in [None, Some("secret")] {
            assert_eq!(load(&path, pass).err(), Some(KeyError::UnsupportedEncryption(cipher.into())), "{cipher}");
        }
    }
    assert_eq!(unsupported_cipher("Proc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n"), None);
}
