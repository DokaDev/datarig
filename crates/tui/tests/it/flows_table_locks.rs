//! A MySQL table tab whose reads lock rows (InnoDB reads as `FOR SHARE` at SERIALIZABLE with
//! autocommit off or a transaction open): its run asks first (`Enter` cancels and nothing is
//! sent, `y` runs, once per session state), also when the level is not known; with autocommit
//! on and no transaction, or a level below SERIALIZABLE, it does not ask. With manual commit the
//! transaction the app opens for the read counts as one open. A yes holds until anything that
//! decides it changes (even back to what it was), but for the transaction its own run opened. A
//! page read again and a count are refused in that state unless the tab said yes to it (never
//! asked mid-paging); an export is not held back by it.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Isolation};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

const ALL: &str = "SELECT * FROM `shop`.`users`";

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with a first page (`more`: rows past it, its portal
/// closed).
fn answer_with(h: &mut Harness, more: bool) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    if more {
        h.tab_db(index, DbEvent::Released { id });
    }
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let n = if more { h.app.page_size } else { 3 };
    let rows = (0..n).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more, elapsed: Duration::from_millis(2) });
}

fn answer(h: &mut Harness) {
    answer_with(h, false);
}

/// A MySQL profile with `shop.users` open and answered (its session with autocommit on, the
/// level not reported yet), nothing sent since.
fn users() -> Harness {
    users_in(false)
}

/// [`users`], committing by hand with `manual`.
fn users_in(manual: bool) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let mut cfg = Config { connections: vec![p], ..Default::default() };
    if manual {
        cfg.connections[0].policy = Some("stg".into());
        cfg.policies.insert("stg", Policy { commit: CommitRule::Manual, ..Policy::default() });
    }
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
    h.tab_db(index, DbEvent::Connected);
    answer(&mut h);
    h.sent();
    h
}

/// The active tab's session says `ev`.
fn session(h: &mut Harness, ev: DbEvent) {
    let index = h.app.tabs.active_index();
    h.tab_db(index, ev);
}

fn asks(h: &Harness) -> bool {
    h.app.overlays.confirm().map(|c| c.action) == Some(ConfirmAction::TableReadLocks(h.app.tab().id))
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// A MySQL profile, connected and known to be MySQL, with `shop.users` opened: the tab's session
/// is still opening.
fn users_opening() -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
    h
}

#[test]
fn the_first_read_waits_for_the_state_a_new_session_says() {
    // Its start-up statements turned autocommit off at SERIALIZABLE: asked, nothing sent yet.
    let mut h = users_opening();
    assert!(runs(&mut h).is_empty(), "the session has not said its state");
    assert!(h.app.tab_busy(h.app.tab().id), "the read waits as work of the tab");
    session(&mut h, DbEvent::Autocommit(false));
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    assert!(runs(&mut h).is_empty(), "not until the session is connected");
    session(&mut h, DbEvent::Connected);
    assert!(asks(&h));
    assert!(runs(&mut h).is_empty());
    h.key(KeyCode::Char('y'));
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);

    // Nothing that locks: it goes as soon as the session is connected, without asking.
    let mut h = users_opening();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    assert!(runs(&mut h).is_empty());
    session(&mut h, DbEvent::Connected);
    assert!(!asks(&h));
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
}

