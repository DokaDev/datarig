use super::*;

fn meta(name: &str, ty: &str, numeric: bool) -> ColumnMeta {
    // The kind a driver gives a column of that type.
    let kind = match ty {
        "int4" | "int8" => ValueKind::Integer,
        "numeric" => ValueKind::Decimal,
        "float8" => ValueKind::Float,
        "int4[]" => ValueKind::Array(crate::driver::ArrayElement::Number),
        "jsonb" => ValueKind::Json,
        "date" => ValueKind::Date,
        "time" | "time with time zone" => ValueKind::Time,
        "timestamptz" => ValueKind::TimestampTz,
        "text" | "varchar" => ValueKind::Text,
        _ => ValueKind::Other,
    };
    ColumnMeta { name: name.into(), type_name: ty.into(), numeric, json: ty.starts_with("json"), kind, origin: None }
}

fn rows(v: &[&[Option<&str>]]) -> Vec<Vec<Cell>> {
    v.iter().map(|r| r.iter().map(|c| c.map(str::to_string)).collect()).collect()
}

fn build(spec: &Spec, cols: &[ColumnMeta], data: &[Vec<Cell>]) -> Result<Model, Unsuitable> {
    let roles = roles(cols, data);
    let mut b = Builder::new(spec, cols, &roles);
    for (i, r) in data.iter().enumerate() {
        b.push(i, r);
    }
    b.finish()
}

#[test]
fn roles_come_from_the_driver_and_from_text_that_holds_dates() {
    let cols = [
        meta("n", "numeric", true),
        meta("d", "date", false),
        meta("ts", "timestamptz", false),
        meta("t", "time", false),
        meta("tz", "time with time zone", false),
        meta("s", "text", false),
        meta("day", "text", false),
        meta("stamp", "varchar", false),
        meta("j", "jsonb", false),
        meta("a", "int4[]", false),
        meta("dt", "DATETIME", false),
    ];
    let data = rows(&[
        &[
            Some("1"),
            Some("2026-10-07"),
            Some("2026-10-07 10:00:00+09"),
            Some("10:00"),
            Some("10:00+09"),
            Some("x"),
            Some("2026-10-07"),
            Some("2026-10-07 10:00"),
            Some("{}"),
            Some("{1}"),
            Some("2026-10-07 10:00:00"),
        ],
        &[None, None, None, None, None, Some("2026-10-08"), None, Some("2026-10-07 11:00"), None, None, None],
    ]);
    assert_eq!(
        roles(&cols, &data),
        [
            Role::Number,
            Role::Time(TimeKind::Date),
            Role::Time(TimeKind::DateTime),
            Role::Time(TimeKind::Time),
            Role::Time(TimeKind::Time),
            Role::Category,
            Role::Time(TimeKind::Date),
            Role::Time(TimeKind::DateTime),
            Role::Unusable,
            Role::Unusable,
            Role::Time(TimeKind::DateTime),
        ]
    );
    // A text column without values is a category; one value that is not a date makes it one.
    let cols = [meta("s", "text", false)];
    assert_eq!(roles(&cols, &rows(&[&[None]])), [Role::Category]);
    assert_eq!(roles(&cols, &rows(&[&[Some("2026-10-07")], &[Some("soon")]])), [Role::Category]);
}

