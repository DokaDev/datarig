//! A table tab's filter and sort on the bottom border of its results pane: the first condition
//! and the first key of the order with how many more, cut and then counted where the pane is
//! narrow; the spinner while the run of the query is on its way; `▼ failed` when the server
//! refused it; nothing with no condition and no order. A click on it opens the filter dialog.

use crate::common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::time::Duration;

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

/// Answer the active tab's current run with a first page of `shop.users` rows.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// `shop.users` open and answered, the grid focused, nothing sent since.
fn users() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // The pane at its widest: no inspector beside it.
    h.app.detail.visible = false;
    h
}

/// `users` with two conditions and an order applied and answered.
fn filtered() -> Harness {
    let mut h = users();
    h.command("where name <> 'a name long enough to be cut' AND id > 1");
    answer(&mut h);
    h.command("orderby name desc");
    answer(&mut h);
    assert_eq!(runs(&mut h).len(), 2);
    h
}

/// The bottom border of the results pane drawn at `w`×`hh`.
fn border(h: &mut Harness, w: u16, hh: u16) -> String {
    let screen = h.screen(w, hh);
    let r = h.app.layout.results;
    let line = screen.lines().nth(usize::from(r.y + r.height - 1)).unwrap();
    let cols: Vec<char> = line.chars().collect();
    let x = usize::from(r.x);
    cols[x..x + usize::from(r.width)].iter().collect()
}

#[test]
fn the_border_says_the_first_condition_and_key_and_how_many_more() {
    let mut h = filtered();
    // From the widest pane down: whole, then the condition cut (counted only in a pane
    // narrower than any here); never without the filter's mark.
    let whole = "\u{25bc} name <> 'a name long enough to be cut' +1 \u{00b7} name \u{2193} \u{2500}";
    let (mut seen_whole, mut seen_cut, mut seen_counted) = (false, false, false);
    for w in (80..=240).rev() {
        let b = border(&mut h, w, 34);
        assert!(b.starts_with("\u{2570} \u{25bc} "), "{w}: {b}");
        if b.contains(whole) {
            assert!(!seen_cut && !seen_counted, "{w}: {b}");
            seen_whole = true;
        } else if b.contains("\u{2026} +1 \u{00b7} name \u{2193} ") {
            assert!(!seen_counted, "{w}: {b}");
            // At least `CUT_MIN` columns of the condition's text.
            assert!(b.contains("\u{25bc} name <> \u{2026}") || b.contains("\u{25bc} name <> '"), "{w}: {b}");
            seen_cut = true;
        } else {
            assert!(b.contains("\u{25bc} 2 \u{00b7} \u{2193} 1 "), "{w}: {b}");
            seen_counted = true;
        }
    }
    assert!(seen_whole && seen_cut, "{seen_counted}");
    // Wide and narrow, the strip names the connection whole and short.
    assert_screen!("table_status_applied_en_120x34", Lang::En, h.draw(120, 34));
    assert_screen!("table_status_applied_en_80x24", Lang::En, h.draw(80, 24));
    // Nothing applied: nothing on the border.
    h.keys(" rx");
    answer(&mut h);
    let plain = border(&mut h, 120, 34);
    assert!(plain.chars().skip(1).take(10).all(|c| c == '\u{2500}'), "{plain}");
    assert_eq!(h.app.layout.table_status, ratatui::layout::Rect::default());
}

#[test]
fn a_click_on_the_border_status_opens_the_dialog() {
    let mut h = filtered();
    h.draw(120, 34);
    let r = h.app.layout.table_status;
    assert_eq!(r.y, h.app.layout.results.y + h.app.layout.results.height - 1, "on the bottom border");
    // The pointer lights it.
    h.mouse(MouseEventKind::Moved, r.x + 3, r.y);
    let t = h.draw(120, 34);
    assert!(t.backend().buffer()[(r.x + 3, r.y)].modifier.contains(Modifier::BOLD), "lit under the pointer");
    h.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 3, r.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 3, r.y);
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    assert_eq!(h.app.key_context(), Ctx::FilterDialog);
    assert_eq!(
        h.app.filter_dialog_draft(),
        Some(("name <> 'a name long enough to be cut' AND id > 1".into(), "name DESC".into()))
    );
    assert!(runs(&mut h).is_empty(), "nothing sent");
    // Esc closes it; a click on the border right of the status is the border's.
    h.key(KeyCode::Esc);
    h.draw(120, 34);
    let r = h.app.layout.table_status;
    h.mouse(MouseEventKind::Down(MouseButton::Left), r.x + r.width + 2, r.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), r.x + r.width + 2, r.y);
    assert!(!h.app.overlays.is_open(OverlayKind::FilterDialog));
}

#[test]
fn a_run_on_its_way_shows_the_spinner_and_a_refused_one_failed() {
    let mut h = users();
    h.command("where id > 1");
    runs(&mut h);
    let running = border(&mut h, 120, 34);
    assert!(running.starts_with("\u{2570} \u{280b} \u{25bc} id > 1 \u{2500}"), "{running}");
    assert_screen!("table_status_running_en_120x34", Lang::En, h.draw(120, 34));
    assert_screen!("table_status_running_en_80x24", Lang::En, h.draw(80, 24));
    // The server refuses it: failed, in the warning colour, the earlier rows dimmed under it.
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let error = datarig_core::driver::DbError::Server(datarig_core::driver::ServerMessage {
        text: "permission denied for table users".into(),
        position: None,
        hint: None,
    });
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
    let failed = border(&mut h, 120, 34);
    assert!(failed.starts_with("\u{2570} \u{25bc} failed \u{2500}"), "{failed}");
    assert_screen!("table_status_failed_en_120x34", Lang::En, h.draw(120, 34));
    assert_screen!("table_status_failed_en_80x24", Lang::En, h.draw(80, 24));
    let t = h.draw(120, 34);
    let r = h.app.layout.table_status;
    let th = datarig_tui::theme::cur();
    assert_eq!(t.backend().buffer()[(r.x + 1, r.y)].fg, th.warning);
    // A click shows why in the dialog.
    h.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y);
    let screen = h.screen(120, 34);
    assert!(screen.contains("permission denied for table users"), "{screen}");
    // Cancelled: said so.
    h.key(KeyCode::Esc);
    h.command("where id > 2");
    runs(&mut h);
    let id = h.app.tab().exec.query_id;
    h.tab_db(index, DbEvent::Failed { id, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    let cancelled = border(&mut h, 120, 34);
    assert!(cancelled.starts_with("\u{2570} \u{25bc} cancelled \u{2500}"), "{cancelled}");
}

#[test]
fn an_order_alone_then_with_a_condition_and_icons() {
    let mut h = users();
    h.command("orderby name, id desc");
    runs(&mut h);
    answer(&mut h);
    let b = border(&mut h, 120, 34);
    assert!(b.starts_with("\u{2570} name \u{2191} +1 \u{2500}"), "{b}");
    // With icons the filter's mark is the glyph; the arrows stay.
    h.command("where id > 1");
    runs(&mut h);
    answer(&mut h);
    h.app.icons = IconsSetting::On;
    let b = border(&mut h, 120, 34);
    assert!(b.starts_with("\u{2570} \u{f0b0} id > 1 \u{00b7} name \u{2191} +1 \u{2500}"), "{b}");
}
