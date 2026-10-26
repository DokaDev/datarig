//! Idle connections and the lock wait limit (`idle_disconnect`, `lock_wait`): what the sessions
//! are opened with, a tab's session closing after its limit, then the profile's, the warning for
//! an open transaction, connecting again on the next action without asking, a close the server
//! made, the machine sleeping, and how a lock wait past the limit is said. Also
//! `tx_idle_rollback`, which the same timer keeps on every driver: an idle transaction opened for
//! manual commit is rolled back on its session, which stays (closed instead when the rollback
//! fails or is not answered in time); PostgreSQL's server gets a later limit as a backstop.

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
use std::sync::atomic::Ordering;
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

const ROLLBACK: Duration = Duration::from_secs(90);

/// Connected to the test profile on MySQL's driver (PostgreSQL's without `mysql`), in an
/// environment that commits by hand and rolls back a transaction idle for [`ROLLBACK`], warned
/// about after a minute (`idle_disconnect` the config's 5m, or `rules`' own).
fn manual_on(mysql: bool, rules: Policy) -> Harness {
    let mut cfg = config(rules);
    if mysql {
        cfg.connections[0].driver = "mysql".into();
    }
    let driver = FakeDriver::default();
    driver.mysql.store(mysql, Ordering::SeqCst);
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into(), "shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}

fn rollback_rules() -> Policy {
    Policy {
        commit: CommitRule::Manual,
        tx_idle_warning: Some(Duration::from_secs(60)),
        tx_idle_rollback: Some(ROLLBACK),
        ..Policy::default()
    }
}

/// A write that opens the transaction for manual commit in the active tab, answered.
fn write_in_tx(h: &mut Harness) {
    let id = run(h, "UPDATE users SET name = 'x' WHERE id < 4");
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(3), elapsed: MS });
    h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    assert!(h.app.tab().exec.user_tx());
    h.sent();
}

/// The commits and rollbacks sent since the last look: their ids and whether each commits.
fn ends(h: &mut Harness) -> Vec<(u64, bool)> {
    use datarig_core::driver::DbCommand;
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::EndTx { id, commit } => Some((id, commit)),
            _ => None,
        })
        .collect()
}

/// The session confirms rollback `id`: the transaction ended as asked.
fn rolled_back(h: &mut Harness, id: u64) {
    h.tab_db(0, DbEvent::TxEnded { id, outcome: TxOutcome::RolledBack });
    h.tab_db(0, DbEvent::Block(false));
    h.tab_db(0, DbEvent::TxOpen(false));
}

fn dbms(mysql: bool) -> &'static str {
    if mysql { "mysql" } else { "postgres" }
}

/// The rollback the app sends for an idle transaction is ROLLBACK on the same session, which
/// stays open: the tab says it rolls back while it waits (the transaction still shown open:
/// nothing changes before the server confirms), then that it was rolled back, and the next run
/// uses the same session.
#[test]
fn an_idle_manual_transaction_is_rolled_back_and_its_session_stays() {
    for mysql in [true, false] {
        let mut h = manual_on(mysql, rollback_rules());
        write_in_tx(&mut h);
        let now = h.clock.now();
        h.app.transient = None;
        assert!(h.app.next_tick(now).is_some_and(|t| t <= now + Duration::from_secs(60)), "a timer runs");
        // The warning first, at a minute.
        h.advance(Duration::from_secs(60));
        let warning = Msg::TxIdleWarning {
            tab: "1".into(),
            time: "1:00".into(),
            commit: "Space x c".into(),
            rollback: "Space x r".into(),
        };
        assert_eq!(status(&h), Some(warning), "{}", dbms(mysql));
        // The deadline is the transaction's, not the 5m of `idle_disconnect`.
        h.app.transient = None;
        let now = h.clock.now();
        assert!(h.app.next_tick(now).is_some_and(|t| t <= now + Duration::from_secs(30)));
        h.advance(Duration::from_secs(29));
        assert!(ends(&mut h).is_empty(), "{}", dbms(mysql));
        h.advance(Duration::from_secs(1));
        let sent = ends(&mut h);
        assert_eq!(sent.len(), 1, "{}: one rollback after 90s idle: {sent:?}", dbms(mysql));
        let (end, commit) = sent[0];
        assert!(!commit, "a rollback, never a commit");
        assert!(!h.session_closed(1), "{}: the session stays", dbms(mysql));
        assert_eq!(status(&h), Some(Msg::TxIdleRollingBack { time: "90s".into() }));
        assert!(h.app.tab().exec.user_tx(), "still open until the server says");
        assert!(h.app.tx_status().is_some_and(|s| s.starts_with("TX open")), "{:?}", h.app.tx_status());
        assert!(h.app.tab_busy(h.app.tab().id), "nothing else is sent meanwhile");
        h.advance(Duration::from_secs(5));
        assert!(ends(&mut h).is_empty(), "sent once");
        rolled_back(&mut h, end);
        let done = Msg::TxIdleRolledBack { count: 1, time: "90s".into() };
        assert!(said(&h, |m| *m == done), "{}", dbms(mysql));
        assert_eq!(status(&h), Some(done));
        assert!(!h.app.tab().exec.user_tx());
        assert_eq!(h.app.tx_status(), None, "the indicator is off");
        assert!(!h.app.tab_busy(h.app.tab().id));
        assert!(!h.session_closed(1));
        assert!(!said(&h, |m| matches!(
            m,
            Msg::QueryTxIdleClosed { .. }
                | Msg::QueryIdleClosedTx { .. }
                | Msg::TxIdleCloseWarning { .. }
                | Msg::TxIdleRollbackWarning { .. }
        )));
        let screen = h.screen(200, 45);
        assert!(screen.contains("Rolled back after 90s idle: 1 change undone; the session stays open"), "{screen}");
        // The summary has nothing to show.
        h.keys(" xs");
        assert_ne!(h.overlay_kind(), Some(OverlayKind::TxSummary));
        // The next run uses the same session, nothing asked.
        ran(&mut h, "SELECT 1");
        assert_eq!(h.overlay_kind(), None);
        assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query], "{}", dbms(mysql));
        // The answer was a use: the session closes for being unused only 5m later.
        h.advance(Duration::from_secs(290));
        assert!(!h.session_closed(1));
    }
}

