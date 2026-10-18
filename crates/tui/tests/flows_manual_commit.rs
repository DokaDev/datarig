//! Manual commit (`commit = "manual"`): what each run tells the session about transactions
//! (`TxRule::Manual`, statement by statement), the checks before a run that the open
//! transaction needs, committing and rolling back (keys, commands, menus), what the status bar
//! and the run's Messages say about the open transaction, and the timer that counts its time.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Outcome, ServerEnd, TxOutcome, TxRule, TxStep};
use datarig_core::i18n::{I18n, Label, Lang, Msg};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::action::Action;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

const MS: Duration = Duration::from_millis(3);
const SEC: Duration = Duration::from_secs(1);
const THIRTY_MIN: Duration = Duration::from_secs(30 * 60);

/// The test profile in environment `env`: `stg` commits by hand, `plain` by itself.
fn config(env: &str, mysql: bool) -> Config {
    let mut p = ConnectionConfig { policy: Some(env.to_string()), ..ConnectionConfig::test_db() };
    if mysql {
        p.driver = "mysql".into();
    }
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("stg", Policy { commit: CommitRule::Manual, ..Policy::default() });
    cfg.policies.insert("plain", Policy::default());
    cfg
}

/// Connected to the test profile in environment `env` (MySQL's driver with `mysql`), in `lang`.
fn connected_in(env: &str, mysql: bool, lang: Lang) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(mysql, Ordering::SeqCst);
    let mut h = Harness::with_driver(&config(env, mysql), lang, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into(), "shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}

fn connected(env: &str, mysql: bool) -> Harness {
    connected_in(env, mysql, Lang::En)
}

/// Run all of `sql` (selected) with Ctrl+E.
fn run_all(h: &mut Harness, sql: &str) {
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
}

/// The runs sent since the last look: id, statements, rule.
fn executes(h: &mut Harness) -> Vec<(u64, Vec<String>, TxRule)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { id, statements, tx, .. } => Some((id, statements, tx)),
            _ => None,
        })
        .collect()
}

/// Run all of `sql`; the one run that was sent.
fn sent_run(h: &mut Harness, sql: &str) -> (u64, Vec<String>, TxRule) {
    run_all(h, sql);
    let mut runs = executes(h);
    assert_eq!(runs.len(), 1, "one run: {runs:?}");
    runs.remove(0)
}

fn manual(steps: &[TxStep], idle: Option<Duration>) -> TxRule {
    TxRule::Manual { steps: steps.to_vec(), idle_timeout: idle }
}

/// Run `sql` (one statement) and answer it with `affected` rows; statement 0 of the run opened
/// the transaction for manual commit when `opens`.
fn write(h: &mut Harness, sql: &str, affected: u64, opens: bool) -> u64 {
    let (id, ..) = sent_run(h, sql);
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(affected), elapsed: MS });
    if opens {
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.tab_db(0, DbEvent::Block(true));
        h.tab_db(0, DbEvent::TxOpen(true));
    }
    id
}

/// The block ends as the driver reports it.
fn end_block(h: &mut Harness) {
    h.tab_db(0, DbEvent::Block(false));
    h.tab_db(0, DbEvent::TxOpen(false));
}

fn ends(h: &mut Harness) -> Vec<(u64, bool)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::EndTx { id, commit } => Some((id, commit)),
            _ => None,
        })
        .collect()
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

fn notes(h: &Harness) -> Vec<Msg> {
    h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect()
}

fn keys() -> (String, String) {
    ("Space x c".into(), "Space x r".into())
}

// ── What each run says ──────────────────────────────────────────────────────

