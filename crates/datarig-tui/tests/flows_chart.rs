//! The Chart tab: the rows a result holds drawn as bars or lines, never asking the server; its
//! kind and columns change with keys, a list of the columns, the action menu and the mouse;
//! the cursor shows the exact values and their row; it follows the shown result.

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent, PagingMode};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_tui::app::Focus;
use datarig_tui::app::chart::ChartTab;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::tabs::ResultView;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// Run `sql` in `h`'s tab and answer it with `cols` and `rows` (`more`: the server has more).
fn answer(h: &mut Harness, sql: &str, cols: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>, more: bool) {
    h.sent();
    h.app.run(vec![sql.into()]);
    let sent = h.sent();
    let [DbCommand::Execute { id, paging: PagingMode::NoHold, .. }] = &sent[..] else { panic!("one run: {sent:?}") };
    h.db(DbEvent::Page { id: *id, columns: Some(cols), rows, more, elapsed: Duration::from_millis(3) });
}

/// Orders and refunds per day of September 2026 (30 rows, the server has more).
fn per_day(h: &mut Harness) {
    let rows = (1..=30)
        .map(|d| vec![s(&format!("2026-09-{d:02}")), s(&((d * 37) % 101).to_string()), s(&((d * 13) % 50).to_string())])
        .collect();
    let cols = vec![
        meta("day", "date", false, false),
        meta("orders", "int8", true, false),
        meta("refunds", "int8", true, false),
    ];
    answer(h, "SELECT day, orders, refunds FROM daily", cols, rows, true);
}

/// Sales per product: long names, one CJK (wide), a NULL value.
fn per_product(h: &mut Harness) {
    let rows = vec![
        vec![s("green tea"), s("12")],
        vec![s("espresso"), s("40")],
        vec![s("\u{6A59}\u{5B50}\u{6C41} juice"), s("-7")],
        vec![s("lemon cake"), None],
        vec![s("hot chocolate with cream"), s("23")],
    ];
    answer(
        h,
        "SELECT product, sold FROM sales",
        vec![meta("product", "text", false, false), meta("sold", "int4", true, false)],
        rows,
        false,
    );
}

/// The tab's chart as the last frame drew it (a frame makes it follow the shown result).
fn chart(h: &mut Harness) -> &ChartTab {
    h.draw(120, 34);
    h.app.tab().exec.chart.as_ref().expect("a chart")
}

fn view(h: &Harness) -> ResultView {
    h.app.tab().exec.view
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

/// Where `text` is on the last screen drawn `w`×`hh`.
fn find(h: &mut Harness, text: &str, w: u16, hh: u16) -> (u16, u16) {
    let t = h.draw(w, hh);
    let buf = t.backend().buffer();
    for y in 0..hh {
        let row = row_text(buf, y);
        if let Some(i) = row.find(text) {
            return (width_of(&row[..i]), y);
        }
    }
    panic!("{text} is not on the screen:\n{}", buffer_text(buf));
}

fn click(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

#[test]
fn c_draws_the_fetched_rows_without_asking_the_server() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c");
    assert_eq!(view(&h), ResultView::Chart);
    assert!(h.sent().is_empty(), "nothing is asked of the server");
    let screen = h.screen(120, 34);
    for want in [
        "Chart · Line",
        "[Chart]",
        "[Line]",
        "first 30 fetched rows · more on the server",
        "X day · Y orders, refunds · by none · linear scale",
        "2026-09-14",
        "■ orders  ■ refunds",
        "▸ 2026-09-01 · orders 37 · refunds 13 · row 1",
    ] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    assert!(screen.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)), "braille lines:\n{screen}");
    insta::assert_snapshot!("chart_line_en_120x34", h.draw(120, 34).backend());
    insta::assert_snapshot!("chart_line_en_80x24", h.draw(80, 24).backend());
    remember_english("chart_line_en_120x34", h.draw(120, 34).backend().buffer());
    // `c` again: the rows; the strip keeps the chart to go back to.
    h.keys("c");
    assert_eq!(view(&h), ResultView::Rows);
    assert!(h.screen(120, 34).contains("Result 1   Chart   Messages") || h.screen(120, 34).contains(" Chart "));
    h.keys("L");
    assert_eq!(view(&h), ResultView::Chart);
    h.keys("L");
    assert_eq!(view(&h), ResultView::Messages);
    h.keys("L");
    assert_eq!(view(&h), ResultView::Rows);
    // A click on Chart in the strip.
    h.draw(120, 34);
    let (x0, ..) = h.app.strip_hits().into_iter().find(|(.., v)| *v == ResultView::Chart).expect("Chart in the strip");
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0 + 1, h.app.layout.strip.y);
    assert_eq!(view(&h), ResultView::Chart);
    // From the results menu and `Space r c` too.
    h.keys("c");
    h.keys("  ");
    h.menu_pick("Results → show as a chart / as rows");
    assert_eq!(view(&h), ResultView::Chart);
    h.keys("c");
    h.app.focus = Focus::Editor;
    h.keys(" rc");
    assert_eq!(view(&h), ResultView::Chart);
    assert!(h.sent().is_empty());
}

