//! An open transaction meets what would end its session: closing the tab, disconnecting,
//! switching the connection or the database, deleting the profile, quitting. The question
//! offers to commit (`c`) or roll back (`r`) first, and goes on only once the server confirmed
//! each; anything else stops it. The idle warning, the explorer's mark, and the notes about
//! locks and rule changes.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Outcome, TxOutcome};
use datarig_core::i18n::{I18n, Label, Lang, Msg};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::action::Action;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

const MS: Duration = Duration::from_millis(3);

/// The test profile in environment `stg` (manual commit; idle warning `warning`).
fn config(warning: Option<Duration>) -> Config {
    let p = ConnectionConfig { policy: Some("stg".to_string()), ..ConnectionConfig::test_db() };
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("stg", Policy { commit: CommitRule::Manual, tx_idle_warning: warning, ..Policy::default() });
    cfg
}

fn connected_in(lang: Lang, warning: Option<Duration>) -> Harness {
    let mut h = Harness::with_config(&config(warning), lang);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into(), "shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}

fn connected() -> Harness {
    connected_in(Lang::En, Some(Duration::from_secs(300)))
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

/// A write in the active tab that opens the transaction (`affected` rows).
fn write(h: &mut Harness, sql: &str, affected: u64) {
    let i = h.app.tabs.active_index();
    let id = run(h, sql);
    h.tab_db(i, DbEvent::Done { id, outcome: Outcome::Affected(affected), elapsed: MS });
    if !h.app.tab().exec.user_tx() {
        h.tab_db(i, DbEvent::ManualBegun { id, index: 0 });
        h.tab_db(i, DbEvent::Block(true));
        h.tab_db(i, DbEvent::TxOpen(true));
    }
}

/// The `EndTx` requests sent since the last look, with the session they went to.
fn ends(h: &mut Harness) -> Vec<(u64, bool)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::EndTx { id, commit } => Some((id, commit)),
            _ => None,
        })
        .collect()
}

/// Tab `i`'s session answers its `EndTx` `id` with `outcome`, and the block ends.
fn ended(h: &mut Harness, i: usize, id: u64, outcome: TxOutcome) {
    let tabs = h.app.tabs.len();
    h.tab_db(i, DbEvent::TxEnded { id, outcome });
    h.tab_db(i, DbEvent::Block(false));
    // The answer may have gone on already (the tab closed).
    if h.app.tabs.len() == tabs {
        h.tab_db(i, DbEvent::TxOpen(false));
    }
}

