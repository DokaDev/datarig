//! Safety checks before a run: a read-only policy refuses what may write,
//! before anything is sent or queued, and opens every session of its profile read-only.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, SessionRole};
use datarig_core::i18n::{Lang, Msg};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, PasswordSource};
use datarig_tui::app::{AppEvent, Focus, Startup};
use datarig_tui::widgets::editor::Editor;
use std::sync::Arc;
use std::time::Duration;

/// The test profile with policy `policy`, where `prod` is read-only.
fn config(policy: Option<&str>) -> Config {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: policy.map(str::to_string), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    cfg
}

/// Connected to a profile with policy `policy`.
fn connected(policy: Option<&str>, lang: Lang) -> Harness {
    let mut h = Harness::with_config(&config(policy), lang);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into(), "shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h
}

/// Run `sql` (the whole editor) with Ctrl+E; what was sent.
fn run(h: &mut Harness, sql: &str) -> Vec<String> {
    h.app.tab_mut().editor = Editor::new(sql);
    h.sent();
    h.ctrl('e');
    h.sent()
        .into_iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements.join(";")) } else { None })
        .collect()
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

#[test]
fn a_read_only_policy_refuses_what_may_write_and_sends_nothing() {
    let mut h = connected(Some("prod"), Lang::En);
    for (sql, what) in [
        ("INSERT INTO shop.users (name) VALUES ('x')", "this statement writes"),
        ("SELECT * FROM shop.users FOR UPDATE", "this statement writes"),
        ("WITH d AS (DELETE FROM shop.users RETURNING *) SELECT * FROM d", "this statement writes"),
        ("EXPLAIN ANALYZE DELETE FROM shop.users WHERE id = 1", "this statement writes"),
        ("CREATE TABLE zz_x (id int)", "this statement changes the schema"),
        ("VACUUM shop.users", "this is a maintenance statement"),
        ("DO $$ BEGIN END $$", "this statement runs code that cannot be checked"),
        ("EXECUTE p", "this statement is not known to be a read"),
        ("SET default_transaction_read_only = off", "this statement asks for a read-write transaction"),
        ("BEGIN READ WRITE", "this statement asks for a read-write transaction"),
        ("SET session_replication_role = replica", "this setting is not on the read-only allowlist"),
    ] {
        assert!(run(&mut h, sql).is_empty(), "nothing is sent: {sql}");
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: what.into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected), "{sql}");
        assert!(h.app.tab().exec.running.is_none(), "{sql}");
    }
    let screen = h.screen(160, 45);
    assert!(
        screen.contains("Not run: policy \"prod\" is read-only and this setting is not on the read-only"),
        "{screen}"
    );
    for sql in [
        "SELECT * FROM shop.users",
        "EXPLAIN DELETE FROM shop.users",
        "SHOW search_path",
        "SET search_path TO shop",
        "BEGIN",
        "COMMIT",
    ] {
        assert_eq!(run(&mut h, sql), [sql], "{sql}");
        h.tab_db(
            0,
            DbEvent::Done {
                id: h.app.tab().exec.query_id,
                outcome: datarig_core::driver::Outcome::Command("X".into()),
                elapsed: std::time::Duration::ZERO,
            },
        );
    }
}

/// One statement a read-only policy refuses stops the whole run: nothing of it is sent.
#[test]
fn one_refused_statement_stops_the_whole_run() {
    let mut h = connected(Some("prod"), Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT 1;\nDELETE FROM shop.users WHERE id = 1;\nSELECT 2;");
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
    assert!(h.sent().is_empty());
    assert!(
        matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { sql, .. }) if sql == "DELETE FROM shop.users WHERE id = 1")
    );
}

/// Every session of a read-only profile is opened read-only on the server: its metadata
/// session and each tab's query session. Other profiles' sessions are not.
#[test]
fn every_session_of_a_read_only_profile_is_read_only() {
    let opened = |h: &Harness| -> Vec<(SessionRole, bool)> {
        h.driver.sessions.lock().unwrap().iter().map(|s| (s.role, s.opts.read_only)).collect()
    };
    let mut h = connected(Some("prod"), Lang::En);
    run(&mut h, "SELECT 1");
    assert_eq!(opened(&h), [(SessionRole::Meta, true), (SessionRole::Query, true)]);
    let mut h = connected(None, Lang::En);
    run(&mut h, "INSERT INTO t VALUES (1)");
    assert_eq!(opened(&h), [(SessionRole::Meta, false), (SessionRole::Query, false)]);
}

/// A statement for a profile that is not connected yet is checked before it is queued (its
/// password comes from a command, held back here like a slow connect).
#[tokio::test(flavor = "multi_thread")]
async fn a_statement_waiting_for_its_connection_is_checked_before_it_is_queued() {
    let mut cfg = config(Some("prod"));
    cfg.connections[0].password = String::new();
    cfg.connections[0].set_source(PasswordSource::Command("printf datarig".into()));
    let (h, mut rx) = Harness::started(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    let mut h = h.with_fake_driver();
    h.explore("local-pg");
    h.keys("o");
    let resolved = loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("an event").expect("open");
        if matches!(ev, AppEvent::Resolved { .. }) {
            break ev;
        }
    };
    h.app.focus = Focus::Editor;
    let tab = h.app.tab().id;
    assert!(run(&mut h, "DELETE FROM shop.users WHERE id = 1").is_empty());
    assert!(!h.app.is_queued(tab), "refused, not queued");
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })));
    run(&mut h, "SELECT 1");
    assert!(h.app.is_queued(tab), "a read waits for the connection");
    h.app.on_app_event(resolved);
    h.meta_db("local-pg", DbEvent::Connected);
    let sent: Vec<String> = h
        .sent()
        .into_iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements.join(";")) } else { None })
        .collect();
    assert_eq!(sent, ["SELECT 1"]);
}

