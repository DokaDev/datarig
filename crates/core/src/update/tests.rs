use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("datarig-update-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn t(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn on() -> Switches {
    Switches { config_on: true, env_off: false, ci: false, debug_build: false, lock_holder: true }
}

fn known(state: State) -> Cache {
    Cache::Known(PathBuf::from("update.toml"), Box::new(state))
}

#[test]
fn versions_parse_as_the_release_script_takes_them() {
    assert_eq!(v("0.39.0"), Version { major: 0, minor: 39, patch: 0, rc: None });
    assert_eq!(v("1.2.3-rc.4").rc, Some(4));
    assert_eq!(Version::parse_tag("v0.38.3"), Some(v("0.38.3")));
    assert_eq!(Version::parse_tag("0.38.3"), Some(v("0.38.3")));
    for bad in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "01.2.3",
        "1.02.3",
        "1.2.03",
        "1.2.3-rc.0",
        "1.2.3-rc.01",
        "1.2.3-beta.1",
        "1.2.3-rc",
        "1.2.3+b",
        "v",
        " 1.2.3",
        "1.2.-3",
        "1.2.3-rc.1.2",
        "99999999999999999999.0.0",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad:?}");
    }
    assert_eq!(v("1.2.3-rc.4").to_string(), "1.2.3-rc.4");
    assert_eq!(v("0.39.0").to_string(), "0.39.0");
    assert_eq!(Version::running().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_release_candidate_comes_before_its_release() {
    let order = ["0.9.9", "0.10.0-rc.1", "0.10.0-rc.2", "0.10.0-rc.10", "0.10.0", "0.10.1", "0.11.0", "1.0.0"];
    for w in order.windows(2) {
        assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
    }
}

#[test]
fn a_missing_file_knows_nothing_and_the_state_round_trips() {
    let d = dir("round");
    let path = d.join(FILE);
    assert_eq!(load(&path), Ok(State::default()));
    let state = State {
        last_attempt: Some(t("2026-11-06T09:12:00Z")),
        result: Some(Outcome::Ok),
        etag: Some("W/\"5d1c\"".into()),
        latest: Some(v("0.39.1")),
        latest_date: Some("2026-11-10".into()),
        summary: Some("The grid \"sums\" a range.".into()),
        seen: Some(v("0.39.0")),
        skipped: Some(v("0.39.1")),
        reminded: Some((v("0.39.1"), t("2026-11-06T09:13:00Z"))),
    };
    save(&path, &state).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for line in [
        "last_attempt = 2026-11-06T09:12:00Z",
        "result = \"ok\"",
        "latest = \"0.39.1\"",
        "latest_date = 2026-11-10",
        "seen = \"0.39.0\"",
        "reminded = { version = \"0.39.1\", at = 2026-11-06T09:13:00Z }",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
    assert_eq!(load(&path), Ok(state));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn every_outcome_round_trips_and_one_a_newer_version_wrote_is_kept() {
    let d = dir("outcomes");
    let path = d.join(FILE);
    for (o, _) in Outcome::NAMES {
        let s = State { result: Some(o), ..State::default() };
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap().result, Some(o));
    }
    std::fs::write(&path, "result = \"captive_portal\"\nfuture = 1\n").unwrap();
    let mut s = load(&path).unwrap();
    assert_eq!(s.result, Some(Outcome::Other));
    s.seen = Some(v("0.39.0"));
    save(&path, &s).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("result = \"captive_portal\""), "{text}");
    assert!(text.contains("future = 1"), "{text}");
    assert!(text.contains("seen = \"0.39.0\""), "{text}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unreadable_file_is_unknown_and_never_written_over() {
    let d = dir("damaged");
    let path = d.join(FILE);
    for damaged in [
        "last_attempt = \n",
        "last_attempt = \"yesterday\"\n",
        "last_attempt = 2026-11-06T09:12:00\n",
        "latest = \"0.39\"\n",
        "seen = 39\n",
        "latest_date = 2026-11-10T00:00:00Z\n",
        "reminded = { version = \"0.39.1\" }\n",
        "reminded = \"0.39.1\"\n",
        "etag = 5\n",
    ] {
        std::fs::write(&path, damaged).unwrap();
        assert_eq!(load(&path), Err(Unreadable), "{damaged}");
        let e = save(&path, &State { seen: Some(v("0.39.0")), ..State::default() }).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData, "{damaged}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged, "written over");
        let paths = crate::paths::Paths { data: None, state: Some(d.clone()) };
        assert_eq!(Cache::load(&paths), Cache::Unreadable(path.clone()));
    }
    // A folder in the file's place cannot be read either.
    let folder = d.join("folder");
    std::fs::create_dir_all(&folder).unwrap();
    assert_eq!(load(&folder), Err(Unreadable));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_state_directory_is_its_own_case() {
    assert_eq!(Cache::load(&crate::paths::Paths::default()), Cache::NoStateDir);
    let now = t("2026-11-06T09:00:00Z");
    assert_eq!(decide(now, &Cache::NoStateDir, &on()), Decision::Off(Off::NoStateDir));
}

#[test]
fn the_check_is_due_once_a_day() {
    let now = t("2026-11-06T09:00:00Z");
    let at = |s: &str| known(State { last_attempt: Some(t(s)), ..State::default() });
    assert_eq!(decide(now, &known(State::default()), &on()), Decision::Due);
    assert_eq!(decide(now, &at("2026-11-05T09:00:01Z"), &on()), Decision::NotDue);
    assert_eq!(decide(now, &at("2026-11-06T09:00:00Z"), &on()), Decision::NotDue);
    assert_eq!(decide(now, &at("2026-11-05T09:00:00Z"), &on()), Decision::Due);
    assert_eq!(decide(now, &at("2025-01-01T00:00:00Z"), &on()), Decision::Due);
    // A failed attempt counts as the day's too.
    let failed = known(State {
        last_attempt: Some(t("2026-11-06T08:00:00Z")),
        result: Some(Outcome::Unreachable),
        ..State::default()
    });
    assert_eq!(decide(now, &failed, &on()), Decision::NotDue);
    // An attempt that never ended (no result) as well.
    let hung = known(State { last_attempt: Some(t("2026-11-06T08:00:00Z")), result: None, ..State::default() });
    assert_eq!(decide(now, &hung, &on()), Decision::NotDue);
    // A clock that went back cannot hold checks off for ever.
    assert_eq!(decide(now, &at("2026-11-06T09:59:00Z"), &on()), Decision::NotDue);
    assert_eq!(decide(now, &at("2026-11-06T10:00:01Z"), &on()), Decision::Due);
    assert_eq!(decide(now, &at("2027-11-06T09:00:00Z"), &on()), Decision::Due);
}

#[test]
fn each_switch_turns_the_check_off() {
    let now = t("2026-11-06T09:00:00Z");
    let c = known(State::default());
    let off = |sw: Switches| decide(now, &c, &sw);
    assert_eq!(off(Switches { env_off: true, ..on() }), Decision::Off(Off::Env));
    assert_eq!(off(Switches { config_on: false, ..on() }), Decision::Off(Off::Config));
    assert_eq!(off(Switches { ci: true, ..on() }), Decision::Off(Off::Ci));
    assert_eq!(off(Switches { debug_build: true, ..on() }), Decision::Off(Off::DebugBuild));
    assert_eq!(off(Switches { lock_holder: false, ..on() }), Decision::Off(Off::SecondInstance));
    assert_eq!(decide(now, &Cache::Unreadable("x".into()), &on()), Decision::Off(Off::Unreadable));
    // The environment wins over the config, and the config is named before the rest.
    let all = Switches { config_on: false, env_off: true, ci: true, debug_build: true, lock_holder: false };
    assert_eq!(off(all), Decision::Off(Off::Env));
    assert_eq!(off(Switches { env_off: false, ..all }), Decision::Off(Off::Config));
}

#[test]
fn a_debug_build_never_decides_to_check() {
    // What `cargo test` builds: whatever the config, the file and the environment say.
    let now = t("2026-11-06T09:00:00Z");
    let sw = Switches::new(true, false, false, true);
    assert_eq!(sw.debug_build, cfg!(debug_assertions));
    if cfg!(debug_assertions) {
        assert_eq!(decide(now, &known(State::default()), &sw), Decision::Off(Off::DebugBuild));
    }
}

#[test]
fn claim_and_record() {
    let now = t("2026-11-06T09:00:00Z");
    let mut s =
        State { result: Some(Outcome::Ok), latest: Some(v("0.39.0")), etag: Some("a".into()), ..State::default() };
    s.claim(now);
    assert_eq!((s.last_attempt, s.result), (Some(now), None));
    s.record(Answer::Failed(Outcome::TimedOut));
    assert_eq!(s.result, Some(Outcome::TimedOut));
    assert_eq!((s.latest, s.etag.as_deref()), (Some(v("0.39.0")), Some("a")), "a failure keeps what was known");
    s.record(Answer::NotModified);
    assert_eq!((s.result, s.latest), (Some(Outcome::NotModified), Some(v("0.39.0"))));
    let r = Release { version: v("0.39.1"), date: Some("2026-11-10".into()), summary: Some("New.".into()) };
    s.record(Answer::Release(r, Some("b".into())));
    assert_eq!(s.result, Some(Outcome::Ok));
    assert_eq!(
        (s.latest, s.latest_date.as_deref(), s.summary.as_deref(), s.etag.as_deref()),
        (Some(v("0.39.1")), Some("2026-11-10"), Some("New."), Some("b"))
    );
    // A release without a summary does not keep the last one's.
    s.record(Answer::Release(Release { version: v("0.39.2"), date: None, summary: None }, None));
    assert_eq!((s.summary, s.latest_date, s.etag), (None, None, None));
}

#[test]
fn the_notice_tells_about_a_newer_release_until_skipped_and_again_a_week_after_closing() {
    let now = t("2026-11-06T09:00:00Z");
    let running = v("0.39.0");
    let base = State { latest: Some(v("0.39.1")), ..State::default() };
    let notice_for = |s: &State, sw: Switches| notice(now, &known(s.clone()), &sw, &running);
    assert_eq!(notice_for(&base, on()), Some(v("0.39.1")));
    // Not newer.
    for l in ["0.39.0", "0.38.3", "0.39.1-rc.1"] {
        assert_eq!(notice_for(&State { latest: Some(v(l)), ..State::default() }, on()), None, "{l}");
    }
    assert_eq!(notice_for(&State::default(), on()), None);
    // Skipped, but a newer one is told about.
    let mut s = base.clone();
    s.skip(v("0.39.1"));
    assert_eq!(notice_for(&s, on()), None);
    s.latest = Some(v("0.39.2"));
    assert_eq!(notice_for(&s, on()), Some(v("0.39.2")));
    // Closed: back after seven days, and at once for a newer version.
    let mut s = base.clone();
    s.remind(v("0.39.1"), t("2026-11-01T09:00:01Z"));
    assert_eq!(notice_for(&s, on()), None);
    s.remind(v("0.39.1"), t("2026-10-30T09:00:00Z"));
    assert_eq!(notice_for(&s, on()), Some(v("0.39.1")));
    s.remind(v("0.39.0"), t("2026-11-06T08:00:00Z"));
    assert_eq!(notice_for(&s, on()), Some(v("0.39.1")));
    // A close in the future (the clock went back) holds nothing off.
    s.remind(v("0.39.1"), t("2026-11-08T09:00:00Z"));
    assert_eq!(notice_for(&s, on()), Some(v("0.39.1")));
    // Checks off: no notice. A second instance tells (it does not check).
    for sw in [
        Switches { config_on: false, ..on() },
        Switches { env_off: true, ..on() },
        Switches { ci: true, ..on() },
        Switches { debug_build: true, ..on() },
    ] {
        assert_eq!(notice_for(&base, sw), None, "{sw:?}");
    }
    assert_eq!(notice_for(&base, Switches { lock_holder: false, ..on() }), Some(v("0.39.1")));
    assert_eq!(notice(now, &Cache::Unreadable("x".into()), &on(), &running), None);
    assert_eq!(notice(now, &Cache::NoStateDir, &on(), &running), None);
}

#[test]
fn whats_new_shows_once_after_an_update() {
    let running = v("0.39.0");
    let seen = |s: &str| known(State { seen: Some(v(s)), ..State::default() });
    // An update: the versions after the last one seen.
    assert_eq!(
        whats_new(&seen("0.38.3"), &running, true, true, true),
        WhatsNew { show: Some(Some(v("0.38.3"))), seen: Some(running) }
    );
    // The same version, or an older one (a downgrade): nothing, and the newest stays.
    assert_eq!(whats_new(&seen("0.39.0"), &running, true, true, true), WhatsNew::default());
    assert_eq!(whats_new(&seen("0.40.0"), &running, true, true, true), WhatsNew::default());
    // A first start: nothing is new, but the version is kept.
    assert_eq!(
        whats_new(&known(State::default()), &running, false, true, true),
        WhatsNew { show: None, seen: Some(running) }
    );
    // A user from before What's new: the running version's notes.
    assert_eq!(
        whats_new(&known(State::default()), &running, true, true, true),
        WhatsNew { show: Some(None), seen: Some(running) }
    );
    // Turned off: never by itself, but the version still advances.
    assert_eq!(whats_new(&seen("0.38.3"), &running, true, false, true), WhatsNew { show: None, seen: Some(running) });
    // A second instance, an unreadable file, no state directory: nothing shown, nothing written.
    assert_eq!(whats_new(&seen("0.38.3"), &running, true, true, false), WhatsNew::default());
    assert_eq!(whats_new(&Cache::Unreadable("x".into()), &running, true, true, true), WhatsNew::default());
    assert_eq!(whats_new(&Cache::NoStateDir, &running, true, true, true), WhatsNew::default());
}