#[test]
fn a_manual_run_says_which_statements_open_the_transaction() {
    let mut h = connected("stg", false);
    let (_, statements, tx) = sent_run(
        &mut h,
        "INSERT INTO shop.users (name) VALUES ('a');\nCOMMIT;\nINSERT INTO shop.users (name) VALUES ('b');\nSELECT * FROM shop.users;",
    );
    assert_eq!(statements.len(), 4);
    assert_eq!(
        tx,
        manual(&[TxStep::Open, TxStep::AsIs, TxStep::Open, TxStep::ReadOnly], Some(THIRTY_MIN)),
        "the idle limit is PostgreSQL's 30m by default"
    );
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Closed, cancelled: true });
    // Reads, session settings, transaction control and EXPLAIN ANALYZE open nothing; what may
    // write does.
    for (sql, step) in [
        ("SELECT * FROM shop.users", TxStep::ReadOnly),
        ("SET search_path = shop", TxStep::AsIs),
        ("BEGIN", TxStep::AsIs),
        ("EXPLAIN ANALYZE DELETE FROM shop.users WHERE id = 1", TxStep::AsIs),
        ("SELECT * FROM shop.users FOR UPDATE", TxStep::Open),
        ("UPDATE shop.users SET name = 'x' WHERE id = 1", TxStep::Open),
        ("CREATE TABLE zz_t (id int)", TxStep::Open),
        ("SELECT nextval('shop.users_id_seq')", TxStep::Open),
    ] {
        let (id, _, tx) = sent_run(&mut h, sql);
        assert_eq!(tx, manual(&[step], Some(THIRTY_MIN)), "{sql}");
        h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("X".into()), elapsed: MS });
    }
}

#[test]
fn the_idle_limit_goes_with_postgresql_only_and_can_be_off() {
    let mut cfg = config("stg", false);
    cfg.policies.insert(
        "stg",
        Policy { commit: CommitRule::Manual, tx_idle_timeout: Some(Duration::from_secs(90)), ..Policy::default() },
    );
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    let (.., tx) = sent_run(&mut h, "DELETE FROM shop.users WHERE id = 1");
    assert_eq!(tx, manual(&[TxStep::Open], Some(Duration::from_secs(90))));

    let mut cfg = config("stg", false);
    cfg.policies.insert("stg", Policy { commit: CommitRule::Manual, tx_idle_timeout: None, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    let (.., tx) = sent_run(&mut h, "DELETE FROM shop.users WHERE id = 1");
    assert_eq!(tx, manual(&[TxStep::Open], None), "off: the server's own setting");

    // MySQL has no such limit: none is sent.
    let mut h = connected("stg", true);
    let (.., tx) = sent_run(&mut h, "INSERT INTO users (name) VALUES ('a'); SELECT 1;");
    assert_eq!(tx, manual(&[TxStep::Open, TxStep::ReadOnly], None));
}

#[test]
fn automatic_commit_and_drivers_that_cannot_commit_by_hand_send_auto() {
    let mut h = connected("plain", false);
    let (.., tx) = sent_run(&mut h, "INSERT INTO shop.users (name) VALUES ('a')");
    assert_eq!(tx, TxRule::Auto);
    // A driver without manual commit runs as before.
    let driver = FakeDriver::default();
    driver.no_manual.store(true, Ordering::SeqCst);
    let mut h = Harness::with_driver(&config("stg", false), Lang::En, driver);
    h.db(DbEvent::Connected);
    let (.., tx) = sent_run(&mut h, "INSERT INTO shop.users (name) VALUES ('a')");
    assert_eq!(tx, TxRule::Auto);
    assert!(!h.app.manual_commit(h.app.tab().id));
    // The default rules commit by themselves too.
    let mut h = Harness::connected(Lang::En);
    let (.., tx) = sent_run(&mut h, "INSERT INTO shop.users (name) VALUES ('a')");
    assert_eq!(tx, TxRule::Auto);
}

// ── Checks before a run ─────────────────────────────────────────────────────

#[test]
fn a_statement_that_cannot_run_in_a_transaction_asks_with_manual_commit() {
    let mut h = connected("stg", false);
    run_all(&mut h, "VACUUM shop.users");
    assert!(executes(&mut h).is_empty(), "nothing is sent before the answer");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
    let screen = h.screen(160, 45);
    assert!(screen.contains("runs outside a transaction, at once: can't be rolled back"), "{screen}");
    // Enter keeps it unrun.
    h.key(KeyCode::Enter);
    assert!(executes(&mut h).is_empty());
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyConfirmCancelled)));
    // y runs it on its own.
    run_all(&mut h, "VACUUM shop.users");
    h.keys("y");
    let runs = executes(&mut h);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].2, manual(&[TxStep::AsIs], Some(THIRTY_MIN)));
    // With automatic commit nothing asks.
    let mut h = connected("plain", false);
    run_all(&mut h, "VACUUM shop.users");
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(executes(&mut h).len(), 1);
}