/// The policy became read-only while the tab's session was open (the profile was edited): that
/// session is not read-only on the server, so nothing runs on it until it connects again.
#[test]
fn a_policy_that_became_read_only_asks_for_a_reconnect() {
    let mut h = connected(None, Lang::En);
    assert_eq!(run(&mut h, "SELECT 1"), ["SELECT 1"]);
    h.tab_db(
        0,
        DbEvent::Done {
            id: h.app.tab().exec.query_id,
            outcome: datarig_core::driver::Outcome::Command("SELECT".into()),
            elapsed: std::time::Duration::ZERO,
        },
    );
    h.app.profiles[0].policy = Some("prod".into());
    assert!(run(&mut h, "SELECT 1").is_empty(), "not even a read on the old session");
    assert_eq!(status(&h), Some(Msg::SafetyReadOnlyReconnect { policy: "prod".into(), key: "Space c r".into() }));
}

/// The markers: `RO` on the tab and in the explorer, a `READ-ONLY` badge in the status bar,
/// in words (policies have no color).
#[test]
fn read_only_markers_on_the_tab_the_explorer_and_the_status_bar() {
    let mut h = connected(Some("prod"), Lang::En);
    let screen = h.screen(160, 45);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[0].contains("console 1 RO ×"), "tab bar: {}", lines[0]);
    assert!(screen.contains("local-pg RO"), "explorer");
    assert!(lines[44].contains("policy: prod  READ-ONLY "), "status bar: {}", lines[44]);
    let mut h = connected(None, Lang::En);
    let screen = h.screen(160, 45);
    assert!(!screen.contains(" RO") && !screen.contains("READ-ONLY"), "{screen}");
}

#[test]
fn read_only_markers_snapshots() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = connected(Some("prod"), lang);
        let code = if lang == Lang::En { "en" } else { "ko" };
        for (w, hgt) in [(80, 24), (160, 45)] {
            assert_screen!(format!("read_only_markers_{code}_{w}x{hgt}"), lang, h.draw(w, hgt));
        }
    }
}

// ── asking before statements that may do harm ─────────────────────

use datarig_core::i18n::Label;
use datarig_core::policy::Confirm;
use datarig_tui::app::overlay::OverlayKind;
use ratatui::crossterm::event::KeyCode;

/// The indexes of the statements the open run confirmation lists.
fn asked(h: &Harness) -> Option<Vec<usize>> {
    h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| d.index).collect())
}

fn executes(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements) } else { None })
        .collect()
}

/// Select the whole editor (Visual from the top to the end) and run it.
fn run_all(h: &mut Harness, sql: &str) {
    h.app.tab_mut().editor = Editor::new(sql);
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
}

#[test]
fn a_destructive_statement_asks_first_and_cancel_has_the_focus() {
    let mut h = connected(None, Lang::En);
    assert!(run(&mut h, "DROP TABLE shop.orders").is_empty(), "nothing is sent before the answer");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
    assert_eq!(asked(&h), Some(vec![0]));
    let screen = h.screen(160, 45);
    assert!(screen.contains("Run a statement that may do harm?"), "{screen}");
    assert!(screen.contains("1/1  drops it  target: shop.orders"), "{screen}");
    assert!(screen.contains("DROP TABLE shop.orders"), "{screen}");
    assert!(screen.contains("● local-pg · policy: default"), "the connection: {screen}");
    // Enter on the focused button (Cancel) does not run it.
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert!(executes(&mut h).is_empty());
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyConfirmCancelled)));
    // y runs it.
    run(&mut h, "DROP TABLE shop.orders");
    h.keys("y");
    assert_eq!(executes(&mut h), [vec!["DROP TABLE shop.orders".to_string()]]);
}

#[test]
fn enter_runs_only_once_run_has_the_focus() {
    let mut h = connected(None, Lang::En);
    for (keys, runs) in [
        (&[KeyCode::Tab][..], true),
        (&[KeyCode::Right], true),
        (&[KeyCode::Char('l')], true),
        (&[KeyCode::Tab, KeyCode::Tab], false),
        (&[KeyCode::Right, KeyCode::Left], false),
        (&[KeyCode::Char('l'), KeyCode::Char('h')], false),
        (&[KeyCode::BackTab], true),
    ] {
        run(&mut h, "TRUNCATE shop.orders");
        for k in keys {
            h.key(*k);
        }
        h.key(KeyCode::Enter);
        assert_eq!(h.overlay_kind(), None, "{keys:?}");
        assert_eq!(!executes(&mut h).is_empty(), runs, "{keys:?}");
        let id = h.app.tab().exec.query_id;
        h.tab_db(
            0,
            DbEvent::Done { id, outcome: datarig_core::driver::Outcome::Command("X".into()), elapsed: Duration::ZERO },
        );
    }
    for cancel in [KeyCode::Esc, KeyCode::Char('n')] {
        run(&mut h, "TRUNCATE shop.orders");
        h.key(KeyCode::Tab);
        h.key(cancel);
        assert_eq!(h.overlay_kind(), None);
        assert!(executes(&mut h).is_empty(), "{cancel:?}");
    }
}

/// A run of several statements asks once, listing every statement that asks; nothing of the
/// run is sent before the answer, and all of it after.
#[test]
fn a_run_of_several_statements_asks_once_for_all_of_them() {
    let mut h = connected(None, Lang::En);
    run_all(
        &mut h,
        "SELECT 1;\nDELETE FROM shop.orders;\nUPDATE shop.users SET name = 'x' WHERE 1 = 1;\nINSERT INTO t VALUES (1);\nALTER TABLE shop.users DROP COLUMN name;\nEXPLAIN ANALYZE DELETE FROM shop.reviews;",
    );
    assert!(executes(&mut h).is_empty());
    assert_eq!(asked(&h), Some(vec![1, 2, 4, 5]));
    let screen = h.screen(160, 45);
    for line in [
        "Run 4 statements that may do harm?",
        "2/6  deletes every row · no WHERE  target: shop.orders",
        "3/6  updates every row · its WHERE is always true  target: shop.users",
        "5/6  drops a column  target: shop.users",
        "6/6  deletes every row · no WHERE · EXPLAIN ANALYZE runs it, then rolls it back  target: shop.reviews",
    ] {
        assert!(screen.contains(line), "{line}\n{screen}");
    }
    h.keys("y");
    let sent = executes(&mut h);
    assert_eq!(sent.len(), 1, "one run");
    assert_eq!(sent[0].len(), 6, "all of it");
}

