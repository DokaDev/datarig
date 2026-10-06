use super::*;
use crate::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn keys(e: &mut Editor, s: &str) {
    for c in s.chars() {
        let (code, m) = match c {
            '\x1b' => (KeyCode::Esc, KeyModifiers::NONE),
            '\x16' => (KeyCode::Char('v'), KeyModifiers::CONTROL),
            c => (KeyCode::Char(c), KeyModifiers::NONE),
        };
        e.handle_key(KeyEvent::new(code, m));
    }
}

fn hint(text: &str) -> Option<RunHint> {
    Some(RunHint { kind: HintKind::Ok, text: text.into() })
}

/// Run what `Ctrl+E` takes as query `q`; its statements.
fn run(e: &mut Editor, q: u64) -> Vec<String> {
    let (stmts, spans) = e.run_statements();
    e.stage_run(&stmts, spans);
    e.start_run(q, &stmts);
    stmts
}

fn spans(e: &Editor) -> Vec<(usize, usize)> {
    e.runs.active.as_ref().map(|(_, s)| s.iter().map(|s| (s.start, s.end)).collect()).unwrap_or_default()
}

#[test]
fn spans_follow_edits_before_them_and_note_edits_of_their_text() {
    let mut s = Span::new(10, 20, true);
    s.adjust(0, 0, 3, false);
    assert_eq!((s.start, s.end, s.edited), (13, 23, false), "typed before it");
    s.adjust(13, 13, 2, false);
    assert_eq!((s.start, s.end, s.edited), (15, 25, false), "typed right before its first character");
    s.adjust(25, 25, 4, false);
    assert_eq!((s.start, s.end, s.edited), (15, 25, false), "typed right after its `;`");
    s.adjust(30, 40, 0, false);
    assert_eq!((s.start, s.end, s.edited), (15, 25, false), "after it");
    s.adjust(0, 5, 0, false);
    assert_eq!((s.start, s.end, s.edited), (10, 20, false), "deleted before it");
    s.adjust(12, 13, 0, false);
    assert_eq!((s.start, s.end, s.edited), (10, 19, true), "inside it");
    // A statement without `;`: text typed at its end is its own, blanks (a new line) are not.
    let mut s = Span::new(0, 8, false);
    s.adjust(8, 8, 1, true);
    assert!(!s.edited);
    s.adjust(8, 8, 1, false);
    assert!(s.edited);
    // Deleted with what is around it.
    let mut s = Span::new(10, 20, true);
    s.adjust(5, 25, 0, false);
    assert_eq!((s.start, s.end, s.edited), (5, 5, true));
}

#[test]
fn a_run_takes_the_statement_under_the_cursor_or_the_selection_with_their_places() {
    let mut e = Editor::new("SELECT 1;\n  SELECT 2;\nSELECT 3");
    assert_eq!(run(&mut e, 1), ["SELECT 1"]);
    assert_eq!(spans(&e), [(0, 9)]);
    keys(&mut e, "jVj");
    assert_eq!(run(&mut e, 2), ["SELECT 2", "SELECT 3"]);
    assert_eq!(spans(&e), [(12, 21), (22, 30)]);
    // By character, from the middle of a line.
    keys(&mut e, "gg0wvj$");
    let stmts = run(&mut e, 3);
    assert_eq!(stmts, ["1", "SELECT 2"]);
    assert_eq!(spans(&e), [(7, 9), (12, 21)]);
    // A block has no places: nothing is marked.
    keys(&mut e, "gg0\x16j$");
    let (stmts, sp) = e.run_statements();
    assert!(!stmts.is_empty() && sp.is_empty());
}

#[test]
fn only_the_staged_statements_are_marked_and_only_their_run_gets_hints() {
    let mut e = Editor::new("SELECT 1;\nSELECT 2;");
    let (stmts, spans) = e.run_statements();
    e.stage_run(&stmts, spans);
    // Something else is sent: not marked.
    e.start_run(1, &["SELECT 9".to_string()]);
    assert_eq!(e.active_run(), None);
    run(&mut e, 2);
    assert_eq!(e.active_run(), Some(2));
    e.finish_run(1, vec![hint("other run")]);
    assert_eq!(e.run_hints().count(), 0, "another run's answer");
    e.finish_run(2, vec![hint("one")]);
    assert_eq!(e.active_run(), None);
    assert_eq!(e.run_hints().map(|(_, h)| h.text.as_str()).collect::<Vec<_>>(), ["one"]);
    assert_eq!(e.last_run(), Some(2));
    // A later run of the same statement replaces its hint; another statement's stays.
    keys(&mut e, "j");
    run(&mut e, 3);
    e.finish_run(3, vec![hint("two")]);
    keys(&mut e, "k");
    run(&mut e, 4);
    assert_eq!(e.run_hints().map(|(_, h)| h.text.as_str()).collect::<Vec<_>>(), ["two"], "its old hint goes");
    e.finish_run(4, vec![None]);
    assert_eq!(e.run_hints().count(), 1, "nothing to say: no hint");
    e.amend_run_hint(3, 0, hint("amended").unwrap());
    assert_eq!(e.run_hints().next().unwrap().1.text, "amended");
}

#[test]
fn hints_are_capped() {
    let mut e = Editor::new(&"SELECT 1;\n".repeat(MAX_HINTS + 10));
    for q in 0..(MAX_HINTS + 10) as u64 {
        run(&mut e, q);
        e.finish_run(q, vec![hint("x")]);
        keys(&mut e, "j");
    }
    assert_eq!(e.run_hints().count(), MAX_HINTS);
}

#[test]
fn hint_marks_are_glyphs_or_text_never_emoji() {
    let all = [HintKind::Ok, HintKind::Failed, HintKind::RolledBack, HintKind::Cancelled];
    // nf-fa-check, nf-fa-xmark, nf-fa-undo, nf-fa-ban (Nerd Fonts 3.4.0 glyphnames.json).
    assert_eq!(all.map(|k| k.mark(true)), ["\u{f00c}", "\u{f00d}", "\u{f0e2}", "\u{f05e}"]);
    assert_eq!(all.map(|k| k.mark(false)), ["\u{2713}", "\u{2717}", "\u{21ba}", "\u{2298}"]);
    for k in all {
        for on in [true, false] {
            assert_eq!(crate::text::width(k.mark(on)), 1, "{k:?}");
        }
    }
}
