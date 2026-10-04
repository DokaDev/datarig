//! `Ctrl+G` (the query in the user's editor) and `Ctrl+Z` / `:suspend` through the app: the
//! keys queue an effect for the binary, and what the editor brought back replaces the text as
//! one undo step, opens in a new console (a table tab's copy), or is said and changes nothing.
//! The binary's part (the file, the editor, the terminal) has its own tests in
//! `src/external/tests.rs` and `src/terminal/tests.rs`.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent, Outcome};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::paths::Paths;
use datarig_tui::app::Focus;
use datarig_tui::app::effects::Effect;
use datarig_tui::external::{EditFailure, Edited, Ended};
use datarig_tui::keymap::Ctx;
use datarig_tui::widgets::editor::{Editor, Mode};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::time::Duration;

fn editor_with(text: &str) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new(text);
    h.draw(100, 30);
    h
}

/// `Ctrl+G`, and the text the binary is asked to edit.
fn ctrl_g(h: &mut Harness) -> String {
    h.ctrl('g');
    match h.app.take_effect() {
        Some(Effect::Edit { text, .. }) => text,
        other => panic!("{other:?}"),
    }
}

fn text(h: &Harness) -> String {
    h.app.tab().editor.text()
}

#[test]
fn ctrl_g_hands_the_text_to_the_editor_and_its_change_is_one_undo_step() {
    let mut h = editor_with("SELECT 1;\nSELECT 2;");
    h.keys("j");
    assert_eq!(ctrl_g(&mut h), "SELECT 1;\nSELECT 2;");
    assert_eq!(h.app.take_effect(), None, "once");
    h.app.external_edit_done(Edited::Changed("SELECT 1;\nSELECT 20;\nSELECT 3;".into()));
    assert_eq!(text(&h), "SELECT 1;\nSELECT 20;\nSELECT 3;");
    assert_eq!(h.app.tab().editor.mode, Mode::Normal);
    assert_eq!(h.app.tab().editor.row, 1, "the cursor keeps its line");
    assert!(h.status(100, 30).contains("Text replaced with the editor's (u undoes it)"), "{}", h.status(100, 30));
    h.keys("u");
    assert_eq!(text(&h), "SELECT 1;\nSELECT 2;", "one undo step");
    h.ctrl('r');
    assert_eq!(text(&h), "SELECT 1;\nSELECT 20;\nSELECT 3;");
}

/// The file lives in the state directory's `edit` folder; without a state directory the
/// binary says so (`None`).
#[test]
fn the_file_goes_to_the_state_directory() {
    let mut h = editor_with("SELECT 1");
    let state = std::env::temp_dir().join(format!("datarig-flows-external-{}", std::process::id()));
    h.app.set_paths(Paths { data: None, state: Some(state.clone()) });
    h.ctrl('g');
    assert_eq!(h.app.take_effect(), Some(Effect::Edit { text: "SELECT 1".into(), dir: Some(state.join("edit")) }));
    h.app.external_edit_done(Edited::Unchanged);
    let mut h = editor_with("SELECT 1");
    h.ctrl('g');
    assert_eq!(h.app.take_effect(), Some(Effect::Edit { text: "SELECT 1".into(), dir: None }));
}

/// Like `Esc`: Insert mode ends (what was typed is in the text that goes), a selection goes.
#[test]
fn ctrl_g_ends_insert_and_visual_mode_first() {
    let mut h = editor_with("SELECT 1");
    h.keys("A");
    h.type_text(" + 1");
    assert_eq!(h.app.key_context(), Ctx::VimInsert);
    assert_eq!(ctrl_g(&mut h), "SELECT 1 + 1");
    assert_eq!(h.app.tab().editor.mode, Mode::Normal);
    h.app.external_edit_done(Edited::Changed("SELECT 2".into()));
    h.keys("u");
    assert_eq!(text(&h), "SELECT 1 + 1", "the typing is its own undo step");
    h.keys("u");
    assert_eq!(text(&h), "SELECT 1");

    let mut h = editor_with("SELECT 1");
    h.keys("vl");
    assert_eq!(h.app.tab().editor.mode, Mode::Visual);
    ctrl_g(&mut h);
    assert_eq!(h.app.tab().editor.mode, Mode::Normal);
    h.app.external_edit_done(Edited::Unchanged);

    // The search prompt closes too.
    let mut h = editor_with("SELECT 1");
    h.keys("/SEL");
    assert_eq!(h.app.key_context(), Ctx::VimSearch);
    ctrl_g(&mut h);
    assert_eq!(h.app.key_context(), Ctx::VimNormal);
}

