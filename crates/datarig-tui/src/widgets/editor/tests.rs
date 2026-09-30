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
/// `<Home>`, `<End>`, `<C-x>`.
pub(super) fn typ(e: &mut Editor, keys: &str) {
    let mut rest = keys;
    while let Some(c) = rest.chars().next() {
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

/// Run each case from a fresh editor and compare the text, the cursor and the register.
pub(super) fn check(cases: &[Case]) {
    for &(text, cursor, keys, want, want_cursor, want_reg) in cases {
        let mut e = at(text, cursor);
        typ(&mut e, keys);
        let reg = e.register().map(|r| (r.text.as_str(), r.linewise));
        assert_eq!(
            (e.text().as_str(), (e.row, e.col), reg),
            (want, want_cursor, want_reg),
            "{keys:?} on {text:?} at {cursor:?}"
        );
        assert_eq!(e.mode, Mode::Normal, "{keys:?}: back in Normal mode");
        assert_eq!(e.len_bytes(), e.text().len());
    }
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

/// Every write to the register can be taken once by the app (to pass on to the clipboard).
#[test]
fn register_writes_are_offered_once() {
    let mut e = at("one two\nthree", (0, 0));
    assert_eq!(e.take_yank(), None);
    typ(&mut e, "yw");
    assert_eq!(e.take_yank(), Some(Register { text: "one ".into(), linewise: false }));
    assert_eq!(e.take_yank(), None, "taken");
    typ(&mut e, "jdd");
    assert_eq!(e.take_yank(), Some(Register { text: "three".into(), linewise: true }));
    typ(&mut e, "x");
    assert_eq!(e.take_yank(), Some(Register { text: "o".into(), linewise: false }));
    typ(&mut e, "vy");
    assert_eq!(e.take_yank().map(|r| r.text), Some("n".into()));
    typ(&mut e, "l");
    assert_eq!(e.take_yank(), None, "a motion writes nothing");
    assert_eq!(e.register(), Some(&Register { text: "n".into(), linewise: false }));
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