/// A selection that cuts a DELETE before its WHERE runs a DELETE of every row: it asks.
#[test]
fn a_selection_that_cuts_off_the_where_asks() {
    let mut h = connected(None, Lang::En);
    h.app.tab_mut().editor = Editor::new("DELETE FROM shop.users WHERE id = 1;");
    h.keys("gg0veeeee");
    h.sent();
    h.ctrl('e');
    assert_eq!(asked(&h), Some(vec![0]));
    let c = h.app.overlays.run_confirm().unwrap();
    assert_eq!(c.statements, ["DELETE FROM shop.users"]);
    assert!(executes(&mut h).is_empty());
    h.keys("n");
    assert!(executes(&mut h).is_empty());
}

#[test]
fn harmless_statements_do_not_ask() {
    let mut h = connected(None, Lang::En);
    for sql in [
        "SELECT * FROM shop.users",
        "INSERT INTO t VALUES (1)",
        "UPDATE shop.users SET name = 'x' WHERE id = 1",
        "DELETE FROM shop.users WHERE id = 1",
        "EXPLAIN DELETE FROM shop.users",
        "ALTER TABLE shop.users DROP CONSTRAINT users_pkey",
        "SELECT 'DROP TABLE x'",
        "SELECT 1 -- DROP TABLE x",
        "CREATE TABLE zz_new (id int)",
        "SELECT set_config('search_path', 'shop', false)",
        "MERGE INTO shop.users u USING shop.users s ON u.id = s.id WHEN NOT MATCHED THEN INSERT (name) VALUES (s.name)",
    ] {
        assert_eq!(run(&mut h, sql).len(), 1, "{sql}");
        assert_eq!(h.overlay_kind(), None, "{sql}");
        let id = h.app.tab().exec.query_id;
        h.tab_db(
            0,
            DbEvent::Done { id, outcome: datarig_core::driver::Outcome::Command("X".into()), elapsed: Duration::ZERO },
        );
    }
}

/// `confirm = "writes"` asks for everything that may change something, unknown and procedural
/// statements included; reads, settings and transaction control still run at once.
#[test]
fn confirm_writes_asks_for_every_write() {
    let mut cfg = config(Some("careful"));
    cfg.policies.insert("careful", Policy { confirm: Confirm::Writes, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    for sql in ["INSERT INTO t VALUES (1)", "CREATE INDEX i ON t (a)", "DO $$ BEGIN END $$", "EXECUTE p", "VACUUM"] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        h.keys("n");
    }
    for sql in ["SELECT 1", "SET search_path TO shop", "BEGIN", "EXPLAIN DELETE FROM t"] {
        assert_eq!(run(&mut h, sql).len(), 1, "{sql}");
        let id = h.app.tab().exec.query_id;
        h.tab_db(
            0,
            DbEvent::Done { id, outcome: datarig_core::driver::Outcome::Command("X".into()), elapsed: Duration::ZERO },
        );
    }
}

/// A read-only policy refuses before it would ask.
#[test]
fn a_read_only_policy_refuses_before_it_asks() {
    let mut h = connected(Some("prod"), Lang::En);
    assert!(run(&mut h, "DROP TABLE shop.orders").is_empty());
    assert_eq!(h.overlay_kind(), None);
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })));
}

/// The tab moved to another connection (or was rebound) while the question was open: the
/// answer runs nothing.
#[test]
fn a_confirmation_for_a_tab_that_was_rebound_runs_nothing() {
    let mut h = connected(None, Lang::En);
    run(&mut h, "DROP TABLE shop.orders");
    let (tab, profile) = (h.app.tab().id, h.app.tab().profile);
    h.app.tabs.bind(tab, profile);
    h.keys("y");
    assert!(executes(&mut h).is_empty());
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyConfirmMoved)));
}

/// A statement for a profile that is not connected yet asks before it is queued, and runs
/// without asking again once the connection is up.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_statement_asks_before_it_is_queued_and_not_again() {
    let mut cfg = config(None);
    cfg.connections[0].password = String::new();
    cfg.connections[0].set_source(PasswordSource::Command("printf datarig".into()));
    let (h, mut rx) = Harness::started(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    let mut h = h.with_fake_driver();
    h.explore("local-pg");
    h.keys("o");
    let resolved = loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("an event").expect("open");
        if matches!(ev, AppEvent::Resolved { .. }) {
            break ev;
        }
    };
    h.app.focus = Focus::Editor;
    let tab = h.app.tab().id;
    run(&mut h, "DELETE FROM shop.users");
    assert_eq!(asked(&h), Some(vec![0]));
    assert!(!h.app.is_queued(tab), "not queued before the answer");
    h.keys("n");
    assert!(!h.app.is_queued(tab), "cancelled: not queued");
    run(&mut h, "DELETE FROM shop.users");
    h.keys("y");
    assert!(h.app.is_queued(tab), "queued once confirmed");
    h.app.on_app_event(resolved);
    h.meta_db("local-pg", DbEvent::Connected);
    assert_eq!(h.overlay_kind(), None, "not asked again");
    assert_eq!(executes(&mut h), [vec!["DELETE FROM shop.users".to_string()]]);
}

#[test]
fn run_confirmation_snapshots() {
    for lang in [Lang::En, Lang::Ko] {
        let code = if lang == Lang::En { "en" } else { "ko" };
        let mut cfg = config(Some("careful"));
        cfg.policies.insert("careful", Policy { confirm: Confirm::Writes, ..Policy::default() });
        let mut h = Harness::with_config(&cfg, lang);
        h.db(DbEvent::Connected);
        run_all(
            &mut h,
            "SELECT 1;\nDELETE FROM shop.orders;\nUPDATE shop.users SET name = 'x' WHERE true;\nDROP TABLE IF EXISTS shop.reviews CASCADE;\nINSERT INTO shop.audit_log (note) VALUES ('中文 備註');",
        );
        assert_eq!(asked(&h), Some(vec![1, 2, 3, 4]));
        for (w, hgt) in [(80, 24), (160, 45)] {
            assert_screen!(format!("run_confirm_{code}_{w}x{hgt}"), lang, h.draw(w, hgt));
        }
        h.key(KeyCode::Tab);
        assert_screen!(format!("run_confirm_run_focused_{code}_80x24"), lang, h.draw(80, 24));
    }
}

// ── EXPLAIN ANALYZE ──────────────────────────────────────────────────────────

