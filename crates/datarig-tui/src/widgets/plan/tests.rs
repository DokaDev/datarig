use super::*;

#[test]
fn bars_fill_by_eighths_and_keep_their_width() {
    assert_eq!(bar(0.0, 4), "    ");
    assert_eq!(bar(1.0, 4), "████");
    assert_eq!(bar(0.5, 4), "██  ");
    assert_eq!(bar(0.5 + 1.0 / 32.0, 4), "██▏ ");
    assert_eq!(bar(0.0001, 4), "▏   ", "a share that is not zero shows");
    assert_eq!(bar(2.0, 3), "███", "never past its width");
    for s in [0.0, 0.13, 0.5, 0.99, 1.0] {
        assert_eq!(width(&bar(s, 9)), 9);
    }
}

#[test]
fn numbers_fit_a_few_columns() {
    assert_eq!(fmt_num(999.0), "999");
    assert_eq!(fmt_num(61855.67), "61.9k");
    assert_eq!(fmt_num(1_234_567.0), "1.2M");
    assert_eq!(fmt_num(3.4e9), "3.4G");
    assert_eq!(fmt_num(0.5), "0.5");
    assert_eq!(fmt_num(12.0), "12");
    assert_eq!(fmt_num(0.04), "0");
    assert_eq!(fmt_ms(0.0123), "0.012 ms");
    assert_eq!(fmt_ms(54.71), "54.7 ms");
    assert_eq!(fmt_ms(2345.0), "2.35 s");
    assert_eq!(fmt_share(0.0), "0%");
    assert_eq!(fmt_share(0.004), "<1%");
    assert_eq!(fmt_share(0.4149), "41%");
    assert_eq!(fmt_share(0.1996), "19%", "never rounded up to hot");
    assert_eq!(fmt_share(0.2), "20%");
}

#[test]
fn deep_guides_are_cut_at_the_left_and_lines_move_sideways() {
    assert_eq!(tree::cut_left("│ │ ├─▾ ", 20), "│ │ ├─▾ ");
    assert_eq!(tree::cut_left("│ │ │ │ ├─▾ ", 6), "… ├─▾ ");
    assert_eq!(raw::skip_cols("  ->  Seq Scan", 6), "Seq Scan");
    assert_eq!(raw::skip_cols("ab\u{AC00}c", 3), " c", "a wide character cut in two is a space");
    assert_eq!(raw::skip_cols("abc", 9), "");
}
