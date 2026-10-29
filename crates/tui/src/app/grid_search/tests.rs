use super::*;
use datarig_core::results::RowStore;

fn rows(cells: &[&[Option<&str>]]) -> RowsSnapshot {
    let rows: Vec<Vec<Option<String>>> =
        cells.iter().map(|r| r.iter().map(|c| c.map(str::to_string)).collect()).collect();
    let n = rows.len();
    RowStore::in_memory(rows).snapshot(0..n).unwrap()
}

/// Every match of `pattern` in `snap`, and the first one from `origin`.
fn search(snap: &RowsSnapshot, ncols: usize, pattern: &str, origin: CellIdx, forward: bool, cap: usize) -> Found {
    let re = compile(pattern).unwrap();
    let mut firsts = Vec::new();
    let stop = AtomicBool::new(false);
    let (cells, capped) =
        scan(snap, ncols, &re, origin, forward, cap, &stop, |c, w| firsts.push((c, w))).unwrap().unwrap();
    Found { cells, capped, firsts }
}

#[derive(Debug, PartialEq)]
struct Found {
    cells: Vec<CellIdx>,
    capped: bool,
    firsts: Vec<(CellIdx, bool)>,
}

#[test]
fn cells_match_in_row_major_order_and_null_never_matches() {
    let s = rows(&[
        &[Some("apple"), None, Some("pear")],
        &[Some("x"), Some("apple pie, apple tart"), None],
        &[None, None, Some("APPLE")],
    ]);
    let f = search(&s, 3, "apple", 0, true, MAX_MATCHES);
    // A cell matching twice counts once; case matters.
    assert_eq!(f.cells, vec![0, 4]);
    assert_eq!(f.firsts, vec![(4, false)], "after the cursor's own cell");
    assert_eq!(search(&s, 3, "(?i)apple", 0, true, MAX_MATCHES).cells, vec![0, 4, 8]);
    // NULL has no text: not even a pattern that matches the empty text finds it.
    assert_eq!(search(&s, 3, "^$", 0, true, MAX_MATCHES).cells, Vec::<CellIdx>::new());
    assert_eq!(search(&s, 3, "NULL", 0, true, MAX_MATCHES).cells, Vec::<CellIdx>::new());
}