/// The driver rolls back what EXPLAIN ANALYZE ran; when it wrapped more than a read, the tab
/// says so once the plan comes.
#[test]
fn explain_analyze_of_a_write_says_it_was_rolled_back() {
    let mut h = connected(None, Lang::En);
    let plan = |h: &mut Harness| {
        let id = h.app.tab().exec.query_id;
        let cols = vec![meta("QUERY PLAN", "text", false, false)];
        h.tab_db(
            0,
            DbEvent::Page {
                id,
                columns: Some(cols),
                rows: vec![vec![Some("Update".into())]],
                more: false,
                elapsed: Duration::ZERO,
            },
        );
    };
    assert_eq!(run(&mut h, "EXPLAIN ANALYZE UPDATE shop.users SET name = 'x' WHERE id = 1").len(), 1);
    plan(&mut h);
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyExplainRolledBack)));
    assert!(h.screen(160, 45).contains("EXPLAIN ANALYZE ran the statement to measure it, then rolled it back"));
    // A read: nothing to say.
    run(&mut h, "EXPLAIN ANALYZE SELECT 1");
    plan(&mut h);
    assert!(matches!(status(&h), Some(Msg::QueryDoneRows { .. })), "{:?}", status(&h));
    // Destructive: it asks first (it does run), then says it was rolled back.
    run(&mut h, "EXPLAIN (ANALYZE, BUFFERS) DELETE FROM shop.orders");
    assert_eq!(asked(&h), Some(vec![0]));
    h.keys("y");
    plan(&mut h);
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyExplainRolledBack)));
}

// ── adversarial probes ─────────────────────────────────────────

fn done(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    h.tab_db(
        0,
        DbEvent::Done { id, outcome: datarig_core::driver::Outcome::Command("X".into()), elapsed: Duration::ZERO },
    );
}

/// (1) `EXPLAIN ("analyze") DELETE …` runs the DELETE: it asks (and says it is rolled back), and
/// a read-only policy refuses it.
#[test]
fn explain_with_a_quoted_analyze_option_asks_and_is_refused_when_read_only() {
    let mut h = connected(None, Lang::En);
    assert!(run(&mut h, "EXPLAIN (\"analyze\") DELETE FROM shop.users").is_empty());
    assert_eq!(asked(&h), Some(vec![0]));
    assert!(
        h.screen(160, 45)
            .contains("deletes every row · no WHERE · EXPLAIN ANALYZE runs it, then rolls it back  target: shop.users")
    );
    let mut h = connected(Some("prod"), Lang::En);
    assert!(run(&mut h, "EXPLAIN (\"analyze\") DELETE FROM shop.users").is_empty());
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })));
}

/// (2) A file saved with a lone carriage return after `--`: the condition goes on after it, so
/// the UPDATE changes every row and asks.
#[test]
fn a_carriage_return_after_a_line_comment_does_not_hide_or_true() {
    let mut h = connected(None, Lang::En);
    let sent = run(&mut h, "UPDATE shop.users SET name = 'x' WHERE id = 5 --\r OR true\nRETURNING id");
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(asked(&h), Some(vec![0]));
    assert!(h.screen(160, 45).contains("updates every row · its WHERE is always true  target: shop.users"));
}

/// (3) A `$$` right after a no-break space (or an emoji) belongs to the identifier: the DELETE in
/// the next CTE is seen.
#[test]
fn a_dollar_after_a_no_break_space_does_not_hide_a_delete() {
    let mut h = connected(None, Lang::En);
    for sql in [
        "WITH a AS (SELECT 1 AS x\u{a0}$$), d AS (DELETE FROM shop.users RETURNING 1) SELECT 1 AS y\u{a0}$$",
        "WITH a AS (SELECT 1 AS \u{1f418}$$), d AS (DELETE FROM shop.users RETURNING 1) SELECT 1 AS \u{1f418}$$",
    ] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        h.keys("n");
    }
}

/// (4) `PREPARE p AS DELETE …` then `EXECUTE p`: the EXECUTE asks as the DELETE would, in the
/// same run or a later one; after `DEALLOCATE` (or on a new session) the name is unknown, and
/// an EXECUTE of an unknown name asks too.
#[test]
fn execute_is_checked_as_the_statement_it_prepared() {
    let mut h = connected(None, Lang::En);
    run_all(&mut h, "PREPARE pz AS DELETE FROM shop.users;\nEXECUTE pz;");
    assert_eq!(asked(&h), Some(vec![1]));
    assert!(h.screen(160, 45).contains("2/2  deletes every row · no WHERE  target: shop.users"));
    h.keys("n");
    assert_eq!(run(&mut h, "PREPARE pz AS DELETE FROM shop.users"), ["PREPARE pz AS DELETE FROM shop.users"]);
    done(&mut h);
    assert!(run(&mut h, "EXECUTE pz").is_empty());
    assert_eq!(asked(&h), Some(vec![0]));
    h.keys("n");
    assert_eq!(run(&mut h, "PREPARE ok AS SELECT * FROM shop.users"), ["PREPARE ok AS SELECT * FROM shop.users"]);
    done(&mut h);
    assert_eq!(run(&mut h, "EXECUTE ok"), ["EXECUTE ok"], "a prepared read runs at once");
    done(&mut h);
    assert_eq!(run(&mut h, "DEALLOCATE pz"), ["DEALLOCATE pz"]);
    done(&mut h);
    assert!(run(&mut h, "EXECUTE pz").is_empty());
    assert!(h.screen(160, 45).contains("runs a prepared statement not known to this tab"));
    h.keys("n");
    // A new session prepared nothing.
    h.tab_db(0, DbEvent::Lost { error: datarig_core::driver::DbError::Closed });
    assert!(run(&mut h, "EXECUTE ok").is_empty());
    assert_eq!(asked(&h), Some(vec![0]));
}

/// A read-only policy refuses what the parser rejects and an EXECUTE it cannot check, and
/// allows one of a prepared read.
#[test]
fn read_only_and_prepared_statements() {
    let mut h = connected(Some("prod"), Lang::En);
    assert_eq!(run(&mut h, "PREPARE r AS SELECT 1"), ["PREPARE r AS SELECT 1"]);
    done(&mut h);
    assert_eq!(run(&mut h, "EXECUTE r"), ["EXECUTE r"]);
    done(&mut h);
    for sql in ["EXECUTE w", "SELEC 1"] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{sql}");
    }
    // Prepared and executed in one run: the EXECUTE runs a DELETE.
    run_all(&mut h, "PREPARE w AS DELETE FROM shop.users;\nEXECUTE w;");
    assert!(executes(&mut h).is_empty());
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { sql, .. }) if sql == "EXECUTE w"));
}

