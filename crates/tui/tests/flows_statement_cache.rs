//! The server-side statement cache (PgBouncer handling): the profile's option
//! reaches every session the app opens, and a session that turned its cache off by itself says
//! so in its run's Messages, and in the status bar once per connection of the profile.

mod common;

use common::*;
use datarig_core::driver::{DbEvent, SessionRole};
use datarig_core::i18n::{I18n, Lang, Msg};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::{AppEvent, EventTarget};

/// The sessions the fake driver opened so far: (role, statement cache).
fn opened(h: &Harness) -> Vec<(SessionRole, bool)> {
    h.driver.sessions.lock().unwrap().iter().map(|s| (s.role, s.opts.statement_cache)).collect()
}

#[test]
fn the_profiles_statement_cache_reaches_every_session() {
    for cache in [true, false] {
        let cfg = datarig_core::config::Config {
            connections: vec![ConnectionConfig { statement_cache: cache, ..ConnectionConfig::test_db() }],
            ..datarig_core::config::Config::default()
        };
        let mut h = Harness::with_config(&cfg, Lang::En);
        h.db(DbEvent::Connected);
        h.ctrl('e');
        let sessions = opened(&h);
        assert!(sessions.iter().any(|(r, _)| *r == SessionRole::Meta), "{sessions:?}");
        assert!(sessions.iter().any(|(r, _)| *r == SessionRole::Query), "{sessions:?}");
        assert!(sessions.iter().all(|(_, c)| *c == cache), "cache {cache}: {sessions:?}");
    }
}

#[test]
fn a_session_that_turned_its_cache_off_says_so_once_per_connection() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let first = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    let off =
        |tab, generation| AppEvent::Db { target: EventTarget::Tab(tab), generation, ev: DbEvent::StatementCacheOff };
    h.app.on_app_event(off(first, generation));
    let want = Msg::DbStatementCacheOff { name: "local-pg".into() };
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == want), "{:?}", h.app.tab().exec.run.notes);
    assert_eq!(h.app.status.as_ref().map(|n| n.msg.clone()), Some(want.clone()));
    let text = I18n::new(Lang::En).msg(&want).to_string();
    assert!(text.contains("PgBouncer") && text.contains("Advanced"), "{text}");

    // Another tab of the profile: its Messages say it too, the status bar does not again.
    h.app.status = None;
    h.ctrl('t');
    h.ctrl('e');
    let second = h.app.tab().id;
    assert_ne!(first, second);
    let generation = h.app.tab().exec.generation;
    h.app.on_app_event(off(second, generation));
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == want));
    assert_eq!(h.app.status.as_ref().map(|n| n.msg.clone()), None, "said once per connection");
}
