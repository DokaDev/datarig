use super::*;

#[test]
fn memory_store_roundtrip() {
    let mut s = Secrets::new(Arc::new(MemoryStore::new()));
    assert_eq!(s.resolve("a"), None);
    s.save("a", "pw").unwrap();
    assert_eq!(s.resolve("a").as_deref(), Some("pw"));
    s.remove("a").unwrap();
    assert_eq!(s.resolve("a"), None);
    assert!(s.unavailable.is_none());
}

#[test]
fn unavailable_store_falls_back_to_session_prompt() {
    let mut s = Secrets::new(Arc::new(MemoryStore::unavailable("no secret service")));
    assert_eq!(s.resolve("a"), None, "nothing stored, no panic");
    assert_eq!(s.unavailable, Some(nk("no secret service")));
    // Saving fails but the password is kept for this session (prompt-each-time mode).
    assert_eq!(s.save("a", "typed").unwrap_err(), SourceError::Keychain(nk("no secret service")));
    assert_eq!(s.resolve("a").as_deref(), Some("typed"));
    s.remember("b", "prompted");
    assert_eq!(s.resolve("b").as_deref(), Some("prompted"));
    assert!(s.remove("a").is_err());
    assert_eq!(s.resolve("a"), None, "session copy dropped on remove");
}

#[test]
fn session_password_wins_over_store() {
    let store = Arc::new(MemoryStore::new());
    store.set("a", "stale").unwrap();
    let mut s = Secrets::new(store);
    s.remember("a", "fresh");
    assert_eq!(s.resolve("a").as_deref(), Some("fresh"));
}

#[test]
fn store_kind_from_env() {
    assert_eq!(StoreKind::from_env(None), Ok(StoreKind::Keychain), "the default");
    assert_eq!(StoreKind::from_env(Some("")), Ok(StoreKind::Keychain));
    assert_eq!(StoreKind::from_env(Some("keychain")), Ok(StoreKind::Keychain));
    assert_eq!(StoreKind::from_env(Some("memory")), Ok(StoreKind::Memory));
    assert_eq!(StoreKind::from_env(Some(" Memory ")), Ok(StoreKind::Memory));
    let e = StoreKind::from_env(Some("memroy")).unwrap_err();
    assert!(e.contains(STORE_ENV) && e.contains("memroy"), "{e}");
    // `memory` starts empty and never shares entries with another store.
    let store = StoreKind::Memory.open();
    assert_eq!(store.get("a"), Ok(None));
    store.set("a", "pw").unwrap();
    assert_eq!(StoreKind::Memory.open().get("a"), Ok(None));
}