#[test]
fn the_chart_screens_in_korean() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c");
    remember_english("chart_line_en_120x34", h.draw(120, 34).backend().buffer());
    let mut k = Harness::connected(Lang::Ko);
    per_day(&mut k);
    k.app.focus = Focus::Results;
    k.keys("c");
    check_localized("chart_line_ko_120x34", Lang::Ko, k.draw(120, 34).backend().buffer());
    let screen = k.screen(120, 34);
    assert!(screen.contains(&ko_msg(&Msg::ChartRowsMore { count: 30 })), "{screen}");
}

#[test]
fn kinds_switch_with_keys_the_menu_and_a_click() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c1");
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Bar);
    let screen = h.screen(120, 34);
    assert!(screen.contains("Chart · Bars") && screen.contains("[Bars]") && screen.contains("█"), "{screen}");
    insta::assert_snapshot!("chart_bars_en_120x34", h.draw(120, 34).backend());
    insta::assert_snapshot!("chart_bars_en_80x24", h.draw(80, 24).backend());
    h.keys("v");
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::HBar);
    insta::assert_snapshot!("chart_hbars_en_120x34", h.draw(120, 34).backend());
    insta::assert_snapshot!("chart_hbars_en_80x24", h.draw(80, 24).backend());
    h.keys("V");
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Bar);
    h.keys("3");
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Line);
    // The kind's name takes a click (and the pointer highlights it first).
    let at = find(&mut h, " Horizontal bars ", 120, 34);
    h.mouse(MouseEventKind::Moved, at.0 + 2, at.1);
    click(&mut h, (at.0 + 2, at.1));
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::HBar);
    // The menu: the kinds and the columns.
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let labels = h.menu_labels();
    for want in [
        "Chart: show the point's row in the grid",
        "Chart: copy its numbers as TSV",
        "Chart: copy the drawing as text",
        "Chart kind: bars",
        "Chart kind: line",
        "Chart: choose the X column",
        "Chart: choose the value columns (Y)",
        "Chart: split the values into series by a column",
        "Chart: logarithmic scale on or off",
        "Results → show as a chart / as rows",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    assert_eq!(h.app.menu_lines()[0].1.to_string(), "2026-09-01", "the point at the cursor");
    insta::assert_snapshot!("chart_menu_en_120x34", h.draw(120, 34).backend());
    h.menu_pick("Chart kind: bars");
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Bar);
    // `S`: a logarithmic scale, said on the columns' line.
    h.keys("S");
    assert!(chart(&mut h).spec.log);
    assert!(h.screen(120, 34).contains("log scale"));
    assert!(h.sent().is_empty(), "a kind or a scale never asks the server");
}

