use super::*;
use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyEventState, KeyModifiers};

pub(super) fn key(c: KeyCode) -> KeyEvent {
    KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

pub(super) fn ctrl(c: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

/// Type `keys`: characters, `\n` Enter, `\x1b` Esc, `\x08` Backspace, and Vim's notation
/// `<Esc>`, `<CR>`, `<BS>`, `<Del>`, `<Tab>`, `<Left>`, `<Right>`, `<Up>`, `<Down>`,
/// `<Home>`, `<End>`, `<C-x>`. `:` up to `<CR>` in Normal or Visual mode is an Ex command.
pub(super) fn typ(e: &mut Editor, keys: &str) {
    let mut rest = keys;
    while let Some(c) = rest.chars().next() {
        // `:` and a command up to `<CR>`, as the app's command line hands it over.
        if c == ':'
            && e.mode != Mode::Insert
            && !e.awaiting_key()
            && !e.searching()
            && let Some(end) = rest.find("<CR>")
        {
            let line = e.cmdline_range() + &rest[1..end];
            let _ = e.ex(&line);
            rest = &rest[end + 4..];
            continue;
        }
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            let name = &rest[1..end];
            let k = match name {
                "Esc" => Some(key(KeyCode::Esc)),
                "CR" => Some(key(KeyCode::Enter)),
                "BS" => Some(key(KeyCode::Backspace)),
                "Del" => Some(key(KeyCode::Delete)),
                "Tab" => Some(key(KeyCode::Tab)),
                "Left" => Some(key(KeyCode::Left)),
                "Right" => Some(key(KeyCode::Right)),
                "Up" => Some(key(KeyCode::Up)),
                "Down" => Some(key(KeyCode::Down)),
                "Home" => Some(key(KeyCode::Home)),
                "End" => Some(key(KeyCode::End)),
                _ => name.strip_prefix("C-").and_then(|c| c.chars().next()).map(ctrl),
            };
            if let Some(k) = k {
                e.handle_key(k);
                rest = &rest[end + 1..];
                continue;
            }
        }
        let code = match c {
            '\n' => KeyCode::Enter,
            '\x1b' => KeyCode::Esc,
            '\x08' => KeyCode::Backspace,
            c => KeyCode::Char(c),
        };
        e.handle_key(key(code));
        rest = &rest[c.len_utf8()..];
    }
}

/// An editor with `text` and the cursor at `(row, col)`.
pub(super) fn at(text: &str, (row, col): (usize, usize)) -> Editor {
    let mut e = Editor::new(text);
    e.row = row;
    e.col = col;
    e
}

/// `(text, cursor, keys, text after, cursor after, register after)`.
pub(super) type Case =
    (&'static str, (usize, usize), &'static str, &'static str, (usize, usize), Option<(&'static str, bool)>);

/// Run each case from a fresh editor and compare the text, the cursor and the register; the
/// failures are listed together.
pub(super) fn check(cases: &[Case]) {
    let mut failed = Vec::new();
    for &(text, cursor, keys, want, want_cursor, want_reg) in cases {
        let mut e = at(text, cursor);
        typ(&mut e, keys);
        let reg = e.register('"').map(|r| (r.text.as_str(), r.linewise()));
        let have = (e.text(), (e.row, e.col), reg, e.mode);
        if have != (want.to_string(), want_cursor, want_reg, Mode::Normal) {
            failed
                .push(format!("{keys:?} on {text:?} at {cursor:?}: {have:?}, not {:?}", (want, want_cursor, want_reg)));
        }
        assert_eq!(e.len_bytes(), e.text().len());
    }
    assert!(failed.is_empty(), "{} of {} cases:\n{}", failed.len(), cases.len(), failed.join("\n"));
}

/// `(text, cursor, keys, text after, cursor after, marks after)`: a mark's position, or `None`
/// when it is not set.
pub(super) type MarkCase = (
    &'static str,
    (usize, usize),
    &'static str,
    &'static str,
    (usize, usize),
    &'static [(char, Option<(usize, usize)>)],
);