/// The escape: `set_config()` of the read-only settings is refused under a read-only policy.
#[test]
fn set_config_of_the_read_only_settings_is_refused_when_read_only() {
    let mut h = connected(Some("prod"), Lang::En);
    for sql in [
        "SELECT set_config('default_transaction_read_only','off',false)",
        "SELECT pg_catalog.set_config('transaction_read_only', 'off', true)",
    ] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: "this statement asks for a read-write transaction".into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected), "{sql}");
    }
    assert_eq!(run(&mut h, "SELECT set_config('search_path', 'shop', false)").len(), 1);
}

/// What else asks under the default policy now: DO/CALL, a MERGE that updates or deletes,
/// ALTER … TYPE (with or without USING), a WHERE that reads no column, a text the parser rejects.
#[test]
fn the_default_policy_asks_for_code_merges_rewrites_and_unreadable_text() {
    let mut h = connected(None, Lang::En);
    for (sql, why) in [
        ("DO $$ BEGIN END $$", "runs code that cannot be checked (DO/CALL)"),
        ("CALL shop.make_tags()", "runs code that cannot be checked (DO/CALL)"),
        (
            "MERGE INTO shop.users u USING shop.users s ON true WHEN MATCHED THEN DELETE",
            "MERGE that updates or deletes rows  target: shop.users",
        ),
        (
            "ALTER TABLE shop.users ALTER name TYPE text USING NULL",
            "converts a column to another type, which may lose data",
        ),
        ("ALTER TABLE shop.orders ALTER total TYPE int", "converts a column to another type, which may lose data"),
        ("DELETE FROM shop.users WHERE 1 <> 0", "deletes every row · its WHERE reads no column"),
        ("SELEC * FROM shop.users", "PostgreSQL's parser cannot read it"),
    ] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        let screen = h.screen(160, 45);
        assert!(screen.contains(why), "{why}\n{screen}");
        h.keys("n");
    }
}

/// A server that ignored the read-only session default (a pooler): the connection stays up,
/// and the status bar says each transaction is made read-only instead.
#[test]
fn a_pooler_that_ignores_the_read_only_default_is_reported_not_refused() {
    let mut h = connected(Some("prod"), Lang::En);
    h.db(DbEvent::ReadOnlyPerTransaction);
    assert_eq!(status(&h), Some(Msg::DbReadOnlyPerTransaction { name: "local-pg".into() }));
    assert!(h.screen(160, 45).contains("local-pg: the server ignored the read-only session default"));
    assert_eq!(run(&mut h, "SELECT 1"), ["SELECT 1"], "it runs");
}

// ── adversarial probes ─────────────────────────────────────────

/// X2: a statement nested too deeply for the parser's stack (`SELECT 1 + 1 + …`, 30 000 terms,
/// which crashed the app) or too long is not parsed: it asks under the default policy and a
/// read-only policy refuses it, each saying why. Nothing is sent before the answer.
#[test]
fn a_statement_too_deep_or_too_long_to_check_asks_or_is_refused() {
    let deep = format!("SELECT 1{}", " + 1".repeat(30_000));
    let long = format!("SELECT 1{}", " + 1".repeat(262_144));
    let mut h = connected(None, Lang::En);
    for sql in [&deep, &long] {
        assert!(run(&mut h, sql).is_empty(), "nothing is sent before the answer");
        assert_eq!(asked(&h), Some(vec![0]));
        assert!(h.screen(160, 45).contains("1/1  too long or too deeply nested to check"));
        h.keys("n");
    }
    let mut h = connected(Some("prod"), Lang::En);
    for (sql, what) in [
        (deep.as_str(), "this statement is too long or too deeply nested to check"),
        ("SELEC 1", "PostgreSQL's parser cannot read this statement"),
    ] {
        assert!(run(&mut h, sql).is_empty());
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: what.into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected));
    }
}

/// X1 with the driver's events: what a statement prepares or deallocates counts once the
/// server says it succeeded (`Finished` of a statement before the last, the run's answer for
/// the last); a failure, a cancel or a statement the run never reached leaves the names it
/// may touch unknown, so their `EXECUTE` asks.
#[test]
fn prepared_names_count_only_once_the_server_confirms_them() {
    use datarig_core::driver::{DbError, Outcome};
    let mut h = connected(None, Lang::En);
    let failed = |h: &mut Harness, cancelled: bool| {
        let id = h.app.tab().exec.query_id;
        let error = if cancelled { DbError::Cancelled } else { DbError::Server("ERROR: no".into()) };
        h.tab_db(0, DbEvent::Failed { id, error, cancelled });
    };
    run(&mut h, "PREPARE pz AS DELETE FROM shop.users");
    done(&mut h);
    run(&mut h, "PREPARE ok AS SELECT 1");
    done(&mut h);
    // Failed on the server ("already exists"): pz is not assumed to be a read.
    run(&mut h, "PREPARE pz AS SELECT 1");
    failed(&mut h, false);
    assert!(run(&mut h, "EXECUTE pz").is_empty());
    assert_eq!(asked(&h), Some(vec![0]));
    assert!(h.screen(160, 45).contains("runs a prepared statement not known to this tab"));
    h.keys("n");
    assert_eq!(run(&mut h, "EXECUTE ok"), ["EXECUTE ok"], "a confirmed read runs at once");
    done(&mut h);
    // A run of three: the second succeeded, the third failed.
    run_all(&mut h, "PREPARE two AS SELECT 2;\nDEALLOCATE ok;\nSELECT 1/0;");
    assert_eq!(executes(&mut h).len(), 1);
    let id = h.app.tab().exec.query_id;
    for index in 0..2 {
        h.tab_db(0, DbEvent::Started { id, index });
        let outcome = Outcome::Command("X".into());
        h.tab_db(0, DbEvent::Finished { id, index, outcome, elapsed: Duration::ZERO });
    }
    h.tab_db(0, DbEvent::Started { id, index: 2 });
    failed(&mut h, false);
    assert_eq!(run(&mut h, "EXECUTE two"), ["EXECUTE two"]);
    done(&mut h);
    assert!(run(&mut h, "EXECUTE ok").is_empty(), "deallocated");
    h.keys("n");
    // A run stopped before its PREPARE (the first statement failed), and a cancelled one.
    run(&mut h, "PREPARE again AS SELECT 3");
    done(&mut h);
    run_all(&mut h, "SELECT 1/0;\nDEALLOCATE again;");
    failed(&mut h, false);
    assert!(run(&mut h, "EXECUTE again").is_empty(), "whether it was deallocated is unknown");
    h.keys("n");
    run(&mut h, "PREPARE c AS SELECT 4");
    failed(&mut h, true);
    assert!(run(&mut h, "EXECUTE c").is_empty());
    h.keys("n");
    // Before the answer the name is not known either.
    run(&mut h, "PREPARE d AS SELECT 5");
    assert!(!h.app.tab().exec.prepared.knows("d"));
    done(&mut h);
    assert!(h.app.tab().exec.prepared.knows("d"));
}

