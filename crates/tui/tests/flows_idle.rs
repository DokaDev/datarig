//! Idle connections and the lock wait limit (`idle_disconnect`, `lock_wait`): what the sessions
//! are opened with, a tab's session closing after its limit, then the profile's, the warning for
//! an open transaction, connecting again on the next action without asking, a close the server
//! made, the machine sleeping, and how a lock wait past the limit is said.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbError, DbEvent, Outcome, SessionRole, TxOutcome};
use datarig_core::i18n::{Lang, Msg};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::widgets::editor::Editor;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const MS: Duration = Duration::from_millis(3);
const LIMIT: Duration = Duration::from_secs(300);

/// The test profile in environment `stg` (`rules`), with the config's `idle_disconnect` of 5m and
/// `lock_wait` of 5s.
fn config(rules: Policy) -> Config {
    let p = ConnectionConfig { policy: Some("stg".to_string()), ..ConnectionConfig::test_db() };
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.prefs.idle_disconnect = Some(LIMIT);
    cfg.prefs.lock_wait = Some(Duration::from_secs(5));
    cfg.policies.insert("stg", rules);
    cfg
}

fn connected(rules: Policy) -> Harness {
    let mut h = Harness::with_config(&config(rules), Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into(), "shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}

/// Run `sql` in the active tab (yes to a question) and return the run's id.
fn run(h: &mut Harness, sql: &str) -> u64 {
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
    if h.overlay_kind() == Some(OverlayKind::RunConfirm) {
        h.keys("y");
    }
    h.app.tab().exec.query_id
}

/// Run a statement in the active tab that the session answers at once.
fn ran(h: &mut Harness, sql: &str) {
    let i = h.app.tabs.active_index();
    let id = run(h, sql);
    h.tab_db(i, DbEvent::Done { id, outcome: Outcome::Affected(0), elapsed: MS });
}

/// The tab's Messages say `want` (matched by `pick`).
fn said(h: &Harness, pick: impl Fn(&Msg) -> bool) -> bool {
    h.app.tab().exec.run.notes.iter().any(|n| pick(&n.msg))
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

#[test]
fn sessions_are_opened_with_the_lock_wait_limit_and_the_servers_idle_limit() {
    let mut h = connected(Policy::default());
    ran(&mut h, "SELECT 1");
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query]);
    let sessions = h.driver.sessions.lock().unwrap();
    let (meta, query) = (&sessions[0].opts, &sessions[1].opts);
    assert_eq!(query.lock_wait, Some(Duration::from_secs(5)));
    assert_eq!(query.server_idle, Some(LIMIT + Duration::from_secs(60)), "a minute past the app's");
    // The metadata session gets the limit too (its driver keeps its own, shorter one), never the
    // server's idle limit: the app closes it with its profile.
    assert_eq!(meta.server_idle, None);
    drop(sessions);
    // An environment's own: off.
    let rules = Policy { idle_disconnect: Some(None), lock_wait: Some(None), ..Policy::default() };
    let mut h = connected(rules);
    ran(&mut h, "SELECT 1");
    let sessions = h.driver.sessions.lock().unwrap();
    assert_eq!((sessions[1].opts.lock_wait, sessions[1].opts.server_idle), (None, None));
    drop(sessions);
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None, "off: no timer");
}

