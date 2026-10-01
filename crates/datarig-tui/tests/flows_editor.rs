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

/// Text objects, `.` and `u` through the keymap: `f(` then `ci(` and typing changes what is
/// between the brackets; on the next line `.` does it again, and `u` undoes that in one step.
/// `dap` takes a paragraph and the blank line after it, and `.` the next one.
#[test]
fn text_objects_and_dot_through_the_keymap() {
    let mut h = editor_with("SELECT count(a, b) FROM t\nSELECT max(c) FROM u");
    h.keys("f(lci(");
    assert_eq!((h.app.tab().editor.mode, h.app.key_context()), (Mode::Insert, Ctx::VimInsert));
    h.type_text("x");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab().editor.text(), "SELECT count(x) FROM t\nSELECT max(c) FROM u");
    h.keys("j0f(.");
    assert_eq!(h.app.tab().editor.text(), "SELECT count(x) FROM t\nSELECT max(x) FROM u");
    h.keys("u");
    assert_eq!(h.app.tab().editor.text(), "SELECT count(x) FROM t\nSELECT max(c) FROM u");

    let mut h = editor_with("SELECT 1\nFROM a;\n\nSELECT 2;\n\nSELECT 3;");
    h.keys("dap");
    assert_eq!(h.app.tab().editor.text(), "SELECT 2;\n\nSELECT 3;");
    h.keys(".");
    assert_eq!(h.app.tab().editor.text(), "SELECT 3;");
}

/// Keys of more than one stroke reach the editor whole: `g u w` (a case operator and its
/// motion), `g J`, `>>` and `.`, and `Ctrl+D` scrolls the editor (the grid's `Ctrl+D` is not
/// taken there).
#[test]
fn sequences_and_scrolling_reach_the_editor() {
    let mut h = editor_with("SELECT ID FROM T\nWHERE X\n  AND Y");
    h.keys("guw");
    assert_eq!(h.app.tab().editor.lines[0], "select ID FROM T");
    h.keys("jgJ");
    assert_eq!(h.app.tab().editor.lines[1], "WHERE X  AND Y");
    h.keys(">>");
    assert_eq!(h.app.tab().editor.lines[1], "    WHERE X  AND Y");
    h.keys(".");
    assert_eq!(h.app.tab().editor.lines[1], "        WHERE X  AND Y");
    let text: Vec<String> = (1..=200).map(|i| format!("SELECT {i};")).collect();
    let mut h = editor_with(&text.join("\n"));
    let half = h.app.tab().editor.view_height() / 2;
    h.ctrl('d');
    assert_eq!((h.app.tab().editor.row, h.app.tab().editor.top), (half, half));
    h.keys("G");
    h.draw(100, 30);
    h.ctrl('b');
    assert!(h.app.tab().editor.row < 199, "a page back");
}

/// The character `f`, `t` and `r` wait for is taken as typed, also Hangul (not the QWERTY key
/// under it); the jamo that sits on `f` still starts the command.
#[test]
fn a_character_argument_is_taken_as_typed() {
    // SELECT '<two syllables>' x
    let mut h = editor_with("SELECT '\u{D55C}\u{AE00}' x");
    h.keys("f");
    h.type_text("\u{AE00}");
    assert_eq!(h.app.tab().editor.col, 9, "on the second syllable");
    h.keys("r");
    h.type_text("\u{AC00}");
    assert_eq!(h.app.tab().editor.text(), "SELECT '\u{D55C}\u{AC00}' x");
    h.keys("0");
    h.type_text("\u{3139}\u{D55C}"); // the jamo on `f`, then a syllable to find
    assert_eq!(h.app.tab().editor.col, 8);
    h.keys("dt");
    h.type_text("'");
    assert_eq!(h.app.tab().editor.text(), "SELECT '' x");
    assert!(!h.app.tab().editor.awaiting_key());
}

/// The notice flashed last, as text (and cleared, so the next check sees only what follows).
fn notice(h: &mut Harness) -> Option<String> {
    h.app.transient.take().map(|(n, _)| n.render(&h.app.i18n).to_string())
}

/// With the default `[editor] clipboard = on`, a yank, delete or change without a register also
/// goes to the system clipboard (lines with their line break), silently; `p` puts the editor's
/// own register and never reads the clipboard.
#[test]
fn yanks_and_deletes_go_to_the_clipboard_by_default() {
    let mut h = editor_with("one two\nthree\nfour");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("yw");
    assert_eq!(clip.last().as_deref(), Some("one "));
    h.keys("jdd");
    assert_eq!(clip.last().as_deref(), Some("three\n"), "whole lines end with a line break");
    h.keys("x");
    assert_eq!(clip.last().as_deref(), Some("f"));
    h.keys("cwfive");
    h.key(KeyCode::Esc);
    assert_eq!(clip.last().as_deref(), Some("our"));
    assert_eq!(notice(&mut h), None, "said nothing");
    h.keys("p");
    assert_eq!(h.app.tab().editor.text(), "one two\nfiveour");
    assert_eq!(clip.read_count(), 0, "p is the editor's register");
    assert!(h.app.take_terminal_output().is_empty(), "no OSC 52 with a system clipboard");
}