/// A function can re-prepare a name on the server (plpgsql `EXECUTE 'DEALLOCATE
/// pf'; EXECUTE 'PREPARE pf AS DELETE …'`), so after a statement that calls a function that is
/// not a known built-in, whether it succeeded or failed, no name is known: the next `EXECUTE`
/// asks under the default policy and is refused under a read-only one. Built-ins keep them.
#[test]
fn a_function_call_forgets_the_prepared_names() {
    use datarig_core::driver::DbError;
    for policy in [None, Some("prod")] {
        let mut h = connected(policy, Lang::En);
        let refused = |h: &mut Harness, sql: &str| {
            if policy.is_some() {
                assert!(
                    matches!(status(h), Some(Msg::SafetyReadOnlyBlocked { sql: s, .. }) if s == "EXECUTE pf"),
                    "{sql}"
                );
            } else {
                assert_eq!(asked(h), Some(vec![sql.lines().count() - 1]), "{sql}");
                assert!(h.screen(160, 45).contains("runs a prepared statement not known to this tab"), "{sql}");
                h.keys("n");
            }
        };
        let prepare = |h: &mut Harness| {
            assert_eq!(run(h, "PREPARE pf AS SELECT 1"), ["PREPARE pf AS SELECT 1"]);
            done(h);
            assert!(h.app.tab().exec.prepared.knows("pf"));
        };
        prepare(&mut h);
        assert_eq!(run(&mut h, "SELECT zz_swap()"), ["SELECT zz_swap()"]);
        done(&mut h);
        assert!(run(&mut h, "EXECUTE pf").is_empty(), "{policy:?}: nothing is sent");
        refused(&mut h, "EXECUTE pf");
        // The call failed: it may have swapped the name before it did.
        prepare(&mut h);
        assert_eq!(run(&mut h, "SELECT public.zz_swap()"), ["SELECT public.zz_swap()"]);
        let id = h.app.tab().exec.query_id;
        h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("ERROR: no".into()), cancelled: false });
        assert!(run(&mut h, "EXECUTE pf").is_empty(), "{policy:?}: nothing is sent");
        refused(&mut h, "EXECUTE pf");
        // In one run, before anything is sent.
        prepare(&mut h);
        run_all(&mut h, "SELECT zz_swap();\nEXECUTE pf");
        assert!(executes(&mut h).is_empty(), "{policy:?}");
        refused(&mut h, "SELECT zz_swap();\nEXECUTE pf");
        // A built-in keeps the names.
        assert_eq!(run(&mut h, "SELECT lower('A'), now(), count(*) FROM shop.users").len(), 1);
        done(&mut h);
        assert_eq!(run(&mut h, "EXECUTE pf"), ["EXECUTE pf"], "{policy:?}");
        done(&mut h);
    }
}

/// `COPY … FROM/TO PROGRAM` runs a command on the server and `COPY … FROM/TO
/// '<file>'` reads or writes a server file: they ask under the default policy (nothing is sent
/// before the answer) and a read-only policy refuses them.
#[test]
fn copy_with_a_program_or_a_server_file_asks_or_is_refused() {
    let mut h = connected(None, Lang::En);
    for (sql, why) in [
        ("COPY shop.users FROM PROGRAM 'id'", "runs a program on the database server (COPY … PROGRAM)"),
        ("COPY shop.users TO PROGRAM 'cat'", "runs a program on the database server (COPY … PROGRAM)"),
        ("COPY shop.users FROM '/etc/passwd'", "reads or writes a file on the database server"),
        ("COPY (SELECT 1) TO '/tmp/zz_x'", "reads or writes a file on the database server"),
    ] {
        assert!(run(&mut h, sql).is_empty(), "nothing is sent before the answer: {sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        let screen = h.screen(160, 45);
        assert!(screen.contains(why), "{why}\n{screen}");
        h.keys("n");
    }
    let mut h = connected(Some("prod"), Lang::En);
    for sql in ["COPY shop.users FROM PROGRAM 'id'", "COPY shop.users TO '/tmp/zz_x'"] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: "this statement writes".into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected), "{sql}");
    }
}

/// `ts_rewrite(tsquery, text)` runs the query it is given, which may re-prepare a
/// name: after it no name is known, so the next `EXECUTE` asks under the default policy and is
/// refused under a read-only one, with nothing sent.
#[test]
fn a_builtin_that_runs_the_query_it_is_given_forgets_the_prepared_names() {
    let swap = "SELECT ts_rewrite('a'::tsquery, 'SELECT ''a''::tsquery, ''b''::tsquery WHERE zz_swap() = 1')";
    for policy in [None, Some("prod")] {
        let mut h = connected(policy, Lang::En);
        assert_eq!(run(&mut h, "PREPARE pf AS SELECT 1"), ["PREPARE pf AS SELECT 1"]);
        done(&mut h);
        assert_eq!(run(&mut h, swap), [swap], "{policy:?}: a read, sent at once");
        done(&mut h);
        assert!(run(&mut h, "EXECUTE pf").is_empty(), "{policy:?}: nothing is sent");
        if policy.is_some() {
            assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { sql, .. }) if sql == "EXECUTE pf"));
        } else {
            assert_eq!(asked(&h), Some(vec![0]));
            assert!(h.screen(160, 45).contains("runs a prepared statement not known to this tab"));
            h.keys("n");
        }
    }
}