/// Run each case from a fresh editor and compare the text, the cursor and the marks; the
/// failures are listed together.
pub(super) fn check_marks(cases: &[MarkCase]) {
    let mut failed = Vec::new();
    for &(text, cursor, keys, want, want_cursor, want_marks) in cases {
        let mut e = at(text, cursor);
        typ(&mut e, keys);
        let marks: Vec<(char, Option<(usize, usize)>)> =
            want_marks.iter().map(|&(c, _)| (c, e.marks.get(c).ok())).collect();
        let have = (e.text(), (e.row, e.col), marks, e.mode);
        if have != (want.to_string(), want_cursor, want_marks.to_vec(), Mode::Normal) {
            failed.push(format!(
                "{keys:?} on {text:?} at {cursor:?}: {have:?}, not {:?}",
                (want, want_cursor, want_marks)
            ));
        }
    }
    assert!(failed.is_empty(), "{} of {} cases:\n{}", failed.len(), cases.len(), failed.join("\n"));
}

#[test]
fn dd_p_u_redo() {
    let mut e = Editor::new("a\nb\nc");
    typ(&mut e, "ddp");
    assert_eq!(e.text(), "b\na\nc");
    typ(&mut e, "u");
    assert_eq!(e.text(), "b\nc");
    typ(&mut e, "u");
    assert_eq!(e.text(), "a\nb\nc");
    e.handle_key(ctrl('r'));
    assert_eq!(e.text(), "b\nc");
}

#[test]
fn yy_p_o_a() {
    let mut e = Editor::new("one\ntwo");
    typ(&mut e, "yyjp");
    assert_eq!(e.text(), "one\ntwo\none");
    typ(&mut e, "ggoNEW\x1b");
    assert_eq!(e.text(), "one\nNEW\ntwo\none");
    typ(&mut e, "A!\x1b");
    assert_eq!(e.text(), "one\nNEW!\ntwo\none");
    typ(&mut e, "u");
    assert_eq!(e.text(), "one\nNEW\ntwo\none");
    typ(&mut e, "GP");
    assert_eq!(e.text(), "one\nNEW\ntwo\none\none");
}

#[test]
fn grapheme_editing() {
    let mut e = Editor::new("");
    typ(&mut e, "iこんにちは 🐘");
    assert_eq!(e.text(), "こんにちは 🐘");
    assert_eq!(e.col, 7);
    assert_eq!(e.display_x(0, e.col), 13);
    typ(&mut e, "\x08\x08");
    assert_eq!(e.text(), "こんにちは");
    typ(&mut e, " 👨‍👩‍👧‍👦\x08");
    assert_eq!(e.text(), "こんにちは ");
    typ(&mut e, "\x1b0x");
    assert_eq!(e.text(), "んにちは ");
    // Operators count graphemes too.
    typ(&mut e, "0d2l");
    assert_eq!(e.text(), "ちは ");
    typ(&mut e, "yl$p");
    assert_eq!(e.text(), "ちは ち");
}

#[test]
fn paste_in_insert() {
    let mut e = Editor::new("x");
    typ(&mut e, "i");
    e.paste("a\r\nb");
    assert_eq!(e.text(), "a\nbx");
}

/// A paste in Normal mode goes in at the cursor as one undo step (it used to be dropped), and
/// pastes one after the other keep their order.
#[test]
fn paste_in_normal_mode_is_one_undo_step() {
    let mut e = at("SELECT  FROM t", (0, 7));
    assert_eq!(e.paste("a,\r\nb"), EdEvent::Changed { typed: None });
    assert_eq!(e.text(), "SELECT a,\nb FROM t");
    assert_eq!((e.mode, e.row, e.col), (Mode::Normal, 1, 1), "on the character that was under the cursor");
    e.paste(", c");
    assert_eq!(e.text(), "SELECT a,\nb, c FROM t");
    typ(&mut e, "u");
    assert_eq!(e.text(), "SELECT a,\nb FROM t");
    typ(&mut e, "u");
    assert_eq!(e.text(), "SELECT  FROM t");
    assert_eq!(e.paste(""), EdEvent::None);
    // An empty line, and the end of a line.
    let mut e = at("x\n", (1, 0));
    e.paste("y");
    assert_eq!((e.text().as_str(), e.row, e.col), ("x\ny", 1, 0));
}