#[test]
fn a_read_waiting_for_its_session_ends_with_a_cancel_or_a_failure() {
    let mut h = users_opening();
    let id = h.app.tab().id;
    h.app.cancel();
    assert!(status(&h).contains("cancelled"), "{}", status(&h));
    assert!(!h.app.tab_busy(id));
    session(&mut h, DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "cancelled, it never runs");

    let mut h = users_opening();
    let id = h.app.tab().id;
    session(&mut h, DbEvent::ConnectFailed { error: DbError::Closed, auth: false });
    assert!(!h.app.tab_busy(id), "the session it waited for is gone");
    assert!(runs(&mut h).is_empty());

    // While the profile connects (only its own failure is said): the read is dropped all the
    // same, and the next run opens a session of its own instead of waiting on the failed one.
    let mut h = users_opening();
    let id = h.app.tab().id;
    let pid = h.app.tab().profile.expect("a profile");
    h.app.conns.entry(pid).connecting = Some(datarig_tui::app::Connecting {
        name: "local-my".into(),
        started: std::time::Instant::now(),
        had_password: false,
        keychain: false,
        tunnel: None,
    });
    session(&mut h, DbEvent::ConnectFailed { error: DbError::Closed, auth: false });
    assert!(!h.app.tab_busy(id), "not waiting on the failed session");
    assert!(status(&h).starts_with("Not run"), "{}", status(&h));
    h.app.conns.entry(pid).connecting = None;
    let before = h.app.tab().exec.generation;
    h.command("where id > 0");
    assert_ne!(h.app.tab().exec.generation, before, "on a session of its own");
    assert!(runs(&mut h).is_empty());
    session(&mut h, DbEvent::Connected);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]]);

    // The same with the read cancelled before the session failed.
    let mut h = users_opening();
    let pid = h.app.tab().profile.expect("a profile");
    h.app.cancel();
    h.app.conns.entry(pid).connecting = Some(datarig_tui::app::Connecting {
        name: "local-my".into(),
        started: std::time::Instant::now(),
        had_password: false,
        keychain: false,
        tunnel: None,
    });
    session(&mut h, DbEvent::ConnectFailed { error: DbError::Closed, auth: false });
    h.app.conns.entry(pid).connecting = None;
    let before = h.app.tab().exec.generation;
    h.command("where id > 0");
    assert_ne!(h.app.tab().exec.generation, before, "on a session of its own");
    session(&mut h, DbEvent::Connected);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]]);
}

#[test]
fn autocommit_on_without_a_transaction_does_not_ask() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.command("where id > 0");
    assert!(!asks(&h));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]]);
    // Not known either: autocommit on reads without locks whatever the level.
    let mut h = users();
    h.command("where id > 0");
    assert!(!asks(&h));
    assert_eq!(runs(&mut h).len(), 1);
}

#[test]
fn a_level_below_serializable_does_not_ask() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::RepeatableRead)));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    assert!(!asks(&h));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]]);
}

#[test]
fn serializable_without_autocommit_asks_enter_cancels_y_runs() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    assert!(asks(&h), "{}", h.screen(120, 30));
    assert!(runs(&mut h).is_empty(), "nothing sent while it asks");
    let screen = h.screen(120, 30);
    assert!(screen.contains("Run a read that locks rows?"), "{screen}");
    assert_screen!("table_locks_serializable_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_locks_serializable_en_120x30", Lang::En, h.draw(120, 30));
    // Enter cancels: nothing is sent.
    h.key(KeyCode::Enter);
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert!(runs(&mut h).is_empty());
    assert!(status(&h).contains("Not run: the read may lock rows"), "{}", status(&h));
    // Asked again; `y` runs.
    h.command("where id > 0");
    assert!(asks(&h));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]]);
    answer(&mut h);
    // Not asked again in the same session state: another filter, the plain table (run key).
    h.command("where id > 1");
    assert!(!asks(&h));
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.ctrl('e');
    assert!(!asks(&h));
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    // The state changed (the level is not known any more): asked again.
    session(&mut h, DbEvent::Isolation(None));
    h.command("where id > 2");
    assert!(asks(&h));
    assert!(runs(&mut h).is_empty());
}

#[test]
fn a_level_not_known_asks_too() {
    let mut h = users();
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    assert!(asks(&h));
    assert!(runs(&mut h).is_empty());
    let screen = h.screen(120, 30);
    assert!(screen.contains("Run a read that may lock rows?"), "{screen}");
    assert_screen!("table_locks_unknown_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_locks_unknown_en_120x30", Lang::En, h.draw(120, 30));
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
}