/// `editor.clipboard = off` keeps yanks and deletes in the editor; `"+` and `"*` still reach the
/// clipboard. `"_` touches no register and not the clipboard, `"a` only its register.
#[test]
fn the_setting_off_named_registers_and_the_black_hole_keep_the_clipboard() {
    let mut h = editor_with("one\ntwo\nthree");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("\"ayy");
    assert_eq!(clip.all(), Vec::<String>::new(), "\"a stays in the editor");
    assert_eq!(h.app.tab().editor.register('a').map(|r| r.text.as_str()), Some("one"));
    h.keys("\"_dd");
    assert_eq!(h.app.tab().editor.text(), "two\nthree", "the line went");
    assert_eq!(clip.all(), Vec::<String>::new(), "\"_ sends nothing");
    assert_eq!(h.app.tab().editor.register('"').map(|r| r.text.as_str()), Some("one"), "nor fills a register");
    h.command("set editor.clipboard=off");
    assert_eq!(h.app.prefs.editor_clipboard, datarig_core::config::EditorClipboard::Off);
    h.keys("yyx");
    assert_eq!(clip.all(), Vec::<String>::new(), "off: the editor's registers only");
    h.keys("\"+yy");
    assert_eq!(clip.all(), ["wo\n"]);
    h.keys("\"*x");
    assert_eq!(clip.all(), ["wo\n", "w"]);
    h.command("set editor.clipboard=on");
    h.keys("x");
    assert_eq!(clip.last().as_deref(), Some("o"));
}

/// A register write the clipboard cannot take never stops the edit: the text is gone from the
/// buffer and in the register, and a notice says why, once a session (not on every `x`).
#[test]
fn a_clipboard_that_cannot_take_a_yank_leaves_it_in_the_register() {
    // Over SSH (OSC 52) without a system clipboard, a yank longer than osc52_max_bytes is not
    // sent.
    let long = "x".repeat(800);
    let mut h = editor_with(&format!("{long}\nshort\n{long}"));
    FakeClipboard::attach(&mut h, true, &[("SSH_CONNECTION", "10.0.0.1 1 10.0.0.2 22")]);
    h.app.prefs.osc52_max_bytes = 1000;
    h.keys("dd");
    assert_eq!(h.app.tab().editor.text(), format!("short\n{long}"));
    assert_eq!(h.app.tab().editor.register('1').map(|r| r.text.as_str()), Some(long.as_str()));
    assert!(h.app.take_terminal_output().is_empty(), "too long for OSC 52");
    let said = notice(&mut h).expect("a notice");
    assert!(
        said.starts_with(
            "Kept in the editor's register, not sent to the clipboard: 2 KB of base64 is more than OSC \
         52 may carry (1 KB, osc52_max_bytes)"
        ),
        "{said}"
    );
    h.keys("x");
    assert_eq!(h.app.take_terminal_output(), ["\x1b]52;c;cw==\x07"], "a short one still goes");
    h.keys("jdd");
    assert_eq!(h.app.tab().editor.text(), "hort");
    assert_eq!(notice(&mut h), None, "said once");
    h.keys("u");
    assert_eq!(h.app.tab().editor.text(), format!("hort\n{long}"));

    // `clipboard = system` without one: the same, once.
    let mut h = editor_with("one\ntwo");
    FakeClipboard::attach(&mut h, true, &[]);
    h.command("set clipboard=system");
    h.app.transient = None;
    h.keys("yy");
    let said = notice(&mut h).expect("a notice");
    assert!(said.contains("the system clipboard is not available"), "{said}");
    h.keys("dd");
    assert_eq!(h.app.tab().editor.text(), "two");
    assert_eq!(h.app.tab().editor.register('"').map(|r| r.text.as_str()), Some("one"));
    assert_eq!(notice(&mut h), None, "said once");
}

