//! A table tab's filter, order and state in its query bar: the conditions as chips and the keys
//! of the order, counted where the pane is narrow (never left out); the spinner while the run
//! of the query is on its way; `failed` when the server refused it, `cancelled` when it was
//! cancelled; nothing on the bottom border any more. A click on a chip opens the filter dialog.

use crate::common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::qbar::BarHit;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
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

/// The query bar's line of the results pane drawn at `w`×`hh`.
fn bar(h: &mut Harness, w: u16, hh: u16) -> String {
    let screen = h.screen(w, hh);
    let r = h.app.layout.results;
    let line = screen.lines().nth(usize::from(r.y + 1)).unwrap();
    let cols: Vec<char> = line.chars().collect();
    let x = usize::from(r.x) + 1;
    cols[x..x + usize::from(r.width) - 2].iter().collect::<String>().trim_start().to_string()
}

/// The parts of the bar drawn last.
fn hits(h: &Harness) -> Vec<BarHit> {
    h.app.layout.qbar.iter().map(|(_, b)| *b).collect()
}

#[test]
fn the_bar_says_the_conditions_and_the_order_at_every_width() {
    let mut h = filtered();
    let whole = bar(&mut h, 240, 34);
    assert!(whole.contains("name <> 'a name long enough to be cut' \u{d7}"), "{whole}");
    assert!(whole.contains("id > 1 \u{d7}"), "{whole}");
    assert!(whole.contains("ORDER BY \u{2193} name"), "{whole}");
    for w in (80..=240).rev() {
        bar(&mut h, w, 34);
        let hs = hits(&h);
        assert!(hs.contains(&BarHit::Chip(0)) || hs.contains(&BarHit::More), "{w}: {}", bar(&mut h, w, 34));
        assert!(hs.contains(&BarHit::Order), "{w}");
        let r = h.app.layout.results;
        let bottom = h.screen(w, 34).lines().nth(usize::from(r.y + r.height - 1)).unwrap().to_string();
        assert!(!bottom.contains('\u{25bc}'), "{w}: nothing on the bottom border: {bottom}");
    }
    // Nothing applied: no chip.
    h.keys(" rx");
    answer(&mut h);
    bar(&mut h, 120, 34);
    assert!(!hits(&h).iter().any(|b| matches!(b, BarHit::Chip(_) | BarHit::More)));
}

#[test]
fn a_click_on_a_chip_opens_the_dialog_on_its_column() {
    let mut h = filtered();
    h.draw(120, 34);
    let r = h.app.layout.qbar.iter().find(|(_, b)| *b == BarHit::Chip(0)).unwrap().0;
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y);
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    assert_eq!(h.app.key_context(), Ctx::FilterDialog);
    assert_eq!(
        h.app.filter_dialog_draft(),
        Some(("name <> 'a name long enough to be cut' AND id > 1".into(), "name DESC".into()))
    );
    assert!(runs(&mut h).is_empty(), "nothing sent");
}

#[test]
fn a_run_on_its_way_shows_the_spinner_and_a_refused_one_failed() {
    let mut h = users();
    h.command("where id > 1");
    runs(&mut h);
    let running = bar(&mut h, 120, 34);
    assert!(running.starts_with("\u{280b} WHERE  id > 1 \u{d7}"), "{running}");
    // The server refuses it: failed, in the warning colour; the chips say what was asked.
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let error = datarig_core::driver::DbError::Server(datarig_core::driver::ServerMessage {
        text: "permission denied for table users".into(),
        position: None,
        hint: None,
    });
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
    let failed = bar(&mut h, 120, 34);
    assert!(failed.starts_with("failed WHERE  id > 1 \u{d7}"), "{failed}");
    let t = h.draw(120, 34);
    let r = h.app.layout.results;
    let th = datarig_tui::theme::cur();
    assert_eq!(t.backend().buffer()[(r.x + 2, r.y + 1)].fg, th.warning);
    // A click on the chip shows why in the dialog.
    let c = h.app.layout.qbar.iter().find(|(_, b)| *b == BarHit::Chip(0)).unwrap().0;
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), c.x + 1, c.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), c.x + 1, c.y);
    let screen = h.screen(120, 34);
    assert!(screen.contains("permission denied for table users"), "{screen}");
    // Cancelled: said so.
    h.key(KeyCode::Esc);
    h.command("where id > 2");
    runs(&mut h);
    let id = h.app.tab().exec.query_id;
    h.tab_db(index, DbEvent::Failed { id, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    let cancelled = bar(&mut h, 120, 34);
    assert!(cancelled.starts_with("cancelled WHERE"), "{cancelled}");
}

#[test]
fn an_order_alone_then_with_a_condition_and_icons() {
    let mut h = users();
    h.command("orderby name, id desc");
    runs(&mut h);
    answer(&mut h);
    let b = bar(&mut h, 120, 34);
    assert!(b.starts_with("WHERE + \u{2502} ORDER BY \u{2191} name, \u{2193} id +"), "{b}");
    // With icons the condition's chip is the same.
    h.command("where id > 1");
    runs(&mut h);
    answer(&mut h);
    h.app.icons = IconsSetting::On;
    let b = bar(&mut h, 120, 34);
    assert!(b.contains("id > 1 \u{d7}"), "{b}");
    assert!(hits(&h).contains(&BarHit::Order));
}
