//! A table tab's query bar: the result strip on the results pane's top border, the bar on the
//! first line inside it (`WHERE` chips, `ORDER BY`, `LIMIT`, the columns, the rows of the result
//! shown, `⋯ Advanced`), the grid as tall as before. A chip's `×` takes its condition out and
//! runs the others as they were written; `Space r l` types the limit in the bar; `Space r w`
//! puts the keys on the conditions; the page arrows page.

use crate::common;

use common::*;
use datarig_core::driver::{DbCommand, DbError, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::Focus;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::qbar::BarHit;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// Open `shop.users` from the explorer (a table tab, which runs its query).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with `n` rows of `shop.users`, all of them.
fn answer_n(h: &mut Harness, n: usize) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..n).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

fn answer(h: &mut Harness) {
    answer_n(h, 3);
}

/// `shop.users` open and answered with `n` rows, the grid focused, nothing sent since.
fn users_n(n: usize) -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer_n(&mut h, n);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // The pane at its widest: no inspector beside it.
    h.app.detail.visible = false;
    h
}

fn users() -> Harness {
    users_n(3)
}

/// `where` applied and answered.
fn filtered(h: &mut Harness, text: &str) {
    h.command(&format!("where {text}"));
    runs(h);
    answer(h);
    h.app.focus = Focus::Results;
}

/// The screen drawn at 160x34 and where `hit` was drawn on it.
fn hit_at(h: &mut Harness, hit: BarHit) -> (u16, u16) {
    h.draw(160, 34);
    let r = h.app.layout.qbar.iter().find(|(_, x)| *x == hit).map(|(r, _)| *r);
    let r = r.unwrap_or_else(|| panic!("{hit:?} not drawn: {:?}", h.app.layout.qbar));
    (r.x, r.y)
}

/// A click (press and release) on `hit`, once the bar has been on screen long enough to take it.
fn click(h: &mut Harness, hit: BarHit) {
    let (x, y) = hit_at(h, hit);
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The caps (`LIMIT`) of the runs sent since the last look, with their statements.
fn caps(h: &mut Harness) -> Vec<(Vec<String>, Option<u64>)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, max_rows, .. } => Some((statements, max_rows)),
            _ => None,
        })
        .collect()
}

/// The text of the bar's line at 160x34.
fn bar_line(h: &mut Harness) -> String {
    let t = h.draw(160, 34);
    let y = h.app.layout.results.y + 1;
    row_text(t.backend().buffer(), y)
}

#[test]
fn the_grid_keeps_its_rows_with_the_strip_on_the_border_and_the_bar_inside() {
    for (w, hh, rows) in [(120, 34, 27), (80, 24, 17)] {
        let mut h = users_n(200);
        assert!(h.app.explorer_shown());
        let t = h.draw(w, hh);
        assert_eq!(h.app.tab().grid.page_rows, rows, "{w}x{hh}");
        let buf = t.backend().buffer();
        let top = row_text(buf, h.app.layout.results.y);
        assert!(top.contains("[Result 1]") && top.contains("Messages"), "the strip is on the border: {top}");
        let bar = row_text(buf, h.app.layout.results.y + 1);
        assert!(bar.contains("WHERE") && bar.contains("LIMIT"), "the bar is under it: {bar}");
        let bottom = row_text(buf, h.app.layout.results.y + h.app.layout.results.height - 1);
        assert!(!bottom.contains('\u{25bc}'), "nothing on the bottom border: {bottom}");
    }
}

#[test]
fn a_console_tab_has_no_bar() {
    let mut h = Harness::connected(Lang::En);
    h.draw(120, 34);
    assert!(!h.app.tab().is_table());
    assert!(h.app.layout.qbar.is_empty(), "no bar on a console tab");
}

#[test]
fn a_chips_cross_takes_its_condition_out_and_sends_the_others_as_written() {
    let mut h = users();
    filtered(&mut h, "name = 'a'  AND id>1 AND (id < 9 OR  id = 20)");
    assert!(bar_line(&mut h).contains("name = 'a'"), "{}", bar_line(&mut h));
    click(&mut h, BarHit::Remove(0));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id>1 AND (id < 9 OR  id = 20))")]]);
    answer(&mut h);
    let line = bar_line(&mut h);
    assert!(!line.contains("name = 'a'") && line.contains("id>1"), "{line}");
    // The last one taken out: no condition.
    click(&mut h, BarHit::Remove(1));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id>1)")]]);
    answer(&mut h);
    click(&mut h, BarHit::Remove(0));
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
}