#[test]
fn memory_replaces_both_the_keychain_and_the_secrets_file() {
    let dir = std::env::temp_dir().join(format!("datarig-memstores-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("secrets.toml");
    let log = Arc::new(DeletionLog::new(None));
    let stores = StoreKind::Memory.open_stores(Some(file.clone()), log.clone());
    assert_eq!(stores.file_path, None, "no permission check on a file that is not used");
    stores.file.set("profile:a", "f").unwrap();
    stores.keychain.set("profile:a", "k").unwrap();
    assert!(!file.exists() && !dir.exists(), "the real secrets file is never touched");
    assert_eq!(stores.file.get("profile:a").unwrap().as_deref(), Some("f"), "two separate stores");
    assert_eq!(stores.keychain.get("profile:a").unwrap().as_deref(), Some("k"));
    // `keychain` uses the file at the given path (never touched here: only its check).
    let real = StoreKind::Keychain.open_stores(Some(file.clone()), log.clone());
    assert_eq!(real.file_path.as_deref(), Some(file.as_path()));
    assert_eq!(real.check_file(), Ok(()), "a missing file is fine");
    let none = StoreKind::Keychain.open_stores(None, log);
    assert!(none.file.get("profile:a").is_err(), "no config directory: no secrets file");
}

#[test]
fn secrets_save_and_remove_per_store() {
    let mut s = Secrets::new(Arc::new(MemoryStore::new()));
    s.save_in(SourceKind::File, "a", "f").unwrap();
    assert_eq!(s.stores().file.get("a").unwrap().as_deref(), Some("f"));
    assert_eq!(s.stores().keychain.get("a").unwrap(), None);
    s.remove_in(SourceKind::File, "a").unwrap();
    assert_eq!(s.stores().file.get("a").unwrap(), None);
    // A failing file store is not "the keychain is unavailable".
    let stores = Stores {
        file: Arc::new(MemoryStore::unavailable("disk")),
        ..Stores::with_keychain(Arc::new(MemoryStore::new()))
    };
    let mut s = Secrets::with_stores(stores);
    assert_eq!(s.save_in(SourceKind::File, "a", "f"), Err(SourceError::File(file::FileError::Io(nk("disk")))));
    assert!(s.unavailable.is_none());
    assert_eq!(s.session("a"), Some("f"), "kept for the session");
    s.remember("b", "typed");
    s.forget("b");
    assert_eq!(s.session("b"), None);
    assert_eq!(s.save_in(SourceKind::Env, "c", "x"), Ok(()), "nothing to store for env");
}

/// Returns another value than the one stored (a store that does not keep what it is given).
struct Forgetful;
impl SecretStore for Forgetful {
    fn get(&self, _: &str) -> Result<Option<String>, Unavailable> {
        Ok(Some("something else".into()))
    }
    fn set(&self, _: &str, _: &str) -> Result<(), Unavailable> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<bool, Unavailable> {
        Ok(true)
    }
}

#[test]
fn store_verified_reads_the_secret_back() {
    let store = MemoryStore::new();
    assert_eq!(store_verified(&store, "a", "pw"), Ok(()));
    assert_eq!(store.get("a").unwrap().as_deref(), Some("pw"));
    assert_eq!(store_verified(&Forgetful, "a", "pw"), Err(StoreError::Mismatch));
    let down = MemoryStore::unavailable("no secret service");
    assert_eq!(store_verified(&down, "a", "pw"), Err(StoreError::Unavailable(nk("no secret service"))));
}

#[test]
fn deletions_are_logged_without_the_password() {
    let dir = std::env::temp_dir().join(format!("datarig-dellog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("state").join("secrets.log");
    let log = Arc::new(DeletionLog::new(Some(path.clone())));
    let store = Logged::new(Arc::new(MemoryStore::new()), "keychain", log.clone());
    store.set("profile:1", "hunter2").unwrap();
    assert_eq!(store.delete("missing"), Ok(false));
    assert!(!path.exists(), "nothing removed, nothing logged");
    assert_eq!(store.delete("profile:1"), Ok(true));
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].ends_with(" deleted source=keychain account=profile:1"), "{text}");
    assert!(!text.contains("hunter2"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // A store that fails logs nothing; a log without a path writes nothing.
    let down = Logged::new(Arc::new(MemoryStore::unavailable("locked")), "file", log);
    assert!(down.delete("profile:1").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    DeletionLog::new(None).record("keychain", "profile:1");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn log_timestamps_are_utc() {
    use std::time::{Duration, UNIX_EPOCH};
    let at = |s: u64| log::utc_timestamp(UNIX_EPOCH + Duration::from_secs(s));
    assert_eq!(at(0), "1970-01-01T00:00:00Z");
    assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(at(1_790_305_445), "2026-09-25T03:04:05Z");
}

/// Real OS keychain. Never runs in CI; locally opt in with `DATARIG_TEST_KEYCHAIN=1`.
/// Only touches a throwaway `datarig` item and deletes it again.
#[test]
fn os_keychain_roundtrip() {
    if std::env::var_os("DATARIG_TEST_KEYCHAIN").is_none() || std::env::var_os("CI").is_some() {
        eprintln!("SKIPPED os_keychain_roundtrip: set DATARIG_TEST_KEYCHAIN=1 (never under CI)");
        return;
    }
    let store = KeyringStore::new();
    let account = format!("__datarig_test_{}__", std::process::id());
    store.set(&account, "s3cret 🐘").expect("keychain set");
    assert_eq!(store.get(&account).expect("keychain get").as_deref(), Some("s3cret 🐘"));
    assert!(store.delete(&account).expect("keychain delete"), "an entry was removed");
    assert_eq!(store.get(&account).expect("keychain get after delete"), None);
    assert!(!store.delete(&account).expect("deleting a missing entry is ok"));
}

/// The fault a `MemoryStore::unavailable(detail)` gives.
fn nk(detail: &str) -> crate::fault::Fault {
    crate::fault::Fault::new(crate::fault::FaultKind::Keychain(crate::fault::KeychainFault::NoStore), detail)
}

/// A keychain whose `get` waits until the test opens it (a permission dialog nobody sees), or
/// takes `delay` first.
#[derive(Default)]
struct Held {
    inner: MemoryStore,
    open: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
    delay: std::time::Duration,
}

impl Held {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

impl SecretStore for Held {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        if self.delay.is_zero() {
            let open = self.open.lock().unwrap();
            let limit = std::time::Duration::from_secs(30);
            drop(self.wake.wait_timeout_while(open, limit, |o| !*o).unwrap());
        } else {
            std::thread::sleep(self.delay);
        }
        self.inner.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.inner.set(account, secret)
    }
    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        self.inner.delete(account)
    }
}

fn no_answer(e: &Unavailable) -> bool {
    e.0.kind == crate::fault::FaultKind::Keychain(crate::fault::KeychainFault::NoAnswer)
}

/// A keychain call that does not answer ends at the limit as "no answer" (unknown,
/// never "nothing stored"); while it still hangs the next calls fail at once; once it returns
/// the keychain is asked again.
#[test]
fn a_guarded_keychain_call_that_does_not_answer_ends_at_the_limit() {
    use std::time::{Duration, Instant};
    let held = Arc::new(Held::default());
    held.inner.set("a", "pw").unwrap();
    let limit = Duration::from_millis(100);
    let g = Guarded::new(held.clone(), limit);
    let t = Instant::now();
    let e = g.get("a").unwrap_err();
    assert!(no_answer(&e), "{e:?}");
    assert!(t.elapsed() >= limit && t.elapsed() < limit * 20, "{:?}", t.elapsed());
    assert!(g.hanging());
    // No second thread piles up behind the first.
    let t = Instant::now();
    assert!(no_answer(&g.set("b", "x").unwrap_err()));
    assert!(t.elapsed() < limit, "at once: {:?}", t.elapsed());
    assert_eq!(held.inner.get("b").unwrap(), None, "nothing was written");
    // The dialog is answered: the hanging call returns and the keychain is asked again.
    held.release();
    let t = Instant::now();
    while g.hanging() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!g.hanging());
    assert_eq!(g.get("a").unwrap().as_deref(), Some("pw"));
    g.set("b", "x").unwrap();
    assert_eq!(g.delete("b"), Ok(true));
}

#[test]
fn a_guarded_keychain_passes_slow_answers_and_failures_through() {
    use std::time::Duration;
    let slow = Arc::new(Held { delay: Duration::from_millis(30), ..Held::default() });
    slow.inner.set("a", "pw").unwrap();
    let g = Guarded::new(slow, Duration::from_secs(5));
    assert_eq!(g.get("a").unwrap().as_deref(), Some("pw"), "slow but in time");
    assert_eq!(g.get("missing").unwrap(), None, "absent is absent");
    let broken = Guarded::new(Arc::new(MemoryStore::unavailable("no secret service")), Duration::from_secs(5));
    assert_eq!(broken.get("a").unwrap_err(), Unavailable(nk("no secret service")));
    assert!(!broken.hanging());
}

/// `stall` (debug builds only) is a keychain that never answers, for checking the time limit
/// by hand; release builds refuse the word like any other typo.
#[test]
fn a_stalled_keychain_is_for_debug_builds_and_ends_at_the_limit() {
    if !cfg!(debug_assertions) {
        assert!(StoreKind::from_env(Some("stall")).is_err());
        return;
    }
    assert_eq!(StoreKind::from_env(Some("stall")), Ok(StoreKind::Stalled));
    let g = Guarded::new(StoreKind::Stalled.open(), std::time::Duration::from_millis(50));
    assert!(no_answer(&g.get("a").unwrap_err()));
    assert!(g.hanging(), "its thread waits for good");
}