#[test]
fn the_first_sight_picks_lines_over_time_bars_over_categories() {
    let n = Role::Number;
    let c = Role::Category;
    let d = Role::Time(TimeKind::Date);
    let few = rows(&[&[Some("a")], &[Some("b")]]);
    // A time and numbers: lines over time, every number a series.
    let s = infer(&[c, d, n, n], &[], 10).unwrap();
    assert_eq!(s, Spec { kind: Kind::Line, x: Some(1), ys: vec![2, 3], by: None, log: false });
    // A category and numbers: bars.
    let s = infer(&[c, n], &few, 2).unwrap();
    assert_eq!(s, Spec { kind: Kind::Bar, x: Some(0), ys: vec![1], by: None, log: false });
    // Many categories, or long labels: horizontal bars.
    let many: Vec<Vec<Cell>> = (0..13).map(|i| vec![Some(format!("p{i}"))]).collect();
    assert_eq!(infer(&[c, n], &many, 13).unwrap().kind, Kind::HBar);
    let long = rows(&[&[Some("a rather long product name")], &[Some("b")]]);
    assert_eq!(infer(&[c, n], &long, 2).unwrap().kind, Kind::HBar);
    // Wide letters count as letters (one each), not bytes.
    let wide = rows(&[&[Some("\u{AC00}\u{AC01}\u{AC02}\u{AC03}")], &[Some("b")]]);
    assert_eq!(infer(&[c, n], &wide, 2).unwrap().kind, Kind::Bar);
    // A time, a category with a few values and one number: a line per category.
    let data = rows(&[&[None, Some("x")], &[None, Some("y")], &[None, Some("x")]]);
    let s = infer(&[d, c, n], &data, 3).unwrap();
    assert_eq!(s, Spec { kind: Kind::Line, x: Some(0), ys: vec![2], by: Some(1), log: false });
    // Numbers only: lines over the first one; one number: over the row number.
    assert_eq!(
        infer(&[n, n, n], &[], 5).unwrap(),
        Spec { kind: Kind::Line, x: Some(0), ys: vec![1, 2], by: None, log: false }
    );
    assert_eq!(infer(&[n], &[], 5).unwrap(), Spec { kind: Kind::Bar, x: None, ys: vec![0], by: None, log: false });
    assert_eq!(infer(&[n], &[], 500).unwrap().kind, Kind::Line);
    // At most six series.
    assert_eq!(infer(&[c, n, n, n, n, n, n, n, n], &[], 5).unwrap().ys.len(), MAX_SERIES);
    // Nothing to draw.
    assert_eq!(infer(&[c, d], &[], 5), Err(Unsuitable::NoNumber));
    assert_eq!(infer(&[c, n], &[], 0), Err(Unsuitable::NoRows));
    assert_eq!(infer(&[Role::Unusable, n], &[], 3).unwrap().x, None);
}

#[test]
fn rows_with_the_same_x_are_summed_and_nulls_are_counted() {
    let cols = [meta("product", "text", false), meta("qty", "int4", true), meta("price", "numeric", true)];
    let data = rows(&[
        &[Some("tea"), Some("2"), Some("1.5")],
        &[Some("coffee"), Some("3"), None],
        &[Some("tea"), Some("1"), Some("abc")],
        &[None, Some("9"), Some("9")],
        &[Some("cake"), None, Some("$1,200.25")],
    ]);
    let spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1, 2], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.points.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["tea", "coffee", "cake"]);
    assert_eq!(
        m.points.iter().map(|p| (p.first_row, p.rows)).collect::<Vec<_>>(),
        [(Some(0), 2), (Some(1), 1), (Some(4), 1)]
    );
    assert_eq!(m.series[0].name, "qty");
    assert_eq!(m.series[0].values, [Some(3.0), Some(3.0), None]);
    assert_eq!(m.series[1].values, [Some(1.5), None, Some(1200.25)]);
    // What each value sums, and its first row: "abc" is not a number, so tea's price is one row.
    assert_eq!(m.series[0].rows, [2, 1, 0]);
    assert_eq!(m.series[0].first, [Some(0), Some(1), None]);
    assert_eq!(m.series[1].rows, [1, 0, 1]);
    assert_eq!(m.series[1].first, [Some(0), None, Some(4)]);
    assert_eq!(m.skipped, Skipped { null_x: 1, bad_x: 0, null_y: 2, bad_y: 1, null_by: 0 });
    assert_eq!(m.skipped.nulls(), 3);
    assert!(m.merged);
    assert_eq!(m.rows, 5);
    assert_eq!(m.range(false), Some((1.5, 1200.25)));
}

#[test]
fn number_and_time_axes_sort_their_points() {
    let cols = [meta("day", "date", false), meta("n", "int8", true)];
    let data = rows(&[
        &[Some("2026-10-03"), Some("3")],
        &[Some("2026-10-01"), Some("1")],
        &[Some("soon"), Some("7")],
        &[Some("2026-10-02"), Some("2")],
        &[Some("2026-10-01"), Some("10")],
    ]);
    let spec = Spec { kind: Kind::Line, x: Some(0), ys: vec![1], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.axis, Axis::Time(TimeKind::Date));
    assert_eq!(
        m.points.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(),
        ["2026-10-01", "2026-10-02", "2026-10-03"]
    );
    assert_eq!(m.series[0].values, [Some(11.0), Some(2.0), Some(3.0)]);
    assert_eq!(m.skipped.bad_x, 1);
    assert!(m.points.windows(2).all(|w| w[0].x < w[1].x));
    // Numbers: -0 and 0 are one point.
    let cols = [meta("x", "float8", true), meta("y", "int4", true)];
    let data = rows(&[&[Some("0"), Some("1")], &[Some("-0"), Some("1")], &[Some("-5"), Some("4")]]);
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.points.iter().map(|p| p.x).collect::<Vec<_>>(), [-5.0, 0.0]);
    assert_eq!(m.series[0].values, [Some(4.0), Some(2.0)]);
}