#[test]
fn categories_pick_bars_and_wide_labels_keep_their_width() {
    let mut h = Harness::connected(Lang::En);
    per_product(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c");
    // Long names: horizontal bars.
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::HBar);
    let screen = h.screen(100, 30);
    for want in ["X product · Y sold", "5 fetched rows", "espresso│", "1 NULL skipped", "12"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    insta::assert_snapshot!("chart_hbars_categories_en_100x30", h.draw(100, 30).backend());
    // Every label ends at the axis: wide characters are two columns each.
    let t = h.draw(100, 30);
    let buf = t.backend().buffer();
    let axis: Vec<u16> = (0..30)
        .filter_map(|y| {
            let row = row_text(buf, y);
            let i = row.find("juice│").or(row.find("espresso│"))?;
            Some(width_of(&row[..i + row[i..].find('│')?]))
        })
        .collect();
    assert_eq!(axis.len(), 2, "{}", buffer_text(buf));
    assert_eq!(axis[0], axis[1], "the wide label's axis is where the others' is");
    // Vertical bars cut the labels to their group.
    h.keys("1");
    insta::assert_snapshot!("chart_bars_categories_en_100x30", h.draw(100, 30).backend());
}

#[test]
fn the_cursor_shows_exact_values_and_the_row_they_come_from() {
    let mut h = Harness::connected(Lang::En);
    let cols = vec![meta("k", "text", false, false), meta("big", "int8", true, false)];
    let rows = vec![
        vec![s("a"), s("9007199254740993")],
        vec![s("b"), s("1")],
        vec![s("a"), s("2")],
        vec![s("c"), s("3")],
        vec![s("b"), s("5")],
    ];
    answer(&mut h, "SELECT k, big FROM t", cols, rows, false);
    h.app.focus = Focus::Results;
    h.keys("c");
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Bar);
    let screen = h.screen(120, 34);
    // Two rows summed: the sum, and where the first one is.
    assert!(screen.contains("▸ a · big 9007199254740994 · 2 rows summed (first: row 1)"), "{screen}");
    assert!(screen.contains("rows with the same X summed"), "{screen}");
    h.keys("l");
    assert!(h.screen(120, 34).contains("▸ b · big 6 · 2 rows summed (first: row 2)"));
    h.keys("l");
    assert!(h.screen(120, 34).contains("▸ c · big 3 · row 4"));
    h.keys("l");
    assert_eq!(chart(&mut h).cursor, 2, "stops at the last");
    h.keys("gg");
    assert_eq!(chart(&mut h).cursor, 0);
    h.keys("G");
    assert_eq!(chart(&mut h).cursor, 2);
    // One row: its cell as the server wrote it (no rounding).
    let cols = vec![meta("k", "text", false, false), meta("big", "int8", true, false)];
    let rows = vec![vec![s("x"), s("9007199254740993")], vec![s("y"), s("1")]];
    answer(&mut h, "SELECT k, big FROM u", cols, rows, false);
    assert_eq!(view(&h), ResultView::Chart, "the chart stays shown for the new rows");
    assert!(h.screen(120, 34).contains("▸ x · big 9007199254740993 · row 1"), "{}", h.screen(120, 34));
    // Horizontal bars move down with `j`, the series across with `l`.
    h.keys("2j");
    assert_eq!(chart(&mut h).cursor, 1);
    h.keys("k");
    assert_eq!(chart(&mut h).cursor, 0);
    // `Enter`: the grid, on the point's row.
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(view(&h), ResultView::Rows);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (1, 0));
}

#[test]
fn several_series_move_across_and_show_a_legend() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c");
    assert_eq!(chart(&mut h).series, 0);
    h.keys("j");
    assert_eq!(chart(&mut h).series, 1, "lines: up and down pick the series");
    h.keys("j");
    assert_eq!(chart(&mut h).series, 1);
    h.keys("ll");
    assert_eq!(chart(&mut h).cursor, 2);
    assert!(h.screen(120, 34).contains("▸ 2026-09-03 · orders 10 · refunds 39 · row 3"));
}