#[test]
fn an_idle_tab_closes_its_session_then_the_profile_closes_and_the_next_run_connects_again() {
    let mut h = connected(Policy::default());
    ran(&mut h, "SELECT 1");
    let now = h.clock.now();
    h.app.transient = None;
    assert_eq!(h.app.next_tick(now), Some(now + LIMIT), "one wakeup, at the deadline");
    // The explorer reads something later: the profile is used, the tab is not.
    h.advance(Duration::from_secs(200));
    h.db(DbEvent::Schemas(Ok(vec!["public".into(), "shop".into()])));
    h.advance(Duration::from_secs(99));
    assert!(!h.session_closed(1));
    h.advance(Duration::from_secs(1));
    assert!(h.session_closed(1), "the tab's session closed after 5m");
    assert!(!h.session_closed(0), "the profile's metadata session was used since");
    assert!(said(&h, |m| *m == Msg::QueryIdleClosed { time: "5m".into() }));
    let screen = h.screen(160, 45);
    assert!(screen.contains("This tab's session closed after 5m unused"), "{screen}");
    h.app.transient = None;
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), Some(now + Duration::from_secs(200)), "the profile's deadline");
    h.advance(Duration::from_secs(200));
    assert!(h.session_closed(0), "the metadata session closed with the profile");
    assert_eq!(status(&h), Some(Msg::ConnIdleClosed { name: "local-pg".into(), time: "5m".into() }));
    assert!(
        h.explorer_line().contains("closed after 5m unused") || h.screen(160, 45).contains("closed after 5m unused")
    );
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None, "nothing open: no timer");
    // The next run connects again with the password it connected with (nothing is asked),
    // and runs once connected.
    let passwords = h.driver.passwords.lock().unwrap().clone();
    let id = run(&mut h, "SELECT 2");
    assert_eq!(h.overlay_kind(), None, "no password prompt");
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query, SessionRole::Meta]);
    assert_eq!(h.driver.passwords.lock().unwrap().last(), passwords.first());
    h.db(DbEvent::Connected);
    assert_eq!(h.roles().last(), Some(&SessionRole::Query), "the waiting run opened its session");
    assert!(h.app.tab().exec.running.is_some_and(|r| r.id == id) || h.app.tab().exec.query_id >= id);
    let i = h.app.tabs.active_index();
    let id = h.app.tab().exec.query_id;
    h.tab_db(i, DbEvent::Done { id, outcome: Outcome::Affected(0), elapsed: MS });
    h.app.transient = None;
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), Some(now + LIMIT), "a new idle period");
}

#[test]
fn an_open_transaction_is_warned_about_a_minute_before_its_idle_session_closes() {
    let rules = Policy { commit: CommitRule::Manual, tx_idle_warning: None, ..Policy::default() };
    let mut h = connected(rules);
    let id = run(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4");
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(3), elapsed: MS });
    h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    assert!(h.app.tab().exec.user_tx());
    let now = h.clock.now();
    h.app.transient = None;
    // The status bar's second of the open transaction ticks too; the warning is due at 4m.
    assert!(h.app.next_tick(now).is_some_and(|t| t <= now + Duration::from_secs(240)));
    h.advance(Duration::from_secs(239));
    assert!(!said(&h, |m| matches!(m, Msg::TxIdleCloseWarning { .. })));
    h.advance(Duration::from_secs(1));
    let warning = Msg::TxIdleCloseWarning {
        tab: "1".into(),
        time: "1:00".into(),
        commit: "Space x c".into(),
        rollback: "Space x r".into(),
    };
    assert_eq!(status(&h), Some(warning.clone()));
    // Once per idle period.
    h.app.status = None;
    h.advance(Duration::from_secs(30));
    assert_eq!(status(&h), None);
    h.advance(Duration::from_secs(30));
    assert!(h.session_closed(1));
    assert!(said(&h, |m| *m == Msg::QueryIdleClosedTx { time: "5m".into() }));
    assert!(!h.app.tab().exec.user_tx(), "the transaction is gone with its session");
    assert_eq!(status(&h), Some(Msg::ConnIdleClosedTx { name: "local-pg".into(), time: "5m".into() }));
    // Committing in time keeps it open: a new idle period.
    let rules = Policy { commit: CommitRule::Manual, tx_idle_warning: None, ..Policy::default() };
    let mut h = connected(rules);
    let id = run(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4");
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(3), elapsed: MS });
    h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.advance(Duration::from_secs(250));
    h.keys(" xc");
    let end = h.sent().into_iter().find_map(|c| match c {
        datarig_core::driver::DbCommand::EndTx { id, .. } => Some(id),
        _ => None,
    });
    h.tab_db(0, DbEvent::TxEnded { id: end.expect("a commit"), outcome: TxOutcome::Committed });
    h.tab_db(0, DbEvent::Block(false));
    h.tab_db(0, DbEvent::TxOpen(false));
    h.advance(Duration::from_secs(100));
    assert!(!h.session_closed(1), "the commit was a use");
}

