use super::{drag_window, window};

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