/// PostgreSQL's server gets the limit too, later than the app's own rollback and its wait for the
/// answer: it acts only when the app could not.
#[test]
fn postgresql_gets_the_limit_plus_a_margin_with_the_transaction() {
    use datarig_core::driver::{DbCommand, TxRule};
    let mut h = manual_on(false, rollback_rules());
    run(&mut h, "UPDATE users SET name = 'x' WHERE id < 4");
    let rule = h.sent().into_iter().find_map(|c| match c {
        DbCommand::Execute { tx, .. } => Some(tx),
        _ => None,
    });
    let Some(TxRule::Manual { idle_timeout, .. }) = rule else { panic!("{rule:?}") };
    assert_eq!(idle_timeout, Some(ROLLBACK + Duration::from_secs(30)));
    assert!(idle_timeout.unwrap() > ROLLBACK + datarig_tui::app::idle::TX_ROLLBACK_ANSWER_WAIT);
    // Off: none (the server's own setting).
    let mut h = manual_on(false, Policy { tx_idle_rollback: None, ..rollback_rules() });
    run(&mut h, "UPDATE users SET name = 'x' WHERE id < 4");
    let rule = h.sent().into_iter().find_map(|c| match c {
        DbCommand::Execute { tx, .. } => Some(tx),
        _ => None,
    });
    assert!(matches!(rule, Some(TxRule::Manual { idle_timeout: None, .. })), "{rule:?}");
}