#[test]
fn a_statement_that_cannot_run_in_a_transaction_is_refused_while_one_is_open() {
    // A transaction opened for manual commit.
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id = 1", 1, true);
    run_all(&mut h, "VACUUM shop.users");
    assert!(executes(&mut h).is_empty());
    assert_eq!(h.overlay_kind(), None, "refused, not asked");
    let (commit, rollback) = keys();
    let refused = Msg::TxNoTxBlockRefused { sql: "VACUUM shop.users".into(), commit, rollback };
    assert_eq!(status(&h), Some(refused.clone()));
    // The user's own BEGIN block, with automatic commit: refused too (the server would fail it
    // and abort the block).
    let mut h = connected("plain", false);
    run_all(&mut h, "BEGIN");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("BEGIN".into()), elapsed: MS });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    run_all(&mut h, "CREATE INDEX CONCURRENTLY zz_i ON shop.users (name)");
    assert!(executes(&mut h).is_empty());
    assert!(matches!(status(&h), Some(Msg::TxNoTxBlockRefused { .. })), "{:?}", status(&h));
    // In one run: after a write that opens the transaction, or after a BEGIN.
    let mut h = connected("stg", false);
    run_all(&mut h, "INSERT INTO shop.users (name) VALUES ('a');\nVACUUM shop.users;");
    assert!(executes(&mut h).is_empty());
    assert!(matches!(status(&h), Some(Msg::TxNoTxBlockRefused { .. })));
    let mut h = connected("plain", false);
    run_all(&mut h, "BEGIN;\nVACUUM shop.users;");
    assert!(executes(&mut h).is_empty());
    // After a COMMIT in the same run it may run (it asks, with manual commit).
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id = 1", 1, true);
    run_all(&mut h, "COMMIT;\nVACUUM shop.users;");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
    assert_eq!(h.app.overlays.run_confirm().unwrap().items.iter().map(|d| d.index).collect::<Vec<_>>(), [1]);
}

#[test]
fn mysql_asks_before_a_statement_commits_the_open_transaction() {
    let mut h = connected("stg", true);
    write(&mut h, "UPDATE users SET name = 'x' WHERE id < 4", 3, true);
    let (id, ..) = sent_run(&mut h, "INSERT INTO users (name) VALUES ('y')");
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(1), elapsed: MS });
    assert_eq!(h.app.tab().exec.tx.as_ref().map(|t| (t.changes, t.rows)), Some((2, 4)));
    run_all(&mut h, "ALTER TABLE users ADD COLUMN zz_c int");
    assert!(executes(&mut h).is_empty(), "asked first");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
    let screen = h.screen(160, 45);
    assert!(screen.contains("first commits the open transaction: 2 statements (4 rows)"), "{screen}");
    h.key(KeyCode::Esc);
    assert!(executes(&mut h).is_empty());
    run_all(&mut h, "ALTER TABLE users ADD COLUMN zz_c int");
    h.keys("y");
    assert_eq!(executes(&mut h).len(), 1);
    // The note of automatic commit is not said again in its Messages.
    assert!(!notes(&h).iter().any(|m| matches!(m, Msg::QueryImplicitCommit { .. })), "{:?}", notes(&h));
    // With automatic commit it is only noted, as before.
    let mut h = connected("plain", true);
    run_all(&mut h, "BEGIN");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("BEGIN".into()), elapsed: MS });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    run_all(&mut h, "ALTER TABLE users ADD COLUMN zz_c int");
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(executes(&mut h).len(), 1);
    assert!(notes(&h).iter().any(|m| matches!(m, Msg::QueryImplicitCommit { .. })));
}

/// The count of what an implicit commit commits starts over where a statement of the same run
/// ends the transaction.
#[test]
fn the_implicit_commit_counts_only_the_transaction_it_ends() {
    let reason = |h: &Harness| match h.app.overlays.run_confirm().map(|c| c.items.clone()) {
        Some(items) => items.into_iter().filter_map(|d| d.tx.map(|t| (d.index, t))).collect::<Vec<_>>(),
        None => Vec::new(),
    };
    use datarig_tui::app::tx::TxReason::ImplicitCommit;
    let open = || {
        let mut h = connected("stg", true);
        write(&mut h, "UPDATE users SET name = 'x' WHERE id < 4", 3, true);
        h
    };
    // COMMIT; INSERT; ALTER: the ALTER commits the INSERT only.
    let mut h = open();
    run_all(
        &mut h,
        "COMMIT;
INSERT INTO users (name) VALUES ('a');
ALTER TABLE users ADD COLUMN zz_c int;",
    );
    assert_eq!(reason(&h), [(2, ImplicitCommit { statements: 1, rows: 0 })]);
    // ALTER; INSERT; ALTER: the first commits what was open, the second the INSERT.
    let mut h = open();
    run_all(
        &mut h,
        "ALTER TABLE users ADD COLUMN zz_c int;
INSERT INTO users (name) VALUES ('a');
ALTER TABLE users DROP COLUMN zz_c;",
    );
    let r = reason(&h);
    assert_eq!(r[0], (0, ImplicitCommit { statements: 1, rows: 3 }));
    assert_eq!(r[1], (2, ImplicitCommit { statements: 1, rows: 0 }));
}