fn buttons(h: &Harness) -> Vec<Label> {
    h.app.overlays.confirm().map(|c| c.buttons().0.into_iter().map(|b| b.0).collect()).unwrap_or_default()
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

#[test]
fn closing_a_tab_commits_or_rolls_back_first_and_closes_once_the_server_said() {
    for (key, commit, outcome) in [("c", true, TxOutcome::Committed), ("r", false, TxOutcome::RolledBack)] {
        let mut h = connected();
        h.ctrl('t');
        write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
        h.advance(Duration::from_secs(12));
        let tabs = h.app.tabs.len();
        h.ctrl('w');
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
        assert_eq!(
            buttons(&h),
            [Label::DialogButtonKeep, Label::TxButtonRollback, Label::TxButtonCommit, Label::DialogButtonClose]
        );
        let screen = h.screen(100, 30);
        assert!(screen.contains("1 change (3 rows) uncommitted for 0:12"), "{screen}");
        assert!(screen.contains("c commit first · r roll back first · y go on · Enter/n stay"), "{screen}");
        // Enter stays.
        h.key(KeyCode::Enter);
        assert_eq!((h.overlay_kind(), h.app.tabs.len()), (None, tabs));
        h.ctrl('w');
        h.keys(key);
        assert_eq!(h.overlay_kind(), None);
        let sent = ends(&mut h);
        assert_eq!(sent.len(), 1, "{key}");
        assert_eq!(sent[0].1, commit);
        assert_eq!(h.app.tabs.len(), tabs, "{key}: nothing closes before the server answered");
        let i = h.app.tabs.active_index();
        h.tab_db(i, DbEvent::TxEnded { id: sent[0].0, outcome });
        h.tab_db(i, DbEvent::Block(false));
        assert_eq!(h.app.tabs.len(), tabs, "{key}: not before the session says no transaction is open");
        h.tab_db(i, DbEvent::TxOpen(false));
        assert_eq!(h.app.tabs.len(), tabs - 1, "{key}: closed once it ended");
        // Closed with nothing open: it does not say a transaction was rolled back.
        let said = h.app.status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
        assert!(!said.contains("rolled back"), "{key}: {said}");
    }
}

#[test]
fn a_commit_that_does_not_happen_stops_and_the_tab_stays() {
    // Refused by the server.
    let mut h = connected();
    h.ctrl('t');
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    let tabs = h.app.tabs.len();
    h.ctrl('w');
    h.keys("c");
    let (id, _) = ends(&mut h)[0];
    let i = h.app.tabs.active_index();
    h.tab_db(i, DbEvent::Failed { id, error: DbError::Server("deferred constraint".into()), cancelled: false });
    h.tab_db(i, DbEvent::Block(false));
    h.tab_db(i, DbEvent::TxOpen(false));
    assert_eq!(h.app.tabs.len(), tabs, "the tab stays");
    assert_eq!(status(&h), Some(Msg::TxEndStopped { tab: (i + 1).to_string() }));
    let notes: Vec<Msg> = h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect();
    assert!(notes.contains(&Msg::TxCommitFailed { error: "deferred constraint".into() }), "{notes:?}");
    // Rolled back instead of committed: not what was asked, it stops too.
    let mut h = connected();
    h.ctrl('t');
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    let tabs = h.app.tabs.len();
    h.ctrl('w');
    h.keys("c");
    let (id, _) = ends(&mut h)[0];
    let i = h.app.tabs.active_index();
    ended(&mut h, i, id, TxOutcome::RolledBackInstead);
    assert_eq!(h.app.tabs.len(), tabs);
    assert!(matches!(status(&h), Some(Msg::TxEndStopped { .. })));
    // The connection drops while committing: unknown, it stops.
    let mut h = connected();
    h.ctrl('t');
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    let tabs = h.app.tabs.len();
    h.ctrl('w');
    h.keys("c");
    let (id, _) = ends(&mut h)[0];
    let i = h.app.tabs.active_index();
    h.tab_db(i, DbEvent::Failed { id, error: DbError::CommitUnknown, cancelled: false });
    assert!(matches!(status(&h), Some(Msg::TxEndStopped { .. })));
    h.tab_db(i, DbEvent::Lost { error: DbError::Closed });
    assert_eq!(h.app.tabs.len(), tabs, "the tab stays");
    assert_eq!(status(&h), Some(Msg::Label(Label::DbCommitUnknown)), "what matters is that it is not known");
    assert!(ends(&mut h).is_empty());
}

#[test]
fn a_failed_transaction_offers_only_rollback_and_running_statements_keep_todays_question() {
    let mut h = connected();
    h.ctrl('t');
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    let i = h.app.tabs.active_index();
    h.tab_db(i, DbEvent::TxAborted(true));
    h.ctrl('w');
    assert_eq!(buttons(&h), [Label::DialogButtonKeep, Label::TxButtonRollback, Label::DialogButtonClose]);
    assert!(h.screen(100, 30).contains("r roll back first · y go on · Enter/n stay"));
    h.keys("c");
    assert!(ends(&mut h).is_empty(), "no commit of a failed transaction");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.key(KeyCode::Esc);
    // A statement runs: cancelling comes first; the question is today's.
    h.tab_db(i, DbEvent::TxAborted(false));
    run(&mut h, "UPDATE shop.users SET name = 'y' WHERE id = 1");
    h.ctrl('w');
    assert_eq!(buttons(&h), [Label::DialogButtonKeep, Label::DialogButtonClose]);
}

#[test]
fn quitting_commits_every_open_transaction_one_by_one_then_quits() {
    let mut h = connected();
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.ctrl('t');
    write(&mut h, "DELETE FROM shop.orders WHERE id = 9", 1);
    h.command("qa");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let screen = h.screen(100, 30);
    assert!(screen.contains("Tab 1: 1 change (3 rows) uncommitted for 0:00"), "{screen}");
    assert!(screen.contains("Tab 2: 1 change (1 row) uncommitted for 0:00"), "{screen}");
    for lang in [Lang::En, Lang::Ko] {
        let mut q = connected_in(lang, Some(Duration::from_secs(300)));
        write(&mut q, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
        q.ctrl('t');
        write(&mut q, "DELETE FROM shop.orders WHERE id = 9", 1);
        q.command("qa");
        let code = if lang == Lang::En { "en" } else { "ko" };
        assert_screen!(format!("tx_quit_choices_{code}_80x24"), lang, q.draw(80, 24));
        assert_screen!(format!("tx_quit_choices_{code}_160x45"), lang, q.draw(160, 45));
    }
    h.keys("c");
    // One at a time: the first tab's first.
    let first = ends(&mut h);
    assert_eq!(first.len(), 1);
    ended(&mut h, 0, first[0].0, TxOutcome::Committed);
    assert!(!h.app.quit, "not before every one ended");
    let second = ends(&mut h);
    assert_eq!(second.len(), 1);
    ended(&mut h, 1, second[0].0, TxOutcome::Committed);
    assert!(h.app.quit, "quit once both committed");

    // The second fails: it stops there, nothing quits.
    let mut h = connected();
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.ctrl('t');
    write(&mut h, "DELETE FROM shop.orders WHERE id = 9", 1);
    h.command("qa");
    h.keys("c");
    let first = ends(&mut h);
    ended(&mut h, 0, first[0].0, TxOutcome::Committed);
    let second = ends(&mut h);
    h.tab_db(1, DbEvent::Failed { id: second[0].0, error: DbError::Server("serialization".into()), cancelled: false });
    assert!(!h.app.quit);
    assert_eq!(status(&h), Some(Msg::TxEndStopped { tab: "2".into() }));
}

#[test]
fn disconnect_rolls_back_first_then_disconnects() {
    let mut h = connected();
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.app.dispatch(Action::DisconnectCurrent);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(buttons(&h).contains(&Label::TxButtonRollback));
    h.keys("r");
    let sent = ends(&mut h);
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].1);
    assert!(h.app.tab().exec.session.is_some(), "not before the server answered");
    ended(&mut h, 0, sent[0].0, TxOutcome::RolledBack);
    assert!(h.app.tab().exec.session.is_none(), "disconnected once it rolled back");
    assert_eq!(h.app.conns.state(h.app.profiles[0].id), datarig_tui::app::NodeState::Disconnected);
}

