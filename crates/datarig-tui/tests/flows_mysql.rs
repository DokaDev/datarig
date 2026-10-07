//! A MySQL profile in the app: what its sessions say about how their text is read reaches the
//! tab's editor and risk classifier.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::DbEvent;
use datarig_core::i18n::Lang;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use std::sync::atomic::Ordering;

/// Connecting to a MySQL profile (`local-my`, another one `other-my` too) with the first tab on
/// it (feed `Connected`).
fn mysql() -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let profile = |name: &str| {
        let mut p = datarig_core::profile::ConnectionConfig::test_db();
        p.name = name.into();
        p.driver = "mysql".into();
        p
    };
    let cfg = Config { connections: vec![profile("local-my"), profile("other-my")], ..Config::default() };
    Harness::with_driver(&cfg, Lang::En, driver)
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

#[test]
fn a_tab_on_another_profile_does_not_keep_the_old_sessions_mode() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    let id = |h: &Harness, name: &str| h.app.profiles.iter().find(|p| p.name == name).unwrap().id;
    h.ctrl('e');
    let ansi = MySqlMode { ansi_quotes: true, ..MySqlMode::default() };
    h.tab_db(0, DbEvent::Language(my(ansi)));
    assert_eq!(h.app.tab_language(tab), my(ansi));
    // The other profile connects (its own console opens), then the first tab moves to it: the
    // tab reads its text as that profile's sessions do (none said yet: the driver's default),
    // not in the mode of the session it had.
    h.command("conn other-my");
    h.db(DbEvent::Connected);
    h.command("conn local-my");
    assert_eq!(h.app.tab().id, tab);
    h.keys(" cs");
    if h.overlay_kind() == Some(datarig_tui::app::overlay::OverlayKind::Confirm) {
        h.keys("y");
    }
    h.type_text("other-my");
    h.key(ratatui::crossterm::event::KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "other-my")));
    assert_eq!(h.app.tab_language(tab), my(MySqlMode::default()));
    assert_eq!(h.app.tab().editor.language(), my(MySqlMode::default()));
    // A new tab of the first profile reads its text in the mode that profile's session said,
    // before a session of its own says anything.
    h.command("conn local-my");
    h.ctrl('t');
    let fresh = h.app.tab().id;
    assert_ne!(fresh, tab);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-my")));
    assert_eq!(h.app.tab_language(fresh), my(ansi));
    assert_eq!(h.app.tab().editor.language(), my(ansi));
}
