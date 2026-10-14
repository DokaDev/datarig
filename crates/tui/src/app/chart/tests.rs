use super::*;
use datarig_core::driver::{ColumnMeta, ValueKind};

fn result(cols: &[(&str, &str, bool)], rows: &[&[&str]]) -> ResultSet {
    let cols = cols
        .iter()
        .map(|(n, t, num)| ColumnMeta {
            name: (*n).into(),
            type_name: (*t).into(),
            numeric: *num,
            json: false,
            // The kind the PostgreSQL driver gives a column of that type.
            kind: match *t {
                "int4" => ValueKind::Integer,
                "date" => ValueKind::Date,
                "text" => ValueKind::Text,
                _ => ValueKind::Other,
            },
            origin: None,
        })
        .collect();
    let rows = rows.iter().map(|r| r.iter().map(|c| Some(c.to_string())).collect()).collect();
    ResultSet::in_memory(cols, rows, false, "NULL")
}

#[test]
fn the_cursor_moves_along_the_bars_and_across_the_series() {
    let rs = result(
        &[("k", "text", false), ("a", "int4", true), ("b", "int4", true)],
        &[&["x", "1", "2"], &["y", "3", "4"], &["z", "5", "6"]],
    );
    let mut c = ChartTab::of(&rs);
    c.build(&rs);
    assert_eq!(c.spec.kind, Kind::Bar);
    c.act(ChartAction::Right);
    c.act(ChartAction::Down);
    assert_eq!((c.cursor, c.series), (1, 1));
    c.act(ChartAction::Down);
    assert_eq!(c.series, 1, "stops at the last series");
    // Horizontal bars run down: the vertical keys move along them.
    c.act(ChartAction::Kind(Kind::HBar));
    c.build(&rs);
    c.act(ChartAction::Down);
    assert_eq!(c.cursor, 2);
    c.act(ChartAction::Left);
    assert_eq!(c.series, 0);
    c.act(ChartAction::First);
    assert_eq!(c.cursor, 0);
    c.act(ChartAction::Last);
    assert_eq!(c.cursor, 2);
    c.act(ChartAction::NextKind(true));
    assert_eq!(c.spec.kind, Kind::Line);
    c.act(ChartAction::Log);
    assert!(c.spec.log);
}

#[test]
fn numbers_are_read_again_only_when_the_rows_or_the_choice_change() {
    let mut rs = result(&[("k", "text", false), ("a", "int4", true)], &[&["x", "1"], &["y", "3"]]);
    let mut c = ChartTab::of(&rs);
    crate::widgets::chart::take_work();
    c.build(&rs);
    assert_eq!(crate::widgets::chart::take_work(), 2);
    c.build(&rs);
    assert_eq!(crate::widgets::chart::take_work(), 0, "kept");
    rs.rows.append(vec![vec![Some("z".into()), Some("5".into())]]).unwrap();
    c.build(&rs);
    assert_eq!(crate::widgets::chart::take_work(), 3, "a page more");
    assert_eq!(c.model().map(|m| m.points.len()), Some(3));
    c.act(ChartAction::Kind(Kind::Line));
    c.build(&rs);
    assert_eq!(crate::widgets::chart::take_work(), 3, "another choice");
    // The cursor stays within the points.
    c.cursor = 99;
    c.spec.kind = Kind::Bar;
    c.build(&rs);
    assert_eq!(c.cursor, 2);
}

#[test]
fn a_chart_follows_another_result_keeping_the_columns_chosen_by_name() {
    let a = result(
        &[("day", "date", false), ("n", "int4", true), ("m", "int4", true)],
        &[&["2026-10-01", "1", "2"], &["2026-10-02", "3", "4"]],
    );
    let mut c = ChartTab::of(&a);
    c.spec.kind = Kind::Bar;
    c.spec.ys = vec![2];
    let b = result(&[("m", "int4", true), ("day", "date", false)], &[&["7", "2026-10-03"], &["8", "2026-10-04"]]);
    c.follow(&b);
    assert_eq!((c.result, c.spec.kind, c.spec.x, c.spec.ys.clone()), (b.id, Kind::Bar, Some(1), vec![0]));
    // Not there by name: chosen afresh.
    let other = result(&[("k", "text", false), ("v", "int4", true)], &[&["x", "1"], &["y", "2"]]);
    c.follow(&other);
    assert_eq!((c.spec.x, c.spec.ys.clone(), c.cursor), (Some(0), vec![1], 0));
    // A result that could not be charted yet is looked at again once it has rows.
    let mut empty = result(&[("k", "text", false), ("v", "int4", true)], &[]);
    let mut c = ChartTab::of(&empty);
    c.build(&empty);
    assert_eq!(c.unsuitable, Some(Unsuitable::NoRows));
    empty
        .rows
        .append(vec![vec![Some("x".into()), Some("1".into())], vec![Some("y".into()), Some("2".into())]])
        .unwrap();
    c.follow(&empty);
    c.build(&empty);
    assert!(c.model().is_some());
}

#[test]
fn the_scale_and_the_bars_direction_reuse_the_numbers() {
    let rs = result(&[("k", "text", false), ("a", "int4", true)], &[&["x", "1"], &["y", "3"]]);
    let mut c = ChartTab::of(&rs);
    c.build(&rs);
    crate::widgets::chart::take_work();
    c.act(ChartAction::Log);
    c.build(&rs);
    c.act(ChartAction::Kind(Kind::HBar));
    c.build(&rs);
    assert_eq!(crate::widgets::chart::take_work(), 0, "nothing read again");
    assert!(c.model().is_some());
    c.act(ChartAction::Kind(Kind::Line));
    c.build(&rs);
    assert_eq!(crate::widgets::chart::take_work(), 2, "lines keep every point: read again");
}
