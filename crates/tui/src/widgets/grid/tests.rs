use super::*;

/// A grid laid out as columns of `widths` (each `"│ " + w + " "`) in a view `view` characters
/// wide right of a gutter that ends at column 4, scrolled `x_off`, the cursor on `col`.
fn laid_out(widths: &[usize], view: u16, x_off: usize, col: usize) -> GridState {
    let mut g = GridState { x_off, col, view_w: view, gutter_x: (0, 4), ..GridState::default() };
    let mut end = 0;
    for w in widths {
        g.col_x.push(end);
        end += w + 3;
    }
    g.col_x.push(end);
    g
}

#[test]
fn the_view_stops_at_the_columns_ends() {
    // 3 columns of 12: 36 characters in a view of 20.
    let mut g = laid_out(&[9, 9, 9], 20, 0, 0);
    assert_eq!(g.x_max(), 16);
    g.scroll(0, -4, 0);
    assert_eq!(g.x_off, 0);
    g.scroll(0, 100, 0);
    assert_eq!(g.x_off, 16);
    // Every column fits: no scrolling at all.
    let mut g = laid_out(&[4, 4], 40, 0, 0);
    assert_eq!(g.x_max(), 0);
    g.scroll(0, 4, 0);
    assert_eq!(g.x_off, 0);
    // No room at all: the row's whole length.
    let g = laid_out(&[9, 9], 0, 0, 0);
    assert_eq!(g.x_max(), 24);
}

#[test]
fn follow_moves_the_view_as_little_as_it_takes() {
    // Whole on screen: nothing moves.
    let mut g = laid_out(&[9, 9, 9], 20, 4, 1);
    g.follow();
    assert_eq!(g.x_off, 4);
    // Cut on the right: up to its end.
    let mut g = laid_out(&[9, 9, 9], 20, 4, 2);
    g.follow();
    assert_eq!(g.x_off, 36 - 20);
    // Cut on the left: from its start.
    let mut g = laid_out(&[9, 9, 9], 20, 16, 0);
    g.follow();
    assert_eq!(g.x_off, 0);
    // Wider than the view: from its start, unless it fills the view already.
    let mut g = laid_out(&[9, 37, 9], 20, 0, 1);
    g.follow();
    assert_eq!(g.x_off, 12);
    let mut g = laid_out(&[9, 37, 9], 20, 24, 1);
    g.follow();
    assert_eq!(g.x_off, 24, "the view is inside the column: it stays");
    let mut g = laid_out(&[9, 37, 9], 20, 40, 1);
    g.follow();
    assert_eq!(g.x_off, 12, "partly shown: from its start");
    // A view of no width: the column's start.
    let mut g = laid_out(&[9, 9], 0, 0, 1);
    g.follow();
    assert_eq!(g.x_off, 12);
}

#[test]
fn a_drag_past_an_edge_steps_four_characters_and_never_snaps_back() {
    // Right edge: the view moves four, the cursor on the last column whole on screen.
    let mut g = laid_out(&[9, 9, 9], 20, 0, 0);
    assert_eq!(g.drag_edge(30), Some(1));
    assert_eq!(g.x_off, 4);
    assert_eq!(g.drag_edge(10), None, "within the view");
    // Left edge (over the gutter): back four, the first column whole on screen.
    assert_eq!(g.drag_edge(2), Some(0));
    assert_eq!(g.x_off, 0);
    // Across a column wider than the view: the cursor on it, the view from its start, then four
    // characters an event where the drag puts it (it fills the view), never back.
    let mut g = laid_out(&[9, 37, 9], 20, 0, 0);
    let drag = |g: &mut GridState| {
        g.col = g.drag_edge(30).unwrap();
        g.follow();
    };
    drag(&mut g);
    assert_eq!((g.col, g.x_off), (1, 12));
    for step in 1..=5 {
        drag(&mut g);
        assert_eq!((g.col, g.x_off), (1, 12 + 4 * step), "step {step}");
    }
    // Past it: the next column, up to its end.
    drag(&mut g);
    assert_eq!((g.col, g.x_off), (2, g.x_max()));
    // A wide last column at the end: the view stays at its end, never flips back.
    let mut g = laid_out(&[9, 37], 20, 0, 0);
    for _ in 0..20 {
        drag(&mut g);
    }
    assert_eq!((g.col, g.x_off), (1, g.x_max()));
    drag(&mut g);
    assert_eq!(g.x_off, g.x_max());
}
