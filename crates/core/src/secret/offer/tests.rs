use super::*;
use crate::fault::Fault;
use crate::secret::Call;
use crate::secret::MemoryStore;

fn binary(version: &str) -> BinaryId {
    BinaryId {
        version: version.into(),
        exe: "/opt/datarig/bin/datarig".into(),
        size: 4_000_000,
        mtime_ns: 1_700_000_000,
    }
}

fn seen(ms: u64) -> Observation {
    Observation { call: Call::Get, elapsed: Duration::from_millis(ms), timed_out: false, ok: true, found: true }
}

const TIMED_OUT: Observation =
    Observation { call: Call::Get, elapsed: Duration::from_secs(10), timed_out: true, ok: false, found: false };

/// A Mac, the OS keychain, `direct`, not over SSH, nothing answered.
fn facts<'a>(state: Option<&'a OfferState>, current: &'a BinaryId, seen: Observation) -> Facts<'a> {
    Facts { macos: true, os_keychain: true, remote: false, access: KeychainAccess::Direct, state, current, seen }
}

fn recorded(b: BinaryId) -> OfferState {
    OfferState { answer: None, binary: Some(b) }
}

#[test]
fn prompt_likely_needs_a_slow_call_and_another_binary() {
    let (old, new) = (binary("0.17.0"), binary("0.18.0"));
    assert!(!prompt_likely(seen(50), Some(&old), &new), "fast + changed: answered by itself");
    assert!(prompt_likely(seen(1_000), Some(&old), &new), "slow + changed");
    assert!(prompt_likely(seen(4_000), Some(&old), &new), "slow + changed");
    assert!(!prompt_likely(seen(4_000), Some(&new), &new), "slow + same: a locked keychain, a slow disk");
    assert!(prompt_likely(TIMED_OUT, Some(&old), &new), "timeout + changed");
    assert!(!prompt_likely(TIMED_OUT, Some(&new), &new), "timeout + same");
    assert!(prompt_likely(seen(1_500), None, &new), "no record + slow: timing alone");
    assert!(!prompt_likely(seen(999), None, &new), "no record + fast");
    // Each part of the identity counts.
    for other in [
        BinaryId { exe: "/usr/local/bin/datarig".into(), ..new.clone() },
        BinaryId { size: 1, ..new.clone() },
        BinaryId { mtime_ns: 1, ..new.clone() },
    ] {
        assert!(prompt_likely(seen(2_000), Some(&other), &new), "{other:?}");
    }
}

#[test]
fn offered_only_on_a_mac_with_the_os_keychain_in_direct_mode_and_not_answered() {
    let (old, new) = (binary("0.17.0"), binary("0.18.0"));
    let state = recorded(old.clone());
    let slow = seen(3_000);
    assert!(should_offer(&facts(Some(&state), &new, slow)), "slow + changed");
    assert!(!should_offer(&facts(Some(&state), &new, seen(10))), "fast + changed");
    assert!(should_offer(&facts(Some(&state), &new, TIMED_OUT)), "timeout + changed");
    let same = recorded(new.clone());
    assert!(!should_offer(&facts(Some(&same), &new, slow)), "slow + same");
    let fresh = OfferState::default();
    assert!(should_offer(&facts(Some(&fresh), &new, slow)), "no record + slow");
    assert!(!should_offer(&facts(Some(&fresh), &new, seen(200))), "no record + fast");
    // A state file that cannot be read: unknown, not "not answered".
    assert!(!should_offer(&facts(None, &new, slow)), "unreadable state file");
    for answer in [Answer::Accepted, Answer::Declined] {
        let answered = OfferState { answer: Some(answer), binary: Some(old.clone()) };
        assert!(!should_offer(&facts(Some(&answered), &new, slow)), "already answered {answer:?}");
    }
    assert!(!should_offer(&Facts { remote: true, ..facts(Some(&state), &new, slow) }), "over SSH");
    let tool = Facts { access: KeychainAccess::SecurityTool, ..facts(Some(&state), &new, slow) };
    assert!(!should_offer(&tool), "security_tool mode");
    assert!(!should_offer(&Facts { os_keychain: false, ..facts(Some(&state), &new, slow) }), "memory store");
    assert!(!should_offer(&Facts { macos: false, ..facts(Some(&state), &new, slow) }), "not macOS");
}