#[test]
fn no_change_keeps_the_text_and_says_so() {
    let mut h = editor_with("SELECT 1");
    let version = h.app.tab().editor.version();
    ctrl_g(&mut h);
    h.app.external_edit_done(Edited::Unchanged);
    assert_eq!((text(&h), h.app.tab().editor.version()), ("SELECT 1".to_string(), version));
    assert!(h.status(100, 30).contains("No change from the editor"));
    h.keys("u");
    assert_eq!(text(&h), "SELECT 1", "nothing to undo");
}

/// Unknown is not absent: a failed editor or an unreadable file never empties the text.
#[test]
fn a_failure_keeps_the_text_and_says_why() {
    let cases = [
        (
            EditFailure::Exit { program: "vim".into(), how: Ended::Code(1) },
            "The editor vim ended with exit code 1: the text is unchanged",
        ),
        (EditFailure::Exit { program: "nano".into(), how: Ended::Signal(9) }, "The editor nano was ended by signal 9"),
        (
            EditFailure::Read("No such file or directory".into()),
            "Could not read the editor's file back: No such file or directory",
        ),
        (EditFailure::NotUtf8, "not UTF-8"),
        (
            EditFailure::Spawn { program: "nvim".into(), error: "not found".into() },
            "Could not start the editor nvim: not found (set $VISUAL or $EDITOR)",
        ),
        (
            EditFailure::Command { var: "VISUAL", error: "missing closing quote".into() },
            "$VISUAL is not a command that can run: missing closing quote",
        ),
        (EditFailure::File("Permission denied".into()), "Could not write the file for the editor: Permission denied"),
        (EditFailure::NoStateDir, "No state directory for the editor's file"),
        (
            EditFailure::Terminal("Input/output error".into()),
            "Could not hand the terminal to the editor: Input/output error",
        ),
    ];
    for (failure, said) in cases {
        let mut h = editor_with("SELECT 1");
        let version = h.app.tab().editor.version();
        ctrl_g(&mut h);
        h.app.external_edit_done(Edited::Failed(failure.clone()));
        assert_eq!((text(&h), h.app.tab().editor.version()), ("SELECT 1".to_string(), version), "{failure:?}");
        let screen = h.screen(160, 30);
        assert!(screen.contains(said), "{failure:?}: {screen}");
        assert_eq!(h.app.tabs.len(), 1);
    }
}

/// A table tab's query is a copy: the edited copy opens in a new console on the same
/// connection; the table tab keeps its query. No change opens nothing.
#[test]
fn a_table_tabs_query_goes_as_a_copy_and_comes_back_in_a_new_console() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let table = h.app.tab().id;
    assert!(h.app.tab().is_table());
    let query = r#"SELECT * FROM "shop"."users""#;
    assert_eq!(ctrl_g(&mut h), query);
    h.app.external_edit_done(Edited::Unchanged);
    assert_eq!((h.app.tabs.len(), h.app.tab().id), (2, table), "no change: nothing opens");

    assert_eq!(ctrl_g(&mut h), query);
    h.app.external_edit_done(Edited::Changed(format!("{query} WHERE id = 1")));
    assert_eq!(h.app.tabs.len(), 3);
    let t = h.app.tab();
    assert!(!t.is_table() && t.id != table);
    assert_eq!(t.editor.text(), format!("{query} WHERE id = 1"));
    assert_eq!(t.profile, h.app.tabs.get(table).unwrap().profile, "the same connection");
    assert_eq!(t.context, h.app.tabs.get(table).unwrap().context, "the same database and schema");
    assert_eq!(h.app.focus, Focus::Editor);
    assert_eq!(h.app.tabs.get(table).unwrap().editor.text(), query, "the table tab keeps its query");
    assert!(h.status(100, 30).contains("The edited query is in a new console"));
}

