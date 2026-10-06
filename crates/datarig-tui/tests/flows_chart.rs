//! The Chart tab: the fetched rows of the shown result as bars or lines.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent, PagingMode};
use datarig_core::i18n::Lang;
use datarig_tui::app::Focus;
use std::time::Duration;

fn run_page(
    h: &mut Harness,
    sql: &str,
    cols: Vec<datarig_core::driver::ColumnMeta>,
    rows: Vec<Vec<Option<String>>>,
    more: bool,
) {
    h.sent();
    h.app.run(vec![sql.into()]);
    let sent = h.sent();
    let [DbCommand::Execute { id, paging: PagingMode::NoHold, .. }] = &sent[..] else { panic!("{sent:?}") };
    h.db(DbEvent::Page { id: *id, columns: Some(cols), rows, more, elapsed: Duration::from_millis(3) });
}

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

#[test]
fn peek() {
    let mut h = Harness::connected(Lang::En);
    let rows: Vec<Vec<Option<String>>> = (1..=30)
        .map(|d| vec![s(&format!("2026-09-{d:02}")), s(&((d * 37) % 101).to_string()), s(&((d * 13) % 50).to_string())])
        .collect();
    run_page(
        &mut h,
        "SELECT day, orders, refunds FROM t",
        vec![
            meta("day", "date", false, false),
            meta("orders", "int8", true, false),
            meta("refunds", "int8", true, false),
        ],
        rows,
        true,
    );
    h.app.focus = Focus::Results;
    h.keys("c");
    println!("{}", h.screen(120, 34));
    h.keys("1");
    println!("{}", h.screen(120, 34));
    h.keys("2");
    println!("{}", h.screen(120, 34));
    let rows = vec![
        vec![s("tea"), s("12")],
        vec![s("coffee"), s("40")],
        vec![s("\u{AC00}\u{AC01} cake"), s("-7")],
        vec![s("juice"), None],
    ];
    run_page(
        &mut h,
        "SELECT p, n FROM t",
        vec![meta("product", "text", false, false), meta("n", "int4", true, false)],
        rows,
        false,
    );
    println!("{}", h.screen(100, 30));
    h.keys("2l");
    println!("{}", h.screen(100, 30));
}