/// A built-in that reads, writes or lists files of the server (`lo_export` wrote a
/// file on a read-only profile) or acts on the server beyond the transaction asks under the
/// default policy, naming the function, and a read-only policy refuses it before sending, since
/// the server's read-only transaction does not stop it.
#[test]
fn builtins_that_act_on_the_server_ask_or_are_refused() {
    let file = "reads, writes or lists files on the database server (a built-in function)";
    let action = "acts on the database server beyond this transaction (a built-in function)";
    let mut h = connected(None, Lang::En);
    for (sql, why, name) in [
        ("SELECT lo_export(16400, '/tmp/zz_safe36_lo')", file, "lo_export"),
        ("SELECT pg_read_file('/etc/passwd')", file, "pg_read_file"),
        ("SELECT * FROM pg_ls_dir('.')", file, "pg_ls_dir"),
        ("SELECT pg_terminate_backend(pid) FROM pg_stat_activity", action, "pg_terminate_backend"),
        ("SELECT pg_reload_conf()", action, "pg_reload_conf"),
    ] {
        assert!(run(&mut h, sql).is_empty(), "nothing is sent before the answer: {sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        let screen = h.screen(160, 45);
        assert!(screen.contains(why) && screen.contains(&format!("target: {name}")), "{sql}\n{screen}");
        h.keys("n");
    }
    // Harmless built-ins run at once.
    let sql = "SELECT pg_notify('c', 'x'), pg_try_advisory_lock(1), now()";
    assert_eq!(run(&mut h, sql), [sql]);
    done(&mut h);
    let mut h = connected(Some("prod"), Lang::En);
    for (sql, what) in [
        (
            "SELECT lo_export(16400, '/tmp/zz_safe36_lo')",
            "this statement reads, writes or lists files on the database server",
        ),
        ("SELECT pg_cancel_backend(1)", "this statement acts on the database server beyond its transaction"),
    ] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: what.into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected), "{sql}");
    }
}

/// A built-in that acts on the server, called with a name qualified with the
/// database (`datarig.pg_catalog.lo_export(…)` wrote a file on a read-only profile) or in
/// attribute notation (`('/etc/hostname'::text).pg_read_file` returned the file), asks under
/// the default policy, naming the function, and a read-only policy refuses it before sending.
#[test]
fn builtins_that_act_on_the_server_in_any_call_form_ask_or_are_refused() {
    let file = "reads, writes or lists files on the database server (a built-in function)";
    let action = "acts on the database server beyond this transaction (a built-in function)";
    let calls = [
        ("SELECT datarig.pg_catalog.lo_export(16400, '/tmp/zz_safe39_3p')", file, "lo_export"),
        ("SELECT ('base'::text).pg_ls_dir", file, "pg_ls_dir"),
        ("SELECT ('/etc/hostname'::text).pg_read_file", file, "pg_read_file"),
        ("SELECT (0).pg_cancel_backend", action, "pg_cancel_backend"),
        ("SELECT ('base'::text).pg_catalog.pg_ls_dir", file, "pg_ls_dir"),
        ("SELECT t.pg_read_file FROM (VALUES ('/etc/hostname'::text)) t(x)", file, "pg_read_file"),
    ];
    let mut h = connected(None, Lang::En);
    for (sql, why, name) in calls {
        assert!(run(&mut h, sql).is_empty(), "nothing is sent before the answer: {sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        let screen = h.screen(160, 45);
        assert!(screen.contains(why) && screen.contains(&format!("target: {name}")), "{sql}\n{screen}");
        h.keys("n");
    }
    let mut h = connected(Some("prod"), Lang::En);
    for (sql, why, _) in calls {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        let what = if why == file {
            "this statement reads, writes or lists files on the database server"
        } else {
            "this statement acts on the database server beyond its transaction"
        };
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: what.into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected), "{sql}");
    }
}

const SERVER_FILE: &str = "reads, writes or lists files on the database server (a built-in function)";
const SERVER_ACTION: &str = "acts on the database server beyond this transaction (a built-in function)";
const QUERY_TEXT: &str = "runs a query given as text that cannot be checked (a built-in function)";

/// After `setup` (sent, and done), each of `calls` (a statement, why it asks, the function the
/// confirm names) asks under the default policy, naming the function, and is refused before
/// sending under a read-only policy, saying why; `harmless` runs at once under both.
fn asks_or_is_refused(setup: &str, calls: &[(&str, &str, &str)], harmless: &str) {
    let prepare = |h: &mut Harness| {
        assert_eq!(run(h, setup), [setup]);
        done(h);
    };
    let mut h = connected(None, Lang::En);
    prepare(&mut h);
    for &(sql, why, name) in calls {
        assert!(run(&mut h, sql).is_empty(), "nothing is sent before the answer: {sql}");
        assert_eq!(asked(&h), Some(vec![0]), "{sql}");
        let screen = h.screen(160, 45);
        assert!(screen.contains(why) && screen.contains(&format!("target: {name}")), "{sql}\n{screen}");
        h.keys("n");
    }
    assert_eq!(run(&mut h, harmless), [harmless]);
    done(&mut h);
    let mut h = connected(Some("prod"), Lang::En);
    prepare(&mut h);
    for &(sql, why, _) in calls {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        let what = match why {
            SERVER_FILE => "this statement reads, writes or lists files on the database server",
            SERVER_ACTION => "this statement acts on the database server beyond its transaction",
            _ => "this statement runs a query given as text that cannot be checked",
        };
        let expected = Msg::SafetyReadOnlyBlocked {
            policy: "prod".into(),
            what: what.into(),
            sql: datarig_tui::app::runlog::excerpt(sql, 60),
        };
        assert_eq!(status(&h), Some(expected), "{sql}");
    }
    assert_eq!(run(&mut h, harmless), [harmless]);
}