#[test]
fn an_open_transaction_asks_at_serializable() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::TxOpen(true));
    session(&mut h, DbEvent::Block(true));
    h.command("where id > 0");
    assert!(asks(&h));
    let text = h.app.overlays.confirm().map(|c| h.app.i18n.msg(&c.text).to_string()).unwrap_or_default();
    assert!(text.contains("(SERIALIZABLE, transaction under way)"), "{text}");
    h.key(KeyCode::Esc);
    assert!(runs(&mut h).is_empty());
}

#[test]
fn paging_and_counting_are_refused_unless_confirmed() {
    let mut h = users();
    h.command("where id > 0");
    runs(&mut h);
    answer_with(&mut h, true);
    h.app.focus = datarig_tui::app::Focus::Results;
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.sent();
    // Not confirmed in this state: refused, never asked mid-paging.
    h.keys("n");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert!(h.sent().is_empty());
    assert!(status(&h).contains("may lock rows, not confirmed in this tab"), "{}", status(&h));
    // Not offered: dimmed, with the reason a count is refused with.
    h.keys("#");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    let said = h.app.menu_dimmed_notice("Exact count").expect("dimmed").render(&h.app.i18n).to_string();
    assert!(said.starts_with("Rows are not counted for you: reads in this session"), "{said}");
    h.key(KeyCode::Esc);
    assert!(h.sent().is_empty());
    // Confirmed by a run: the next page and the count go.
    h.command("where id > 0");
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    answer_with(&mut h, true);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("n");
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Resume { .. })), "{sent:?}");
}

/// A yes is for the state it was given in: a change of the level, of autocommit or of the
/// transaction ends it, even when the state comes back to what it was.
#[test]
fn a_yes_does_not_come_back_with_the_state_it_was_given_in() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    // To READ COMMITTED and back.
    session(&mut h, DbEvent::Isolation(Some(Isolation::ReadCommitted)));
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.command("where id > 1");
    assert!(asks(&h), "the earlier yes is not this state's");
    assert!(runs(&mut h).is_empty());
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    // Autocommit on and off again.
    session(&mut h, DbEvent::Autocommit(true));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 2");
    assert!(asks(&h));
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    // A transaction that started and ended meanwhile.
    session(&mut h, DbEvent::TxOpen(true));
    session(&mut h, DbEvent::TxOpen(false));
    h.command("where id > 3");
    assert!(asks(&h));
    assert!(runs(&mut h).is_empty());
}

/// The transaction the confirmed run opened itself (autocommit off) is the one the yes was
/// for: the next page goes without asking again.
#[test]
fn a_yes_holds_for_the_transaction_its_run_opened() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    session(&mut h, DbEvent::TxOpen(true));
    answer_with(&mut h, true);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("n");
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Resume { .. })), "{sent:?} {}", status(&h));
    // A transaction opened by something else is not.
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    h.keys("y");
    runs(&mut h);
    answer_with(&mut h, true);
    session(&mut h, DbEvent::TxOpen(true));
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("n");
    assert!(h.sent().is_empty());
    assert!(status(&h).contains("may lock rows, not confirmed in this tab"), "{}", status(&h));
}

/// With manual commit, a read the app opens a transaction for (its filter calls a function)
/// reads in that transaction: asked at SERIALIZABLE even with autocommit on and none open. A
/// plain read, which runs outside one, is not.
#[test]
fn manual_commit_asks_when_its_transaction_reads() {
    let mut h = users_in(true);
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.command("where id > 0");
    assert!(!asks(&h));
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    h.command("where my_fn(id) > 0");
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::FilterCalls(h.app.tab().id)));
    h.keys("y");
    assert!(asks(&h), "{}", h.screen(120, 30));
    assert!(runs(&mut h).is_empty(), "nothing sent while it asks");
    let text = h.app.overlays.confirm().map(|c| h.app.i18n.msg(&c.text).to_string()).unwrap_or_default();
    assert!(text.contains("(SERIALIZABLE, manual commit starts a transaction for it)"), "{text}");
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (my_fn(id) > 0)")]]);
    // Without manual commit the same read is not asked about.
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.command("where my_fn(id) > 0");
    h.keys("y");
    assert!(!asks(&h));
    assert_eq!(runs(&mut h).len(), 1);
}