#[test]
fn mysql_ddl_without_a_transaction_is_noted_and_lock_tables_is_refused() {
    let mut h = connected("stg", true);
    run_all(&mut h, "CREATE TABLE zz_t (id int)");
    assert_eq!(h.overlay_kind(), None, "nothing to commit: no question");
    assert_eq!(executes(&mut h).len(), 1);
    assert!(notes(&h).contains(&Msg::TxMysqlDdl { step: "1".into() }), "{:?}", notes(&h));
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("CREATE TABLE".into()), elapsed: MS });
    run_all(&mut h, "LOCK TABLES users WRITE");
    assert!(executes(&mut h).is_empty());
    assert_eq!(status(&h), Some(Msg::Label(Label::TxLockTablesRefused)));
    // With automatic commit LOCK TABLES runs as before (it asks: it locks other sessions out).
    let mut h = connected("plain", true);
    run_all(&mut h, "LOCK TABLES users WRITE");
    assert_ne!(status(&h), Some(Msg::Label(Label::TxLockTablesRefused)));
}

#[test]
fn an_enum_value_added_in_a_transaction_is_noted() {
    let mut h = connected("stg", false);
    run_all(&mut h, "ALTER TYPE shop.mood ADD VALUE 'meh'");
    assert_eq!(executes(&mut h).len(), 1);
    assert!(notes(&h).contains(&Msg::TxEnumValue { step: "1".into() }), "{:?}", notes(&h));
    // Outside a transaction (automatic commit) it is usable at once: nothing to say.
    let mut h = connected("plain", false);
    run_all(&mut h, "ALTER TYPE shop.mood ADD VALUE 'meh'");
    assert!(!notes(&h).iter().any(|m| matches!(m, Msg::TxEnumValue { .. })));
}

#[test]
fn transaction_reasons_snapshots() {
    for lang in [Lang::En, Lang::Ko] {
        let code = if lang == Lang::En { "en" } else { "ko" };
        let mut h = connected_in("stg", false, lang);
        run_all(&mut h, "VACUUM shop.users;\nDROP TABLE shop.reviews;");
        assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
        for (w, hgt) in [(80, 24), (160, 45)] {
            assert_screen!(format!("manual_outside_tx_confirm_{code}_{w}x{hgt}"), lang, h.draw(w, hgt));
        }
        let mut h = connected_in("stg", true, lang);
        write(&mut h, "UPDATE users SET name = 'x' WHERE id < 4", 3, true);
        run_all(&mut h, "ALTER TABLE users ADD COLUMN zz_c int");
        assert_screen!(format!("manual_implicit_commit_confirm_{code}_80x24"), lang, h.draw(80, 24));
    }
}

// ── The open transaction ────────────────────────────────────────────────────

