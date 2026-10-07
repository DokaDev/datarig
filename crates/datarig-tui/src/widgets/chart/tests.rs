use super::*;
use datarig_core::chart::{Builder, Kind, Spec, roles};
use datarig_core::driver::{ColumnMeta, ValueKind};

fn meta(name: &str, ty: &str, numeric: bool) -> ColumnMeta {
    // The kind the PostgreSQL driver gives a column of that type.
    let kind = match ty {
        "int4" => ValueKind::Integer,
        "float8" => ValueKind::Float,
        "date" => ValueKind::Date,
        "text" => ValueKind::Text,
        _ => ValueKind::Other,
    };
    ColumnMeta { name: name.into(), type_name: ty.into(), numeric, json: false, kind, origin: None }
}

/// A line chart's model of `values` over the row number.
fn line_of(values: &[Option<f64>]) -> Model {
    let cols = [meta("n", "float8", true)];
    let spec = Spec { kind: Kind::Line, x: None, ys: vec![0], by: None, log: false };
    let rows: Vec<Vec<Option<String>>> = values.iter().map(|v| vec![v.map(|v| v.to_string())]).collect();
    let roles = roles(&cols, &rows);
    let mut b = Builder::new(&spec, &cols, &roles);
    for (i, r) in rows.iter().enumerate() {
        b.push(i, r);
    }
    b.finish().unwrap()
}

#[test]
fn bars_fill_eighths_from_a_baseline_on_a_cell_boundary() {
    let t = scale::linear(0.0, 100.0, 6, true);
    // 10 cells: 50 fills 40 eighths from 0.
    assert_eq!(bar_cells(&t, 50.0, 10), Some((0, 40)));
    assert_eq!(bar_cells(&t, 100.0, 10), Some((0, 80)));
    // A small value shows at least an eighth; zero shows nothing.
    assert_eq!(bar_cells(&t, 0.01, 10), Some((0, 1)));
    assert_eq!(bar_cells(&t, 0.0, 10), Some((0, 0)));
    // Negative values grow down from a baseline rounded to a cell boundary.
    let t = scale::linear(-50.0, 100.0, 4, true);
    let (base, at) = bar_cells(&t, -25.0, 9).unwrap();
    assert_eq!(base % 8, 0);
    assert!(at < base, "{base} {at}");
    // A log axis has no zero: bars start at its bottom, values of zero or less are not drawn.
    let t = scale::log(1.0, 1000.0, 5);
    assert_eq!(bar_cells(&t, 1000.0, 3), Some((0, 24)));
    assert_eq!(bar_cells(&t, 1.0, 3), Some((0, 1)));
    assert_eq!(bar_cells(&t, 0.0, 3), None);
    assert_eq!(bar_cells(&t, -4.0, 3), None);
}

#[test]
fn lines_stay_in_their_plot_and_break_at_missing_values() {
    let m = line_of(&[Some(0.0), Some(10.0), None, Some(5.0), Some(10.0)]);
    let t = scale::linear(0.0, 10.0, 3, false);
    let r = rasterize(&m, &t, 10, 3);
    assert_eq!(r.cells.len(), 30);
    assert_eq!(r.cols.len(), 5);
    assert_eq!((r.cols[0], r.cols[4]), (0, 9), "the first and last point at the ends");
    // Bottom-left dot (the first value, 0) and top-right (the last, 10).
    assert_ne!(r.cells[20].0 & dot_bit(0, 3), 0);
    assert_ne!(r.cells[9].0 & dot_bit(1, 0), 0);
    // No line through the missing value: the columns between points 1 and 3 hold only
    // what those points drew.
    let between = (r.cols[1] + 1..r.cols[3]).filter(|&x| (0..3).any(|y| r.cells[y * 10 + x as usize].0 != 0)).count();
    assert_eq!(between, 0, "{:?}", r.cells);
}

#[test]
fn a_large_line_chart_walks_each_point_and_dot_column_a_bounded_number_of_times() {
    let n = 100_000;
    let values: Vec<Option<f64>> = (0..n).map(|i| Some(((i * 7919) % 1000) as f64)).collect();
    let m = line_of(&values);
    let t = scale::linear(0.0, 1000.0, 5, false);
    take_work();
    let r = rasterize(&m, &t, 100, 20);
    let walked = take_work();
    assert!(walked <= 2 * n as u64 + 200, "{walked}");
    // Every column has dots: the values jump about.
    assert!((0..100).all(|x| (0..20).any(|y| r.cells[y * 100 + x].0 != 0)));
}

