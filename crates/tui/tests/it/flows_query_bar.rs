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
    // Another tab takes them for good: coming back finds the grid.
    h.keys(" rw");
    assert_eq!(h.app.key_context(), Ctx::QueryBar);
    h.keys("gt");
    assert!(!h.app.tab().is_table());
    h.keys("gT");
    assert!(h.app.tab().is_table());
    h.app.focus = Focus::Results;
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // So does a click on the explorer, and the LIMIT typed so far is gone.
    h.keys(" rl7");
    assert!(bar_line(&mut h).contains("LIMIT 7"));
    let tree = h.app.layout.tree;
    h.mouse(MouseEventKind::Down(MouseButton::Left), tree.x + 2, tree.y + 2);
    h.mouse(MouseEventKind::Up(MouseButton::Left), tree.x + 2, tree.y + 2);
    assert_ne!(h.app.focus, Focus::Results);
    h.app.focus = Focus::Results;
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.keys(" rl");
    assert!(bar_line(&mut h).contains("LIMIT  "), "the applied cap (none), not what was typed: {}", bar_line(&mut h));
}

/// The grid's search prompt and its cell editor take the keys from the bar: what is typed there
/// never takes a condition out.
#[test]
fn a_field_over_the_grid_takes_the_keys_from_the_bar() {
    // `shop.users` with its keys known and rows read from its columns: its cells can be edited.
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    open_users(&mut h);
    keys_came(&mut h, "local-pg", &["id"]);
    let page = |h: &mut Harness| {
        let columns = vec![meta_from("id", "int8", true, USERS, 1), meta_from("name", "text", false, USERS, 3)];
        let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
        let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
        h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    };
    page(&mut h);
    h.command("where name = 'a' AND id > 1");
    page(&mut h);
    runs(&mut h);
    h.app.focus = Focus::Results;
    h.app.detail.visible = false;
    h.draw(160, 34);
    h.keys(" rw");
    assert_eq!(h.app.key_context(), Ctx::QueryBar);
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::GridSearch);
    h.keys("x");
    h.key(KeyCode::Delete);
    assert!(runs(&mut h).is_empty());
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid, "the bar's keys did not come back");
    h.keys(" rwe");
    // `e` reads the table's structure first; the editor opens once it came.
    if !h.app.cell_edit_open() {
        let st = Box::new(users_structure());
        h.meta_db("local-pg", DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(st) });
    }
    assert!(h.app.cell_edit_open(), "{}", h.status(160, 34));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.keys("x");
    h.key(KeyCode::Esc);
    assert!(runs(&mut h).is_empty());
}

/// The keys go to the bar only while the rows are shown: on the Chart view they are the chart's.
#[test]
fn the_bar_has_no_keys_over_another_view() {
    let mut h = users();
    h.keys("c");
    h.keys(" rw");
    assert_ne!(h.app.key_context(), Ctx::QueryBar);
}