#[test]
fn the_row_number_is_an_axis_of_its_own() {
    let cols = [meta("n", "int4", true)];
    let data = rows(&[&[Some("5")], &[Some("5")], &[None], &[Some("7")]]);
    let spec = Spec { kind: Kind::Bar, x: None, ys: vec![0], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.axis, Axis::Row);
    assert_eq!(m.points.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4"]);
    assert_eq!(m.series[0].values, [Some(5.0), Some(5.0), None, Some(7.0)]);
    assert!(!m.merged);
}

#[test]
fn a_by_column_splits_one_value_into_series_and_too_many_go_to_others() {
    let cols = [meta("day", "date", false), meta("shop", "text", false), meta("n", "int4", true)];
    let mut data = Vec::new();
    for (shop, n) in [("a", 1), ("b", 20), ("c", 3), ("d", 40), ("e", 5), ("f", 60), ("g", 7), ("h", 80)] {
        data.push(vec![Some("2026-10-01".to_string()), Some(shop.to_string()), Some(n.to_string())]);
        data.push(vec![Some("2026-10-02".to_string()), Some(shop.to_string()), Some((n + 1).to_string())]);
    }
    data.push(vec![Some("2026-10-02".to_string()), None, Some("1".to_string())]);
    let spec = Spec { kind: Kind::Line, x: Some(0), ys: vec![2], by: Some(1), log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.series.len(), MAX_SERIES);
    // The five largest, in their first order, then the rest summed.
    assert_eq!(m.series.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b", "d", "f", "g", "h", ""]);
    let others = m.series.last().unwrap();
    assert!(others.others);
    assert_eq!(others.values, [Some(1.0 + 3.0 + 5.0), Some(2.0 + 4.0 + 6.0)]);
    assert_eq!(m.other_series, 3);
    assert_eq!(m.skipped.null_by, 1);
    // A series per shop: each value is one row (the day has several, one per shop).
    assert!(!m.merged);
    assert_eq!(m.series[0].rows, [1, 1]);
    assert_eq!(m.series[0].first, [Some(2), Some(3)]);
    assert_eq!(others.rows, [3, 3]);
    assert_eq!(others.first, [None, None]);
}

#[test]
fn too_many_bars_keep_the_largest_and_sum_the_rest() {
    let cols = [meta("k", "text", false), meta("n", "int4", true)];
    let data: Vec<Vec<Cell>> = (0..120).map(|i| vec![Some(format!("k{i}")), Some(i.to_string())]).collect();
    let spec = Spec { kind: Kind::HBar, x: Some(0), ys: vec![1], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.points.len(), MAX_BARS);
    assert_eq!(m.other_points, 120 - (MAX_BARS - 1));
    // In their first order: k71 … k119, then others.
    assert_eq!(m.points[0].label, "k71");
    assert_eq!(m.points[MAX_BARS - 2].label, "k119");
    let others = m.points.last().unwrap();
    assert!(others.others && others.first_row.is_none());
    assert_eq!(others.rows, 71);
    assert_eq!(m.series[0].values.last(), Some(&Some((0..71).sum::<i32>() as f64)));
    assert_eq!((m.series[0].rows.last(), m.series[0].first.last()), (Some(&71), Some(&None)));
    assert_eq!(m.series[0].first[0], Some(71));
    assert!(m.points.iter().enumerate().all(|(i, p)| p.x == i as f64));
    // Lines keep every point.
    let spec = Spec { kind: Kind::Line, ..spec };
    assert_eq!(build(&spec, &cols, &data).unwrap().points.len(), 120);
}

#[test]
fn unsuitable_results_say_why() {
    let cols = [meta("k", "text", false), meta("n", "int4", true)];
    let spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1], by: None, log: false };
    assert_eq!(build(&spec, &cols, &[]), Err(Unsuitable::NoRows));
    assert_eq!(build(&spec, &cols, &rows(&[&[Some("a"), Some("1")]])), Err(Unsuitable::OnePoint));
    assert_eq!(
        build(&spec, &cols, &rows(&[&[Some("a"), Some("1")], &[Some("a"), Some("2")]])),
        Err(Unsuitable::OnePoint)
    );
    assert_eq!(build(&spec, &cols, &rows(&[&[Some("a"), None], &[Some("b"), Some("x")]])), Err(Unsuitable::NoValues));
    assert_eq!(build(&spec, &cols, &rows(&[&[None, Some("1")], &[None, Some("2")]])), Err(Unsuitable::NoValues));
    let none = Spec { ys: vec![], ..spec };
    assert_eq!(build(&none, &cols, &rows(&[&[Some("a"), Some("1")]])), Err(Unsuitable::NoSeries));
}

