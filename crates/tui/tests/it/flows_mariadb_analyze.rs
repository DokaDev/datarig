//! MariaDB's `ANALYZE <statement>` runs the statement: a run asks as that statement asks, and a
//! read-only profile refuses it; `ANALYZE TABLE` runs as before.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, Outcome};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::risk::{Danger, Why};
use datarig_i18n::{Lang, Msg};
use datarig_tui::app::Focus;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Connected to a profile of `driver`, read-only when `read_only`; its session said MariaDB
/// when `maria` (else nothing yet).
fn connected(driver: &str, maria: bool, read_only: bool) -> Harness {
    let p = ConnectionConfig {
        name: "zz-target".into(),
        driver: driver.into(),
        policy: Some(if read_only { "prod" } else { "plain" }.into()),
        ..ConnectionConfig::test_db()
    };
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("plain", Policy::default());
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let fake = FakeDriver::default();
    fake.mysql.store(true, Ordering::SeqCst);
    let mut h = Harness::with_driver(&cfg, Lang::En, fake);
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    if maria {
        run(&mut h, "SELECT 1");
        let id = h.app.tab().exec.query_id;
        let mode = MySqlMode { mariadb: true, ..MySqlMode::default() };
        h.tab_db(0, DbEvent::Language(Language::Sql(Dialect::MySql(mode))));
        h.tab_db(
            0,
            DbEvent::Done { id, outcome: Outcome::Command("SELECT 1".into()), elapsed: Duration::from_millis(1) },
        );
        assert!(h.app.tab().exec.running.is_none());
        h.sent();
    }
    h
}

/// Ctrl+E on `sql` (the statement under the cursor).
fn run(h: &mut Harness, sql: &str) {
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
    h.sent();
    h.ctrl('e');
}

fn executes(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

fn asked(h: &Harness) -> Vec<Option<Why>> {
    h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| d.why).collect()).unwrap_or_default()
}

const READINGS: [(&str, bool); 3] = [("mariadb", true), ("mariadb", false), ("mysql", false)];

/// The default policy asks before it as before the statement it runs, and sends it once
/// confirmed; a read-only profile refuses it and sends nothing.
#[test]
fn analyze_of_a_statement_asks_as_that_statement() {
    for (driver, maria) in READINGS {
        for (sql, why) in [
            ("ANALYZE DELETE FROM zz_t", Danger::DeleteAll),
            ("ANALYZE FORMAT=JSON DELETE FROM zz_t", Danger::DeleteAll),
            ("ANALYZE UPDATE zz_t SET v = 0", Danger::UpdateAll),
            ("ANALYZE FORMAT=JSON UPDATE zz_t SET v = 0", Danger::UpdateAll),
        ] {
            let case = format!("{driver} said MariaDB {maria}: {sql}");
            let mut h = connected(driver, maria, false);
            run(&mut h, sql);
            assert_eq!(asked(&h), [Some(Why::Danger(why))], "{case}");
            assert!(executes(&mut h).is_empty(), "{case}: nothing before the yes");
            h.key(KeyCode::Char('y'));
            assert_eq!(executes(&mut h), [[sql]], "{case}");
        }
        for sql in [
            "ANALYZE DELETE FROM zz_t",
            "ANALYZE FORMAT=JSON UPDATE zz_t SET v = 0",
            "ANALYZE INSERT INTO zz_t VALUES (1)",
            "ANALYZE FORMAT=JSON REPLACE INTO zz_t VALUES (1)",
        ] {
            let case = format!("read-only, {driver} said MariaDB {maria}: {sql}");
            let mut h = connected(driver, maria, true);
            run(&mut h, sql);
            let tab = h.app.tab().status.as_ref().map(|n| n.msg.clone());
            let status = tab.or_else(|| h.app.status.as_ref().map(|n| n.msg.clone()));
            assert!(matches!(status, Some(Msg::SafetyReadOnlyBlocked { .. })), "{case}: {status:?}");
            assert!(executes(&mut h).is_empty(), "{case}: refused, nothing sent");
        }
        // The plan of a read runs on a read-only profile.
        for sql in ["ANALYZE FORMAT=JSON (SELECT * FROM zz_t)", "ANALYZE FORMAT=TRADITIONAL SELECT 1"] {
            let mut h = connected(driver, maria, true);
            run(&mut h, sql);
            assert!(asked(&h).is_empty(), "read-only, {driver} {maria}: {sql}");
            assert_eq!(executes(&mut h), [[sql]], "read-only, {driver} {maria}: {sql}");
        }
        // `ANALYZE TABLE` runs as before.
        let mut h = connected(driver, maria, false);
        run(&mut h, "ANALYZE TABLE zz_t");
        assert!(asked(&h).is_empty(), "{driver} {maria}");
        assert_eq!(executes(&mut h), [["ANALYZE TABLE zz_t"]], "{driver} {maria}");
    }
}
