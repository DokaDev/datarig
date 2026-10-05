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

#[test]
fn a_treemap_covers_its_area_by_weight_without_overlap() {
    use treemap::{Area, squarify};
    let items: Vec<(usize, f64)> = vec![(0, 6.0), (1, 6.0), (2, 4.0), (3, 3.0), (4, 2.0), (5, 2.0), (6, 1.0)];
    let r = Area { x: 0.0, y: 0.0, w: 60.0, h: 20.0 };
    let out = squarify(&items, r);
    assert_eq!(out.len(), items.len());
    let total: f64 = items.iter().map(|i| i.1).sum();
    for (id, a) in &out {
        let want = items[*id].1 / total * r.w * r.h;
        assert!((a.w * a.h - want).abs() < 1e-6, "{id}: {} vs {want}", a.w * a.h);
        assert!(a.x >= -1e-9 && a.y >= -1e-9 && a.x + a.w <= r.w + 1e-6 && a.y + a.h <= r.h + 1e-6, "{id}: {a:?}");
    }
    for (i, (_, a)) in out.iter().enumerate() {
        for (_, b) in &out[i + 1..] {
            let apart = a.x + a.w <= b.x + 1e-6
                || b.x + b.w <= a.x + 1e-6
                || a.y + a.h <= b.y + 1e-6
                || b.y + b.h <= a.y + 1e-6;
            assert!(apart, "{a:?} and {b:?} overlap");
        }
    }
    assert!(squarify(&[], r).is_empty());
    assert!(squarify(&[(0, 0.0)], r).is_empty());
}

#[test]
fn icicle_children_share_their_parents_width() {
    use datarig_core::sql::plan::pg::parse;
    let leaf = |t: f64| {
        format!(
            r#"{{"Node Type": "Seq Scan", "Startup Cost": 0, "Total Cost": 1, "Plan Rows": 1, "Plan Width": 1, "Actual Startup Time": 0, "Actual Total Time": {t}, "Actual Rows": 1, "Actual Loops": 1}}"#
        )
    };
    let json = format!(
        r#"[{{"Plan": {{"Node Type": "Append", "Startup Cost": 0, "Total Cost": 1, "Plan Rows": 1, "Plan Width": 1, "Actual Startup Time": 0, "Actual Total Time": 100, "Actual Rows": 1, "Actual Loops": 1, "Plans": [{}, {}]}}, "Execution Time": 100}}]"#,
        leaf(50.0),
        leaf(25.0)
    );
    let p = crate::app::plan::PlanTab::new(parse(&json).unwrap(), 0, 0, &json);
    let s = icicle::spans(&p, 100.0);
    assert_eq!(s[0], (0.0, 100.0));
    assert_eq!(s[1], (0.0, 50.0));
    assert_eq!(s[2], (50.0, 75.0), "the rest is the parent's own time");
    // Children that say they took longer than their parent share its width.
    let json = json.replace(r#""Actual Total Time": 25"#, r#""Actual Total Time": 150"#);
    let p = crate::app::plan::PlanTab::new(parse(&json).unwrap(), 0, 0, &json);
    let s = icicle::spans(&p, 100.0);
    assert!((s[2].1 - 100.0).abs() < 1e-9, "{s:?}");
}

/// A CTE's time is inside the scans that read it: in the icicle it does not shrink its
/// siblings (its time would count twice), and nothing goes past its parent.
#[test]
fn icicle_ctes_do_not_shrink_their_siblings() {
    use datarig_core::sql::plan::pg::parse;
    let json = include_str!("../../../../datarig-core/src/sql/plan/fixtures/pg18/cte.analyze.json");
    let p = crate::app::plan::PlanTab::new(parse(json).unwrap(), 0, 0, json);
    let s = icicle::spans(&p, 100.0);
    let plan = &p.plan;
    // Hash Join: CTE big (InitPlan), CTE Scan b (170.3 of 171.4 ms), Hash.
    let scan = plan.nodes.iter().position(|n| n.op == "CTE Scan").unwrap();
    let width = s[scan].1 - s[scan].0;
    assert!((width - 100.0 * 170.295 / 171.352).abs() < 1e-6, "the scan keeps its share: {width}");
    for (i, n) in plan.nodes.iter().enumerate() {
        if let Some(parent) = n.parent {
            assert!(s[i].0 >= s[parent].0 - 1e-9 && s[i].1 <= s[parent].1 + 1e-9, "{i} inside {parent}: {s:?}");
        }
    }
}