#[test]
fn switching_the_connection_or_the_database_offers_it_too() {
    let mut h = connected();
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.app.dispatch(Action::SetTabConnection);
    assert!(buttons(&h).contains(&Label::TxButtonCommit), "{:?}", buttons(&h));
    h.keys("c");
    let sent = ends(&mut h);
    ended(&mut h, 0, sent[0].0, TxOutcome::Committed);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect), "then the switch goes on");
}

#[test]
fn an_idle_transaction_is_warned_about_once_per_idle_period() {
    let mut h = connected_in(Lang::En, Some(Duration::from_secs(120)));
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    // The deadline is the only timer besides the status bar's second.
    let now = h.clock.now();
    h.ctrl('t');
    h.app.transient = None;
    assert_eq!(h.app.next_tick(now), Some(now + Duration::from_secs(120)), "another tab active: the deadline only");
    h.app.dispatch(Action::PrevTab);
    h.app.transient = None;
    h.advance(Duration::from_secs(119));
    assert!(!h.app.tab().exec.tx.as_ref().unwrap().warned);
    h.advance(Duration::from_secs(1));
    let tx = h.app.tab().exec.tx.clone().unwrap();
    assert!(tx.warned);
    let warning = Msg::TxIdleWarning {
        tab: "1".into(),
        time: "2:00".into(),
        commit: "Space x c".into(),
        rollback: "Space x r".into(),
    };
    assert_eq!(status(&h), Some(warning.clone()));
    let status_bar = h.status(160, 45);
    assert!(status_bar.contains("idle 2:00 · holding locks"), "{status_bar}");
    // A warning shorter than a minute shows the idle time too.
    let mut q = connected_in(Lang::En, Some(Duration::from_secs(10)));
    write(&mut q, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    q.advance(Duration::from_secs(10));
    assert!(q.status(160, 45).contains("idle 0:10 · holding locks"), "{}", q.status(160, 45));
    assert!(h.explorer_line().contains('◆') || h.screen(160, 45).contains("local-pg ◆"));
    insta::assert_snapshot!("tx_idle_warned_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("tx_idle_warned_en_80x24", h.draw(80, 24).backend());
    // Once per idle period.
    h.app.status = None;
    h.advance(Duration::from_secs(600));
    assert_eq!(status(&h), None);
    let count =
        |h: &Harness| h.app.tab().exec.run.notes.iter().filter(|n| matches!(n.msg, Msg::TxIdleWarning { .. })).count();
    assert_eq!(count(&h), 1);
    // A statement starts a new idle period.
    write(&mut h, "UPDATE shop.users SET name = 'y' WHERE id = 1", 1);
    assert!(!h.app.tab().exec.tx.as_ref().unwrap().warned);
    h.advance(Duration::from_secs(121));
    assert!(h.app.tab().exec.tx.as_ref().unwrap().warned, "warned again");
    // Ended: no timer at all.
    h.keys(" xc");
    let sent = ends(&mut h);
    ended(&mut h, 0, sent[0].0, TxOutcome::Committed);
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None);
    // `off`: no deadline.
    let mut h = connected_in(Lang::En, None);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.ctrl('t');
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None, "no warning: no wakeup from another tab");
}

