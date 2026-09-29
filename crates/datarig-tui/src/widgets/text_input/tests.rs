use super::*;
use ratatui::crossterm::event::KeyEvent;

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typ(t: &mut TextInput, s: &str) {
    for c in s.chars() {
        t.handle_key(&k(KeyCode::Char(c)));
    }
}

#[test]
fn grapheme_editing_and_cursor_column() {
    let mut t = TextInput::default();
    typ(&mut t, "日本🐘e\u{301}");
    assert_eq!(t.text(), "日本🐘e\u{301}");
    assert_eq!(t.cursor(), 4, "é (e + combining) is one grapheme");
    t.handle_key(&k(KeyCode::Backspace));
    assert_eq!(t.text(), "日本🐘");
    t.handle_key(&k(KeyCode::Left));
    t.handle_key(&k(KeyCode::Backspace));
    assert_eq!(t.text(), "日🐘");
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
    let x = t.render(Rect::new(2, 0, 10, 1), &mut buf, Style::new(), true, false, None);
    assert_eq!(x, 2 + 2, "cursor after one wide syllable");
    t.handle_key(&k(KeyCode::End));
    t.handle_key(&k(KeyCode::Delete));
    assert_eq!(t.text(), "日🐘");
    t.handle_key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(t.text(), "");
}

#[test]
fn scrolls_to_keep_cursor_visible_and_masks() {
    let mut t = TextInput::new("あいうえおかき");
    let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
    let x = t.render(Rect::new(0, 0, 6, 1), &mut buf, Style::new(), true, false, None);
    assert_eq!(x, 5);
    t.handle_key(&k(KeyCode::Home));
    assert_eq!(t.render(Rect::new(0, 0, 6, 1), &mut buf, Style::new(), true, false, None), 0);
    let mut p = TextInput::new("secret");
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
    p.render(Rect::new(0, 0, 10, 1), &mut buf, Style::new(), true, true, None);
    assert_eq!(buf[(0, 0)].symbol(), MASK);
    assert_eq!(buf[(5, 0)].symbol(), MASK);
    let mut d = TextInput::new("pg://u:pw@h");
    d.render(Rect::new(0, 0, 10, 1), &mut buf, Style::new(), false, false, Some((7, 9)));
    let shown: String = (0..10).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    assert_eq!(shown, "pg://u:••@", "unfocused input shows its start");
    d.render(Rect::new(0, 0, 10, 1), &mut buf, Style::new(), true, false, Some((7, 9)));
    let shown: String = (0..10).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    assert_eq!(shown, "://u:••@h ", "focused input scrolls to the cursor");
}

#[test]
fn paste_strips_newlines() {
    let mut t = TextInput::new("ab");
    t.handle_key(&k(KeyCode::Left));
    t.insert_str("x\ny");
    assert_eq!(t.text(), "ax yb");
    assert_eq!(t.cursor(), 4);
}
