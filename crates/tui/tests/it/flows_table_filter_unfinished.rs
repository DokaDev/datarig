//! A filter typed on a MySQL or MariaDB table tab that ends inside a token (a backslash that
//! would take the parenthesis after it, a quoted variable name left open) is refused with the
//! place marked, nothing is sent, and the tab goes on working; completing such a text on the
//! command line changes nothing either.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::table_query::Field;
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

/// Answer the active tab's current run with a page of three rows.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// A MySQL-family profile with `shop.users` open in dialect `d` and answered, nothing sent since.
fn users(d: Dialect) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = ConnectionConfig::test_db();
    (p.name, p.driver) = (
        "local-my".into(),
        match d {
            Dialect::MariaDb(_) => "mariadb",
            Dialect::MySql(m) if m.mariadb => "mariadb",
            Dialect::MySql(_) | Dialect::Postgres => "mysql",
        }
        .into(),
    );
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
    h.tab_db(index, DbEvent::Language(Language::Sql(d)));
    h.tab_db(index, DbEvent::Connected);
    answer(&mut h);
    h.sent();
    h
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// MySQL's sql modes the filter is read in, and MariaDB's, also as a MariaDB session says them.
fn dialects() -> Vec<Dialect> {
    let d = MySqlMode::default();
    let maria = MySqlMode { mariadb: true, ..d };
    vec![
        Dialect::MySql(d),
        Dialect::MySql(maria),
        Dialect::MariaDb(maria),
        Dialect::MySql(MySqlMode { ansi_quotes: true, ..d }),
        Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..d }),
    ]
}

#[test]
fn a_filter_ending_in_a_backslash_is_refused_and_the_tab_goes_on() {
    for d in dialects() {
        let mut h = users(d);
        // Completion of the text on the command line first, then the filter itself.
        h.ctrl('k');
        h.type_text("where id = 1 \\");
        h.key(KeyCode::Tab);
        for _ in 0..2 {
            if h.cmdline().is_some() {
                h.key(KeyCode::Esc);
            }
        }
        assert!(h.cmdline().is_none(), "{d:?}");
        assert!(runs(&mut h).is_empty(), "{d:?}");
        h.command("where id = 1 \\");
        assert!(runs(&mut h).is_empty(), "{d:?}");
        assert!(status(&h).starts_with("Filter refused: "), "{d:?}: {}", status(&h));
        let refusal = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("refusal");
        assert_eq!((refusal.field, refusal.at), (Field::Where, Some(7..8)), "{d:?}");
        // A quoted variable name left open.
        h.command("where id = @'x");
        assert!(runs(&mut h).is_empty(), "{d:?}");
        assert!(status(&h).starts_with("Filter refused: "), "{d:?}: {}", status(&h));
        // The tab still runs a filter.
        h.command("where id > 0");
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]], "{d:?}");
    }
}