#[test]
fn numbers_read_plain_or_with_a_currency() {
    assert_eq!(number("42"), Some(42.0));
    assert_eq!(number(" -1.5e3 "), Some(-1500.0));
    assert_eq!(number("$1,234.56"), Some(1234.56));
    assert_eq!(number("-$1,234.56"), Some(-1234.56));
    assert_eq!(number("(12.00)"), Some(-12.0));
    assert_eq!(number("\u{20A9}1,000"), Some(1000.0));
    assert_eq!(number("12 \u{20AC}"), Some(12.0));
    for bad in ["", "NaN", "Infinity", "-inf", "abc", "12 apples", "2026-10-07", "1.2.3x", "$", "()", "\u{AC00}12"] {
        assert_eq!(number(bad), None, "{bad:?}");
    }
}

#[test]
fn a_spec_carries_over_to_a_result_with_the_same_columns() {
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let spec = Spec { kind: Kind::HBar, x: Some(0), ys: vec![2], by: Some(1), log: true };
    let from = names(&["day", "shop", "n"]);
    let roles = [Role::Number, Role::Time(TimeKind::Date), Role::Category, Role::Category];
    let to = names(&["n", "day", "shop", "extra"]);
    assert_eq!(
        carry(&spec, &from, &to, &roles),
        Some(Spec { kind: Kind::HBar, x: Some(1), ys: vec![0], by: Some(2), log: true })
    );
    // A column gone, renamed, twice, or no longer a number: inferred again.
    assert_eq!(carry(&spec, &from, &names(&["day", "shop", "m"]), &roles[..3]), None);
    assert_eq!(carry(&spec, &from, &names(&["n", "day", "shop", "n"]), &roles), None);
    let text_n = [Role::Category, Role::Time(TimeKind::Date), Role::Category];
    assert_eq!(carry(&spec, &from, &names(&["n", "day", "shop"]), &text_n), None);
}

#[test]
fn the_numbers_copy_as_tsv() {
    let cols = [meta("k", "text", false), meta("a", "int4", true), meta("b\tc", "int4", true)];
    let data = rows(&[
        &[Some("x"), Some("1"), None],
        &[Some("y\nz"), Some("2"), Some("0.1")],
        &[Some("x"), Some("1"), Some("0.2")],
    ]);
    let spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1, 2], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.tsv("k", "others"), "k\ta\t\"b\tc\"\nx\t2\t0.2\n\"y\nz\"\t2\t0.1");
}

#[test]
fn kinds_go_around() {
    assert_eq!(Kind::Bar.step(1), Kind::HBar);
    assert_eq!(Kind::Bar.step(-1), Kind::Line);
    assert_eq!(Kind::Line.step(1), Kind::Bar);
    assert!(Kind::HBar.bars() && !Kind::Line.bars());
}

#[test]
fn sums_that_overflow_are_left_out_and_axes_stay_finite() {
    let cols = [meta("k", "text", false), meta("v", "float8", true)];
    let data = rows(&[&[Some("a"), Some("1e308")], &[Some("a"), Some("1e308")], &[Some("b"), Some("1")]]);
    let spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert!(m.series[0].values.iter().flatten().all(|v| v.is_finite()), "{:?}", m.series[0].values);
    assert_eq!(m.skipped.bad_y, 1, "the value that overflowed the sum");
    // An axis over something infinite is a plain one, at once.
    for t in [
        scale::linear(0.0, f64::INFINITY, 6, true),
        scale::linear(f64::NAN, 1.0, 6, false),
        scale::log(1.0, f64::INFINITY, 6),
    ] {
        assert!(t.lo.is_finite() && t.hi.is_finite() && t.values.len() <= 16, "{t:?}");
    }
}