/// `x` takes out only a condition drawn in the bar; on the count of those left out it opens
/// them (the dialog for now).
#[test]
fn x_on_a_condition_left_out_opens_them() {
    let mut h = users();
    filtered(&mut h, "name = 'a' AND id > 1 AND id < 9 AND name <> 'a rather long text to leave out'");
    h.draw(90, 24);
    h.keys(" rw");
    let items: Vec<BarHit> = h.app.layout.qbar.iter().map(|(_, b)| *b).filter(|b| b.in_where()).collect();
    assert!(items.contains(&BarHit::More), "collapsed: {items:?}");
    // The keys start on the first part drawn: on to the count of those left out.
    let at = items.iter().position(|b| *b == BarHit::More).unwrap();
    for _ in 0..at {
        h.key(KeyCode::Right);
    }
    h.key(KeyCode::Char('x'));
    assert!(runs(&mut h).is_empty(), "nothing taken out");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
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

/// The colour a part of the bar is drawn in (at 160x34).
fn fg_of(h: &mut Harness, hit: BarHit) -> ratatui::style::Color {
    let t = h.draw(160, 34);
    let r = h.app.layout.qbar.iter().find(|(_, x)| *x == hit).map(|(r, _)| *r).expect("drawn");
    t.backend().buffer()[(r.x, r.y)].fg
}

#[test]
fn the_page_arrows_page_and_say_when_they_do_nothing() {
    let th = datarig_tui::theme::cur();
    let mut h = users_n(1200);
    let line = bar_line(&mut h);
    assert!(line.contains("1–500 of 1,200 rows"), "{line}");
    // On the first page the previous one is dim; a click says there is none.
    assert_eq!((fg_of(&mut h, BarHit::Prev), fg_of(&mut h, BarHit::Next)), (th.fg_dim, th.accent));
    click(&mut h, BarHit::Prev);
    assert!(h.status(160, 34).contains("First page"), "{}", h.status(160, 34));
    click(&mut h, BarHit::Next);
    click(&mut h, BarHit::Next);
    assert!(bar_line(&mut h).contains("1,001–1,200 of 1,200 rows"), "{}", bar_line(&mut h));
    // The last page of a complete result: the next one is dim, a click says so.
    assert_eq!((fg_of(&mut h, BarHit::Prev), fg_of(&mut h, BarHit::Next)), (th.accent, th.fg_dim));
    click(&mut h, BarHit::Next);
    assert!(h.status(160, 34).contains("Last page"), "{}", h.status(160, 34));
    click(&mut h, BarHit::Prev);
    assert!(bar_line(&mut h).contains("501–1,000 of 1,200 rows"));
    assert!(runs(&mut h).is_empty(), "the pages were read");
    // The rows: counted as `#` counts them (complete here: said so, nothing sent).
    click(&mut h, BarHit::Rows);
    assert!(runs(&mut h).is_empty());
    assert!(h.status(160, 34).contains("Every row is fetched already"), "{}", h.status(160, 34));
}

/// A page with more past it, nothing held on the server: the next page runs the statement again,
/// so its arrow works.
#[test]
fn the_next_arrow_follows_the_paging_state() {
    let th = datarig_tui::theme::cur();
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    assert!(bar_line(&mut h).contains("1–500 of 500+ rows"), "{}", bar_line(&mut h));
    assert_eq!(fg_of(&mut h, BarHit::Next), th.accent);
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
    assert!(!h.app.tab().is_table());
    for keys in [" rw", " rl", " rr"] {
        h.keys(keys);
        assert_ne!(h.app.key_context(), Ctx::QueryBar, "{keys}");
        assert_ne!(h.app.key_context(), Ctx::QueryBarText, "{keys}");
        assert!(h.status(120, 34).contains("Only a table tab"), "{keys}: {}", h.status(120, 34));
    }
}

/// The server's answer of `id` with a refusal (`text`), or with a cancel.
fn fail(h: &mut Harness, cancelled: bool) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let error = if cancelled {
        DbError::Cancelled
    } else {
        DbError::Server(datarig_core::driver::ServerMessage { text: "no".into(), position: None, hint: None })
    };
    h.tab_db(index, DbEvent::Failed { id, error, cancelled });
}

/// While the tab runs, neither a chip's `×` nor the `LIMIT` field sends anything, and what the
/// tab would run next is kept.
#[test]
fn a_busy_tab_takes_no_bar_change() {
    let mut h = users();
    filtered(&mut h, "name = 'a' AND id > 1");
    h.command("orderby id");
    runs(&mut h);
    click(&mut h, BarHit::Remove(0));
    h.keys(" rl5");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(h.app.key_context(), Ctx::QueryBarText, "the field stays");
    h.key(KeyCode::Esc);
    answer(&mut h);
    let line = bar_line(&mut h);
    assert!(line.contains("name = 'a'") && line.contains("id > 1") && line.contains("ORDER BY ↑ id"), "{line}");
}