#[test]
fn the_explorer_marks_a_profile_with_an_open_transaction() {
    let mut h = connected();
    assert!(!h.screen(160, 45).contains("local-pg ◆"));
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    assert!(h.screen(160, 45).contains("local-pg ◆"), "{}", h.screen(160, 45));
    // With icons off the same mark.
    h.app.icons = datarig_core::config::IconsSetting::Off;
    assert!(h.screen(160, 45).contains("local-pg ◆"));
}

#[test]
fn ddl_in_a_transaction_says_which_lock_it_holds() {
    let mut h = connected();
    run(&mut h, "ALTER TABLE shop.users ADD COLUMN zz_c int");
    let notes: Vec<Msg> = h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect();
    assert!(
        notes
            .iter()
            .any(|m| matches!(m, Msg::TxDdlLockExclusive { step, target } if step == "1" && target.contains("users"))),
        "{notes:?}"
    );
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("ALTER TABLE".into()), elapsed: MS });
    run(&mut h, "CREATE INDEX zz_i ON shop.users (name)");
    let notes: Vec<Msg> = h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect();
    assert!(notes.iter().any(|m| matches!(m, Msg::TxDdlLockShare { .. })), "{notes:?}");
}

#[test]
fn a_procedure_that_fails_in_the_open_transaction_says_how_to_run_it() {
    let mut h = connected();
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    let id = run(&mut h, "CALL shop.archive()");
    h.tab_db(
        0,
        DbEvent::Failed { id, error: DbError::Server("invalid transaction termination".into()), cancelled: false },
    );
    h.tab_db(0, DbEvent::RunRolledBack { id });
    let notes: Vec<Msg> = h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect();
    assert!(notes.contains(&Msg::Label(Label::TxCallInTx)), "{notes:?}");
    assert!(h.app.tab().exec.user_tx(), "the transaction stays");
}

