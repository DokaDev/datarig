use super::*;
use crate::widgets::editor::Editor;
use datarig_core::sql::split::split;
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
    s.adjust(13, 13, 2, true);
    assert_eq!((s.start, s.end, s.edited), (15, 25, false), "blanks typed right before its first character");
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
    // Text typed right before its first character joins it (`EXPLAIN `, `-- `).
    let mut s = Span::new(10, 20, true);
    s.adjust(10, 10, 8, false);
    assert!(s.edited, "typed right before its first character");
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

/// Run `text`'s statement under the cursor (the first line) as query 1 and finish it with a
/// hint; the editor.
fn hinted(text: &str) -> Editor {
    let mut e = Editor::new(text);
    run(&mut e, 1);
    e.finish_run(1, vec![hint("one")]);
    assert_eq!(e.run_hints().count(), 1);
    e
}

/// Text joined to a statement changes it: its hint goes. A statement without `;` continued on
/// its line or the next, a word put before it, `gcc` commenting it out.
#[test]
fn joining_text_to_a_statement_is_an_edit_of_it() {
    for (text, keys_) in [
        ("SELECT 1", "A FROM t\x1b"),
        ("SELECT 1", "oWHERE false\x1b"),
        ("SELECT 1;", "0iEXPLAIN \x1b"),
        ("SELECT 1;", "gcc"),
    ] {
        let mut e = hinted(text);
        keys(&mut e, keys_);
        assert_eq!(e.run_hints().count(), 0, "{text:?} then {keys_:?}: {:?}", e.text());
    }
    // Blank lines, and a statement of its own after the `;`, leave it.
    for (text, keys_) in [("SELECT 1", "o\x1b"), ("SELECT 1;", "A SELECT 2;\x1b"), ("SELECT 1;", "O\x1b")] {
        let mut e = hinted(text);
        keys(&mut e, keys_);
        assert_eq!(e.run_hints().count(), 1, "{text:?} then {keys_:?}: {:?}", e.text());
    }
    // The same while it runs: its answer never attaches to the changed text.
    let mut e = Editor::new("SELECT 1");
    run(&mut e, 1);
    keys(&mut e, "A FROM t\x1b");
    e.finish_run(1, vec![hint("one")]);
    assert_eq!(e.run_hints().count(), 0);
}

/// A change before a statement that moves its bounds (a `;` deleted, a comment or a string
/// opened) drops its hint, though the change is not in its text.
#[test]
fn a_change_before_a_statement_that_moves_its_bounds_drops_its_hint() {
    for keys_ in ["gg$x", "ggI/* \x1b", "ggA '\x1b"] {
        let mut e = Editor::new("SELECT 0;\nSELECT 1;");
        keys(&mut e, "j");
        run(&mut e, 1);
        e.finish_run(1, vec![hint("one")]);
        keys(&mut e, keys_);
        assert_eq!(e.run_hints().count(), 0, "{keys_:?}: {:?}", e.text());
    }
}

/// A change after a statement's `;` cannot change it: its hint is not checked again (nothing
/// lexed), however long the statement is. A change before it is checked.
#[test]
fn a_change_after_a_closed_statement_does_not_check_it_again() {
    let mut e = hinted("SELECT 1;\nSELECT 2;");
    e.take_check_work();
    keys(&mut e, "jA -- x\x1b");
    assert_eq!(e.run_hints().count(), 1);
    assert_eq!(e.take_check_work(), 0, "after its `;`");
    keys(&mut e, "ggO\x1b");
    assert_eq!(e.run_hints().count(), 1);
    assert!(e.take_check_work() > 0, "before it: checked");
}

/// A selection of a whole statement without its `;` runs that statement: it gets its hint.
#[test]
fn a_whole_statement_selected_without_its_semicolon_gets_its_hint() {
    let mut e = Editor::new("SELECT 0;\nSELECT 1;\nSELECT 2;");
    keys(&mut e, "j0vt;");
    assert_eq!(run(&mut e, 1), ["SELECT 1"]);
    e.finish_run(1, vec![hint("one")]);
    assert_eq!(e.run_hints().count(), 1);
    // Text put between it and its `;` changes it.
    keys(&mut e, "j0f;i x\x1b");
    assert_eq!(e.run_hints().count(), 1, "another line");
    keys(&mut e, "k0f;i x\x1b");
    assert_eq!(e.run_hints().count(), 0, "{:?}", e.text());
}

/// A change before a hinted statement lexes only from the line it is on (from where the
/// lexer's state is known) up to the statement's first token, not the statement nor the text
/// back to the previous `;`: under a long comment header, and above a huge statement on one
/// line, the hint stays and a key costs a line.
#[test]
fn a_change_before_a_statement_lexes_only_up_to_it() {
    let header = "-- a line of the header that tells what the script does\n".repeat(20_000);
    let mut e = Editor::new(&format!("{header}SELECT 1;"));
    assert!(e.text().len() > 1_000_000);
    keys(&mut e, "G");
    run(&mut e, 1);
    e.finish_run(1, vec![hint("one")]);
    assert_eq!(e.run_hints().count(), 1);
    keys(&mut e, "k");
    e.take_check_work();
    for _ in 0..5 {
        keys(&mut e, "A x\x1b");
        assert_eq!(e.run_hints().count(), 1);
        let work = e.take_check_work();
        assert!(work > 0 && work < 1_000, "under the header: {work} bytes");
    }
    let values: String = (0..50_000).map(|i| format!("({i}, 'row {i}'), ")).collect();
    let mut e = Editor::new(&format!("SELECT 0;\n-- the rows\nINSERT INTO t VALUES {values}(0, 'end');"));
    keys(&mut e, "G");
    run(&mut e, 1);
    e.finish_run(1, vec![hint("one")]);
    assert_eq!(e.run_hints().count(), 1);
    keys(&mut e, "k");
    e.take_check_work();
    for _ in 0..5 {
        keys(&mut e, "A x\x1b");
        assert_eq!(e.run_hints().count(), 1);
        let work = e.take_check_work();
        assert!(work > 0 && work < 1_000, "above a huge line: {work} bytes");
    }
}