#[test]
fn remote_reads_the_three_ssh_variables() {
    let env = |set: &'static str| move |k: &str| (k == set).then(|| "1.2.3.4 22".to_string());
    assert!(remote(env("SSH_CONNECTION")));
    assert!(remote(env("SSH_CLIENT")));
    assert!(remote(env("SSH_TTY")));
    assert!(!remote(env("TERM")));
    assert!(!remote(|k: &str| (k == "SSH_TTY").then(String::new)), "empty is unset");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-offer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_state_file_round_trips_and_an_unreadable_one_is_not_empty() {
    let dir = scratch("state");
    let file = dir.join(STATE_FILE);
    assert_eq!(load(&file), Ok(OfferState::default()), "no file: nothing recorded or answered");
    let state = OfferState { answer: Some(Answer::Declined), binary: Some(binary("0.18.0")) };
    save(&file, &state).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("answer = \"declined\"\n"), "{text}");
    assert!(text.contains("[binary]\nversion = \"0.18.0\""), "{text}");
    assert_eq!(load(&file), Ok(state));
    save(&file, &OfferState { answer: Some(Answer::Accepted), binary: None }).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "answer = \"accepted\"\n");
    // Not TOML, or not this shape: unreadable.
    std::fs::write(&file, "answer = [").unwrap();
    assert_eq!(load(&file), Err(Unreadable));
    std::fs::write(&file, "answer = \"maybe\"\n").unwrap();
    assert_eq!(load(&file), Err(Unreadable));
    // A directory where the file should be.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir_all(&file).unwrap();
    assert_eq!(load(&file), Err(Unreadable));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A keychain that moves a fake clock forward by `delay` on each call.
struct Slow {
    inner: MemoryStore,
    clock: Arc<Mutex<Instant>>,
    delay: Duration,
}

impl Slow {
    fn wait(&self) {
        *self.clock.lock().unwrap() += self.delay;
    }
}

impl SecretStore for Slow {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        self.wait();
        self.inner.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.wait();
        self.inner.set(account, secret)
    }
    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        self.wait();
        self.inner.delete(account)
    }
}

fn timed(store: Arc<dyn SecretStore>, clock: Arc<Mutex<Instant>>, limit: Duration) -> (Timed, KeychainSwitch, Seen) {
    let switch = KeychainSwitch::default();
    let seen = Seen::default();
    let c: Clock = Arc::new(move || *clock.lock().unwrap());
    let t = Timed::new(Arc::new(Guarded::new(store, limit)), switch.clone(), c, seen.clone());
    (t, switch, seen)
}

#[test]
fn timed_records_each_direct_call() {
    let clock = Arc::new(Mutex::new(Instant::now()));
    let slow = Arc::new(Slow { inner: MemoryStore::new(), clock: clock.clone(), delay: Duration::from_millis(1_500) });
    let (store, switch, seen) = timed(slow, clock, Duration::from_secs(10));
    store.set("a", "pw").unwrap();
    assert_eq!(store.get("a").unwrap().as_deref(), Some("pw"));
    assert_eq!(store.get("b").unwrap(), None);
    assert!(store.delete("a").unwrap());
    let all = std::mem::take(&mut *seen.lock().unwrap());
    let slow =
        |call, found| Observation { call, elapsed: Duration::from_millis(1_500), timed_out: false, ok: true, found };
    assert_eq!(
        all,
        vec![slow(Call::Set, false), slow(Call::Get, true), slow(Call::Get, false), slow(Call::Delete, false)]
    );
    // Only the read that returned a password proves macOS trusts this binary.
    assert_eq!(all.iter().map(Observation::trusted).collect::<Vec<_>>(), [false, true, false, false]);
    // The security tool's calls are not this question's.
    switch.set(KeychainAccess::SecurityTool);
    store.get("a").unwrap();
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn timed_records_a_failure_and_a_call_that_did_not_answer() {
    let clock = Arc::new(Mutex::new(Instant::now()));
    let (store, _, seen) = timed(Arc::new(MemoryStore::unavailable("locked")), clock, Duration::from_secs(10));
    assert!(store.get("a").is_err());
    let failed = Observation { call: Call::Get, elapsed: Duration::ZERO, timed_out: false, ok: false, found: false };
    assert_eq!(*seen.lock().unwrap(), vec![failed]);
    assert!(!failed.trusted());

    /// A keychain that never answers within the test's short limit.
    struct Stuck;
    impl SecretStore for Stuck {
        fn get(&self, _: &str) -> Result<Option<String>, Unavailable> {
            std::thread::sleep(Duration::from_millis(300));
            Err(Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::Failure), "late")))
        }
        fn set(&self, _: &str, _: &str) -> Result<(), Unavailable> {
            unreachable!()
        }
        fn delete(&self, _: &str) -> Result<bool, Unavailable> {
            unreachable!()
        }
    }
    let clock = Arc::new(Mutex::new(Instant::now()));
    let (store, _, seen) = timed(Arc::new(Stuck), clock, Duration::from_millis(20));
    let e = store.get("a").unwrap_err();
    assert_eq!(e.0.kind, FaultKind::Keychain(KeychainFault::NoAnswer));
    // Refused at once while the first one hangs: never reached the keychain, not timed.
    assert!(store.get("a").is_err());
    let all = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].timed_out && !all[0].ok && !all[0].trusted());
}
