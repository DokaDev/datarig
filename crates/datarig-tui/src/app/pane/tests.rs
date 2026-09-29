use super::*;

#[test]
fn the_split_keeps_both_panes_usable() {
    assert_eq!(results_rows(40, 50), 20);
    assert_eq!(results_rows(40, PaneLayout::DEFAULT_SHARE), 24);
    // Rounded to the nearest line.
    assert_eq!(results_rows(43, PaneLayout::DEFAULT_SHARE), 26);
    assert_eq!(results_rows(40, 20), 8);
    assert_eq!(results_rows(40, 80), 32);
    // The minimums win over the share.
    assert_eq!(results_rows(20, 20), MIN_RESULTS_ROWS);
    assert_eq!(results_rows(12, 80), 12 - MIN_EDITOR_ROWS);
    // Too little room for both minimums: half each.
    assert_eq!(results_rows(9, 80), 4);
}

#[test]
fn the_share_stays_within_its_limits() {
    assert_eq!(PaneLayout::clamped(5), PaneLayout::MIN);
    assert_eq!(PaneLayout::clamped(95), PaneLayout::MAX);
    assert_eq!(PaneLayout::clamped(55), 55);
}
