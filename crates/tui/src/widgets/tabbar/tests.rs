use super::{Cell, Half, Part, PartKind, TabHit, drag_window, mend, put_tab, window};
use ratatui::style::Style;

#[test]
fn window_keeps_the_active_tab_and_fills_the_room() {
    let w = [10, 10, 10, 10, 10];
    assert_eq!(window(&w, 0, 25), (0, 1));
    assert_eq!(window(&w, 4, 25), (3, 4));
    assert_eq!(window(&w, 2, 30), (1, 3));
    // A tab wider than the room is shown alone (and clipped when drawn).
    assert_eq!(window(&[40, 5], 0, 20), (0, 0));
    assert_eq!(window(&w, 2, 50), (0, 4));
}

#[test]
fn a_dragged_tab_scrolls_the_bar_one_tab_at_a_time() {
    // Ten tabs of 10 columns, room for four.
    let widths = [10; 10];
    assert_eq!(drag_window(&widths, 2, 0, 40), (0, 3), "in view: the bar stays");
    assert_eq!(drag_window(&widths, 4, 0, 40), (1, 4), "one past the right end: one tab");
    assert_eq!(drag_window(&widths, 5, 1, 40), (2, 5));
    assert_eq!(drag_window(&widths, 9, 6, 40), (6, 9));
    assert_eq!(drag_window(&widths, 5, 6, 40), (5, 8), "one past the left end: one tab");
    assert_eq!(drag_window(&widths, 0, 9, 40), (0, 3));
    assert_eq!(drag_window(&widths, 3, 42, 40), (3, 6), "a first tab that is gone");
}

fn plain(text: &str) -> Vec<Part> {
    vec![Part { text: text.to_string(), style: Style::new(), kind: PartKind::Plain }]
}

/// The symbols of a line (`.` for a column nothing was drawn on, `|` for a wide character's
/// second column).
fn symbols(line: &[Option<Cell>]) -> String {
    line.iter()
        .map(|c| match c {
            None => ".".to_string(),
            Some(c) if c.half == Half::Right => "|".to_string(),
            Some(c) => c.symbol.clone(),
        })
        .collect()
}

#[test]
fn a_tab_drawn_over_another_never_leaves_half_a_wide_character() {
    // Six wide characters (Hangul syllables) over twelve columns.
    let wide = "\u{AC00}\u{B098}\u{B2E4}\u{B77C}\u{B9C8}\u{BC14}";
    let mut line = vec![None; 12];
    put_tab(&mut line, 0, 12, 0, &plain(wide), 0, None);
    assert_eq!(symbols(&line), "\u{AC00}|\u{B098}|\u{B2E4}|\u{B77C}|\u{B9C8}|\u{BC14}|");
    // A tab drawn from the second column of one to the first of another: both are cut whole.
    put_tab(&mut line, 0, 12, 3, &plain("abc"), 1, None);
    mend(&mut line);
    assert_eq!(symbols(&line), "\u{AC00}| abc\u{B77C}|\u{B9C8}|\u{BC14}|");
    assert_eq!(line[2].as_ref().map(|c| c.hit), Some(TabHit::Tab(0)), "the blank left is still the tab's");
    assert_eq!(line[3].as_ref().map(|c| c.hit), Some(TabHit::Tab(1)));
    // A wide character over two narrow ones, and the end of a tab cut off at the left edge.
    let mut line = vec![None; 6];
    put_tab(&mut line, 10, 16, 10, &plain("abcdef"), 0, None);
    put_tab(&mut line, 10, 16, 9, &plain("\u{AC00}\u{B098}"), 1, None);
    mend(&mut line);
    assert_eq!(symbols(&line), " \u{B098}|def", "the first one's left half is off the bar");
    // Cut at the right edge: shortened with `…` at a whole character, as before.
    let mut line = vec![None; 4];
    put_tab(&mut line, 0, 4, 0, &plain("a\u{AC00}\u{B098}"), 0, None);
    mend(&mut line);
    assert_eq!(symbols(&line), "a\u{AC00}|\u{2026}");
}