#[test]
fn points_place_by_their_value_or_their_index() {
    let m = line_of(&[Some(1.0), Some(2.0), Some(3.0)]);
    assert_eq!((0..3).map(|i| x_frac(&m, i, m.x_range())).collect::<Vec<_>>(), [0.0, 0.5, 1.0]);
    let cols = [meta("x", "int4", true), meta("y", "int4", true)];
    let spec = Spec { kind: Kind::Line, x: Some(0), ys: vec![1], by: None, log: false };
    let rows = vec![
        vec![Some("0".to_string()), Some("1".to_string())],
        vec![Some("1".into()), Some("1".into())],
        vec![Some("10".into()), Some("1".into())],
    ];
    let roles = roles(&cols, &rows);
    let mut b = Builder::new(&spec, &cols, &roles);
    for (i, r) in rows.iter().enumerate() {
        b.push(i, r);
    }
    let m = b.finish().unwrap();
    assert_eq!((0..3).map(|i| x_frac(&m, i, m.x_range())).collect::<Vec<_>>(), [0.0, 0.1, 1.0]);
}

/// Draw `c` into a buffer exactly `w`×`h` (what a copy of the drawing does).
fn draw_exact(c: &mut ChartTab, w: u16, h: u16) -> Buffer {
    let th = crate::theme::DARK;
    let i18n = I18n::new(datarig_core::i18n::Lang::En);
    let cx = Look { i18n: &i18n, th: &th, focused: true, hover: None, keys: Default::default() };
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    draw_into(&cx, c, Info { rows: 2, more: false }, area, &mut buf);
    buf
}

#[test]
fn many_series_in_a_narrow_pane_stay_inside_it() {
    let cols: Vec<(String, &str, bool)> = (0..6).map(|i| (format!("v{i}"), "float8", true)).collect();
    let cols: Vec<ColumnMeta> = cols.iter().map(|(n, t, num)| meta(n, t, *num)).collect();
    let rows: Vec<Vec<Option<String>>> =
        (0..2).map(|r| (0..6).map(|i| Some(format!("-0.00000{}", r + i + 1))).collect()).collect();
    let rs = crate::widgets::grid::ResultSet::in_memory(cols, rows, false, "NULL");
    for kind in [Kind::Bar, Kind::HBar, Kind::Line] {
        let mut c = crate::app::chart::ChartTab::for_tests(&rs);
        c.spec = Spec { kind, x: None, ys: (0..6).collect(), by: None, log: false };
        c.rebuild_for_tests(&rs);
        for w in 1..=40 {
            for h in 1..=16 {
                let _ = draw_exact(&mut c, w, h);
            }
        }
    }
}

#[test]
fn lines_split_by_a_column_join_their_own_points() {
    let cols = [meta("day", "date", false), meta("region", "text", false), meta("n", "int4", true)];
    let rows: Vec<Vec<Option<String>>> = (1..=8)
        .map(|d| {
            vec![
                Some(format!("2026-10-{d:02}")),
                Some(if d % 2 == 0 { "a" } else { "b" }.to_string()),
                Some(d.to_string()),
            ]
        })
        .collect();
    let spec = Spec { kind: Kind::Line, x: Some(0), ys: vec![2], by: Some(1), log: false };
    let roles = roles(&cols, &rows);
    let mut b = Builder::new(&spec, &cols, &roles);
    for (i, r) in rows.iter().enumerate() {
        b.push(i, r);
    }
    let m = b.finish().unwrap();
    let t = scale::linear(0.0, 8.0, 3, false);
    let r = rasterize(&m, &t, 40, 8);
    let dots: u32 = r.cells.iter().map(|c| c.0.count_ones()).sum();
    // Eight points, joined by four segments a series: far more dots than points.
    assert!(dots > 40, "{dots} dots: the lines were broken at the other series' days");
}

