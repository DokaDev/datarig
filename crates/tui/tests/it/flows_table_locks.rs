//! A MySQL table tab whose reads lock rows (InnoDB reads as `FOR SHARE` at SERIALIZABLE with
//! autocommit off or a transaction open): its run asks first (`Enter` cancels and nothing is
//! sent, `y` runs, once per session state), also when the level is not known; with autocommit
//! on and no transaction, or a level below SERIALIZABLE, it does not ask. A page read again and
//! a count are refused in that state unless the tab said yes to it (never asked mid-paging).

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, Isolation};
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
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
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
    h.keys("#");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert!(h.sent().is_empty());
    assert!(status(&h).starts_with("Rows are not counted for you: reads in this session"), "{}", status(&h));
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