/// Fail closed: a rollback not answered in time, or refused, closes the session (the server
/// rolls back when it ends), says so, and the next run connects again; a late answer of the
/// closed session is dropped.
#[test]
fn a_rollback_not_answered_in_time_or_refused_closes_the_session() {
    for mysql in [true, false] {
        let mut h = manual_on(mysql, rollback_rules());
        write_in_tx(&mut h);
        h.advance(ROLLBACK);
        let sent = ends(&mut h);
        assert_eq!(sent.len(), 1, "{sent:?}");
        h.app.transient = None;
        let now = h.clock.now();
        assert!(h.app.next_tick(now).is_some_and(|t| t <= now + Duration::from_secs(10)), "the wait is bounded");
        h.advance(Duration::from_secs(9));
        assert!(!h.session_closed(1), "{}", dbms(mysql));
        h.advance(Duration::from_secs(1));
        assert!(h.session_closed(1), "{}: closed after 10s without an answer", dbms(mysql));
        let closed = Msg::QueryTxIdleRollbackUnanswered { time: "90s".into(), wait: "10s".into() };
        assert!(said(&h, |m| *m == closed));
        assert_eq!(status(&h), Some(closed));
        assert!(!h.app.tab().exec.user_tx());
        assert!(!h.app.tab_busy(h.app.tab().id));
        assert!(!said(&h, |m| matches!(m, Msg::TxIdleRolledBack { .. })));
        let screen = h.screen(220, 45);
        assert!(screen.contains("got no answer within 10s, so this tab's session was closed instead"), "{screen}");
        // A late answer of the closed session says nothing (another generation).
        let t = h.app.tabs.iter().next().unwrap().id;
        let target = datarig_tui::app::EventTarget::Tab(t);
        let generation = h.app.tabs.get(t).unwrap().exec.generation - 1;
        let ev = DbEvent::TxEnded { id: sent[0].0, outcome: TxOutcome::RolledBack };
        h.app.on_app_event(datarig_tui::app::AppEvent::Db { target, generation, ev });
        assert!(!said(&h, |m| matches!(m, Msg::TxIdleRolledBack { .. })));
        run(&mut h, "SELECT 1");
        assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query, SessionRole::Query]);
        // Refused by the server: closed at once.
        let mut h = manual_on(mysql, rollback_rules());
        write_in_tx(&mut h);
        h.advance(ROLLBACK);
        let (end, _) = ends(&mut h)[0];
        let error = DbError::Server("boom".into());
        h.tab_db(0, DbEvent::Failed { id: end, error, cancelled: false });
        assert!(h.session_closed(1), "{}", dbms(mysql));
        assert!(said(&h, |m| matches!(m, Msg::QueryTxIdleRollbackFailed { time, .. } if time == "90s")));
        assert!(matches!(status(&h), Some(Msg::QueryTxIdleRollbackFailed { .. })), "{:?}", status(&h));
        assert!(!h.app.tab().exec.user_tx());
        assert!(!said(&h, |m| matches!(m, Msg::TxRollbackFailed { .. } | Msg::TxIdleRolledBack { .. })));
        // The bound went with it: nothing more is due for it.
        h.advance(Duration::from_secs(10));
        assert!(!said(&h, |m| matches!(m, Msg::QueryTxIdleRollbackUnanswered { .. })));
    }
}

/// Idle as before: nothing is rolled back while a statement runs in the transaction, and its
/// idle time starts over from the statement's end.
#[test]
fn nothing_is_rolled_back_while_a_statement_runs() {
    for mysql in [true, false] {
        let mut h = manual_on(mysql, rollback_rules());
        write_in_tx(&mut h);
        h.advance(Duration::from_secs(80));
        let id = run(&mut h, "UPDATE users SET name = 'y' WHERE id = 1");
        h.advance(Duration::from_secs(200));
        assert!(ends(&mut h).is_empty(), "{}: a statement runs", dbms(mysql));
        assert!(!h.session_closed(1));
        h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(1), elapsed: MS });
        h.advance(Duration::from_secs(89));
        assert!(ends(&mut h).is_empty(), "{}: 89s since the statement ended", dbms(mysql));
        h.advance(Duration::from_secs(1));
        assert_eq!(ends(&mut h).len(), 1, "{}", dbms(mysql));
    }
}

#[test]
fn the_idle_time_counts_from_the_last_statement_and_the_time_asleep() {
    for mysql in [true, false] {
        let mut h = manual_on(mysql, rollback_rules());
        write_in_tx(&mut h);
        h.advance(Duration::from_secs(80));
        // Another statement in the transaction: a new idle period.
        let id = run(&mut h, "UPDATE users SET name = 'y' WHERE id = 1");
        h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(1), elapsed: MS });
        h.sent();
        h.advance(Duration::from_secs(80));
        assert!(ends(&mut h).is_empty(), "80s since the last statement");
        h.advance(Duration::from_secs(10));
        assert_eq!(ends(&mut h).len(), 1);
        // Asleep (`idle_disconnect` off, else the whole connection closes for that first): the
        // first key after waking sends the rollback before it acts (a key that runs nothing, so
        // the Messages stay), right after the warning, which counts the time asleep too.
        let mut h = manual_on(mysql, Policy { idle_disconnect: Some(None), ..rollback_rules() });
        write_in_tx(&mut h);
        let later = SystemTime::now() + Duration::from_secs(3600);
        h.app.set_wall_clock(Arc::new(move || later));
        h.keys("l");
        let sent = ends(&mut h);
        assert_eq!(sent.len(), 1, "{}: sent at the first key", dbms(mysql));
        assert!(!h.session_closed(1));
        let notes: Vec<Msg> = h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect();
        assert!(notes.iter().any(|m| matches!(m, Msg::TxIdleWarning { .. })), "warned: {notes:?}");
        rolled_back(&mut h, sent[0].0);
        assert!(said(&h, |m| *m == Msg::TxIdleRolledBack { count: 1, time: "90s".into() }));
        // A run typed right after waking: the rollback goes first, and the run is refused (its
        // text stays) with why.
        let mut h = manual_on(mysql, Policy { idle_disconnect: Some(None), ..rollback_rules() });
        write_in_tx(&mut h);
        let mut editor = Editor::new("SELECT 2");
        editor.set_language(h.app.tab_language(h.app.tab().id));
        h.app.tab_mut().editor = editor;
        h.app.set_wall_clock(Arc::new(move || later));
        h.ctrl('e');
        let sent = h.sent();
        assert!(
            matches!(sent.as_slice(), [datarig_core::driver::DbCommand::EndTx { commit: false, .. }]),
            "{}: only the rollback: {sent:?}",
            dbms(mysql)
        );
        assert!(!h.session_closed(1));
        assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query]);
        let flashed = h.app.transient.as_ref().map(|(n, _)| n.msg.clone());
        assert_eq!(flashed, Some(Msg::TxIdleRollingBack { time: "90s".into() }), "says why it did not run");
    }
}

