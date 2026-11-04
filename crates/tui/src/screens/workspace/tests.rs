use super::*;

/// `1  2  3  Chg 2  Chart  Msgs`: the label widths of three results, the Changes, the Chart
/// and Messages.
const WIDTHS: [usize; 6] = [1, 1, 1, 5, 5, 4];

#[test]
fn a_narrow_strip_keeps_the_shown_tab_the_changes_and_messages() {
    // Everything fits.
    assert_eq!(strip_window(&WIDTHS, Some(0), Some(3), 40), Some(vec![0, 1, 2, 3, 4, 5]));
    // The shown one, the Changes and Messages; the neighbours while they fit.
    assert_eq!(strip_window(&WIDTHS, Some(0), Some(3), 23), Some(vec![0, 1, 2, 3, 5]));
    assert_eq!(strip_window(&WIDTHS, Some(0), Some(3), 20), Some(vec![0, 3, 5]));
    // The Changes shown: its neighbours around it.
    assert_eq!(strip_window(&WIDTHS, Some(3), Some(3), 20), Some(vec![2, 3, 5]));
    // Not room for the three: the Changes are left out first.
    assert_eq!(strip_window(&WIDTHS, Some(0), Some(3), 10), Some(vec![0, 5]));
    // Without the Changes, as before.
    assert_eq!(strip_window(&[1, 1, 5, 4], Some(0), None, 10), Some(vec![0, 3]));
    assert_eq!(strip_window(&[1, 1, 5, 4], Some(0), None, 9), None);
}