/// An export of every row is not held back by the ask: its own transaction is the export's.
#[test]
fn an_export_is_not_held_back_by_the_ask() {
    let mut h = users();
    h.command("where id > 0");
    runs(&mut h);
    answer_with(&mut h, true);
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.sent();
    let dir = std::env::temp_dir().join(format!("datarig-table-locks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    h.command(&format!("export {}", dir.join("all.csv").display()));
    if h.app.overlays.confirm().is_some_and(|c| c.action != ConfirmAction::TableReadLocks(h.app.tab().id)) {
        h.key(KeyCode::Char('y'));
    }
    assert!(!asks(&h));
    let start = std::time::Instant::now();
    let mut sent = Vec::new();
    while h.app.exporting(h.app.tab().id) && start.elapsed() < Duration::from_secs(10) {
        h.app.pump_exports(Duration::from_millis(50));
        sent.extend(h.sent());
        if sent.iter().any(|c| matches!(c, DbCommand::Export { .. })) {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Export { .. })), "{sent:?} {}", status(&h));
}

/// A yes kept for the transaction its run opened is not carried by a count of the same query:
/// a count whose transaction the server ends (a deadlock victim) ends the yes, and the next
/// read asks again.
#[test]
fn a_count_that_ends_the_transaction_does_not_keep_the_yes() {
    let mut h = users();
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.command("where id > 0");
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    session(&mut h, DbEvent::TxOpen(true));
    answer_with(&mut h, true);
    h.app.focus = datarig_tui::app::Focus::Results;
    exact_count(&mut h);
    let count = h.sent().into_iter().find_map(|c| match c {
        DbCommand::Count { id, .. } => Some(id),
        _ => None,
    });
    let id = count.expect("the count goes: the yes holds for the run's own transaction");
    // The server rolls the transaction back (a deadlock victim) while it counts.
    session(&mut h, DbEvent::TxClosedBy { id, how: datarig_core::driver::ServerEnd::RolledBack });
    session(&mut h, DbEvent::TxOpen(false));
    session(&mut h, DbEvent::Counted { id, result: Err(datarig_core::driver::DbError::NoResult), snapshot: false });
    h.command("where id > 1");
    assert!(asks(&h), "the yes ended with the transaction the count saw end");
    assert!(runs(&mut h).is_empty());
}

/// A read on a tab whose session is gone opens a new one, and is asked about on it right
/// before it is sent (once): with manual commit its transaction reads at a level not known
/// there. No to it sends nothing; asked again, a yes sends it.
#[test]
fn a_read_that_opens_its_session_is_asked_about_on_it() {
    let mut h = users_in(true);
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    let before = h.app.tab().exec.generation;
    session(&mut h, DbEvent::Lost { error: datarig_core::driver::DbError::Closed });
    h.command("where my_fn(id) > 0");
    h.keys("y");
    assert!(!asks(&h), "not before the session says its state");
    assert!(runs(&mut h).is_empty());
    session(&mut h, DbEvent::Connected);
    assert!(asks(&h), "{}", h.screen(120, 30));
    assert_ne!(h.app.tab().exec.generation, before, "asked on the session it goes to");
    assert!(runs(&mut h).is_empty(), "nothing sent while it asks");
    h.key(KeyCode::Enter);
    assert!(!asks(&h));
    assert!(runs(&mut h).is_empty());
    assert!(status(&h).starts_with("Not run: the read may lock rows"), "{}", status(&h));
    h.command("where my_fn(id) > 0");
    assert!(asks(&h));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (my_fn(id) > 0)")]]);
}

/// A read that waited for its profile to connect is checked again on the new session right
/// before it is sent: with manual commit its transaction reads at a level not known there, so
/// it asks (a yes given for the session before is not this one's), and sends nothing first.
#[test]
fn a_read_that_waited_for_the_connection_asks_on_the_new_session() {
    let mut h = users_in(true);
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.command("where my_fn(id) > 0");
    h.keys("y");
    assert!(asks(&h));
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    // The profile disconnects; the read waits for it to connect again.
    let pid = h.app.tab().profile.expect("a profile");
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    if h.app.overlays.is_open(OverlayKind::Confirm) {
        h.keys("y");
    }
    assert!(!h.app.conns.is_connected(pid));
    h.command("where my_fn(id) > 0");
    if asks(&h) {
        h.keys("y");
    }
    assert!(h.app.is_queued(h.app.tab().id), "it waits for the connection");
    assert!(runs(&mut h).is_empty());
    h.db(DbEvent::Connected);
    assert!(!asks(&h), "not before the session says its state");
    assert!(runs(&mut h).is_empty());
    session(&mut h, DbEvent::Connected);
    assert!(asks(&h), "asked on the session it goes to");
    assert!(runs(&mut h).is_empty(), "nothing sent before the yes");
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (my_fn(id) > 0)")]]);
}

/// `#` on filtered rows, then the exact count of the choice it opens.
fn exact_count(h: &mut Harness) {
    h.keys("#");
    assert!(h.app.overlays.is_open(datarig_tui::app::overlay::OverlayKind::ContextMenu), "the choice opens");
    h.keys("exact");
    h.key(KeyCode::Enter);
}

/// A read applied in the filter dialog that waits for a new session to say its state is
/// cancelled with the dialog: the session that connects runs nothing.
#[test]
fn a_dialog_cancelled_while_its_read_waits_for_the_session_runs_nothing() {
    let mut h = users();
    session(&mut h, DbEvent::Lost { error: DbError::Closed });
    let tab = h.app.tab().id;
    h.key(KeyCode::Tab);
    h.keys("f");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "nothing sent yet");
    assert!(h.app.tab_busy(tab), "the read waits for its session");
    h.key(KeyCode::Esc);
    assert!(!h.app.overlays.is_open(OverlayKind::FilterDialog));
    assert!(!h.app.tab_busy(tab), "cancelled with the dialog");
    session(&mut h, DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "the read cancelled never goes");
}