#[test]
fn no_rollback_when_off_nor_for_the_users_own_begin_nor_without_a_transaction() {
    for mysql in [true, false] {
        // Off: nothing (the 5m of `idle_disconnect` still closes it later).
        let mut h = manual_on(mysql, Policy { tx_idle_rollback: None, ..rollback_rules() });
        write_in_tx(&mut h);
        h.advance(Duration::from_secs(200));
        assert!(ends(&mut h).is_empty());
        assert!(!h.session_closed(1));
        assert!(h.app.tab().exec.user_tx());
        // The user's own `BEGIN` (automatic commit): not a transaction opened for manual
        // commit, so not this limit.
        let mut h = manual_on(mysql, Policy { commit: CommitRule::Auto, ..rollback_rules() });
        let id = run(&mut h, "BEGIN");
        h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("BEGIN".into()), elapsed: MS });
        h.tab_db(0, DbEvent::Block(true));
        h.tab_db(0, DbEvent::TxOpen(true));
        assert!(h.app.tab().exec.user_tx());
        h.sent();
        h.advance(Duration::from_secs(200));
        assert!(ends(&mut h).is_empty(), "{}", dbms(mysql));
        assert!(!h.session_closed(1));
        // No transaction open: the session stays until `idle_disconnect`.
        let mut h = manual_on(mysql, rollback_rules());
        ran(&mut h, "SELECT 1");
        h.sent();
        h.advance(Duration::from_secs(200));
        assert!(ends(&mut h).is_empty());
        assert!(!h.session_closed(1));
    }
}

#[test]
fn idle_disconnect_shorter_than_tx_idle_rollback_closes_first_as_unused() {
    for mysql in [true, false] {
        let rules = Policy { tx_idle_rollback: Some(Duration::from_secs(600)), ..rollback_rules() };
        let mut h = manual_on(mysql, rules);
        write_in_tx(&mut h);
        h.advance(Duration::from_secs(300));
        assert!(h.session_closed(1));
        assert!(ends(&mut h).is_empty());
        assert!(said(&h, |m| *m == Msg::QueryIdleClosedTx { time: "5m".into() }));
        assert!(!said(&h, |m| matches!(m, Msg::QueryTxIdleClosed { .. } | Msg::TxIdleRollingBack { .. })));
    }
}

#[test]
fn the_rollback_is_warned_about_when_tx_idle_warning_does_not() {
    // `tx_idle_warning` off, or longer than the rollback limit: the app's own warning, 45s
    // before (half the limit under two minutes), then the rollback.
    for mysql in [true, false] {
        for warning in [None, Some(Duration::from_secs(300))] {
            let rules = Policy { tx_idle_warning: warning, ..rollback_rules() };
            let mut h = manual_on(mysql, rules);
            write_in_tx(&mut h);
            h.advance(Duration::from_secs(44));
            assert!(!said(&h, |m| matches!(m, Msg::TxIdleRollbackWarning { .. })), "{warning:?}");
            h.advance(Duration::from_secs(1));
            let lead = Msg::TxIdleRollbackWarning {
                tab: "1".into(),
                time: "0:45".into(),
                commit: "Space x c".into(),
                rollback: "Space x r".into(),
            };
            assert_eq!(status(&h), Some(lead.clone()), "{} {warning:?}", dbms(mysql));
            assert!(said(&h, |m| *m == lead));
            let screen = h.screen(220, 45);
            assert!(screen.contains("is rolled back in 0:45 (the session stays open)"), "{screen}");
            h.advance(Duration::from_secs(45));
            let sent = ends(&mut h);
            assert_eq!(sent.len(), 1, "{warning:?}");
            assert!(!h.session_closed(1));
            rolled_back(&mut h, sent[0].0);
            assert!(said(&h, |m| *m == Msg::TxIdleRolledBack { count: 1, time: "90s".into() }));
            assert!(!said(&h, |m| matches!(m, Msg::TxIdleWarning { .. })), "{warning:?}");
        }
    }
}

