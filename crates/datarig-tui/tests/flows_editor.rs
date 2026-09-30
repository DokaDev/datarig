//! The editor through the app: vim keys reach it through the keymap, runs take what it
//! selects, pastes land in it.

mod common;

use common::*;
use datarig_core::driver::DbCommand;
use datarig_core::i18n::Lang;
use datarig_tui::keymap::Ctx;
use datarig_tui::widgets::editor::{Editor, Mode};
use ratatui::crossterm::event::{Event, KeyCode};

/// A connected harness whose editor holds `text`.
fn editor_with(text: &str) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new(text);
    h.draw(100, 30);
    h
}

fn statements(sent: &[DbCommand]) -> Vec<String> {
    match sent {
        [DbCommand::Execute { statements, .. }] => statements.clone(),
        other => panic!("{other:?}"),
    }
}

/// `V` selects whole lines (it used to do nothing): `V`, `j`, then Ctrl+E runs exactly those
/// lines, and the status bar says V-LINE.
#[test]
fn v_then_j_then_ctrl_e_runs_the_selected_lines() {
    let mut h = editor_with("SELECT 1;\nSELECT 2;\nSELECT 3;\nSELECT 4;");
    h.keys("jV");
    assert_eq!((h.app.tab().editor.mode, h.app.key_context()), (Mode::Visual, Ctx::VimVisual));
    assert!(h.status(100, 30).contains("V-LINE"), "{}", h.status(100, 30));
    h.keys("j");
    assert_eq!(h.app.tab().editor.selection().as_deref(), Some("SELECT 2;\nSELECT 3;"));
    h.sent();
    h.ctrl('e');
    assert_eq!(statements(&h.sent()), ["SELECT 2", "SELECT 3"]);
    assert_eq!(h.app.tab().editor.mode, Mode::Normal, "the run ends Visual mode");
}

/// Operators, counts and `D`/`C`/`Y` reach the editor through the keymap, also after a count
/// or an operator that waits for its motion (`d` then `g g`).
#[test]
fn operators_and_counts_reach_the_editor() {
    let mut h = editor_with("a1\na2\na3\na4\na5\na6");
    h.keys("2dd");
    assert_eq!(h.app.tab().editor.text(), "a3\na4\na5\na6");
    h.keys("jdgg");
    assert_eq!(h.app.tab().editor.text(), "a5\na6");
    h.keys("u");
    assert_eq!(h.app.tab().editor.text(), "a3\na4\na5\na6");
    assert_eq!(h.app.tab().editor.row, 1, "back where the delete started");
    h.keys("Dx");
    assert_eq!(h.app.tab().editor.text(), "a3\n\na5\na6");
    h.keys("jYGp");
    assert_eq!(h.app.tab().editor.text(), "a3\n\na5\na6\na5");
    h.keys("ggjCnew");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab().editor.text(), "a3\nnew\na5\na6\na5");
}

/// Ctrl+W and Ctrl+U delete in Insert mode (they used to do nothing); Ctrl+W never closes the
/// tab there, and Ctrl+C still cancels.
#[test]
fn insert_ctrl_w_and_ctrl_u_edit_the_text() {
    let mut h = editor_with("SELECT id FROM users");
    h.keys("A");
    h.ctrl('w');
    assert_eq!(h.app.tab().editor.text(), "SELECT id FROM ");
    assert_eq!(h.app.tabs.len(), 1, "no tab closed");
    h.type_text("orders o");
    h.ctrl('u');
    assert_eq!(h.app.tab().editor.text(), "SELECT id FROM ", "what was typed");
    h.ctrl('u');
    assert_eq!(h.app.tab().editor.text(), "");
    h.key(KeyCode::Esc);
    h.keys("u");
    assert_eq!(h.app.tab().editor.text(), "SELECT id FROM users", "one undo step");
}

/// A paste in Normal mode goes into the text at the cursor, as one undo step (it used to be
/// dropped); in Visual mode it replaces the selection.
#[test]
fn pastes_in_normal_and_visual_mode() {
    let mut h = editor_with("SELECT  FROM t");
    h.keys("07l");
    h.app.handle_event(Event::Paste("a, b".into()));
    assert_eq!(h.app.tab().editor.text(), "SELECT a, b FROM t");
    assert_eq!(h.app.tab().editor.mode, Mode::Normal);
    h.keys("u");
    assert_eq!(h.app.tab().editor.text(), "SELECT  FROM t");
    h.keys("$v");
    h.app.handle_event(Event::Paste("users".into()));
    assert_eq!(h.app.tab().editor.text(), "SELECT  FROM users");
}

/// Ctrl+C in Visual mode cancels the running query (it has no copy meaning).
#[test]
fn ctrl_c_in_visual_mode_cancels_the_run() {
    let mut h = editor_with("SELECT pg_sleep(10);");
    h.ctrl('e');
    assert!(h.app.tab().exec.running.is_some());
    h.keys("v");
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the tab's session was asked to cancel");
}
