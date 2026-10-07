//! A MySQL profile in the app: what its sessions say about how their text is read reaches the
//! tab's editor and risk classifier.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::DbEvent;
use datarig_core::i18n::Lang;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use std::sync::atomic::Ordering;

/// Connecting to a MySQL profile (`local-my`) with its first tab on it (feed `Connected`).
fn mysql() -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    p.name = "local-my".into();
    p.driver = "mysql".into();
    Harness::with_driver(&Config { connections: vec![p], ..Config::default() }, Lang::En, driver)
}

fn my(mode: MySqlMode) -> Language {
    Language::Sql(Dialect::MySql(mode))
}

#[test]
fn a_sessions_sql_mode_reaches_the_editor_and_the_classifier() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    assert_eq!(h.app.tab_language(tab), my(MySqlMode::default()), "the driver's until the session says");
    h.ctrl('e'); // a query session opens for the tab
    assert!(h.roles().contains(&datarig_core::driver::SessionRole::Query), "{:?}", h.roles());
    let ansi = MySqlMode { ansi_quotes: true, dollar_quotes: true, ..MySqlMode::default() };
    h.tab_db(0, DbEvent::Language(my(ansi)));
    assert_eq!(h.app.tab_language(tab), my(ansi));
    assert_eq!(h.app.tab().editor.language(), my(ansi));
    assert_eq!(h.app.tab().exec.prepared.language(), my(ansi));
}
