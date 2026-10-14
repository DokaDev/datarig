use super::*;

/// Three tabs of 10 columns each from column 10, as the bar draws them: the body, its `×` on
/// the 8th column, then the body's last blanks.
fn three() -> Vec<(u16, u16, TabHit)> {
    let mut hits = Vec::new();
    for i in 0..3u16 {
        let x = 10 + i * 10;
        hits.push((x, x + 7, TabHit::Tab(i.into())));
        hits.push((x + 7, x + 8, TabHit::Close(i.into())));
        hits.push((x + 8, x + 10, TabHit::Tab(i.into())));
    }
    hits
}

#[test]
fn a_tab_lands_before_or_after_the_tab_under_the_pointer_by_its_half() {
    let hits = three();
    assert_eq!(drop_slot(&hits, 10), Some(0));
    assert_eq!(drop_slot(&hits, 14), Some(0));
    assert_eq!(drop_slot(&hits, 15), Some(1));
    assert_eq!(drop_slot(&hits, 17), Some(1), "the `×` is part of its tab");
    assert_eq!(drop_slot(&hits, 24), Some(1));
    assert_eq!(drop_slot(&hits, 25), Some(2));
    assert_eq!(drop_slot(&hits, 39), Some(3));
    // Left or right of the tabs: the nearest end.
    assert_eq!(drop_slot(&hits, 0), Some(0));
    assert_eq!(drop_slot(&hits, 70), Some(3));
    assert_eq!(drop_slot(&[], 5), None, "no tab drawn");
}

#[test]
fn over_a_scroll_mark_a_tab_lands_past_the_hidden_tab_on_that_side() {
    // `‹` for hidden tab 3, tabs 4 and 5 drawn, `›` for hidden tab 6.
    let hits = vec![
        (0, 1, TabHit::More(3)),
        (1, 9, TabHit::Tab(4)),
        (9, 10, TabHit::Close(4)),
        (10, 18, TabHit::Tab(5)),
        (18, 19, TabHit::Close(5)),
        (19, 20, TabHit::More(6)),
    ];
    assert_eq!(drop_slot(&hits, 0), Some(3), "before hidden tab 3");
    assert_eq!(drop_slot(&hits, 19), Some(7), "after hidden tab 6");
    assert_eq!(drop_slot(&hits, 40), Some(7), "right of the bar: as over `›`");
    // The bar leaves blanks before `›` when the next tab does not fit: as over the tab before.
    let gap = vec![(0, 9, TabHit::Tab(0)), (9, 10, TabHit::Close(0)), (15, 16, TabHit::More(1))];
    assert_eq!(drop_slot(&gap, 12), Some(1));
    assert_eq!(drop_slot(&gap, 15), Some(2));
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