/// A part of the query bar clicked (once the bar has been on screen long enough to take it).
fn bar_click(h: &mut Harness, hit: datarig_tui::app::qbar::BarHit) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    h.draw(160, 34);
    let r = h.app.layout.qbar.iter().find(|(_, b)| *b == hit).map(|(r, _)| *r).expect("drawn");
    h.advance(Duration::from_millis(500));
    h.draw(160, 34);
    h.mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), r.x, r.y);
}

/// A chip's `×` and a `LIMIT` typed in the query bar ask about reads that lock rows as any run
/// does (`Enter` sends nothing); a yes given in a state that changed before it is asked again.
#[test]
fn bar_changes_ask_about_locking_reads_and_ask_again_when_the_state_moved() {
    use datarig_tui::app::qbar::BarHit;
    let mut h = users();
    h.command("where id > 0 AND id < 9");
    runs(&mut h);
    answer(&mut h);
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    session(&mut h, DbEvent::Autocommit(false));
    h.app.focus = datarig_tui::app::Focus::Results;
    h.app.detail.visible = false;
    bar_click(&mut h, BarHit::Remove(0));
    assert!(asks(&h));
    assert!(runs(&mut h).is_empty(), "nothing sent while it asks");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    // The level becomes unknown: asked too.
    session(&mut h, DbEvent::Isolation(None));
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys(" rl5");
    h.key(KeyCode::Enter);
    assert!(asks(&h));
    assert!(runs(&mut h).is_empty());
    // The state moves again before the yes: asked again, nothing sent; the next yes sends.
    session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.keys("y");
    assert!(asks(&h), "asked again in the new state");
    assert!(runs(&mut h).is_empty());
    h.keys("y");
    let sent: Vec<Option<u64>> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { max_rows, .. } => Some(max_rows),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![Some(5)]);
}