#[test]
fn the_status_bar_counts_what_the_open_transaction_did_and_for_how_long() {
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    assert!(h.status(160, 45).contains("TX open · 1 change · 3 rows · 0:00"), "{}", h.status(160, 45));
    assert!(notes(&h).contains(&Msg::TxBegan { step: "1".into(), commit: keys().0, rollback: keys().1 }));
    // Reads in it change nothing; another write adds.
    let (id, ..) = sent_run(&mut h, "SELECT * FROM shop.users");
    h.tab_db(
        0,
        DbEvent::Page {
            id,
            columns: Some(vec![meta("x", "int4", true, false)]),
            rows: vec![],
            more: false,
            elapsed: MS,
        },
    );
    write(&mut h, "DELETE FROM shop.orders WHERE id = 7", 1, false);
    h.advance(Duration::from_secs(4 * 60 + 12));
    assert!(h.status(160, 45).contains("TX open · 2 changes · 4 rows · 4:12"), "{}", h.status(160, 45));
    insta::assert_snapshot!("manual_tx_status_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("manual_tx_status_en_80x24", h.draw(80, 24).backend());
    // An hour on.
    h.advance(Duration::from_secs(3600));
    assert!(h.status(160, 45).contains("· 1:04:12"), "{}", h.status(160, 45));
    // Korean: from the catalog.
    let mut h = connected_in("stg", false, Lang::Ko);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    let i18n = I18n::new(Lang::Ko);
    let expected = format!(
        "{} · {} · {} · 0:00",
        i18n.label(Label::StatusTxOpen),
        i18n.msg(&Msg::TxChanges { count: 1 }),
        i18n.msg(&Msg::TxRows { count: 3 })
    );
    assert!(h.status(160, 45).contains(&expected), "{}", h.status(160, 45));
}

#[test]
fn a_failed_or_undone_run_leaves_the_log() {
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    // A run of two that failed at its second: the first is booked, then the session rolls the
    // run back to its savepoint.
    run_all(&mut h, "DELETE FROM shop.orders WHERE id = 1;\nINSERT INTO shop.users VALUES (1);");
    let (id, ..) = executes(&mut h).remove(0);
    h.tab_db(0, DbEvent::Started { id, index: 0 });
    h.tab_db(0, DbEvent::Finished { id, index: 0, outcome: Outcome::Affected(1), elapsed: MS });
    h.tab_db(0, DbEvent::Started { id, index: 1 });
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("duplicate key".into()), cancelled: false });
    assert_eq!(h.app.tab().exec.tx.as_ref().map(|t| t.changes), Some(2));
    h.tab_db(0, DbEvent::RunRolledBack { id });
    let tx = h.app.tab().exec.tx.clone().unwrap();
    assert_eq!((tx.changes, tx.rows, tx.log.len()), (1, 3, 1), "only the first run's change");
    assert!(notes(&h).contains(&Msg::Label(Label::TxRunRolledBack)));
    // The log keeps the last 200; the counts go on.
    for i in 0..205 {
        write(&mut h, &format!("UPDATE shop.users SET name = 'x' WHERE id = {i}"), 1, false);
    }
    let tx = h.app.tab().exec.tx.clone().unwrap();
    assert_eq!((tx.changes, tx.rows, tx.log.len()), (206, 208, 200));
    assert_eq!(tx.log.back().unwrap().sql, "UPDATE shop.users SET name = 'x' WHERE id = 204");
}

#[test]
fn a_first_write_that_fails_leaves_no_transaction() {
    let mut h = connected("stg", false);
    let (id, ..) = sent_run(&mut h, "INSERT INTO shop.users VALUES (1)");
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("duplicate key".into()), cancelled: false });
    h.tab_db(0, DbEvent::ManualRolledBack { id, index: 0 });
    assert!(notes(&h).contains(&Msg::TxManualRolledBack { step: "1".into() }), "{:?}", notes(&h));
    assert!(h.app.tab().exec.tx.is_none() && !h.app.tab().exec.user_tx());
    assert!(!h.status(160, 45).contains("TX"));
    // Nothing of it stays for a later block: the user's BEGIN … COMMIT ends as committed.
    assert_eq!(h.app.tab().exec.closing, None);
}

#[test]
fn space_x_c_commits_and_says_what_was_committed() {
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.advance(Duration::from_secs(5));
    h.keys(" xc");
    let sent = ends(&mut h);
    assert_eq!(sent.len(), 1);
    let (id, commit) = sent[0];
    assert!(commit);
    assert_eq!(status(&h), Some(Msg::Label(Label::TxCommitting)));
    // Nothing runs meanwhile.
    run_all(&mut h, "SELECT 1");
    assert!(executes(&mut h).is_empty());
    assert!(matches!(h.app.transient.as_ref().map(|t| &t.0.msg), Some(Msg::QueryBusy { .. })));
    h.tab_db(0, DbEvent::TxEnded { id, outcome: TxOutcome::Committed });
    end_block(&mut h);
    let done = Msg::TxCommitted { count: 1, rows: "3 rows".into(), time: "0:05".into() };
    assert_eq!(status(&h), Some(done.clone()));
    assert!(notes(&h).contains(&done), "and in Messages");
    assert!(!h.status(160, 45).contains("TX open"));
    assert!(h.app.tab().exec.tx.is_none());
    // Nothing open: Space x c says so, sends nothing.
    h.app.transient = None;
    h.keys(" xc");
    assert!(ends(&mut h).is_empty());
    assert_eq!(h.app.transient.as_ref().map(|t| t.0.msg.clone()), Some(Msg::Label(Label::TxNone)));
}