#[test]
fn columns_change_with_the_list_keys_and_the_mouse() {
    let mut h = Harness::connected(Lang::En);
    let cols = vec![
        meta("day", "date", false, false),
        meta("shop", "text", false, false),
        meta("orders", "int4", true, false),
        meta("revenue", "numeric", true, false),
    ];
    let rows = (0..12)
        .map(|i| {
            let shop = ["north", "south", "east"][i % 3];
            vec![
                s(&format!("2026-10-{:02}", i / 3 + 1)),
                s(shop),
                s(&(i * 3 + 1).to_string()),
                s(&format!("{}.50", i * 10)),
            ]
        })
        .collect();
    answer(&mut h, "SELECT day, shop, orders, revenue FROM t", cols, rows, false);
    h.app.focus = Focus::Results;
    h.keys("c");
    assert_eq!(chart(&mut h).spec.ys, [2, 3]);
    assert_eq!(chart(&mut h).spec.by, None, "two values: no split");
    // `x`: the X column, from a list (the row number first).
    h.keys("x");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser));
    let screen = h.screen(120, 34);
    assert!(screen.contains("Chart: X axis") && screen.contains("row number") && screen.contains("a shop"), "{screen}");
    insta::assert_snapshot!("chart_pick_x_en_120x34", h.draw(120, 34).backend());
    // The list opens on the X shown (day): the next is shop.
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(chart(&mut h).spec.x, Some(1), "shop");
    // `s`: the values; Enter adds or removes one and the list stays open.
    h.keys("s");
    let screen = h.screen(120, 34);
    assert!(screen.contains("[x] orders") && screen.contains("[x] revenue"), "{screen}");
    h.key(KeyCode::Enter);
    assert_eq!(chart(&mut h).spec.ys, [3]);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser), "the list stays open");
    assert!(h.screen(120, 34).contains("[ ] orders"));
    h.key(KeyCode::Enter);
    assert_eq!(chart(&mut h).spec.ys, [2, 3]);
    h.key(KeyCode::Esc);
    // `b`: one value split by a column (the first value stays).
    h.keys("b");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Chart: split the values into series by") && screen.contains("\u{25F7} day"), "{screen}");
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(chart(&mut h).spec.by, Some(0));
    assert_eq!(chart(&mut h).spec.ys, [2]);
    // No split again: the values stay as they are (the one left).
    h.keys("s");
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(chart(&mut h).spec.ys, [3], "split: the one value is replaced");
    h.keys("b");
    h.keys("k");
    h.key(KeyCode::Enter);
    assert_eq!((chart(&mut h).spec.by, chart(&mut h).spec.ys.clone()), (None, vec![3]));
    h.keys("s");
    h.keys("k");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Esc);
    assert_eq!(chart(&mut h).spec.ys, [2, 3]);
    h.keys("b");
    h.keys("k");
    h.key(KeyCode::Enter);
    assert_eq!(chart(&mut h).spec.ys, [2, 3], "choosing no split keeps every value");
    // The mouse: the X choice on the columns' line opens the list, a row picks.
    h.keys("x");
    h.key(KeyCode::Esc);
    let at = find(&mut h, "X shop", 120, 34);
    click(&mut h, (at.0 + 1, at.1));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser));
    // A press right after a list opened is ignored (it may be the click that opened it).
    let at = find(&mut h, "row number", 120, 34);
    h.advance(Duration::from_millis(500));
    click(&mut h, (at.0 + 1, at.1));
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(chart(&mut h).spec.x, None, "the row number");
    // No more than six values.
    assert!(h.sent().is_empty());
}

#[test]
fn a_split_column_draws_a_series_per_value() {
    let mut h = Harness::connected(Lang::En);
    let cols = vec![
        meta("day", "date", false, false),
        meta("shop", "text", false, false),
        meta("orders", "int4", true, false),
    ];
    let rows = (0..12)
        .map(|i| {
            vec![
                s(&format!("2026-10-{:02}", i / 3 + 1)),
                s(["north", "south", "east"][i % 3]),
                s(&(i * 3 + 1).to_string()),
            ]
        })
        .collect();
    answer(&mut h, "SELECT day, shop, orders FROM t", cols, rows, false);
    h.app.focus = Focus::Results;
    h.keys("c");
    assert_eq!(chart(&mut h).spec.by, Some(1), "one value and a few shops: a line per shop");
    let screen = h.screen(120, 34);
    assert!(screen.contains("■ north  ■ south  ■ east") && screen.contains("by shop"), "{screen}");
    insta::assert_snapshot!("chart_lines_by_en_120x34", h.draw(120, 34).backend());
    h.keys("1");
    insta::assert_snapshot!("chart_bars_by_en_120x34", h.draw(120, 34).backend());
}

#[test]
fn copies_are_the_numbers_as_tsv_or_the_drawing() {
    let mut h = Harness::connected(Lang::En);
    per_product(&mut h);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.app.focus = Focus::Results;
    h.keys("c");
    h.draw(100, 30);
    h.keys("y");
    assert_eq!(
        clip.last().as_deref(),
        Some(
            "product\tsold\ngreen tea\t12\nespresso\t40\n\u{6A59}\u{5B50}\u{6C41} juice\t-7\nlemon cake\t\nhot chocolate with cream\t23"
        )
    );
    assert_eq!(status(&h), Some(Msg::ChartCopied { count: 6, method: "system clipboard".into() }));
    h.keys("Y");
    let text = clip.last().unwrap();
    assert!(text.contains("[Horizontal bars]") && text.contains("espresso│") && text.contains("█"), "{text}");
    assert_eq!(
        text.lines().count(),
        usize::from(h.app.tab().exec.chart.as_ref().unwrap().area.height),
        "as large as drawn: {text}"
    );
    assert!(text.lines().all(|l| l == l.trim_end()));
}