/// A paste in Visual mode replaces the selection (whole lines by line) as one undo step.
#[test]
fn paste_in_visual_mode_replaces_the_selection() {
    let mut e = at("SELECT old_name FROM t", (0, 7));
    typ(&mut e, "ve");
    e.paste("new");
    assert_eq!((e.text().as_str(), e.mode), ("SELECT new FROM t", Mode::Normal));
    typ(&mut e, "u");
    assert_eq!(e.text(), "SELECT old_name FROM t");
    let mut e = at("a\nb\nc\nd", (1, 0));
    typ(&mut e, "Vj");
    e.paste("x\ny\nz");
    assert_eq!(e.text(), "a\nx\ny\nz\nd");
    typ(&mut e, "u");
    assert_eq!(e.text(), "a\nb\nc\nd");
}

/// Every register write the system clipboard would get can be taken once by the app: those
/// without a register (`"` for the app) and those into `"+` / `"*`, not those into another
/// named register or `"_`.
#[test]
fn register_writes_are_offered_once() {
    let yank = |register: char, text: &str, kind: RegKind| Some(Yank { register, reg: Register::new(text, kind) });
    let mut e = at("one two\nthree", (0, 0));
    assert_eq!(e.take_yank(), None);
    typ(&mut e, "yw");
    assert_eq!(e.take_yank(), yank('"', "one ", RegKind::Charwise));
    assert_eq!(e.take_yank(), None, "taken");
    typ(&mut e, "jdd");
    assert_eq!(e.take_yank(), yank('"', "three", RegKind::Linewise));
    typ(&mut e, "x");
    assert_eq!(e.take_yank(), yank('"', "o", RegKind::Charwise));
    typ(&mut e, "vy");
    assert_eq!(e.take_yank().map(|y| y.reg.text), Some("n".into()));
    typ(&mut e, "l");
    assert_eq!(e.take_yank(), None, "a motion writes nothing");
    assert_eq!(e.register('"'), Some(&Register::new("n", RegKind::Charwise)));
    typ(&mut e, "\"ayw\"Ayw\"_x");
    assert_eq!(e.take_yank(), None, "named registers and the black hole stay in the editor");
    typ(&mut e, "\"+yw");
    assert_eq!(e.take_yank(), yank('+', " ", RegKind::Charwise));
    typ(&mut e, "\"*dd");
    assert_eq!(e.take_yank(), yank('*', "n two", RegKind::Linewise));
}

#[test]
fn the_mode_label_tells_visual_by_line() {
    let mut e = Editor::new("a\nb");
    assert_eq!(e.mode_label(), Label::StatusModeNormal);
    typ(&mut e, "v");
    assert_eq!((e.mode_label(), e.visual_lines()), (Label::StatusModeVisual, false));
    typ(&mut e, "V");
    assert_eq!((e.mode_label(), e.visual_lines()), (Label::StatusModeVisualLine, true));
    typ(&mut e, "<Esc>i");
    assert_eq!((e.mode_label(), e.visual_lines()), (Label::StatusModeInsert, false));
}

/// The keymap hands the next key to the editor while an operator or `g` waits for it, not
/// while only a count was typed.
#[test]
fn awaiting_a_key() {
    let mut e = Editor::new("a b c");
    for (k, waits) in [("3", false), ("d", true), ("2", true), ("g", true), ("<Esc>", false), ("g", true), ("g", false)]
    {
        typ(&mut e, k);
        assert_eq!(e.awaiting_key(), waits, "after {k}");
    }
    typ(&mut e, "v");
    typ(&mut e, "g");
    assert!(e.awaiting_key(), "gg in Visual mode");
    typ(&mut e, "g");
    assert!(!e.awaiting_key());
}