/// Changes before a statement are told apart: a line with its own `;` keeps the hint; a line
/// joined to it, a lone `;` deleted, keep nothing.
#[test]
fn changes_before_a_statement_keep_or_drop_its_hint_as_the_text_says() {
    for (text, keys_, kept) in [
        ("SELECT 0;\nSELECT 1;", "ggox;\x1b", true),
        ("SELECT 0;\nSELECT 1;", "ggOSELECT 9;\x1b", true),
        ("SELECT 0;\nSELECT 1;", "ggoSELECT 9\x1b", false),
        ("SELECT 0\n;\nSELECT 1;", "jx", false),
        ("SELECT 0;\n-- c\nSELECT 1;", "jA */\x1b", true),
        ("SELECT 0;\n-- c\nSELECT 1;", "jI/*\x1b", false),
    ] {
        let mut e = Editor::new(text);
        keys(&mut e, "G");
        run(&mut e, 1);
        e.finish_run(1, vec![hint("one")]);
        assert_eq!(e.run_hints().count(), 1, "{text:?}");
        keys(&mut e, keys_);
        assert_eq!(e.run_hints().count(), usize::from(kept), "{text:?} then {keys_:?}: {:?}", e.text());
    }
}

/// A `;` the check found before a moved statement becomes the bound it relies on next: lines
/// put above a hinted statement end with their own `;`, then that `;` is deleted while the line
/// before it holds only a comment, and the statement before runs into it: its hint goes.
#[test]
fn a_semicolon_found_by_the_check_is_the_bound_from_then_on() {
    let mut e = Editor::new("SELECT 0;\n\nSELECT 1;");
    keys(&mut e, "G");
    run(&mut e, 1);
    e.finish_run(1, vec![hint("one")]);
    assert_eq!(e.run_hints().count(), 1);
    for k in ["ggji;\x1b", "O-- the end\x1b", "OSELECT 5\x1b"] {
        keys(&mut e, k);
        assert_eq!(e.run_hints().count(), 1, "{k:?}: {:?}", e.text());
    }
    keys(&mut e, "jjx");
    assert_eq!(e.text(), "SELECT 0;\nSELECT 5\n-- the end\n\nSELECT 1;");
    assert_eq!(e.run_hints().count(), 0, "`SELECT 5 … SELECT 1` is one statement now");
}

/// A small generator of the same pseudo-random sequence on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Pieces of text that open, close or end things for the lexer and the splitter.
const PIECES: &[&str] = &[
    "SELECT 1", ";", " ", "\n", "\n", "\n", "/*", "*/", "'", "\"", "$$", "$f$", "--", "-- c\n", "E'\\", "x", ";\n",
    "\n;", "\n;\n", "\r\n", "/* /* */", "\\x\n", ".5", "$1", "U&'", "\n\n",
];

fn boundary(t: &str, mut i: usize) -> usize {
    while !t.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Every hint left is exactly a statement of the text (as the whole text splits) with the
/// text it had when it ran.
fn hints_are_statements(e: &mut Editor, ran: &HashMap<usize, String>, ctx: &str) {
    e.check_spans(false, |_| true);
    let text = e.text();
    let stmts = split(&text);
    for h in &e.runs.hints {
        let (a, b) = (h.span.start, h.span.end);
        let statement = stmts.iter().any(|s| s.start == a && s.end == b);
        assert!(statement && text.get(a..b) == Some(ran[&h.index].as_str()), "{ctx}: ({a}, {b}) in {text:?}");
    }
}

/// Random texts before hinted statements and random changes, mostly before the last of them,
/// checked after most changes: a hint never stays on text that is not its statement.
#[test]
fn hints_never_stay_on_text_that_is_not_their_statement() {
    for seed in 1..=20_000u64 {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut pre = String::new();
        for _ in 0..3 + r.below(15) {
            pre.push_str(PIECES[r.below(PIECES.len())]);
        }
        let text = format!("{pre};\n\nSELECT 2;\nSELECT 3;");
        let mut e = Editor::new(&text);
        keys(&mut e, "ggVG");
        let stmts = run(&mut e, 1);
        let ran: HashMap<usize, String> = spans(&e).iter().map(|&(a, b)| text[a..b].to_string()).enumerate().collect();
        e.finish_run(1, (0..stmts.len()).map(|_| hint("h")).collect());
        hints_are_statements(&mut e, &ran, &format!("seed {seed}"));
        for step in 0..40 {
            let Some(last) = e.runs.hints.last().map(|h| h.span.start) else { break };
            let t = e.text();
            let a = boundary(&t, r.below(last + 1));
            let (b, ins) = match r.below(3) {
                0 => (a, PIECES[r.below(PIECES.len())]),
                1 => (boundary(&t, (a + 1 + r.below(3)).min(t.len())).max(a), ""),
                _ => (boundary(&t, (a + r.below(3)).min(t.len())).max(a), PIECES[r.below(PIECES.len())]),
            };
            e.splice_raw(a, b, ins);
            if r.below(4) != 0 {
                hints_are_statements(
                    &mut e,
                    &ran,
                    &format!("seed {seed}, step {step}, ({a}, {b}, {ins:?}) after {t:?}"),
                );
            }
        }
    }
}