#[test]
fn where_focus_moves_among_the_chips_removes_one_and_leaves() {
    let mut h = users();
    filtered(&mut h, "name = 'a' AND id > 1");
    h.draw(120, 34);
    h.keys(" rw");
    assert_eq!(h.app.key_context(), Ctx::QueryBar);
    // The leader still works while the bar has the keys.
    h.keys(" rl");
    assert_eq!(h.app.key_context(), Ctx::QueryBarText);
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.keys(" rw");
    h.draw(120, 34);
    h.key(KeyCode::Right);
    h.key(KeyCode::Char('x'));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'a')")]]);
    answer(&mut h);
    h.draw(120, 34);
    assert_eq!(h.app.key_context(), Ctx::QueryBar, "the keys stay on the bar");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // Enter opens the condition the keys are on in the dialog; the bar lets the keys go.
    h.keys(" rw");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // Another tab takes them too.
    h.keys(" rw");
    h.app.focus = Focus::Tree;
    h.app.focus = Focus::Results;
    assert_eq!(h.app.key_context(), Ctx::QueryBar);
    h.key(KeyCode::Tab);
    assert_ne!(h.app.key_context(), Ctx::QueryBar);
}

#[test]
fn limit_typed_in_the_bar_runs_refuses_and_cancels() {
    let mut h = users();
    h.keys(" rl");
    assert_eq!(h.app.key_context(), Ctx::QueryBarText);
    h.keys("2x5");
    assert!(bar_line(&mut h).contains("LIMIT 25"), "letters are not typed: {}", bar_line(&mut h));
    h.key(KeyCode::Enter);
    assert_eq!(caps(&mut h), vec![(vec![ALL.to_string()], Some(25))]);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    answer(&mut h);
    assert!(bar_line(&mut h).contains("LIMIT 25"));
    // A cap of no rows is refused: said, nothing sent, the field stays to be fixed.
    h.keys(" rl");
    for _ in 0..2 {
        h.key(KeyCode::Backspace);
    }
    h.event(ratatui::crossterm::event::Event::Paste("0".into()));
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(h.app.key_context(), Ctx::QueryBarText);
    assert!(h.status(120, 34).contains("limit"), "{}", h.status(120, 34));
    // Esc leaves the cap as it was.
    h.key(KeyCode::Esc);
    assert!(runs(&mut h).is_empty());
    assert!(bar_line(&mut h).contains("LIMIT 25"));
    // Empty: no cap.
    click(&mut h, BarHit::Limit);
    assert_eq!(h.app.key_context(), Ctx::QueryBarText);
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Enter);
    assert_eq!(caps(&mut h), vec![(vec![ALL.to_string()], None)]);
}

#[test]
fn the_page_arrows_page_and_say_when_they_do_nothing() {
    let mut h = users_n(1200);
    let line = bar_line(&mut h);
    assert!(line.contains("1–500 of 1,200 rows"), "{line}");
    click(&mut h, BarHit::Next);
    assert!(bar_line(&mut h).contains("501–1,000 of 1,200 rows"), "{}", bar_line(&mut h));
    click(&mut h, BarHit::Prev);
    assert!(bar_line(&mut h).contains("1–500 of 1,200 rows"));
    assert!(runs(&mut h).is_empty(), "the pages were read");
    // The rows: counted as `#` counts them (complete here: said so, nothing sent).
    click(&mut h, BarHit::Rows);
    assert!(runs(&mut h).is_empty());
}

#[test]
fn bar_parts_open_the_dialog() {
    for hit in [BarHit::Chip(0), BarHit::Add, BarHit::Order, BarHit::Cols, BarHit::Advanced] {
        let mut h = users();
        filtered(&mut h, "name = 'a'");
        click(&mut h, hit);
        assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "{hit:?}");
    }
    // The keys: columns, Advanced.
    for keys in [" rc", " rA"] {
        let mut h = users();
        h.keys(keys);
        assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "{keys}");
    }
}

#[test]
fn bar_keys_are_only_on_table_tabs() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::Tab);
    h.keys(" rw");
    assert!(h.status(120, 34).contains("only on table tabs") || h.app.key_context() != Ctx::QueryBar);
}

/// The bar's screens: empty, applied, failed, running, a hidden column, the terminal theme.
#[test]
fn the_bar_screens() {
    for (w, hh) in [(80u16, 24u16), (120, 34)] {
        let mut h = users();
        assert_screen!(format!("query_bar_empty_en_{w}x{hh}"), Lang::En, h.draw(w, hh));

        let mut h = users();
        filtered(&mut h, "name = 'a' AND id > 1");
        h.keys("s");
        runs(&mut h);
        answer(&mut h);
        assert_screen!(format!("query_bar_applied_en_{w}x{hh}"), Lang::En, h.draw(w, hh));

        h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
        assert_screen!(format!("query_bar_terminal_en_{w}x{hh}"), Lang::En, h.draw(w, hh));

        let mut h = users();
        h.command("where name = 'b'");
        assert_screen!(format!("query_bar_running_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        let id = h.app.tab().exec.query_id;
        let index = h.app.tabs.active_index();
        let error =
            DbError::Server(datarig_core::driver::ServerMessage { text: "no".into(), position: None, hint: None });
        h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
        assert_screen!(format!("query_bar_failed_en_{w}x{hh}"), Lang::En, h.draw(w, hh));

        let mut h = users();
        h.keys("C");
        assert_screen!(format!("query_bar_hidden_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
}
