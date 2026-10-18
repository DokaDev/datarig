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

/// An EC private key whose scalar has leading zero bytes, in every format a user may have it
/// in. OpenSSH writes the scalar without them (an `mpint`), so about one P-256 key in 512 made
/// by `ssh-keygen -t ecdsa` has a scalar of 31 bytes or fewer; such keys failed to load
/// ("length invalid"), or, encrypted, were reported as a wrong passphrase. The keys are made
/// from fixed scalars by `openssl` (SEC1, PKCS#8) and converted by `ssh-keygen -p` (OpenSSH),
/// and each one must be the key `ssh-keygen -y` names and sign as it. Unix only: the files
/// are `chmod 600`ed for `ssh-keygen` to read them.
#[cfg(unix)]
#[test]
fn ecdsa_keys_whose_scalar_has_leading_zero_bytes_load_in_every_format() {
    use russh::keys::HashAlg;
    let d = dir("ecdsa-zeros");
    let s = |n: &str| d.join(n).to_str().unwrap().to_string();
    // (algorithm, the curve's OID in DER, scalar size, the scalar's first bytes (the rest is
    // 0x5a), the length of the mpint OpenSSH writes it as). The first two did not load before.
    let p256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    let p384: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22];
    let p521: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x23];
    type Case = (&'static str, &'static [u8], usize, &'static [u8], usize);
    let cases: [Case; 6] = [
        ("ecdsa-sha2-nistp256", p256, 32, &[0x00, 0x7f], 31),
        ("ecdsa-sha2-nistp256", p256, 32, &[0x00, 0x00, 0x7f], 30),
        ("ecdsa-sha2-nistp256", p256, 32, &[0x00, 0x80], 32), // 0x00, then 31 bytes
        ("ecdsa-sha2-nistp384", p384, 48, &[0x00, 0x7f], 47),
        ("ecdsa-sha2-nistp521", p521, 66, &[0x00, 0x7f], 65),
        ("ecdsa-sha2-nistp521", p521, 66, &[0x00, 0x00, 0x7f], 64),
    ];
    for (i, (algo, oid, size, first, mpint)) in cases.into_iter().enumerate() {
        let mut scalar = vec![0x5a; size];
        scalar[..first.len()].copy_from_slice(first);
        // SEC1 `ECPrivateKey` without its public key (openssl works it out): version 1, the
        // scalar, the curve.
        let mut body = vec![0x02, 0x01, 0x01, 0x04, u8::try_from(size).unwrap()];
        body.extend(&scalar);
        body.extend([0xa0, u8::try_from(oid.len()).unwrap()]);
        body.extend(oid);
        let mut der = vec![0x30, u8::try_from(body.len()).unwrap()];
        der.extend(body);
        let pem = format!(
            "-----BEGIN EC PRIVATE KEY-----\n{}\n-----END EC PRIVATE KEY-----\n",
            data_encoding::BASE64.encode(&der)
        );
        let n = |kind: &str| format!("{i}-{kind}");
        std::fs::write(d.join(n("raw")), pem).unwrap();
        if !tool("openssl", &["ec", "-in", &s(&n("raw")), "-out", &s(&n("sec1"))]) {
            return;
        }
        tool("openssl", &["pkcs8", "-topk8", "-nocrypt", "-in", &s(&n("sec1")), "-out", &s(&n("pkcs8"))]);
        tool(
            "openssl",
            &[
                "pkcs8",
                "-topk8",
                "-in",
                &s(&n("sec1")),
                "-v2",
                "aes-256-cbc",
                "-passout",
                "pass:pass-four",
                "-out",
                &s(&n("pkcs8_enc")),
            ],
        );
        for (kind, pass) in [("openssh", ""), ("openssh_enc", "pass-five")] {
            std::fs::copy(d.join(n("sec1")), d.join(n(kind))).unwrap();
            private(&d.join(n(kind)));
            if !tool("ssh-keygen", &["-q", "-p", "-a", "2", "-P", "", "-N", pass, "-f", &s(&n(kind))]) {
                return;
            }
        }
        let text = std::fs::read_to_string(d.join(n("openssh"))).unwrap();
        assert_eq!(openssh_scalar_len(&text), mpint, "case {i}: the scalar as ssh-keygen wrote it");
        let out = Command::new("ssh-keygen").args(["-y", "-f", &s(&n("openssh"))]).output().unwrap();
        assert!(out.status.success(), "ssh-keygen -y: {}", String::from_utf8_lossy(&out.stderr));
        let public: Vec<String> =
            String::from_utf8(out.stdout).unwrap().split_whitespace().take(2).map(String::from).collect();
        assert_eq!(public[0], algo, "case {i}");
        let files = [
            ("sec1", None),
            ("pkcs8", None),
            ("pkcs8_enc", Some("pass-four")),
            ("openssh", None),
            ("openssh_enc", Some("pass-five")),
        ];
        for (kind, pass) in files {
            let path = d.join(n(kind));
            private(&path);
            let first = std::fs::read_to_string(&path).unwrap().lines().next().unwrap().to_string();
            let what = format!("case {i} {algo} {kind} ({first})");
            let k = load(&path, pass).unwrap_or_else(|e| panic!("{what}: {e:?}"));
            assert_eq!(algorithm(&k), algo, "{what}");
            let openssh = k.key.public_key().to_openssh().unwrap();
            assert_eq!(openssh.split_whitespace().take(2).collect::<Vec<_>>(), public, "{what}");
            let sig = k.key.sign("datarig-test", HashAlg::Sha256, b"message").unwrap();
            k.key.public_key().verify("datarig-test", b"message", &sig).unwrap_or_else(|e| panic!("{what}: {e}"));
        }
    }
}

/// The length of the private scalar's `mpint` in an unencrypted OpenSSH ECDSA key file.
#[cfg(unix)]
fn openssh_scalar_len(text: &str) -> usize {
    // One length-prefixed field at `at`, and `at` past it.
    fn field<'a>(blob: &'a [u8], at: &mut usize) -> &'a [u8] {
        let len = u32::from_be_bytes(blob[*at..*at + 4].try_into().unwrap()) as usize;
        *at += 4 + len;
        &blob[*at - len..*at]
    }
    let b64: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    let blob = data_encoding::BASE64.decode(b64.as_bytes()).unwrap();
    let mut at = b"openssh-key-v1\0".len();
    assert_eq!(field(&blob, &mut at), b"none", "the cipher");
    field(&blob, &mut at); // the KDF
    field(&blob, &mut at); // its options
    at += 4; // the number of keys
    field(&blob, &mut at); // the public key
    let private = field(&blob, &mut at);
    // Two check numbers, then the key type, the curve, the public point and the scalar.
    let mut at = 8;
    for _ in 0..3 {
        field(private, &mut at);
    }
    field(private, &mut at).len()
}