#[test]
fn the_rollback_limit_is_the_one_its_transaction_began_with() {
    for mysql in [true, false] {
        let mut h = manual_on(mysql, rollback_rules());
        write_in_tx(&mut h);
        // The environment changes while the transaction is open: it keeps 90s.
        h.app.policies.insert("stg", Policy { tx_idle_rollback: Some(Duration::from_secs(600)), ..rollback_rules() });
        h.advance(Duration::from_secs(90));
        assert_eq!(ends(&mut h).len(), 1, "{}", dbms(mysql));
        assert_eq!(status(&h), Some(Msg::TxIdleRollingBack { time: "90s".into() }));
        // The next transaction gets the new limit: 10m, so `idle_disconnect`'s 5m closes it first.
        let mut h = manual_on(mysql, rollback_rules());
        h.app.policies.insert("stg", Policy { tx_idle_rollback: Some(Duration::from_secs(120)), ..rollback_rules() });
        write_in_tx(&mut h);
        assert_eq!(h.app.tab().exec.tx.as_ref().and_then(|x| x.rollback), Some(Duration::from_secs(120)));
        h.app.policies.insert("stg", Policy { tx_idle_rollback: None, ..rollback_rules() });
        h.advance(Duration::from_secs(120));
        assert_eq!(ends(&mut h).len(), 1, "{}: off later: still the 2m it began with", dbms(mysql));
        assert_eq!(status(&h), Some(Msg::TxIdleRollingBack { time: "2m".into() }));
    }
}

/// A question that offers to commit first stays open while the app rolls the idle transaction
/// back: answering `c` then commits nothing, does not go on, and says the transaction is being
/// rolled back; the summary's `c` sends nothing either, and the summary closes once the
/// rollback is confirmed.
#[test]
fn a_commit_asked_for_during_the_idle_rollback_is_not_pretended() {
    let mut h = manual_on(true, Policy { idle_disconnect: Some(None), ..rollback_rules() });
    write_in_tx(&mut h);
    // Another tab, so closing this one asks.
    h.ctrl('t');
    let tabs = h.app.tabs.len();
    h.app.dispatch(datarig_tui::app::action::Action::PrevTab);
    h.ctrl('w');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.sent();
    let later = SystemTime::now() + Duration::from_secs(3600);
    h.app.set_wall_clock(Arc::new(move || later));
    h.keys("c");
    let sent = ends(&mut h);
    assert_eq!(sent, [(sent[0].0, false)], "only the idle rollback is sent, nothing committed");
    assert_eq!(h.app.tabs.len(), tabs, "the answer does not go on");
    assert_eq!(status(&h), Some(Msg::TxEndIdleRollingBack { tab: "1".into() }));
    let screen = h.screen(220, 45);
    assert!(screen.contains("is being rolled back for sitting idle"), "{screen}");
    rolled_back(&mut h, sent[0].0);
    assert!(said(&h, |m| *m == Msg::TxIdleRolledBack { count: 1, time: "90s".into() }));
    assert!(!said(&h, |m| matches!(m, Msg::TxCommitted { .. })));
    // The summary: `c` there sends nothing, says why, and it closes once the rollback is done.
    let mut h = manual_on(true, Policy { idle_disconnect: Some(None), ..rollback_rules() });
    write_in_tx(&mut h);
    h.keys(" xs");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TxSummary));
    h.app.set_wall_clock(Arc::new(move || later));
    h.keys("c");
    let sent = ends(&mut h);
    assert_eq!(sent, [(sent[0].0, false)], "{sent:?}");
    let flashed = h.app.transient.as_ref().map(|(n, _)| n.msg.clone());
    assert_eq!(flashed, Some(Msg::TxIdleRollingBack { time: "90s".into() }));
    rolled_back(&mut h, sent[0].0);
    assert!(!said(&h, |m| matches!(m, Msg::TxCommitted { .. })));
    assert_ne!(h.overlay_kind(), Some(OverlayKind::TxSummary), "{:?}", h.overlay_kind());
}