/// `"+p` / `"+P` / `"*p` read the system clipboard then (whole lines when its text ends with a
/// line break), `.` reads it again, and `Ctrl+R +` types it in Insert mode; nothing else reads
/// it.
#[test]
fn plus_p_reads_the_clipboard_only_then() {
    let mut h = editor_with("one\ntwo");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    clip.hold("SELECT 1;\n");
    h.keys("\"+p");
    assert_eq!(h.app.tab().editor.text(), "one\nSELECT 1;\ntwo");
    assert_eq!(clip.read_count(), 1);
    clip.hold("x");
    h.keys(".");
    assert_eq!(h.app.tab().editor.text(), "one\nSxELECT 1;\ntwo", "read again, by character now");
    assert_eq!(clip.read_count(), 2);
    h.keys("gg\"*P");
    assert_eq!(h.app.tab().editor.text(), "xone\nSxELECT 1;\ntwo");
    h.keys("yyjpuA");
    h.ctrl('r');
    h.keys("+");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab().editor.text(), "xone\nSxELECT 1;xone\n\ntwo", "what yy put there");
    assert_eq!(clip.read_count(), 4, "yy, p and u read nothing");
    // A clipboard that cannot be read: a notice, nothing put.
    *clip.content.lock().unwrap() = None;
    h.keys("\"+p");
    assert_eq!(h.app.tab().editor.text(), "xone\nSxELECT 1;xone\n\ntwo");
    let said = notice(&mut h).expect("a notice");
    assert!(said.starts_with("Could not read the clipboard (the clipboard is empty)"), "{said}");
}

/// Over SSH (the OSC 52 plan) `"+p` never reads the clipboard: a notice points at the
/// terminal's own paste, in each language.
#[test]
fn plus_p_over_ssh_points_at_the_terminals_paste() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::connected(lang);
        h.app.tab_mut().editor = Editor::new("one");
        let clip = FakeClipboard::attach(&mut h, false, &[("SSH_TTY", "/dev/pts/1")]);
        clip.hold("secret");
        h.keys("\"+p");
        assert_eq!((h.app.tab().editor.text().as_str(), clip.read_count()), ("one", 0));
        let want = match lang {
            Lang::Ko => ko(datarig_core::i18n::Label::EditorClipboardNoRead).to_string(),
            _ => "The clipboard cannot be read through the terminal (OSC 52 only writes to it): paste with the \
                  terminal's own paste key instead (Cmd+V on macOS, Ctrl+Shift+V in most Linux terminals)"
                .to_string(),
        };
        assert_eq!(notice(&mut h).as_deref(), Some(want.as_str()));
        h.keys("i");
        h.ctrl('r');
        h.keys("+");
        assert_eq!(clip.read_count(), 0, "Ctrl+R + neither");
    }
}

/// The register's name is a key: Hangul typed for it means the QWERTY key at its place, after
/// `"` and after `Ctrl+R` in Insert mode.
#[test]
fn register_names_typed_in_hangul() {
    let mut h = editor_with("one two");
    h.keys("\"");
    h.type_text("\u{3141}"); // the jamo on `a`
    h.keys("yw");
    assert_eq!(h.app.tab().editor.register('a').map(|r| r.text.as_str()), Some("one "));
    h.keys("A");
    h.ctrl('r');
    h.type_text("\u{3141}");
    assert_eq!(h.app.tab().editor.text(), "one twoone ");
}

/// A clipboard that cannot be read leaves the registers as they were: after `"+yy` over SSH,
/// `"+p` says why and puts nothing, and `p` still puts the line (it used to put nothing).
#[test]
fn an_unreadable_clipboard_keeps_the_registers() {
    let mut h = editor_with("one\ntwo");
    let clip = FakeClipboard::attach(&mut h, true, &[("SSH_TTY", "/dev/pts/1")]);
    h.keys("\"+yy");
    assert_eq!(h.app.take_terminal_output().len(), 1, "sent through OSC 52");
    h.keys("\"+p");
    assert!(notice(&mut h).is_some_and(|n| n.starts_with("The clipboard cannot be read")));
    assert_eq!((h.app.tab().editor.text().as_str(), clip.read_count()), ("one\ntwo", 0));
    h.keys("p");
    assert_eq!(h.app.tab().editor.text(), "one\none\ntwo");
}

/// `p` in Visual mode replaces the selection; `"+p` there reads the clipboard, only then. An
/// empty register is said.
#[test]
fn visual_put_and_empty_registers() {
    let mut h = editor_with("one two");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    clip.hold("three");
    h.keys("wviw\"+p");
    assert_eq!((h.app.tab().editor.text().as_str(), clip.read_count()), ("one three", 1));
    assert_eq!(clip.last().as_deref(), Some("two"), "the text replaced goes to the clipboard, as Vim's");
    h.keys("\"qp");
    assert_eq!(notice(&mut h).as_deref(), Some("Nothing in register \"q"));
    assert_eq!(clip.read_count(), 1);
}
