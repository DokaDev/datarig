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

#[test]
fn delete_word_back_takes_a_word_or_a_run_of_symbols() {
    let mut t = TextInput::default();
    typ(&mut t, "a.b foo_1  ");
    assert_eq!(t.delete_word_back(), InputResult::Changed);
    assert_eq!(t.text(), "a.b ");
    t.delete_word_back();
    assert_eq!(t.text(), "a.");
    t.delete_word_back();
    assert_eq!(t.text(), "a");
    t.delete_word_back();
    assert_eq!((t.text(), t.cursor()), ("", 0));
    assert_eq!(t.delete_word_back(), InputResult::Ignored);
}

#[test]
fn a_click_puts_the_cursor_under_the_pointer_as_drawn() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
    let mut t = TextInput::new("a日本b");
    assert!(!t.click(3, 0), "not drawn yet: no hit");
    t.render(Rect::new(2, 0, 10, 1), &mut buf, Style::new(), false, false, None);
    // Columns from 2: a, 日 (3-4), 本 (5-6), b (7), then past the text.
    assert!(t.click(2, 0));
    assert_eq!(t.cursor(), 0);
    assert!(t.click(4, 0), "the second half of a wide letter: before it");
    assert_eq!(t.cursor(), 1);
    assert!(t.click(5, 0));
    assert_eq!(t.cursor(), 2);
    assert!(t.click(11, 0), "past the text: the end");
    assert_eq!(t.cursor(), 4);
    assert!(!t.click(12, 0) && !t.click(1, 0) && !t.click(3, 1), "off the input");
    // Scrolled: a click counts from the first column shown.
    let mut s = TextInput::new("あいうえおかき");
    s.render(Rect::new(0, 0, 6, 1), &mut buf, Style::new(), true, false, None);
    assert!(s.click(0, 0));
    assert_eq!(s.cursor(), 4, "the view starts in the middle of お (it ends at the cursor)");
    // Masked: one column a grapheme, whatever its width.
    let mut p = TextInput::new("日本語");
    p.render(Rect::new(0, 0, 10, 1), &mut buf, Style::new(), true, true, None);
    assert!(p.click(1, 0));
    assert_eq!(p.cursor(), 1);
    // A hidden part of a DSN: one column per hidden grapheme.
    let mut d = TextInput::new("pg://u:pw@h");
    d.render(Rect::new(0, 0, 20, 1), &mut buf, Style::new(), false, false, Some((7, 9)));
    assert!(d.click(9, 0));
    assert_eq!(d.cursor(), 9, "the `@` after the hidden password");
}

#[test]
fn spans_style_the_graphemes_they_start_in() {
    use ratatui::style::Color;
    let mut t = TextInput::new("a 日b");
    let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
    let red = Style::new().fg(Color::Red);
    // Bytes 2..5 are the wide syllable, 5..6 the `b`.
    t.render_spans(Rect::new(0, 0, 8, 1), &mut buf, Style::new().bg(Color::Blue), false, &[(2..5, red)]);
    assert_eq!(buf[(2, 0)].symbol(), "日");
    assert_eq!((buf[(2, 0)].fg, buf[(2, 0)].bg), (Color::Red, Color::Blue));
    assert_eq!(buf[(0, 0)].fg, Color::Reset, "outside every span: the base style");
    assert_eq!(buf[(4, 0)].fg, Color::Reset);
}

#[test]
fn replace_puts_a_completion_in_and_the_cursor_after_it() {
    let mut t = TextInput::new("日 na = 1");
    // The cursor after `na` (graphemes: 日, ' ', n, a).
    t.handle_key(&KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    for _ in 0..4 {
        t.handle_key(&KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    }
    assert_eq!(t.cursor_byte(), 6);
    t.replace(4..6, "name");
    assert_eq!((t.text(), t.cursor(), t.cursor_byte()), ("日 name = 1", 6, 8));
    // Inside the wide letter, or past the text: nothing changes.
    t.replace(1..2, "x");
    t.replace(4..40, "x");
    assert_eq!(t.text(), "日 name = 1");
}

#[test]
fn a_click_on_a_copy_drawn_places_the_cursor_where_that_copy_showed_the_text() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
    let area = Rect::new(2, 0, 5, 1);
    // Not focused: drawn from its start.
    let mut t = TextInput::new("abcdefghij");
    assert!(t.click_at(area, false, 4, 0));
    assert_eq!(t.cursor(), 2);
    // Focused with the cursor at its end: drawn scrolled so that the cursor shows, as a render
    // of a copy drew it (`ghij` and the cursor).
    let mut t = TextInput::new("abcdefghij");
    let mut copy = t.clone();
    copy.render(area, &mut buf, Style::new(), true, false, None);
    assert_eq!(buf[(2, 0)].symbol(), "g");
    assert!(t.click_at(area, true, 3, 0));
    assert_eq!(t.cursor(), 7, "before the `h` drawn there");
    // Outside it: nothing.
    assert!(!t.click_at(area, true, 8, 0));
    assert_eq!(t.cursor(), 7);
}

#[test]
fn the_cursor_goes_before_the_grapheme_holding_a_byte() {
    let mut t = TextInput::new("a日b");
    t.set_cursor_byte(1);
    assert_eq!(t.cursor(), 1);
    t.set_cursor_byte(2);
    assert_eq!(t.cursor(), 1, "inside the wide letter: before it");
    t.set_cursor_byte(4);
    assert_eq!(t.cursor(), 2);
    t.set_cursor_byte(40);
    assert_eq!(t.cursor(), 3);
}