#[test]
fn time_the_machine_slept_counts() {
    let mut h = connected(Policy::default());
    ran(&mut h, "SELECT 1");
    // The app's clock stood still for the night; the wall clock did not.
    let night = SystemTime::now() + Duration::from_secs(8 * 3600);
    h.app.set_wall_clock(Arc::new(move || night));
    h.app.transient = None;
    let now = h.clock.now();
    assert!(h.app.next_tick(now).is_some_and(|t| t <= now + Duration::from_millis(1)), "due at once");
    // The first key after waking closes it before it acts: the run connects again.
    run(&mut h, "SELECT 2");
    assert!(h.session_closed(1) && h.session_closed(0), "closed before the run");
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query, SessionRole::Meta], "connecting again");
}

#[test]
fn a_session_the_server_closed_for_being_idle_says_so() {
    let mut h = connected(Policy::default());
    ran(&mut h, "SELECT 1");
    let text = "FATAL: terminating connection due to idle-session timeout";
    h.tab_db(0, DbEvent::Lost { error: DbError::IdleSessionTimeout(text.into()) });
    assert!(said(&h, |m| *m == Msg::QueryIdleClosedByServer { error: text.into() }));
    assert!(!said(&h, |m| matches!(m, Msg::QueryLost { .. })), "not a lost connection");
    // The next run connects again (the profile's metadata session is still there).
    run(&mut h, "SELECT 2");
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query, SessionRole::Query]);
    // The metadata session's: no failure either.
    h.db(DbEvent::Lost { error: DbError::IdleSessionTimeout(text.into()) });
    assert!(matches!(status(&h), Some(Msg::ConnIdleClosedByServer { .. })), "{:?}", status(&h));
}

#[test]
fn a_lock_wait_past_the_limit_says_how_long_it_waited() {
    let mut h = connected(Policy::default());
    let id = run(&mut h, "UPDATE shop.users SET name = 'x' WHERE id = 1");
    let server = "ERROR: canceling statement due to lock timeout";
    h.tab_db(0, DbEvent::Failed { id, error: DbError::LockTimeout(server.into()), cancelled: false });
    let screen = h.screen(200, 45);
    assert!(
        screen.contains("Waited longer than 5s for a lock another session holds, so the server cancelled the"),
        "{screen}"
    );
    // Without a limit of the app's (the server's own, or the user's `SET`): no time.
    let rules = Policy { lock_wait: Some(None), ..Policy::default() };
    let mut h = connected(rules);
    let id = run(&mut h, "UPDATE shop.users SET name = 'x' WHERE id = 1");
    h.tab_db(0, DbEvent::Failed { id, error: DbError::LockTimeout(server.into()), cancelled: false });
    let screen = h.screen(200, 45);
    assert!(screen.contains("waited too long for a lock another session holds"), "{screen}");
}

#[test]
fn set_takes_any_duration_in_range_and_refuses_the_rest() {
    let mut h = connected(Policy::default());
    h.command("set idle_disconnect=7m");
    assert_eq!(h.app.prefs.idle_disconnect, Some(Duration::from_secs(420)));
    h.command("set lock_wait=off");
    assert_eq!(h.app.prefs.lock_wait, None);
    h.command("set lock_wait=10s");
    assert_eq!(h.app.prefs.lock_wait, Some(Duration::from_secs(10)));
    for bad in ["set lock_wait=500ms", "set idle_disconnect=30s", "set idle_disconnect=soon"] {
        h.command(bad);
        // Refused: the command line stays open with why.
        assert!(h.cmdline().is_some(), "{bad}");
        h.key(ratatui::crossterm::event::KeyCode::Esc);
        assert_eq!(h.app.prefs.lock_wait, Some(Duration::from_secs(10)), "{bad}");
        assert_eq!(h.app.prefs.idle_disconnect, Some(Duration::from_secs(420)), "{bad}");
    }
}
