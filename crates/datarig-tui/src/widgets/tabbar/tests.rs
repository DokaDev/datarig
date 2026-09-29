use super::window;

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