#[test]
fn rollback_by_key_and_by_command_and_what_a_rollback_instead_says() {
    for how in [" xr", ":rollback", ":rb"] {
        let mut h = connected("stg", false);
        write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
        match how.strip_prefix(':') {
            Some(c) => h.command(c),
            None => h.keys(how),
        }
        let sent = ends(&mut h);
        assert_eq!(sent.len(), 1, "{how}");
        assert!(!sent[0].1, "{how}: a rollback");
        h.tab_db(0, DbEvent::TxEnded { id: sent[0].0, outcome: TxOutcome::RolledBack });
        end_block(&mut h);
        assert_eq!(status(&h), Some(Msg::TxRolledBack { count: 1, time: "0:00".into() }), "{how}");
    }
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.command("commit");
    let (id, commit) = ends(&mut h)[0];
    assert!(commit);
    h.tab_db(0, DbEvent::TxEnded { id, outcome: TxOutcome::RolledBackInstead });
    end_block(&mut h);
    assert_eq!(status(&h), Some(Msg::Label(Label::TxRolledBackInstead)), "never shown as committed");
}

#[test]
fn an_aborted_transaction_can_only_be_rolled_back() {
    let mut h = connected("plain", false);
    run_all(&mut h, "BEGIN");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("BEGIN".into()), elapsed: MS });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.tab_db(0, DbEvent::TxAborted(true));
    assert!(h.status(160, 45).contains("TX aborted — ROLLBACK required"), "unchanged");
    h.keys(" xc");
    assert!(ends(&mut h).is_empty());
    let refused = Msg::TxOnlyRollback { key: "Space x r".into() };
    assert_eq!(h.app.transient.as_ref().map(|t| t.0.msg.clone()), Some(refused));
    // The user's own block (automatic commit) is rolled back by the key.
    h.keys(" xr");
    assert_eq!(ends(&mut h).len(), 1);
}

#[test]
fn a_commit_that_fails_or_whose_outcome_is_unknown_says_so() {
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.keys(" xc");
    let (id, _) = ends(&mut h)[0];
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("deferred constraint".into()), cancelled: false });
    assert_eq!(status(&h), Some(Msg::TxCommitFailed { error: "deferred constraint".into() }));
    end_block(&mut h);
    assert!(h.app.tab().exec.ending.is_none() && h.app.tab().exec.tx.is_none());

    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.keys(" xc");
    let (id, _) = ends(&mut h)[0];
    h.tab_db(0, DbEvent::Failed { id, error: DbError::CommitUnknown, cancelled: false });
    assert_eq!(status(&h), Some(Msg::Label(Label::DbCommitUnknown)));
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(!h.app.tab_busy(h.app.tab().id), "nothing waits after the connection is gone");
    // Only that the outcome is unknown: never that the server rolled it back, nor (on the next
    // run) that the transaction is gone.
    assert_eq!(status(&h), Some(Msg::Label(Label::DbCommitUnknown)), "the status stays");
    assert!(!notes(&h).contains(&Msg::Label(Label::TxLost)), "{:?}", notes(&h));
    assert!(notes(&h).iter().any(|m| matches!(m, Msg::QueryLost { .. })), "{:?}", notes(&h));
    assert!(!h.app.tab().exec.lost_tx && !h.app.tab().exec.user_tx() && h.app.tab().exec.tx.is_none());
    let screen = h.screen(160, 45);
    assert!(!screen.contains("rolled back") && !screen.contains("rolls back"), "{screen}");
    run_all(&mut h, "SELECT 1");
    assert_ne!(
        h.app.transient.as_ref().map(|t| t.0.msg.clone()),
        Some(Msg::Label(Label::QueryReconnectedTx)),
        "the next run does not say the transaction is gone"
    );
}