#[test]
fn results_without_a_chart_say_why() {
    let mut h = Harness::connected(Lang::En);
    answer(
        &mut h,
        "SELECT name FROM t",
        vec![meta("name", "text", false, false)],
        vec![vec![s("a")], vec![s("b")]],
        false,
    );
    h.app.focus = Focus::Results;
    h.keys("c");
    assert_eq!(view(&h), ResultView::Chart);
    let screen = h.screen(100, 30);
    assert!(screen.contains("Nothing to chart: no column holds numbers."), "{screen}");
    insta::assert_snapshot!("chart_no_number_en_100x30", h.draw(100, 30).backend());
    // One row.
    answer(
        &mut h,
        "SELECT 'a', 1",
        vec![meta("k", "text", false, false), meta("n", "int4", true, false)],
        vec![vec![s("a"), s("1")]],
        false,
    );
    let screen = h.screen(100, 30);
    assert!(screen.contains("Only one point to draw: nothing to compare it with."), "{screen}");
    assert!(screen.contains("x X column · s values · b series by a column · v kind"), "{screen}");
    // Every value NULL or not a number.
    let rows = vec![vec![s("a"), None], vec![s("b"), None]];
    answer(
        &mut h,
        "SELECT k, n FROM t",
        vec![meta("k", "text", false, false), meta("n", "int4", true, false)],
        rows,
        false,
    );
    assert!(h.screen(100, 30).contains("Nothing to chart: every value to draw is NULL or not a number."));
    // No rows.
    answer(
        &mut h,
        "SELECT k, n FROM t WHERE false",
        vec![meta("k", "text", false, false), meta("n", "int4", true, false)],
        vec![],
        false,
    );
    assert!(h.screen(100, 30).contains("Nothing to chart: the result has no rows."));
    // No value column chosen.
    answer(
        &mut h,
        "SELECT k, n FROM t",
        vec![meta("k", "text", false, false), meta("n", "int4", true, false)],
        vec![vec![s("a"), s("1")], vec![s("b"), s("2")]],
        false,
    );
    h.keys("s");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Esc);
    assert!(h.screen(100, 30).contains("No values chosen: pick at least one numeric column."));
    // Copy says there is nothing.
    let _clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    assert!(h.status(100, 30).contains("Nothing to copy: the chart has no numbers."), "{}", h.status(100, 30));
}

#[test]
fn a_chart_follows_the_shown_result_and_drops_stale_choices() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c1");
    // The same columns again: the kind chosen stays.
    per_day(&mut h);
    assert_eq!(view(&h), ResultView::Chart);
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Bar, "kept by name");
    // Other columns: chosen afresh.
    per_product(&mut h);
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::HBar);
    assert_eq!(chart(&mut h).names, ["product", "sold"]);
    // A list opened on a result that a later run replaced changes nothing.
    h.keys("x");
    per_day(&mut h);
    h.key(KeyCode::Enter);
    assert!(h.status(120, 34).contains(&Label::ChartPickStale.text(Lang::En).to_string()), "{}", h.status(120, 34));
    assert_eq!(chart(&mut h).spec.x, Some(0), "the new result's choice");
    // A menu opened on a chart whose result changed (a run's rows came while it was open)
    // runs nothing.
    h.sent();
    h.app.run(vec!["SELECT product, sold FROM sales".into()]);
    let sent = h.sent();
    let [DbCommand::Execute { id, .. }] = &sent[..] else { panic!("{sent:?}") };
    let id = *id;
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let cols = vec![meta("product", "text", false, false), meta("sold", "int4", true, false)];
    let rows = vec![vec![s("a"), s("1")], vec![s("b"), s("2")]];
    h.db(DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
    h.menu_pick("Chart kind: line");
    assert!(h.status(120, 34).contains("Not run: what the menu was opened on has changed"), "{}", h.status(120, 34));
    assert_ne!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Line);
    assert_eq!(chart(&mut h).model().map(|m| m.rows), Some(2));
}

