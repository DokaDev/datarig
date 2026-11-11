use super::*;

/// The text of `pieces`.
fn text(pieces: &[Piece]) -> String {
    pieces.iter().map(|(t, _)| t.as_str()).collect()
}

/// A filter of two conditions and an order of two keys.
fn both() -> Vec<Part> {
    vec![Part::filter("\u{25bc}", "status = 'shipped'", 2), Part::order("total_amount", Dir::Desc, 2)]
}

#[test]
fn the_whole_status_when_it_fits() {
    let s = fit(&both(), None, 80);
    assert_eq!(text(&s), "\u{25bc} status = 'shipped' +1 \u{00b7} total_amount \u{2193} +1");
    // One condition and one key: no count.
    let one = [Part::filter("\u{25bc}", "id = 1", 1), Part::order("id", Dir::Asc, 1)];
    assert_eq!(text(&fit(&one, None, 80)), "\u{25bc} id = 1 \u{00b7} id \u{2191}");
    // The glyphs and the counts in the accent, the texts not.
    assert_eq!(s[0], ("\u{25bc}".to_string(), Role::Glyph));
    assert_eq!(s[1].1, Role::Text);
    assert_eq!(s[2], (" +1".to_string(), Role::Count));
    assert_eq!(s[3].1, Role::Sep);
}

#[test]
fn narrower_the_first_condition_is_cut_then_each_part_counted_then_tighter() {
    let whole = width(&text(&fit(&both(), None, 80)));
    // One column short: the condition's text is cut with `…`.
    let s = text(&fit(&both(), None, whole - 1));
    assert_eq!(s, "\u{25bc} status = 'shippe\u{2026} +1 \u{00b7} total_amount \u{2193} +1");
    assert_eq!(width(&s), whole - 1);
    // Cut down to `CUT_MIN` columns of it at the least.
    let least = whole - (width("status = 'shipped'") - CUT_MIN);
    assert_eq!(text(&fit(&both(), None, least)), "\u{25bc} status \u{2026} +1 \u{00b7} total_amount \u{2193} +1");
    // Fewer: counted.
    assert_eq!(text(&fit(&both(), None, least - 1)), "\u{25bc} 2 \u{00b7} \u{2193} 2");
    assert_eq!(text(&fit(&both(), None, 9)), "\u{25bc} 2 \u{00b7} \u{2193} 2");
    // Tighter.
    assert_eq!(text(&fit(&both(), None, 8)), "\u{25bc}2 \u{2193}2");
    assert_eq!(text(&fit(&both(), None, 5)), "\u{25bc}2 \u{2193}2");
    // Even narrower: what fits of it, the filter first.
    assert_eq!(text(&fit(&both(), None, 3)), "\u{25bc}2 ");
}

#[test]
fn an_order_alone_is_counted_but_never_cut() {
    let order = [Part::order("total_amount", Dir::Desc, 3)];
    assert_eq!(text(&fit(&order, None, 40)), "total_amount \u{2193} +2");
    assert_eq!(text(&fit(&order, None, 16)), "\u{2193} 3");
    assert_eq!(text(&fit(&order, None, 2)), "\u{2193}3");
}

#[test]
fn a_run_on_its_way_puts_the_spinner_first() {
    let s = fit(&both(), Some("\u{280b}"), 80);
    assert_eq!(s[0], ("\u{280b} ".to_string(), Role::Spinner));
    assert!(text(&s).starts_with("\u{280b} \u{25bc} status"));
    assert_eq!(text(&fit(&both(), Some("\u{280b}"), 10)), "\u{280b} \u{25bc}2 \u{2193}2");
}

#[test]
fn a_failed_run_says_so_in_every_form() {
    let failed = [Part::failed("\u{25bc}", "failed")];
    for room in [40, 8] {
        let s = fit(&failed, None, room);
        assert_eq!(s, vec![("\u{25bc} failed".to_string(), Role::Warning)]);
    }
    assert!(fit(&[], None, 40).is_empty(), "nothing applied: nothing");
}