#[test]
fn the_server_ending_the_transaction_is_said() {
    // MySQL's deadlock: the whole transaction is gone.
    let mut h = connected("stg", true);
    write(&mut h, "UPDATE users SET name = 'x' WHERE id < 4", 3, true);
    let (id, ..) = sent_run(&mut h, "UPDATE users SET name = 'y' WHERE id = 9");
    h.tab_db(0, DbEvent::TxClosedBy { id, how: ServerEnd::RolledBack });
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("Deadlock found".into()), cancelled: false });
    end_block(&mut h);
    assert!(notes(&h).contains(&Msg::TxServerRolledBack { count: 1 }), "{:?}", notes(&h));
    // An implicit commit (asked for first).
    let mut h = connected("stg", true);
    write(&mut h, "UPDATE users SET name = 'x' WHERE id < 4", 3, true);
    run_all(&mut h, "ALTER TABLE users ADD COLUMN zz_c int");
    h.keys("y");
    let (id, ..) = executes(&mut h).remove(0);
    h.tab_db(0, DbEvent::TxClosedBy { id, how: ServerEnd::Committed });
    end_block(&mut h);
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("ALTER TABLE".into()), elapsed: MS });
    assert!(notes(&h).contains(&Msg::TxCommittedImplicitly { step: "1".into(), count: 1 }), "{:?}", notes(&h));
    // PostgreSQL's idle limit ended the session.
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    let error = "terminating connection due to idle-in-transaction timeout";
    h.tab_db(0, DbEvent::Lost { error: DbError::IdleInTransactionTimeout(error.into()) });
    assert_eq!(status(&h), Some(Msg::TxIdleTimeout { error: error.into() }));
    assert!(!h.app.tab().exec.user_tx() && h.app.tab().exec.tx.is_none());
    // Another loss with a transaction open: the server rolls it back, said in Messages.
    let mut h = connected("stg", false);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(notes(&h).contains(&Msg::Label(Label::TxLost)));
}

#[test]
fn autocommit_turned_off_shows_in_the_status_bar() {
    let mut h = connected("plain", true);
    let (id, ..) = sent_run(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
    // The user's `SET autocommit = 0`, as the session reports it.
    h.tab_db(0, DbEvent::Autocommit(false));
    assert!(h.status(160, 45).contains("autocommit off (session)"), "{}", h.status(160, 45));
    insta::assert_snapshot!("manual_autocommit_off_en_80x24", h.draw(80, 24).backend());
    h.tab_db(0, DbEvent::Autocommit(true));
    assert!(!h.status(160, 45).contains("autocommit off"));
}

#[test]
fn a_procedure_that_commits_by_itself_can_run_outside_a_transaction() {
    let mut h = connected("stg", false);
    run_all(&mut h, "CALL shop.archive()");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm), "a CALL asks: it runs code");
    h.keys("y");
    let (id2, _, tx) = executes(&mut h).pop().unwrap();
    assert_eq!(tx, manual(&[TxStep::Open], Some(THIRTY_MIN)));
    let error = "invalid transaction termination";
    h.tab_db(0, DbEvent::Failed { id: id2, error: DbError::NeedsNoTransaction(error.into()), cancelled: false });
    assert_eq!(h.overlay_kind(), None, "asked once the transaction is rolled back");
    h.tab_db(0, DbEvent::ManualRolledBack { id: id2, index: 0 });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
    // As the run confirmation: the connection with its environment, the statement, the reason.
    let screen = h.screen(100, 30);
    assert!(screen.contains("Run outside a transaction?"), "{screen}");
    assert!(screen.contains("● local-pg · stg"), "{screen}");
    assert!(screen.contains("commits by itself: runs outside, at once; can't be rolled back"), "{screen}");
    assert!(screen.contains("     CALL shop.archive()"), "{screen}");
    insta::assert_snapshot!("manual_outside_procedure_en_80x24", h.draw(80, 24).backend());
    // Enter keeps it unrun.
    h.key(KeyCode::Enter);
    assert!(executes(&mut h).is_empty());
    assert_eq!(status(&h), Some(Msg::Label(Label::TxOutsideKept)));
    // Asked again: y runs it once as it is.
    run_all(&mut h, "CALL shop.archive()");
    h.keys("y");
    let (id3, ..) = executes(&mut h).pop().unwrap();
    h.tab_db(0, DbEvent::Failed { id: id3, error: DbError::NeedsNoTransaction(error.into()), cancelled: false });
    h.tab_db(0, DbEvent::ManualRolledBack { id: id3, index: 0 });
    h.keys("y");
    let runs = executes(&mut h);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1, ["CALL shop.archive()"]);
    assert_eq!(runs[0].2, manual(&[TxStep::AsIs], Some(THIRTY_MIN)));
    // The next run of it opens a transaction again.
    h.tab_db(0, DbEvent::Done { id: runs[0].0, outcome: Outcome::Command("CALL".into()), elapsed: MS });
    run_all(&mut h, "CALL shop.archive()");
    h.keys("y");
    let (id4, _, tx) = executes(&mut h).pop().unwrap();
    assert_eq!(tx, manual(&[TxStep::Open], Some(THIRTY_MIN)));
    // Asked, then the connection is lost: y runs nothing (not on the next session either).
    h.tab_db(0, DbEvent::Failed { id: id4, error: DbError::NeedsNoTransaction(error.into()), cancelled: false });
    h.tab_db(0, DbEvent::ManualRolledBack { id: id4, index: 0 });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm));
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    h.keys("y");
    assert!(executes(&mut h).is_empty(), "stale: nothing runs");
    assert_eq!(status(&h), Some(Msg::Label(Label::SafetyConfirmMoved)));
    assert!(h.app.tab().exec.session.is_none(), "no new session was opened for it");
}

