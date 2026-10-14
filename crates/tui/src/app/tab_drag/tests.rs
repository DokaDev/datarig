use super::*;

#[test]
fn the_dragged_tab_is_shown_at_its_place_among_the_others() {
    assert_eq!(shown_order(4, 1, 1), [0, 1, 2, 3]);
    assert_eq!(shown_order(4, 1, 3), [0, 2, 3, 1]);
    assert_eq!(shown_order(4, 3, 0), [3, 0, 1, 2]);
    assert_eq!(shown_order(4, 0, 9), [1, 2, 3, 0], "past the end: last");
    assert_eq!(shown_order(1, 0, 0), [0]);
}

#[test]
fn a_tab_next_to_the_gap_moves_over_once_the_dragged_tab_went_past_its_middle() {
    // Three other tabs 10 wide from column 10, the dragged one (any width) at place 0: its gap
    // starts at 10, the next tab is drawn at 10 + its width.
    let others = [10, 10, 10];
    assert_eq!(follow(&others, 0, 0, 3, 10, 10), 0);
    assert_eq!(follow(&others, 0, 0, 3, 10, 15), 0, "half way over the next one: not yet");
    assert_eq!(follow(&others, 0, 0, 3, 10, 16), 1, "past its middle");
    // Back left: past the middle of the tab before the gap (now at 10..20).
    assert_eq!(follow(&others, 1, 0, 3, 10, 15), 1, "on its middle: stays");
    assert_eq!(follow(&others, 1, 0, 3, 10, 14), 0);
    // Not past half of it either way after a move: no back and forth.
    assert_eq!(follow(&others, 1, 0, 3, 10, 16), 1);
}

#[test]
fn a_pointer_that_jumped_passes_every_tab_in_between() {
    let others = [10, 4, 20, 10];
    assert_eq!(follow(&others, 0, 0, 4, 0, 38), 3, "past 10/2, 10+4/2, 14+20/2, not 34+10/2");
    assert_eq!(follow(&others, 0, 0, 4, 0, 40), 4);
    assert_eq!(follow(&others, 4, 0, 4, 0, 0), 0, "and all the way back");
    assert_eq!(follow(&others, 4, 0, 4, 0, 12), 2, "on the middle of the 4 wide tab: not past it");
}

#[test]
fn only_tabs_shown_move_over() {
    // Places 1..=3 shown (from column 1, after `‹`): place 4 is hidden.
    let others = [10, 10, 10, 10, 10];
    assert_eq!(follow(&others, 2, 1, 3, 1, 60), 3, "not past the last place shown");
    assert_eq!(follow(&others, 2, 1, 3, 1, 0), 1, "nor before the first");
    assert_eq!(follow(&others, 0, 1, 3, 1, 30), 0, "a place not shown stays");
}

#[test]
fn a_slot_is_counted_before_the_move() {
    // Tab 1 of four: slots 1 and 2 leave it where it is.
    assert_eq!(slot_index(1, 0), 0);
    assert_eq!(slot_index(1, 1), 1);
    assert_eq!(slot_index(1, 2), 1);
    assert_eq!(slot_index(1, 3), 2);
    assert_eq!(slot_index(1, 4), 3);
}

#[test]
fn tabs_slide_to_new_places_only_when_the_drag_moved_them() {
    let t0 = Instant::now();
    let (a, b) = (TabId(1), TabId(2));
    let mut s = TabSlide::default();
    assert_eq!(s.place(&[(a, 0, false), (b, 10, false)], t0), [0, 10]);
    assert_eq!(s.due(t0), None);
    // Another kind of change (a tab switched, the screen resized): drawn there at once.
    assert_eq!(s.place(&[(a, 5, false), (b, 15, false)], t0), [5, 15]);
    assert_eq!(s.due(t0), None);
    // The drag's change: from where it was drawn to its new place, fast first, then slower.
    s.armed = true;
    assert_eq!(s.place(&[(a, 25, false), (b, 15, false)], t0), [5, 15]);
    assert!(s.moving(a) && !s.moving(b));
    assert_eq!(s.due(t0), Some(t0 + SLIDE_FRAME));
    let mut xs = Vec::new();
    for k in 1..=5 {
        let now = t0 + SLIDE_FRAME * k;
        xs.push(s.place(&[(a, 25, false), (b, 15, false)], now)[0]);
        s.tick(now);
    }
    assert_eq!(xs, [12, 18, 22, 24, 25]);
    assert_eq!(s.due(t0 + SLIDE), None, "over: no more frames");
    // Moved again while sliding: on from where it is drawn (5 + 20 * 0.64 after a frame).
    let t1 = t0 + Duration::from_secs(1);
    s.armed = true;
    s.place(&[(a, 5, false), (b, 15, false)], t1);
    s.armed = true;
    assert_eq!(s.place(&[(a, 25, false), (b, 15, false)], t1 + SLIDE_FRAME)[0], 18);
    assert_eq!(s.place(&[(a, 25, false), (b, 15, false)], t1 + SLIDE_FRAME + SLIDE)[0], 25);
}

#[test]
fn a_tab_held_under_the_pointer_never_slides() {
    let t0 = Instant::now();
    let a = TabId(1);
    let mut s = TabSlide::default();
    s.place(&[(a, 0, false)], t0);
    s.armed = true;
    assert_eq!(s.place(&[(a, 30, true)], t0), [30]);
    assert_eq!(s.due(t0), None);
    // Let go: from where it was held to its place.
    s.armed = true;
    assert_eq!(s.place(&[(a, 10, false)], t0), [30]);
    assert!(s.moving(a));
    s.tick(t0 + SLIDE);
    assert!(!s.moving(a));
    assert_eq!(s.place(&[(a, 10, false)], t0 + SLIDE), [10]);
}