/// The answer is bound to the tab and its text as they were: when they changed meanwhile, the
/// edited text is not lost and does not overwrite them; it opens in a new console.
#[test]
fn an_answer_for_a_changed_tab_opens_in_a_new_console() {
    let mut h = editor_with("SELECT 1");
    ctrl_g(&mut h);
    h.app.tab_mut().editor = Editor::new("SELECT other");
    h.app.external_edit_done(Edited::Changed("SELECT 2".into()));
    assert_eq!(h.app.tabs.len(), 2);
    assert_eq!(text(&h), "SELECT 2");
    assert_eq!(h.app.tabs.iter().next().unwrap().editor.text(), "SELECT other");
    assert!(h.screen(200, 30).contains("the edited text is in a new console"));
    // An answer nobody asked for does nothing.
    h.app.external_edit_done(Edited::Changed("SELECT 3".into()));
    assert_eq!((h.app.tabs.len(), text(&h).as_str()), (2, "SELECT 2"));
}

/// A query running in the tab keeps running while the editor is open, and its result is
/// applied as it arrives.
#[test]
fn a_running_query_keeps_running_while_the_editor_is_open() {
    let mut h = editor_with("SELECT 1");
    h.sent();
    h.ctrl('e');
    let sent = h.sent();
    let [DbCommand::Execute { id, .. }] = &sent[..] else { panic!("{sent:?}") };
    let id = *id;
    ctrl_g(&mut h);
    assert!(h.app.tab().exec.running.is_some());
    assert!(!h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "not cancelled");
    // What arrives meanwhile (the binary feeds it in after the editor) is applied.
    h.db(DbEvent::Done { id, outcome: Outcome::Command("SELECT".into()), elapsed: Duration::from_millis(3) });
    h.app.external_edit_done(Edited::Changed("SELECT 2".into()));
    assert!(h.app.tab().exec.running.is_none(), "the run ended");
    assert_eq!(text(&h), "SELECT 2");
}

#[test]
fn ctrl_z_and_suspend_ask_the_binary_to_stop_everywhere_in_the_workspace() {
    let mut h = editor_with("SELECT 1");
    let suspend = |h: &mut Harness| h.app.take_effect() == Some(Effect::Suspend);
    if cfg!(windows) {
        h.ctrl('z');
        assert_eq!(h.app.take_effect(), None, "Ctrl+Z does nothing on Windows");
        h.command("suspend");
        assert_eq!(h.app.take_effect(), None);
        assert!(h.status(100, 30).contains("Suspending is not supported on Windows"));
        return;
    }
    // Normal, Insert (the text stays as typed), Visual, the explorer, the grid.
    h.ctrl('z');
    assert!(suspend(&mut h), "Normal");
    h.keys("A");
    h.type_text(" + 1");
    h.ctrl('z');
    assert!(suspend(&mut h), "Insert");
    assert_eq!((h.app.tab().editor.mode, text(&h).as_str()), (Mode::Insert, "SELECT 1 + 1"));
    h.key(KeyCode::Esc);
    h.keys("v");
    h.ctrl('z');
    assert!(suspend(&mut h), "Visual");
    h.key(KeyCode::Esc);
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.ctrl('z');
    assert!(suspend(&mut h), "explorer");
    for name in ["suspend", "sus", "stop"] {
        h.command(name);
        assert!(suspend(&mut h), ":{name}");
    }
    // Ctrl+C stays the query cancel.
    h.key(KeyCode::Tab);
    h.sent();
    h.ctrl('e');
    assert!(h.app.tab().exec.running.is_some());
    h.key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "Ctrl+C cancels");
    assert_eq!(h.app.take_effect(), None);
    // A failure to stop is said; the program goes on.
    h.app.suspend_failed("Operation not permitted".into());
    assert!(h.status(100, 30).contains("Could not suspend: Operation not permitted"));
}

/// Korean: the notices come from the catalog.
#[test]
fn the_notices_are_localized() {
    let mut h = Harness::connected(Lang::Ko);
    h.app.tab_mut().editor = Editor::new("SELECT 1");
    ctrl_g(&mut h);
    h.app.external_edit_done(Edited::Unchanged);
    assert!(h.status(100, 30).contains(ko(Label::ExternalUnchanged)), "{}", h.status(100, 30));
    ctrl_g(&mut h);
    let f = EditFailure::Exit { program: "vim".into(), how: Ended::Code(1) };
    h.app.external_edit_done(Edited::Failed(f));
    let want = ko_msg(&Msg::ExternalExitCode { program: "vim".into(), code: "1".into() });
    assert!(h.screen(160, 30).contains(&want), "{want}");
}

/// From the editor, open `shop.users` in the explorer (the schema's objects answered).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys("jjjj");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".to_string(), "users".to_string()], Vec::new()).into()),
    });
    h.keys("jjj");
    h.key(KeyCode::Enter);
}
