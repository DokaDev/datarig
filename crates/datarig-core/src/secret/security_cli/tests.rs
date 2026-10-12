use super::*;
use std::path::Path;
use std::time::Duration;

/// A fake `security`: a shell script in a directory of its own that appends its arguments to
/// `argv` (one per line) and its standard input (one request line) to `stdin`. For a
/// `find-generic-password` request it prints the file `find_stderr` on standard error and exits
/// with the code in `find_code`; for any other request `stderr` and `code` (0 without the file);
/// with a file `kill` it ends by SIGKILL.
struct Fake {
    dir: PathBuf,
    program: PathBuf,
}

const FAKE: &str = r#"#!/bin/sh
d=$(dirname "$0")
[ "$1" = "--probe" ] && exit 0
for a in "$@"; do printf '%s\n' "$a" >> "$d/argv"; done
line=$(cat)
printf '%s\n' "$line" >> "$d/stdin"
case "$line" in
find-generic-password*) e=find_stderr; c=find_code ;;
*) e=stderr; c=code ;;
esac
[ -f "$d/stdout" ] && cat "$d/stdout"
[ -f "$d/$e" ] && cat "$d/$e" >&2
[ -f "$d/kill" ] && kill -KILL $$
exit "$(cat "$d/$c" 2>/dev/null || echo 0)"
"#;