/// The keymap takes the character `f`, `t` and `r` wait for as it is typed; a count, an
/// operator or a text object waits for keys that still are commands.
#[test]
fn awaiting_a_character() {
    let mut e = Editor::new("a b c");
    for (k, key, char) in [
        ("f", true, true),
        ("x", false, false),
        ("d", true, false),
        ("t", true, true),
        ("<Esc>", false, false),
        ("r", true, true),
        ("<Esc>", false, false),
        ("di", true, false),
        ("<Esc>v", false, false),
        ("F", true, true),
        ("<Esc><Esc>i", false, false),
    ] {
        typ(&mut e, k);
        assert_eq!((e.awaiting_key(), e.awaiting_char()), (key, char), "after {k}");
    }
}

/// What an external editor saved: one undo step, only the changed middle spliced (marks
/// before and after it keep their places), the cursor on its line and column as far as they
/// exist, Normal mode; the same text changes nothing.
#[test]
fn replace_text_is_one_undo_step_that_keeps_the_marks_around_the_change() {
    let mut e = at("SELECT 1;\nSELECT 2;\nSELECT 3;", (0, 0));
    typ(&mut e, "majjmbk");
    let version = e.version();
    assert!(!e.replace_text("SELECT 1;\nSELECT 2;\nSELECT 3;"));
    assert_eq!(e.version(), version, "the same text: nothing changes");
    assert!(e.replace_text("SELECT 1;\nSELECT 22, 'x';\nSELECT 3;"));
    assert_eq!(e.text(), "SELECT 1;\nSELECT 22, 'x';\nSELECT 3;");
    assert_eq!((e.row, e.col), (1, 0));
    assert_eq!((e.marks.get('a').ok(), e.marks.get('b').ok()), (Some((0, 0)), Some((2, 0))));
    assert_eq!(e.last_step_changes(), 1);
    typ(&mut e, "u");
    assert_eq!(e.text(), "SELECT 1;\nSELECT 2;\nSELECT 3;");
    typ(&mut e, "<C-r>");
    assert_eq!(e.text(), "SELECT 1;\nSELECT 22, 'x';\nSELECT 3;");
    // A shorter text: the cursor stays on the text.
    assert!(e.replace_text("x"));
    assert_eq!((e.text().as_str(), e.row, e.col), ("x", 0, 0));
    assert!(e.replace_text(""));
    assert_eq!(e.text(), "");
    typ(&mut e, "uu");
    assert_eq!(e.text(), "SELECT 1;\nSELECT 22, 'x';\nSELECT 3;");
    assert_eq!(e.len_bytes(), e.text().len());
}

/// The common start and end are cut on character boundaries: `é` (C3 A9) and `ê` (C3 AA)
/// share their first byte, U+AC00 and U+AC01 (Hangul syllables) their first two.
#[test]
fn replace_text_cuts_on_character_boundaries() {
    for (old, new) in [
        ("café", "cafê"),
        ("\u{AC00}\u{B098}", "\u{AC01}\u{B098}"),
        ("ab\u{AC00}", "ab\u{AC01}"),
        ("é", "ê"),
        ("aé", "a"),
        ("x", "xé"),
    ] {
        let mut e = Editor::new(old);
        assert!(e.replace_text(new), "{old} -> {new}");
        assert_eq!(e.text(), new);
        assert_eq!(e.len_bytes(), new.len());
        typ(&mut e, "u");
        assert_eq!(e.text(), old);
    }
}

/// From Insert or Visual mode (or with the search prompt open) it ends in Normal mode.
#[test]
fn replace_text_ends_in_normal_mode() {
    for keys in ["A typed", "vl", "/SEL", "d"] {
        let mut e = at("SELECT 1", (0, 0));
        typ(&mut e, keys);
        assert!(e.replace_text("SELECT 2"), "{keys}");
        assert_eq!((e.mode, e.awaiting_key(), e.searching()), (Mode::Normal, false, false), "{keys}");
        assert_eq!(e.text(), "SELECT 2");
    }
}