#[test]
fn the_raw_value_is_searched_not_the_cut_shown() {
    let long = format!("{}needle", "x".repeat(200));
    let tab = "a\tb\nc";
    let s = rows(&[&[Some(&long), Some(tab), Some(r#"{"k": "v"}"#)]]);
    assert_eq!(search(&s, 3, "needle", 0, true, MAX_MATCHES).cells, vec![0], "past the 40-column cut");
    assert_eq!(search(&s, 3, "a\tb\nc", 0, true, MAX_MATCHES).cells, vec![1], "tab and line break as sent");
    assert_eq!(search(&s, 3, r#"\{"k": "v"\}"#, 0, true, MAX_MATCHES).cells, vec![2], "JSON as sent");
}

#[test]
fn the_first_match_goes_around_the_ends() {
    let s = rows(&[&[Some("a"), Some("b")], &[Some("a"), Some("b")], &[Some("a"), Some("b")]]);
    // Forward from the last `a` (4): around to the first.
    assert_eq!(search(&s, 2, "a", 4, true, MAX_MATCHES).firsts, vec![(0, true)]);
    assert_eq!(search(&s, 2, "a", 1, true, MAX_MATCHES).firsts, vec![(2, false)]);
    // Backward: the match before the cursor, else around to the last one.
    assert_eq!(search(&s, 2, "a", 3, false, MAX_MATCHES).firsts, vec![(2, false)]);
    assert_eq!(search(&s, 2, "a", 0, false, MAX_MATCHES).firsts, vec![(4, true)]);
    // The only match is the cursor's own cell: found again around the end.
    let one = rows(&[&[Some("z"), Some("q")]]);
    assert_eq!(search(&one, 2, "z", 0, true, MAX_MATCHES).firsts, vec![(0, true)]);
    assert_eq!(search(&one, 2, "nowhere", 0, true, MAX_MATCHES).firsts, vec![]);
}

#[test]
fn matches_past_the_cap_are_counted_as_more_and_the_first_is_still_found() {
    let s = rows(&[&[Some("a"); 4], &[Some("a"); 4], &[Some("a"); 4]]);
    let f = search(&s, 4, "a", 9, true, 3);
    assert_eq!((f.cells, f.capped), (vec![0, 1, 2], true));
    assert_eq!(f.firsts, vec![(10, false)], "the first after the cursor, past the cap");
    let f = search(&s, 4, "a", 9, false, 3);
    assert_eq!(f.firsts, vec![(8, false)]);
    assert!(!search(&s, 4, "a", 0, true, 12).capped, "exactly the cap is not more");
}

#[test]
fn a_stopped_scan_says_nothing() {
    let s = rows(&[&[Some("a")]]);
    let stop = AtomicBool::new(true);
    let re = compile("a").unwrap();
    let mut told = false;
    assert!(scan(&s, 1, &re, 0, true, MAX_MATCHES, &stop, |_, _| told = true).unwrap().is_none());
    assert!(!told);
}

#[test]
fn n_and_shift_n_step_through_the_list_and_around_it() {
    let cells = [2, 5, 9];
    assert_eq!(step(&cells, false, 2, true), Some(Some((5, false))));
    assert_eq!(step(&cells, false, 3, true), Some(Some((5, false))), "off a match");
    assert_eq!(step(&cells, false, 9, true), Some(Some((2, true))));
    assert_eq!(step(&cells, false, 5, false), Some(Some((2, false))));
    assert_eq!(step(&cells, false, 2, false), Some(Some((9, true))));
    assert_eq!(step(&[], false, 2, true), Some(None), "nothing matches");
    // A capped list cannot tell what lies past its last cell, nor what is last around the start.
    assert_eq!(step(&cells, true, 5, true), Some(Some((9, false))));
    assert_eq!(step(&cells, true, 9, true), None);
    assert_eq!(step(&cells, true, 12, false), None);
    assert_eq!(step(&cells, true, 2, false), None);
    assert_eq!(step(&cells, true, 6, false), Some(Some((5, false))));
}

#[test]
fn a_match_on_another_page_shows_that_page() {
    let mut g = GridState::paged(500, 20);
    g.anchor = Some((3, 0));
    goto(&mut g, cell(1234, 2, 5), 5);
    assert_eq!((g.page, g.row, g.col, g.anchor), (2, 1234, 2, None));
    g.anchor = Some((1100, 0));
    goto(&mut g, cell(1300, 0, 5), 5);
    assert_eq!((g.page, g.row, g.anchor), (2, 1300, Some((1100, 0))), "the same page keeps a range");
}

/// The next match from `origin` as [`seek`] finds it.
fn seeks(snap: &RowsSnapshot, ncols: usize, pattern: &str, origin: CellIdx, forward: bool) -> Option<(CellIdx, bool)> {
    let re = compile(pattern).unwrap();
    seek(snap, ncols, &re, origin, forward, &AtomicBool::new(false)).unwrap().unwrap()
}

#[test]
fn seeking_finds_the_next_match_from_the_cursor_and_around_the_ends() {
    // 3,000 rows of 2 columns (more than a chunk): `x` in column 1 of rows 5, 1500 and 2990.
    let rows: Vec<Vec<Option<String>>> = (0..3_000)
        .map(|r| vec![None, Some(if [5, 1500, 2990].contains(&r) { "x".into() } else { "y".into() })])
        .collect();
    let snap = RowStore::in_memory(rows).snapshot(0..3_000).unwrap();
    let at = |r: usize| cell(r, 1, 2);
    assert_eq!(seeks(&snap, 2, "x", at(0), true), Some((at(5), false)));
    assert_eq!(seeks(&snap, 2, "x", at(5), true), Some((at(1500), false)), "after the cursor's own cell");
    assert_eq!(seeks(&snap, 2, "x", at(2995), true), Some((at(5), true)));
    assert_eq!(seeks(&snap, 2, "x", at(1500), false), Some((at(5), false)));
    assert_eq!(seeks(&snap, 2, "x", at(3), false), Some((at(2990), true)));
    assert_eq!(seeks(&snap, 2, "x", cell(1500, 0, 2), false), Some((at(5), false)), "before it on its row");
    // The only match is the cursor's: found again around the end; none at all: none.
    let one = rows_of(&[&[Some("z")], &[Some("q")]]);
    assert_eq!(seeks(&one, 1, "z", 0, true), Some((0, true)));
    assert_eq!(seeks(&one, 1, "z", 0, false), Some((0, true)));
    assert_eq!(seeks(&one, 1, "nothing", 0, true), None);
    // NULL never matches.
    assert_eq!(seeks(&snap, 2, "^$", 0, true), None);
    // Told to stop: nothing said.
    let re = compile("x").unwrap();
    assert!(seek(&snap, 2, &re, 0, true, &AtomicBool::new(true)).unwrap().is_none());
}

fn rows_of(cells: &[&[Option<&str>]]) -> RowsSnapshot {
    rows(cells)
}