/// A condition left after a `×` that calls a function datarig cannot check is asked about first:
/// nothing is sent before the answer. On a read-only connection it is refused, said in the bar.
#[test]
fn bar_changes_go_through_the_checks() {
    let mut h = users();
    h.command("where my_fn(id) > 0 AND id > 1");
    h.keys("y");
    runs(&mut h);
    answer(&mut h);
    h.app.focus = Focus::Results;
    click(&mut h, BarHit::Remove(1));
    assert!(runs(&mut h).is_empty(), "asked first");
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (my_fn(id) > 0)")]]);
    answer(&mut h);
    // Read-only: the function's call is refused before anything is sent.
    let ro = datarig_core::policy::Environment::new(
        "ro",
        datarig_core::policy::Policy { read_only: true, ..datarig_core::policy::Policy::default() },
    );
    h.app.policies.environments_mut().push(ro);
    h.app.profiles[0].policy = Some("ro".into());
    h.app.focus = Focus::Results;
    h.keys(" rl5");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "refused");
    assert!(bar_line(&mut h).contains("refused"), "{}", bar_line(&mut h));
}

/// The state goes first whatever part failed: an order alone, a limit alone; and the last
/// condition taken out then cancelled says so over an empty `WHERE`.
/// The cap in the field is selected when it opens: typing replaces it, `Backspace` clears it, a
/// key that moves keeps it and types after it.
#[test]
fn the_limit_field_opens_with_its_cap_selected() {
    let mut h = users();
    h.keys(" rl500");
    h.key(KeyCode::Enter);
    runs(&mut h);
    answer(&mut h);
    h.keys(" rl25");
    assert!(bar_line(&mut h).contains("LIMIT 25 "), "replaced, not 50025: {}", bar_line(&mut h));
    h.key(KeyCode::Esc);
    h.keys(" rl");
    h.key(KeyCode::Backspace);
    assert!(bar_line(&mut h).contains("LIMIT  "), "cleared: {}", bar_line(&mut h));
    h.key(KeyCode::Esc);
    h.keys(" rl");
    h.key(KeyCode::End);
    h.keys("1");
    assert!(bar_line(&mut h).contains("LIMIT 5001"), "{}", bar_line(&mut h));
}

#[test]
fn the_state_of_a_failed_order_limit_or_cancelled_run() {
    let mut h = users();
    h.command("orderby name desc");
    runs(&mut h);
    fail(&mut h, false);
    let line = bar_line(&mut h);
    assert!(line.contains(" failed WHERE + │ ORDER BY ↓ name +"), "{line}");

    let mut h = users();
    h.keys(" rl5");
    h.key(KeyCode::Enter);
    runs(&mut h);
    fail(&mut h, false);
    let line = bar_line(&mut h);
    assert!(line.contains(" failed WHERE") && line.contains("LIMIT 5"), "{line}");

    let mut h = users();
    filtered(&mut h, "id > 1");
    click(&mut h, BarHit::Remove(0));
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
    fail(&mut h, true);
    let line = bar_line(&mut h);
    assert!(line.contains(" cancelled WHERE + │"), "{line}");
}

/// A `LIMIT` typed as it is applied runs nothing; a paste longer than any cap is cut.
#[test]
fn limit_unchanged_runs_nothing_and_a_long_paste_is_cut() {
    let mut h = users();
    h.keys(" rl5");
    h.key(KeyCode::Enter);
    runs(&mut h);
    answer(&mut h);
    h.keys(" rl");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // The cap opens selected: the first digit replaces it. A click on the field being typed in
    // keeps what is typed.
    h.keys(" rl7");
    click(&mut h, BarHit::Limit);
    assert!(bar_line(&mut h).contains("LIMIT 7 "), "{}", bar_line(&mut h));
    h.event(ratatui::crossterm::event::Event::Paste("9".repeat(100)));
    let line = bar_line(&mut h);
    let digits = line.split("LIMIT ").nth(1).unwrap().chars().take_while(|c| c.is_ascii_digit()).count();
    assert_eq!(digits, 20, "{line}");
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