impl Fake {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-fake-security-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("security");
        write_script(&program, FAKE);
        Self { dir, program }
    }

    /// The next runs print `stderr` and exit with `code` (every request: a read as well); the
    /// record of earlier runs is dropped.
    fn answer(&self, stderr: &str, code: i32) {
        for (e, c) in [("stderr", "code"), ("find_stderr", "find_code")] {
            std::fs::write(self.dir.join(e), stderr).unwrap();
            std::fs::write(self.dir.join(c), code.to_string()).unwrap();
        }
        for f in ["argv", "stdin"] {
            let _ = std::fs::remove_file(self.dir.join(f));
        }
    }

    /// A read answers `stderr` and `code` (after [`Fake::answer`]).
    fn answer_find(&self, stderr: &str, code: i32) {
        std::fs::write(self.dir.join("find_stderr"), stderr).unwrap();
        std::fs::write(self.dir.join("find_code"), code.to_string()).unwrap();
    }

    /// A write succeeds, and the read that checks it gives `secret` back.
    fn answer_set(&self, secret: &str) {
        self.answer("", 0);
        self.answer_find(&password_line(secret), 0);
    }

    fn store(&self) -> SecurityCliStore {
        SecurityCliStore::with_tool(self.program.clone(), SERVICE, None, None)
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join(name)).ok()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Write an executable script and wait until it can be run: a process forked by another test
/// thread while the file was open for writing keeps it busy (ETXTBSY) until that process execs.
fn write_script(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..500 {
        match Command::new(path).arg("--probe").status() {
            Ok(_) => return,
            Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}

/// The `password: ` line the tool prints for `secret`: quoted when it is printable ASCII without a
/// backslash, else in hex.
fn password_line(secret: &str) -> String {
    if secret.is_empty() {
        "password: \n".to_string()
    } else if secret.bytes().all(|b| (0x20..0x7f).contains(&b) && b != b'\\') {
        format!("password: \"{secret}\"\n")
    } else {
        let hex: String = secret.bytes().map(|b| format!("{b:02X}")).collect();
        format!("password: 0x{hex}  \"(escaped)\"\n")
    }
}

/// The read a write checks itself with, on `profile:1`.
const FIND: &str = "find-generic-password -s \"datarig\" -a \"profile:1\" -g\n";

fn kind(r: Unavailable) -> KeychainFault {
    match r.0.kind {
        FaultKind::Keychain(k) => k,
        other => panic!("not a keychain fault: {other:?}"),
    }
}

#[test]
fn quoting_escapes_only_backslash_and_quote() {
    assert_eq!(quote("plain"), r#""plain""#);
    assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    assert_eq!(quote("$HOME `id` ' ; | #"), "\"$HOME `id` ' ; | #\"");
    assert_eq!(quote(""), r#""""#);
}

#[test]
fn set_passes_the_secret_on_standard_input_only() {
    let fake = Fake::new("set");
    let secrets = [
        "s3cret",
        "-w x",
        "with  spaces ",
        r#"quote"and\backslash"#,
        "$HOME `id` ' ; | # *",
        "\u{d55c}\u{ad6d}\u{c5b4} caf\u{e9}",
        "0x41",
        "",
    ];
    for s in secrets {
        fake.answer_set(s);
        fake.store().set("profile:1", s).unwrap();
        assert_eq!(fake.read("argv").as_deref(), Some("-i\n-i\n"), "only -i on the command line ({s:?})");
        let want = format!("add-generic-password -U -s \"datarig\" -a \"profile:1\" -w {}\n{FIND}", quote(s));
        assert_eq!(fake.read("stdin").as_deref(), Some(want.as_str()), "written, then read back");
    }
    // The escaping, spelled out.
    fake.answer_set(r#"a"b\c"#);
    fake.store().set("profile:1", r#"a"b\c"#).unwrap();
    let stdin = fake.read("stdin").unwrap();
    assert!(stdin.ends_with(&format!(" -w \"a\\\"b\\\\c\"\n{FIND}")), "{stdin}");
}

#[test]
fn a_write_is_checked_by_reading_it_back() {
    let fake = Fake::new("readback");
    // Another password back, or none at all: the write did not happen as asked.
    fake.answer_set("other");
    let e = fake.store().set("profile:1", "hunter2").unwrap_err();
    assert!(!e.0.detail.contains("hunter2") && !e.0.detail.contains("other"), "{e:?}");
    assert_eq!(kind(e), KeychainFault::Mismatch);
    fake.answer("", 0);
    fake.answer_find("find-generic-password: returned -25300\n", 44);
    assert_eq!(kind(fake.store().set("profile:1", "hunter2").unwrap_err()), KeychainFault::Mismatch);
    // A read back that fails is that failure.
    fake.answer("", 0);
    fake.answer_find("find-generic-password: returned -25293\n", 51);
    assert_eq!(kind(fake.store().set("profile:1", "hunter2").unwrap_err()), KeychainFault::Locked);
}

#[test]
fn a_write_that_says_anything_has_failed_and_a_delete_is_checked() {
    let fake = Fake::new("noise");
    let usage = "Usage: add-generic-password [-a account] [-s service] [-w password] [keychain]\n";
    fake.answer(usage, 0);
    fake.answer_find(&password_line("pw"), 0);
    let e = fake.store().set("profile:1", "pw").unwrap_err();
    assert_eq!(e.0.detail, "add-generic-password failed (unexpected output from the security tool)");
    assert_eq!(kind(e), KeychainFault::Failure);
    assert_eq!(fake.read("stdin").as_deref().map(|s| s.lines().count()), Some(1), "no read back after it");
    // A delete that says something counts only when the entry is gone after it.
    fake.answer("security: something odd\n", 0);
    fake.answer_find(&password_line("pw"), 0);
    let e = fake.store().delete("profile:1").unwrap_err();
    assert_eq!(e.0.detail, "security: something odd (unexpected output from the security tool)");
    assert_eq!(kind(e), KeychainFault::Failure);
    fake.answer(usage, 0);
    assert_eq!(kind(fake.store().delete("profile:1").unwrap_err()), KeychainFault::Failure, "the read is odd too");
    fake.answer("password has been deleted.\n", 0);
    fake.answer_find("find-generic-password: returned -25300\n", 44);
    assert_eq!(fake.store().delete("profile:1"), Ok(true));
}

#[test]
fn a_keychain_file_is_named_last() {
    let fake = Fake::new("keychain");
    let path = fake.dir.join("my \"test\".keychain-db");
    let store = SecurityCliStore::with_tool(fake.program.clone(), "datarig-test", Some(path.clone()), None);
    fake.answer_set("pw");
    store.set("profile:1", "pw").unwrap();
    let kc = quote(path.to_str().unwrap());
    let want = format!(
        "add-generic-password -U -s \"datarig-test\" -a \"profile:1\" -w \"pw\" {kc}\n\
         find-generic-password -s \"datarig-test\" -a \"profile:1\" -g {kc}\n"
    );
    assert_eq!(fake.read("stdin").as_deref(), Some(want.as_str()));
    fake.answer("", 0);
    assert!(store.delete("profile:1").unwrap());
    let want =
        format!("delete-generic-password -s \"datarig-test\" -a \"profile:1\" {}\n", quote(path.to_str().unwrap()));
    assert_eq!(fake.read("stdin").as_deref(), Some(want.as_str()));
}

#[test]
fn get_reads_every_shape_of_the_password_line() {
    let fake = Fake::new("get");
    let cases = [
        ("password: \n", ""),
        ("password: \"\"\n", ""),
        ("password: \"s3cret\"\n", "s3cret"),
        ("password: \"\"\"\n", "\""),
        ("password: \"say \"hi\" now\"\n", "say \"hi\" now"),
        ("password: \"0x41\"\n", "0x41"),
        ("password: \"$HOME `id` ' ; | #\"\n", "$HOME `id` ' ; | #"),
        ("password: 0x41  \"A\"\n", "A"),
        ("password: 0x5C  \"\\\\\"\n", "\\"),
        ("password: 0xED959C0A  \"\\355\\225\\234\\012\"\n", "\u{d55c}\n"),
        ("password: 0xed959c  \"\\355\\225\\234\"\n", "\u{d55c}"),
        // A password that looks like a status line is a password.
        ("password: \"find-generic-password: returned -25300\"\n", "find-generic-password: returned -25300"),
        // Other lines around it.
        ("security: a warning\npassword: \"pw\"\nmore\n", "pw"),
    ];
    for (stderr, want) in cases {
        fake.answer(stderr, 0);
        assert_eq!(fake.store().get("profile:1").unwrap().as_deref(), Some(want), "{stderr:?}");
        assert_eq!(fake.read("argv").as_deref(), Some("-i\n"));
        assert_eq!(fake.read("stdin").as_deref(), Some("find-generic-password -s \"datarig\" -a \"profile:1\" -g\n"));
    }
}

#[test]
fn get_never_takes_odd_output_for_no_password() {
    let fake = Fake::new("odd");
    for stderr in [
        "",
        "keychain: \"login\"\n",
        "password: hunter2-plain\n",
        "password:\"hunter2\"\n",
        "password: \"unterminated\n",
        "password: \"back\\slash\"\n",
        "password: 0x\n",
        "password: 0x414\n",
        "password: 0xZZ  \"?\"\n",
        "password: \"one\"\npassword: \"two\"\n",
    ] {
        fake.answer(stderr, 0);
        let e = fake.store().get("profile:1").unwrap_err();
        assert!(!e.0.detail.contains("hunter2") && !e.0.detail.contains("one"), "{e:?}");
        assert_eq!(e.0.detail, "unexpected output from the security tool", "{stderr:?}");
        assert_eq!(kind(e), KeychainFault::Failure, "{stderr:?}");
    }
    // Hex that is not UTF-8: the entry is damaged.
    fake.answer("password: 0xFFFE  \"\\377\\376\"\n", 0);
    assert_eq!(kind(fake.store().get("profile:1").unwrap_err()), KeychainFault::Damaged);
}

#[test]
fn only_item_not_found_means_no_entry() {
    let fake = Fake::new("status");
    let missing = "security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.\n";
    fake.answer(&format!("{missing}find-generic-password: returned -25300\n"), 44);
    assert_eq!(fake.store().get("profile:1"), Ok(None));
    fake.answer(&format!("{missing}delete-generic-password: returned -25300\n"), 44);
    assert_eq!(fake.store().delete("profile:1"), Ok(false));
    fake.answer("", 0);
    assert_eq!(fake.store().delete("profile:1"), Ok(true));
    // The same exit code without the status line is unknown, not "no entry".
    fake.answer(missing, 44);
    let e = fake.store().get("profile:1").unwrap_err();
    assert!(e.0.detail.contains("exit code 44"), "{e:?}");
    assert_eq!(kind(e), KeychainFault::Failure);
    assert_eq!(kind(fake.store().delete("profile:1").unwrap_err()), KeychainFault::Failure);
    // The status of another command does not count.
    fake.answer("add-generic-password: returned -25300\n", 44);
    assert_eq!(kind(fake.store().get("profile:1").unwrap_err()), KeychainFault::Failure);
}

#[test]
fn statuses_map_to_fault_kinds() {
    let fake = Fake::new("kinds");
    let cases = [
        (-25293, KeychainFault::Locked),
        (-128, KeychainFault::Locked),
        (-25308, KeychainFault::Locked),
        (-25294, KeychainFault::NoStore),
        (-25307, KeychainFault::NoStore),
        (-25299, KeychainFault::Ambiguous),
        (-26275, KeychainFault::Damaged),
        (-9999, KeychainFault::Failure),
    ];
    for (status, want) in cases {
        let code = status & 0xff;
        fake.answer(&format!("security: it failed\nfind-generic-password: returned {status}\n"), code);
        let e = fake.store().get("profile:1").unwrap_err();
        assert_eq!(e.0.detail, format!("security: it failed (OSStatus {status})"));
        assert_eq!(kind(e), want, "{status}");
        fake.answer(&format!("add-generic-password: returned {status}\n"), code);
        let e = fake.store().set("profile:1", "pw").unwrap_err();
        assert_eq!(e.0.detail, format!("add-generic-password failed (OSStatus {status})"));
        assert_eq!(kind(e), want, "{status}");
    }
}

#[test]
fn failure_details_never_hold_the_secret() {
    let fake = Fake::new("detail");
    fake.answer("security: could not store \"hunter2\"\nsecurity: the keychain is locked\nadd-generic-password: returned -25293\n", 51);
    let e = fake.store().set("profile:1", "hunter2").unwrap_err();
    assert_eq!(e.0.detail, "security: the keychain is locked (OSStatus -25293)");
    assert_eq!(kind(e), KeychainFault::Locked);
    // A failed read shows the tool's message, never the rest of its output.
    std::fs::write(fake.dir.join("stdout"), "\"acct\"<blob>=\"profile:1\"\n").unwrap();
    fake.answer("password: \"hunter2\"\nsecurity: odd\nfind-generic-password: returned -25293\n", 51);
    let e = fake.store().get("profile:1").unwrap_err();
    assert_eq!(e.0.detail, "security: odd (OSStatus -25293)");
    // Echoed back escaped, as the tool's quoted word or as the text inside it: dropped too.
    let secret = r#"a"b\c"#;
    let echoed = format!(
        "security: bad word {}\nsecurity: near a\\\"b\\\\c here\nsecurity: raw {secret}\nsecurity: kept\n\
         add-generic-password: returned -25293\n",
        quote(secret)
    );
    fake.answer(&echoed, 51);
    let e = fake.store().set("profile:1", secret).unwrap_err();
    assert_eq!(e.0.detail, "security: kept (OSStatus -25293)");
}

#[test]
fn a_tool_ended_by_a_signal_is_a_failure() {
    let fake = Fake::new("signal");
    fake.answer("password: \"pw\"\n", 0);
    std::fs::write(fake.dir.join("kill"), "").unwrap();
    let e = fake.store().get("profile:1").unwrap_err();
    assert!(e.0.detail.contains("signal") && !e.0.detail.contains("pw\""), "{e:?}");
    assert_eq!(kind(e), KeychainFault::Failure);
}

#[test]
fn a_missing_or_unrunnable_tool_is_no_tool() {
    let fake = Fake::new("missing");
    let missing = SecurityCliStore::with_tool(fake.dir.join("no-such-security"), SERVICE, None, None);
    assert_eq!(kind(missing.get("profile:1").unwrap_err()), KeychainFault::NoTool);
    assert_eq!(kind(missing.set("profile:1", "pw").unwrap_err()), KeychainFault::NoTool);
    assert_eq!(kind(missing.delete("profile:1").unwrap_err()), KeychainFault::NoTool);
    let plain = fake.dir.join("not-executable");
    std::fs::write(&plain, "#!/bin/sh\n").unwrap();
    let e = SecurityCliStore::with_tool(plain, SERVICE, None, None).get("profile:1").unwrap_err();
    assert_eq!(kind(e), KeychainFault::NoTool);
}

#[test]
fn unwritable_requests_are_refused_before_anything_runs() {
    let fake = Fake::new("refused");
    fake.answer("", 0);
    for s in ["two\nlines", "carriage\rreturn", "nul\0byte"] {
        let e = fake.store().set("profile:1", s).unwrap_err();
        assert_eq!(e.0.detail, "a password with a line break or NUL cannot be passed to the security tool");
        assert_eq!(kind(e), KeychainFault::Refused);
    }
    let long = "x".repeat(LINE_LIMIT);
    let e = fake.store().set("profile:1", &long).unwrap_err();
    assert_eq!(e.0.detail, "request too long for the security tool");
    assert_eq!(kind(e), KeychainFault::Refused);
    // Escapes count: 2,000 quotes take 4,000 bytes.
    assert_eq!(kind(fake.store().set("profile:1", &"\"".repeat(2000)).unwrap_err()), KeychainFault::Refused);
    assert!(fake.read("argv").is_none() && fake.read("stdin").is_none(), "the tool never ran");
    // An account with a line break is refused as well (another detail).
    let e = fake.store().get("profile:1\nx").unwrap_err();
    assert_eq!(e.0.detail, "a name with a line break or NUL cannot be passed to the security tool");
    assert!(fake.read("argv").is_none(), "the tool never ran");
    // 3,000 characters fit.
    let fits = "y".repeat(3000);
    fake.answer_set(&fits);
    fake.store().set("profile:1", &fits).unwrap();
    assert_eq!(fake.read("argv").as_deref(), Some("-i\n-i\n"));
}

/// The real `/usr/bin/security`, on a scratch keychain file only. Opt in with
/// `DATARIG_TEST_SECURITY_CLI=1`. The keychain is created in this test's own directory with a
/// random password, never added to the user's search list nor made the default (every call
/// names it, and the tool's `HOME` is a scratch directory), kept unlocked, and deleted at the
/// end. The user's search list and default keychain are compared before and after.
#[cfg(target_os = "macos")]
#[test]
fn real_security_tool_on_a_scratch_keychain() {
    if std::env::var("DATARIG_TEST_SECURITY_CLI").as_deref() != Ok("1") {
        eprintln!(
            "SKIPPED real_security_tool_on_a_scratch_keychain: set DATARIG_TEST_SECURITY_CLI=1 (macOS; a scratch \
             keychain file only)"
        );
        return;
    }
    let user_keychains = || {
        let read = |args: &[&str]| {
            let out = Command::new(PROGRAM).args(args).output().unwrap();
            assert!(out.status.success(), "security {args:?}");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        (read(&["list-keychains", "-d", "user"]), read(&["default-keychain", "-d", "user"]))
    };
    let before = user_keychains();
    let dir = std::env::temp_dir().join(format!("datarig-security-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let scratch = Scratch::create(&dir, &home);
    assert_eq!(user_keychains(), before, "creating the scratch keychain left the user's keychains alone");

    let store = SecurityCliStore::with_tool(
        PathBuf::from(PROGRAM),
        "datarig-test",
        Some(scratch.keychain.clone()),
        Some(home.clone()),
    );
    let long = "a1".repeat(1500);
    let battery = [
        "plain",
        "with  spaces ",
        "quote\"inside",
        "\"",
        "back\\slash",
        "it's",
        "$HOME `id` ; | # *",
        "\u{d55c}\u{ad6d}\u{c5b4} caf\u{e9}",
        "0x41",
        "",
        long.as_str(),
    ];
    for (i, s) in battery.iter().enumerate() {
        let account = format!("profile:test-{i}");
        store.set(&account, s).unwrap_or_else(|e| panic!("set #{i}: {e:?}"));
        assert_eq!(store.get(&account).unwrap_or_else(|e| panic!("get #{i}: {e:?}")).as_deref(), Some(*s), "#{i}");
    }
    // `-U` replaces; delete; a deleted or missing entry is "no entry".
    store.set("profile:test-0", "second").unwrap();
    assert_eq!(store.get("profile:test-0").unwrap().as_deref(), Some("second"));
    assert_eq!(store.delete("profile:test-0"), Ok(true));
    assert_eq!(store.get("profile:test-0"), Ok(None));
    assert_eq!(store.delete("profile:test-0"), Ok(false));
    assert_eq!(store.get("profile:never"), Ok(None));

    // Through a wrapper that logs its arguments: no password ever reaches them.
    let log = dir.join("argv.log");
    let wrapper = dir.join("wrapper");
    let script = format!(
        "#!/bin/sh\n[ \"$1\" = \"--probe\" ] && exit 0\nprintf '%s\\n' \"$@\" >> {}\nexec {PROGRAM} \"$@\"\n",
        quote(log.to_str().unwrap())
    );
    write_script(&wrapper, &script);
    let wrapped = SecurityCliStore::with_tool(wrapper, "datarig-test", Some(scratch.keychain.clone()), Some(home));
    for (i, s) in battery.iter().enumerate() {
        let account = format!("profile:wrapped-{i}");
        wrapped.set(&account, s).unwrap();
        assert_eq!(wrapped.get(&account).unwrap().as_deref(), Some(*s));
        assert_eq!(wrapped.delete(&account), Ok(true));
    }
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.lines().all(|l| l == "-i"), "only -i was passed: {logged:?}");
    assert_eq!(logged.lines().count(), battery.len() * 5, "a write, its read back, a read, a delete and its check");

    drop(scratch);
    assert_eq!(user_keychains(), before, "the user's keychains are as they were");
}

/// The scratch keychain of [`real_security_tool_on_a_scratch_keychain`], deleted on drop.
#[cfg(target_os = "macos")]
struct Scratch {
    keychain: PathBuf,
    home: PathBuf,
    dir: PathBuf,
}

#[cfg(target_os = "macos")]
impl Scratch {
    fn create(dir: &Path, home: &Path) -> Self {
        let keychain = dir.join("scratch.keychain-db");
        let kc = keychain.to_str().expect("a UTF-8 temporary directory").to_string();
        let kc = kc.as_str();
        let password = random_hex();
        let s = Self { keychain: keychain.clone(), home: home.to_path_buf(), dir: dir.to_path_buf() };
        s.security(&["create-keychain", "-p", &password, kc]);
        // No lock timeout, no lock on sleep; then unlocked.
        s.security(&["set-keychain-settings", kc]);
        s.security(&["unlock-keychain", "-p", &password, kc]);
        s
    }

    fn security(&self, args: &[&str]) {
        let out = Command::new(PROGRAM).args(args).env("HOME", &self.home).stdin(Stdio::null()).output().unwrap();
        assert!(out.status.success(), "security {}: {}", args[0], String::from_utf8_lossy(&out.stderr));
    }
}

#[cfg(target_os = "macos")]
impl Drop for Scratch {
    fn drop(&mut self) {
        // Never without the keychain's name (that would be the default keychain).
        if let Some(kc) = self.keychain.to_str() {
            let _ = Command::new(PROGRAM)
                .args(["delete-keychain", kc])
                .env("HOME", &self.home)
                .stdin(Stdio::null())
                .output();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 32 random hex digits (the scratch keychain's password).
#[cfg(target_os = "macos")]
fn random_hex() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").unwrap().read_exact(&mut bytes).unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