#[test]
fn the_summary_and_the_menus() {
    let mut h = connected("stg", false);
    h.command("tx");
    assert_eq!(h.app.transient.as_ref().map(|t| t.0.msg.clone()), Some(Msg::Label(Label::TxNone)));
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.advance(Duration::from_secs(75));
    h.keys(" xs");
    let summary = Msg::TxSummary { how: "manual commit".into(), count: 1, rows: "3 rows".into(), time: "1:15".into() };
    assert_eq!(status(&h), Some(summary));
    // The tab's menu has them while a transaction is open.
    h.draw(160, 45);
    h.app.dispatch(Action::TabMenu);
    let labels = h.menu_labels();
    for l in ["Commit the open transaction", "Roll back the open transaction", "What the open transaction did"] {
        assert!(labels.iter().any(|x| x == l), "{l}: {labels:?}");
    }
    h.menu_pick("Commit the open transaction");
    assert_eq!(ends(&mut h).len(), 1);
    // No transaction: not in the menu.
    let mut h = connected("stg", false);
    h.draw(160, 45);
    h.app.dispatch(Action::TabMenu);
    assert!(!h.menu_labels().iter().any(|x| x.contains("open transaction")), "{:?}", h.menu_labels());
}

// ── Timers ──────────────────────────────────────────────────────────────────

#[test]
fn the_transaction_time_wakes_the_app_once_a_second_only_while_shown() {
    let mut h = connected("stg", false);
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), None, "nothing open: no timer");
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3, true);
    h.app.transient = None;
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), Some(now + SEC), "the next whole second");
    h.advance(Duration::from_millis(400));
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), Some(now + Duration::from_millis(600)));
    // Over ten minutes: one wakeup a second, never more.
    let mut ticks = 0;
    let end = h.clock.now() + Duration::from_secs(600);
    while let Some(next) = h.app.next_tick(h.clock.now()) {
        if next > end {
            break;
        }
        h.clock.advance(next - h.clock.now());
        h.app.on_tick(h.clock.now());
        ticks += 1;
    }
    assert!((600..=601).contains(&ticks), "{ticks}");
    // Another tab without one is shown: no timer.
    h.ctrl('t');
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None);
    // Back, then the transaction ends: no timer.
    h.app.dispatch(Action::PrevTab);
    h.app.transient = None;
    assert!(h.app.next_tick(h.clock.now()).is_some());
    h.keys(" xc");
    let (id, _) = ends(&mut h)[0];
    h.tab_db(0, DbEvent::TxEnded { id, outcome: TxOutcome::Committed });
    end_block(&mut h);
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None);
}

#[test]
fn the_environment_says_manual_commit_in_its_rules() {
    use datarig_tui::app::environments::rules_tones;
    let i18n = I18n::new(Lang::En);
    let manual = Policy { commit: CommitRule::Manual, ..Policy::default() };
    let words: String = rules_tones(&i18n, &manual, false).into_iter().map(|(t, _)| t).collect();
    assert_eq!(words, "manual commit");
    let staging = Policy { confirm: datarig_core::policy::Confirm::Writes, ..manual.clone() };
    let words: String = rules_tones(&i18n, &staging, false).into_iter().map(|(t, _)| t).collect();
    assert_eq!(words, "asks before every write · manual commit");
    // Read-only: nothing writes, so no transaction starts.
    let ro = Policy { read_only: true, ..manual };
    let words: String = rules_tones(&i18n, &ro, false).into_iter().map(|(t, _)| t).collect();
    assert_eq!(words, "read-only");
}