#[test]
fn bar_labels_never_run_into_each_other() {
    let cols = vec![meta("name", "text", false), meta("n", "int4", true)];
    let names: Vec<String> =
        (0..15).map(|i| format!("{}{} {}", "\u{6A59}".repeat(1 + i % 3), "x".repeat(i % 4), i)).collect();
    let rows: Vec<Vec<Option<String>>> =
        names.iter().enumerate().map(|(i, n)| vec![Some(n.clone()), Some((100 - i).to_string())]).collect();
    let rs = crate::widgets::grid::ResultSet::in_memory(cols, rows, false, "NULL");
    let mut c = crate::app::chart::ChartTab::for_tests(&rs);
    c.spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1], by: None, log: false };
    c.rebuild_for_tests(&rs);
    for w in [60, 90, 120, 148, 150] {
        for cursor in [0, 3, 7, 14] {
            c.cursor = cursor;
            let buf = draw_exact(&mut c, w, 20);
            // The labels' line: under the axis line.
            let y = (0..20).find(|&y| buf[(c.plot.x, y)].symbol() == "─").unwrap() + 1;
            let mut line = String::new();
            let mut x = 0;
            while x < w {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += width(sym).max(1) as u16;
            }
            for word in line.split("  ").map(str::trim).filter(|t| !t.is_empty()) {
                let whole = names.iter().any(|n| n == word);
                let cut =
                    word.strip_suffix('\u{2026}').is_some_and(|p| names.iter().any(|n| n.starts_with(p.trim_end())));
                assert!(
                    whole || cut || word.chars().all(|c| c == ' '),
                    "{w} wide, cursor {cursor}: {word:?} in {line:?}"
                );
            }
        }
    }
}

#[test]
fn a_split_line_breaks_at_its_own_missing_value() {
    let cols = [meta("day", "date", false), meta("shop", "text", false), meta("n", "int4", true)];
    let rows: Vec<Vec<Option<String>>> = [("01", "a", Some("0")), ("02", "a", None), ("03", "a", Some("8"))]
        .iter()
        .chain([("01", "b", Some("4")), ("02", "b", Some("4")), ("03", "b", Some("4"))].iter())
        .map(|(d, s, v)| vec![Some(format!("2026-10-{d}")), Some(s.to_string()), v.map(String::from)])
        .collect();
    let spec = Spec { kind: Kind::Line, x: Some(0), ys: vec![2], by: Some(1), log: false };
    let roles = roles(&cols, &rows);
    let mut b = Builder::new(&spec, &cols, &roles);
    for (i, r) in rows.iter().enumerate() {
        b.push(i, r);
    }
    let m = b.finish().unwrap();
    let t = scale::linear(0.0, 8.0, 3, false);
    let r = rasterize(&m, &t, 40, 8);
    // Series a: only its two dots (the NULL between them breaks its line).
    let dots: u32 = r.cells.iter().filter(|c| c.1 == 1).map(|c| c.0.count_ones()).sum();
    assert!(dots <= 2, "{dots} dots of series a: drawn through its NULL");
}

#[test]
fn a_long_label_stays_inside_the_plot() {
    let cols = vec![meta("region", "text", false), meta("a", "int4", true), meta("b", "int4", true)];
    let rows =
        vec![vec![Some("Northern Europe and the Baltic states".to_string()), Some("100".into()), Some("200".into())]];
    let rs = crate::widgets::grid::ResultSet::in_memory(cols, rows, false, "NULL");
    let mut c = crate::app::chart::ChartTab::for_tests(&rs);
    c.spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1, 2], by: None, log: false };
    c.rebuild_for_tests(&rs);
    let th = crate::theme::DARK;
    let i18n = I18n::new(datarig_core::i18n::Lang::En);
    let cx = Look { i18n: &i18n, th: &th, focused: true, hover: None, keys: Default::default() };
    for w in 12..=30 {
        let area = Rect::new(30, 0, w, 14);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30 + w, 14));
        draw_into(&cx, &mut c, Info { rows: 1, more: false }, area, &mut buf);
        for y in 0..14 {
            for x in 0..30 {
                assert_eq!(buf[(x, y)].symbol(), " ", "{w} wide: drawn left of the chart at ({x}, {y})");
            }
        }
    }
}