#[test]
fn the_chart_redraws_on_resize_and_zooms() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Results;
    h.keys("c1");
    let small = h.screen(80, 24);
    let large = h.screen(160, 45);
    assert_ne!(small, large);
    assert!(large.contains("2026-09-29") && !small.contains("2026-09-29"), "more labels fit:\n{small}\n{large}");
    // The pane zoomed: the chart takes the workspace.
    h.keys(" z");
    assert!(h.app.zoomed().is_some());
    let zoomed = h.screen(120, 34);
    assert!(zoomed.contains("Chart · Bars") && !zoomed.contains("Query · console"), "{zoomed}");
    insta::assert_snapshot!("chart_zoomed_en_120x34", h.draw(120, 34).backend());
    h.keys("z");
    assert!(h.app.zoomed().is_none());
    // Small: what fits (the legend and notes go first), never a broken chart.
    for (w, hh) in [(60, 16), (50, 12), (40, 10)] {
        h.draw(w, hh);
    }
}

#[test]
fn the_pointer_picks_points_and_the_wheel_moves_the_cursor() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h);
    h.app.focus = Focus::Editor;
    h.app.focus = Focus::Results;
    h.keys("c1");
    h.app.focus = Focus::Editor;
    h.draw(120, 34);
    let (rect, i) = chart(&mut h).hits.iter().copied().find(|(_, i)| *i == 4).unwrap();
    click(&mut h, (rect.x + 1, rect.y + 2));
    assert_eq!((chart(&mut h).cursor, i), (4, 4));
    assert_eq!(h.app.focus, Focus::Results, "the click focuses the chart");
    // A second click on the same point: its row in the grid.
    click(&mut h, (rect.x + 1, rect.y + 2));
    assert_eq!(view(&h), ResultView::Rows);
    assert_eq!(h.app.tab().grid.row, 4);
    h.keys("c");
    h.draw(120, 34);
    let plot = chart(&mut h).plot;
    h.mouse(MouseEventKind::ScrollDown, plot.x + 2, plot.y + 1);
    assert_eq!(chart(&mut h).cursor, 5);
    h.mouse(MouseEventKind::ScrollUp, plot.x + 2, plot.y + 1);
    assert_eq!(chart(&mut h).cursor, 4);
    // Lines: the point nearest the column clicked.
    h.keys("3");
    h.draw(120, 34);
    let plot = chart(&mut h).plot;
    click(&mut h, (plot.x + plot.width - 1, plot.y + 2));
    assert_eq!(chart(&mut h).cursor, 29);
    // A right click on the kinds or the columns' line changes nothing: it opens the menu.
    let at = find(&mut h, " Bars ", 120, 34);
    h.mouse(MouseEventKind::Down(MouseButton::Right), at.0 + 2, at.1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    assert_eq!(chart(&mut h).spec.kind, datarig_core::chart::Kind::Line);
    h.key(KeyCode::Esc);
    let at = find(&mut h, "linear scale", 120, 34);
    h.mouse(MouseEventKind::Down(MouseButton::Right), at.0 + 2, at.1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu), "no column list under it");
    assert!(!chart(&mut h).spec.log);
    h.key(KeyCode::Esc);
    // A right click: the chart's menu, on the point there.
    h.mouse(MouseEventKind::Down(MouseButton::Right), plot.x, plot.y + 2);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    assert_eq!(chart(&mut h).cursor, 0);
}

/// Series are drawn in the theme's chart colors, in the default `terminal` theme and every
/// built-in; "others" in the muted text color.
#[test]
fn series_use_the_themes_chart_colors() {
    for (name, th) in datarig_tui::theme::BUILTINS.iter().copied() {
        let mut h = Harness::connected(Lang::En);
        per_day(&mut h);
        h.app.theme = std::sync::Arc::new(th.clone());
        h.app.focus = Focus::Results;
        h.keys("c1");
        h.app.focus = Focus::Editor;
        let t = h.draw(120, 34);
        let buf = t.backend().buffer();
        let colors: std::collections::HashSet<_> = (0..34u16)
            .flat_map(|y| (0..120u16).map(move |x| (x, y)))
            .filter(|&(x, y)| matches!(buf[(x, y)].symbol(), "█" | "▁" | "▂" | "▃" | "▄" | "▅" | "▆" | "▇"))
            .map(|(x, y)| buf[(x, y)].fg)
            .collect();
        assert_eq!(colors, [th.chart_1, th.chart_2].into_iter().collect(), "{name}");
    }
}

/// Columns `s` takes on screen.
fn width_of(s: &str) -> u16 {
    datarig_tui::text::width(s) as u16
}