#[test]
fn a_null_by_value_makes_no_point() {
    let cols = [meta("day", "date", false), meta("shop", "text", false), meta("n", "int4", true)];
    let data = rows(&[
        &[Some("2026-10-01"), Some("a"), Some("1")],
        &[Some("2026-10-02"), None, Some("5")],
        &[Some("2026-10-03"), Some("a"), Some("2")],
    ]);
    let spec = Spec { kind: Kind::Line, x: Some(0), ys: vec![2], by: Some(1), log: false };
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.points.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["2026-10-01", "2026-10-03"]);
    assert_eq!(m.skipped.null_by, 1);
}

#[test]
fn one_row_of_several_numbers_compares_them_as_bars() {
    let cols = [meta("a", "int8", true), meta("b", "int8", true), meta("c", "int8", true)];
    let data = rows(&[&[Some("1"), Some("2"), Some("3")]]);
    let roles = roles(&cols, &data);
    let spec = infer(&roles, &data, 1).unwrap();
    assert_eq!(spec, Spec { kind: Kind::Bar, x: None, ys: vec![0, 1, 2], by: None, log: false });
    let m = build(&spec, &cols, &data).unwrap();
    assert_eq!(m.series.len(), 3);
    // One number of one row: nothing to compare.
    let spec = Spec { ys: vec![0], ..spec };
    assert_eq!(build(&spec, &cols, &data), Err(Unsuitable::OnePoint));
}

#[test]
fn locale_money_and_other_minus_signs_are_not_misread() {
    // Grouped with dots and a decimal comma: not guessed.
    assert_eq!(number("1.234,56 \u{20AC}"), None);
    assert_eq!(number("1,23"), None);
    assert_eq!(number("12,34,567.00"), None);
    assert_eq!(number("$1,234,567.25"), Some(1_234_567.25));
    assert_eq!(number("\u{2212}5"), Some(-5.0));
    assert_eq!(number("\u{2212}$5.00"), Some(-5.0));
}

#[test]
fn others_leave_out_an_overflowing_sum_and_count_it() {
    let cols = [meta("k", "text", false), meta("v", "float8", true)];
    // The largest bars are kept; the two left out sum past the largest number.
    let mut data: Vec<Vec<Cell>> =
        (0..MAX_BARS - 1).map(|i| vec![Some(format!("k{i}")), Some("1.5e308".into())]).collect();
    data.push(vec![Some("x".into()), Some("1e308".into())]);
    data.push(vec![Some("y".into()), Some("1e308".into())]);
    let spec = Spec { kind: Kind::Bar, x: Some(0), ys: vec![1], by: None, log: false };
    let m = build(&spec, &cols, &data).unwrap();
    let others = m.points.len() - 1;
    assert!(m.series[0].values[others].is_some_and(f64::is_finite));
    // The value that could not be added is not counted among the others' rows, but as left out.
    assert_eq!(m.series[0].rows[others] as usize, m.points[others].rows - 1);
    assert_eq!(m.skipped.bad_y, 1);
}

#[test]
fn one_point_compares_only_series_that_have_a_value() {
    let cols = [meta("a", "int8", true), meta("b", "int8", true)];
    let data = rows(&[&[Some("5"), None]]);
    let spec = Spec { kind: Kind::Bar, x: None, ys: vec![0, 1], by: None, log: false };
    assert_eq!(build(&spec, &cols, &data), Err(Unsuitable::OnePoint));
}

/// A type of the database's own that the driver does not name keeps the role its name gives it.
#[test]
fn a_type_the_driver_does_not_name_is_read_by_its_name() {
    let other = |ty: &str, numeric: bool| ColumnMeta { kind: ValueKind::Other, ..meta("c", ty, numeric) };
    let none: Vec<Vec<Cell>> = Vec::new();
    let role = |c: ColumnMeta| roles(&[c], &none)[0];
    assert_eq!(role(other("timestamp_kind", false)), Role::Time(TimeKind::DateTime));
    assert_eq!(role(other("My_DateTime", false)), Role::Time(TimeKind::DateTime));
    assert_eq!(role(other("date", false)), Role::Time(TimeKind::Date));
    assert_eq!(role(other("time of day", false)), Role::Time(TimeKind::Time));
    assert_eq!(role(other("mood[]", false)), Role::Unusable);
    assert_eq!(role(other("mood", false)), Role::Category);
    let numeric_named_like_an_array = ColumnMeta { kind: ValueKind::Integer, ..meta("c", "x[]", true) };
    assert_eq!(role(numeric_named_like_an_array), Role::Unusable);
}