#[test]
fn the_korean_close_question_comes_from_the_catalog() {
    let mut h = connected_in(Lang::Ko, Some(Duration::from_secs(300)));
    h.ctrl('t');
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.ctrl('w');
    let ko = I18n::new(Lang::Ko);
    let screen = h.screen(100, 30);
    for l in [Label::TxButtonRollback, Label::TxButtonCommit] {
        assert!(screen.contains(ko.label(l).as_str()), "{screen}");
    }
}

#[test]
fn changing_the_commit_rule_keeps_the_open_transaction_and_says_so() {
    use datarig_tui::app::explorer::RowKind;
    use datarig_tui::app::profiles::Field;
    let dir = std::env::temp_dir().join(format!("datarig-txrule-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = config(Some(Duration::from_secs(300)));
    cfg.path = Some(dir.join("config.toml"));
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    let env = h.app.environments().iter().find(|e| e.name == "stg").unwrap().id;
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.app.explorer.select_kind(RowKind::Environment(env));
    h.keys("e");
    while h.form().focus != Field::EnvCommit {
        h.key(KeyCode::Tab);
    }
    h.key(KeyCode::Left);
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(h.app.transient.as_ref().map(|t| t.0.msg.clone()), Some(Msg::Label(Label::TxRuleChanged)));
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == Msg::Label(Label::TxRuleChanged)));
    assert!(h.app.tab().exec.user_tx(), "the transaction stays open");
    assert!(!h.app.manual_commit(h.app.tab().id), "the next runs commit by themselves");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A press and a release on `text` (past the dialog's arming delay).
fn click_on(h: &mut Harness, text: &str) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let t = h.draw(100, 30);
    let buf = t.backend().buffer();
    let (x, y) = (0..30)
        .find_map(|y| {
            let row = row_text(buf, y);
            row.find(text).map(|i| (datarig_tui::text::width(&row[..i]) as u16 + 1, y))
        })
        .unwrap_or_else(|| panic!("no {text:?}:\n{}", h.screen(100, 30)));
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

#[test]
fn the_buttons_take_clicks() {
    // The close question's "Commit first".
    let mut h = connected();
    h.ctrl('t');
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.ctrl('w');
    click_on(&mut h, "Commit first");
    let sent = ends(&mut h);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1);
    // The summary's "Roll back" (and "Close").
    let mut h = connected();
    write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
    h.keys(" xs");
    click_on(&mut h, "[ Close ]");
    assert_eq!(h.overlay_kind(), None);
    h.keys(" xs");
    click_on(&mut h, "Roll back  ");
    assert_eq!(h.overlay_kind(), None);
    let sent = ends(&mut h);
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].1);
}

#[test]
fn close_question_and_summary_snapshots() {
    for lang in [Lang::En, Lang::Ko] {
        let code = if lang == Lang::En { "en" } else { "ko" };
        let mut h = connected_in(lang, Some(Duration::from_secs(300)));
        h.ctrl('t');
        write(&mut h, "UPDATE shop.users SET name = 'x' WHERE id < 4", 3);
        h.ctrl('w');
        for (w, hgt) in [(80, 24), (160, 45)] {
            assert_screen!(format!("tx_close_choices_{code}_{w}x{hgt}"), lang, h.draw(w, hgt));
        }
        h.key(KeyCode::Esc);
        h.keys(" xs");
        assert_screen!(format!("tx_summary_{code}_80x24"), lang, h.draw(80, 24));
    }
}