/// On a read-only profile a query given as text to a built-in ran the server
/// built-ins it calls (`query_to_xml('select lo_export(…)', …)` wrote a file, `ts_stat('select
/// … pg_read_file(…)')` read one). A read-only policy refuses them before sending, and the
/// default policy asks, naming the function; a query given as text that cannot be read asks or
/// is refused too, and a harmless one runs at once under both.
#[test]
fn queries_given_as_text_to_a_builtin_ask_or_are_refused() {
    let calls = [
        (
            "SELECT query_to_xml('select datarig.pg_catalog.lo_export(7964525, ''/tmp/zz_safe41_c'')', true, false, '')",
            SERVER_FILE,
            "lo_export",
        ),
        (
            "SELECT word FROM ts_stat('select to_tsvector(''simple'', pg_read_file(''/etc/hostname''))')",
            SERVER_FILE,
            "pg_read_file",
        ),
        (
            "SELECT query_to_xml('SELECT query_to_xml(''SELECT pg_terminate_backend(-1)'', true, false, '''')', true, false, '')",
            SERVER_ACTION,
            "pg_terminate_backend",
        ),
        ("SELECT query_to_xml(q, true, false, '') FROM (VALUES ('SELECT 1')) v(q)", QUERY_TEXT, "query_to_xml"),
    ];
    asks_or_is_refused("SELECT 1", &calls, "SELECT query_to_xml('select 1', true, false, '')");
}

/// A plain `EXPLAIN EXECUTE` evaluates the parameters on the server to plan the
/// statement: `EXPLAIN EXECUTE zz_p(lo_export(…)::text)` wrote a file on a read-only profile.
/// A read-only policy refuses it before sending, and the default policy asks, naming the
/// function; one with harmless parameters runs at once under both.
#[test]
fn the_parameters_of_a_plain_explain_execute_ask_or_are_refused() {
    let calls = [
        ("EXPLAIN EXECUTE zz_p(lo_export(7964525, '/tmp/zz_safe41_d')::text)", SERVER_FILE, "lo_export"),
        ("EXPLAIN EXECUTE zz_p(pg_terminate_backend(-1)::text)", SERVER_ACTION, "pg_terminate_backend"),
        (
            "EXPLAIN (COSTS off) EXECUTE zz_p(query_to_xml(current_user, true, false, '')::text)",
            QUERY_TEXT,
            "query_to_xml",
        ),
    ];
    asks_or_is_refused("PREPARE zz_p(text) AS SELECT length($1)", &calls, "EXPLAIN EXECUTE zz_p('x')");
}

/// The confirm of `COPY <table> TO '<file>'` names the file it writes, not the
/// table it only reads; `COPY … FROM '<file>'` names the table it writes.
#[test]
fn the_confirm_of_copy_to_a_file_names_the_file() {
    let mut h = connected(None, Lang::En);
    for (sql, target, not) in [
        ("COPY shop.users TO '/tmp/zz_x'", "target: '/tmp/zz_x'", "target: shop.users"),
        ("COPY shop.users FROM '/tmp/zz_x'", "target: shop.users", "target: '/tmp/zz_x'"),
    ] {
        assert!(run(&mut h, sql).is_empty(), "{sql}");
        let screen = h.screen(160, 45);
        assert!(screen.contains(target) && !screen.contains(not), "{sql}\n{screen}");
        h.keys("n");
    }
}

/// `COPY … FROM STDIN` broke the tab's connection (the app does not carry COPY
/// data yet) and `COPY … TO STDOUT` failed: both are refused before anything is sent, under
/// any policy, with a message that says so.
#[test]
fn copy_through_the_connection_is_refused_as_not_supported_yet() {
    for policy in [None, Some("prod")] {
        let mut h = connected(policy, Lang::En);
        for sql in
            ["COPY shop.users FROM STDIN", "COPY shop.users TO STDOUT", "COPY (SELECT 1) TO STDOUT WITH (FORMAT csv)"]
        {
            assert!(run(&mut h, sql).is_empty(), "{sql}");
            assert_eq!(h.overlay_kind(), None, "{sql}: refused, not asked");
            assert_eq!(status(&h), Some(Msg::SafetyCopyStdio { sql: sql.into() }), "{sql}");
        }
        assert!(h.screen(160, 45).contains("Not run: COPY … FROM STDIN and COPY … TO STDOUT are not supported yet"));
    }
}

/// The mouse on the run confirmation: the pointer on Run underlines it but Cancel keeps the
/// focus (Enter still cancels); a click on Cancel cancels, one on Run runs, one outside does
/// nothing.
#[test]
fn the_run_confirmation_takes_clicks_and_the_pointer_moves_no_focus() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    // A press and its release, past the arming delay.
    let click = |h: &mut Harness, (x, y): (u16, u16)| {
        h.advance(Duration::from_millis(500));
        h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    };
    let buttons = |h: &mut Harness| {
        h.draw(160, 45);
        let r = &h.app.overlays.run_confirm().expect("asked").buttons.rects;
        (r[0], r[1])
    };
    let mut h = connected(None, Lang::En);
    run(&mut h, "DROP TABLE shop.orders");
    let (cancel, run_b) = buttons(&mut h);
    h.mouse(MouseEventKind::Moved, run_b.x + 1, run_b.y);
    assert!(!h.app.take_idle_event(), "the highlight is a frame");
    let c = h.app.overlays.run_confirm().unwrap();
    assert!(!c.run_focused && c.buttons.hover == Some(1));
    let t = h.draw(160, 45);
    let cell = &t.backend().buffer()[(run_b.x + 2, run_b.y)];
    assert!(cell.modifier.contains(ratatui::style::Modifier::UNDERLINED));
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert!(executes(&mut h).is_empty(), "Enter after the pointer was on Run still cancels");
    run(&mut h, "DROP TABLE shop.orders");
    let (cancel2, _) = buttons(&mut h);
    assert_eq!(cancel, cancel2);
    click(&mut h, (0, 0));
    click(&mut h, (cancel.x + cancel.width + 1, cancel.y));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm), "outside and between: nothing");
    click(&mut h, (cancel.x + 1, cancel.y));
    assert_eq!(h.overlay_kind(), None);
    assert!(executes(&mut h).is_empty());
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyConfirmCancelled)));
    run(&mut h, "DROP TABLE shop.orders");
    let (_, run_b) = buttons(&mut h);
    click(&mut h, (run_b.x + 1, run_b.y));
    assert_eq!(executes(&mut h), [vec!["DROP TABLE shop.orders".to_string()]]);
}
