//! The query bar's condition popup: a click on a column's condition opens the filter dialog as
//! a small popup under it, on that column alone. It edits the dialog's one draft (undo, the
//! apply waiting, the checks and questions of a run all as the whole dialog's), `Enter` applies
//! through the tab's run and it closes once the statement is sent; `Esc` or a press outside
//! closes it without applying; `A` or `⋯ Advanced` turns it into the whole dialog.
//!
//! `WHERE` and `+` open the column list of a new condition (the add popup): picking a column
//! turns it into that column's condition popup with a new condition waiting for its value.
//! The count of conditions left out of the bar (`+N`) opens the WHERE list: one condition a
//! line, `Enter` opens one, `+` adds, `x` takes it out and runs the query as the bar's `×`
//! does (the other conditions sent byte for byte), the list closing once it is sent.
//!
//! `ORDER BY`, the column count and the rows open their popups: the order's keys then the other
//! columns (`Space` direction, `J`/`K` move, `x` drop, the clause sent underneath with its
//! tie-breaker dimmed), the columns shown and hidden (`Space`, `a`), and the rows shown with
//! their pages and counts (the bar's arrows and `#`, never a count of values).

use crate::common;

use common::*;
use datarig_core::driver::keys::OrderKeys;
use datarig_core::driver::{DbCommand, DbError, DbEvent, ServerMessage};
use datarig_i18n::Lang;
use datarig_tui::app::Focus;
use datarig_tui::app::filter_dialog::view::PopupKindId;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::qbar::BarHit;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
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

/// Answer the active tab's current run with `n` rows of `shop.users`: ids, and names that
/// repeat every five rows.
fn answer_n(h: &mut Harness, n: usize) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..n).map(|i| vec![Some(i.to_string()), Some(format!("n{}", i % 5))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

fn answer(h: &mut Harness) {
    answer_n(h, 3);
}

/// `shop.users` open and answered, filtered with `filter` (answered too), the grid focused,
/// nothing sent since.
fn users_where(filter: &str) -> Harness {
    users_where_in(&test_db_config(), filter)
}

/// [`users_where`] with `cfg` as the configuration.
fn users_where_in(cfg: &datarig_core::config::Config, filter: &str) -> Harness {
    let mut h = Harness::connected_with(cfg, Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.app.detail.visible = false;
    h.command(&format!("where {filter}"));
    if h.app.overlays.is_open(OverlayKind::Confirm) {
        h.keys("y");
    }
    runs(&mut h);
    answer(&mut h);
    h.app.focus = Focus::Results;
    h
}

/// [`users_where`] with the table's keys read (`id` its primary key), which make an order
/// total.
fn users_keyed(filter: &str) -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let sent = h.sent();
    let ask = sent.iter().find_map(|c| match c {
        DbCommand::LoadTableKeys { id, .. } => Some(*id),
        _ => None,
    });
    answer(&mut h);
    let keys = OrderKeys { primary_key: vec!["id".into()], ..OrderKeys::default() };
    h.meta_db("local-pg", DbEvent::TableKeys { id: ask.expect("the keys read"), result: Ok(keys) });
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    h.command(&format!("where {filter}"));
    runs(&mut h);
    answer(&mut h);
    h.app.focus = Focus::Results;
    h
}

/// Where `hit` of the bar is drawn at `w` x `hh`.
fn hit_rect(h: &mut Harness, hit: BarHit, w: u16, hh: u16) -> ratatui::layout::Rect {
    h.draw(w, hh);
    let r = h.app.layout.qbar.iter().find(|(_, x)| *x == hit).map(|(r, _)| *r);
    r.unwrap_or_else(|| panic!("{hit:?} not drawn: {:?}", h.app.layout.qbar))
}

/// A press and a release at (x, y).
fn click_at(h: &mut Harness, x: u16, y: u16) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// A click on the condition `term` of the bar at 120x34, then the popup on screen long enough
/// to take clicks.
fn open_chip(h: &mut Harness, term: usize) {
    let r = hit_rect(h, BarHit::Chip(term), 120, 34);
    h.advance(Duration::from_millis(500));
    click_at(h, r.x, r.y);
    assert_eq!(h.app.filter_dialog_popup(), Some(PopupKindId::Condition));
    h.draw(120, 34);
    h.advance(Duration::from_millis(500));
}

/// The first place `text` is on the screen at `w` x `hh` (all of the line before it one
/// column a character).
fn find(h: &mut Harness, w: u16, hh: u16, text: &str) -> Option<(u16, u16)> {
    let t = h.draw(w, hh);
    let buf = t.backend().buffer();
    (0..hh).find_map(|y| {
        let line = row_text(buf, y);
        line.find(text).map(|at| (line[..at].chars().count() as u16, y))
    })
}

fn popup(h: &Harness) -> Option<PopupKindId> {
    h.app.filter_dialog_popup()
}

fn open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::FilterDialog)
}

fn draft_where(h: &Harness) -> String {
    h.app.filter_dialog_draft().map(|d| d.0).unwrap_or_default()
}

/// From the operator of the popup's condition into its value, `text` typed at its end.
fn type_value(h: &mut Harness, text: &str) {
    h.key(KeyCode::Right);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    h.key(KeyCode::End);
    h.type_text(text);
}

#[test]
fn a_chip_opens_its_column_popup_under_it() {
    let mut h = users_where("name = 'n1' AND id > 1");
    let chip = hit_rect(&mut h, BarHit::Chip(0), 120, 34);
    open_chip(&mut h, 0);
    let t = h.draw(120, 34);
    let buf = t.backend().buffer();
    // Its top border on the row under the chip, from the chip's left edge; the column named
    // on it, its type muted beside it.
    assert_eq!(buf[(chip.x, chip.y + 1)].symbol(), "\u{256d}");
    let top = row_text(buf, chip.y + 1);
    assert!(top.contains(" name  text "), "{top}");
    // The keys on the condition's operator; the hint line on the bottom border.
    let screen = h.screen(120, 34);
    assert!(screen.contains("Space operator") && screen.contains("Enter apply"), "{screen}");
    assert!(screen.contains("\u{22ef} Advanced"), "{screen}");
    assert_eq!(h.app.key_context(), Ctx::FilterDialog);
    // The other condition opens on its own column.
    h.key(KeyCode::Esc);
    open_chip(&mut h, 1);
    let r = hit_rect(&mut h, BarHit::Chip(1), 120, 34);
    let top = row_text(h.draw(120, 34).backend().buffer(), r.y + 1);
    assert!(top.contains(" id  int8 "), "{top}");
}

/// An expression is no one column's condition: its chip opens the whole dialog.
#[test]
fn an_expression_chip_opens_the_whole_dialog() {
    let mut h = users_where("name = 'n1' OR id > 1");
    open_chip_full(&mut h);
    assert!(open(&h));
    assert_eq!(popup(&h), None);
}

fn open_chip_full(h: &mut Harness) {
    let r = hit_rect(h, BarHit::Chip(0), 120, 34);
    h.advance(Duration::from_millis(500));
    click_at(h, r.x, r.y);
}

/// What the popup applies is sent through the tab's run, byte for byte what the whole dialog
/// sends for the same change; the popup closes once it is sent.
#[test]
fn the_popup_applies_through_the_run_as_the_dialog_does() {
    let mut h = users_where("name = 'n1' AND id > 1");
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Enter);
    let popup_sent = runs(&mut h);
    assert!(!open(&h), "closed once sent");

    // The whole dialog, on the same condition (the popup turned into it at once): the same edit.
    let mut full = users_where("name = 'n1' AND id > 1");
    open_chip(&mut full, 0);
    full.keys("A");
    assert_eq!(popup(&full), None);
    type_value(&mut full, "x");
    full.key(KeyCode::Enter);
    let full_sent = runs(&mut full);
    assert_eq!(popup_sent, full_sent);
    assert_eq!(popup_sent.len(), 1);
    assert!(
        popup_sent[0][0].starts_with(&format!("{ALL} WHERE ")) && popup_sent[0][0].contains("n1x"),
        "{popup_sent:?}"
    );
}

/// `Tab`, `Shift+Tab`, `←` on the operator and `↑` on the first line have nowhere to go in a
/// popup: nothing changes.
#[test]
fn tab_left_and_up_stay_in_the_popup() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    let before = h.screen(120, 34);
    h.key(KeyCode::Tab);
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Left);
    h.key(KeyCode::Char('h'));
    h.key(KeyCode::Up);
    h.key(KeyCode::Char('k'));
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert_eq!(h.screen(120, 34), before);
    // The sort line keeps its `←`/`→`.
    for _ in 0..5 {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Right);
    assert_eq!(h.app.filter_dialog_draft().map(|d| d.1), Some("name ASC".to_string()));
    h.key(KeyCode::Left);
    assert_eq!(h.app.filter_dialog_draft().map(|d| d.1), Some(String::new()));
}

#[test]
fn esc_drops_the_popup_and_its_draft() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    assert!(draft_where(&h).contains("n1x"));
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    assert!(runs(&mut h).is_empty());
    open_chip(&mut h, 0);
    assert_eq!(draft_where(&h), "name = 'n1'");
}

/// A press outside the popup closes it without applying; the press goes no further (the grid
/// selects nothing).
#[test]
fn a_click_outside_closes_the_popup_and_selects_nothing() {
    let mut h = users_where("name = 'n1'");
    let grid = (h.app.tab().grid.row, h.app.tab().grid.col, h.app.tab().grid.anchor);
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Home);
    h.key(KeyCode::Left);
    let rows = h.app.layout.results;
    // A cell of the grid's last row, below the popup.
    let (x, y) = (rows.x + 3, rows.bottom() - 3);
    click_at(&mut h, x, y);
    assert!(!open(&h));
    assert!(runs(&mut h).is_empty());
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col, h.app.tab().grid.anchor), grid);
    // Applied as it was.
    open_chip(&mut h, 0);
    assert_eq!(draft_where(&h), "name = 'n1'");
}

/// `Ctrl+Z` and `u` undo the popup's steps, then the change applied last.
#[test]
fn undo_steps_back_then_to_the_query_before_the_last_change() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(draft_where(&h), "name = 'n1'");
    h.key(KeyCode::Home);
    h.key(KeyCode::Left);
    assert_eq!(h.app.key_context(), Ctx::FilterDialog);
    h.keys("u");
    assert_eq!(draft_where(&h), "", "the query before `where`");
    assert_eq!(popup(&h), Some(PopupKindId::Condition), "on its column still");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
}

/// Refused by the app (a read-only profile and a function it cannot check), the popup stays
/// open and says why on its border; nothing runs.
#[test]
fn a_refusal_keeps_the_popup_open_and_says_why() {
    let mut h = users_where("name = 'n1' AND my_fn(id) > 0");
    let ro = datarig_core::policy::Environment::new(
        "ro",
        datarig_core::policy::Policy { read_only: true, ..datarig_core::policy::Policy::default() },
    );
    h.app.policies.environments_mut().push(ro);
    h.app.profiles[0].policy = Some("ro".into());
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    let screen = h.screen(120, 34);
    assert!(screen.contains("Filter refused"), "{screen}");
    for (w, hh) in [(80u16, 24u16), (120, 34)] {
        assert_screen!(format!("query_popup_condition_refused_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
}

/// A server error: the popup has closed (the statement was sent), the bar says the run failed;
/// the popup opened again says what the server said, and the value it points into is underlined
/// in the error colour.
#[test]
fn a_server_error_is_said_when_the_popup_opens_again() {
    let mut h = users_where("name = 'n1' AND id > 1");
    open_chip(&mut h, 1);
    type_value(&mut h, "0");
    h.key(KeyCode::Enter);
    let sent = runs(&mut h);
    assert!(!open(&h));
    let sql = &sent[0][0];
    let position = sql[..sql.find("10").unwrap()].chars().count() as u32 + 1;
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let error =
        DbError::Server(ServerMessage { text: "ERROR: out of range".into(), position: Some(position), hint: None });
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
    let bar = row_text(h.draw(120, 34).backend().buffer(), h.app.layout.results.y + 1);
    assert!(bar.contains("failed"), "{bar}");
    open_chip(&mut h, 1);
    let screen = h.screen(120, 34);
    assert!(screen.contains("out of range"), "{screen}");
    let th = datarig_tui::theme::cur();
    // The value's first character: after the operator's box and the field's blank.
    let value = "\u{25be}    10";
    let (x, y) = find(&mut h, 120, 34, value).expect("the value field");
    let t = h.draw(120, 34);
    let cell = &t.backend().buffer()[(x + 5, y)];
    assert_eq!((cell.symbol(), cell.fg), ("1", th.error), "{screen}");
    assert!(cell.modifier.contains(Modifier::UNDERLINED));
    // The whole dialog underlines it too.
    h.keys("A");
    let (x, y) = find(&mut h, 120, 34, value).expect("the value field");
    let t = h.draw(120, 34);
    let cell = &t.backend().buffer()[(x + 5, y)];
    assert_eq!(cell.fg, th.error);
    assert!(cell.modifier.contains(Modifier::UNDERLINED));
}

/// Picking an operator changes the draft and applies nothing (`Enter` in the picker picks).
#[test]
fn an_operator_pick_never_applies() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    h.key(KeyCode::Char(' '));
    for (w, hh) in [(80u16, 24u16), (120, 34)] {
        assert_screen!(format!("query_popup_condition_picker_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(draft_where(&h).contains("<>") || draft_where(&h).contains("!="), "{}", draft_where(&h));
}

/// `A` turns the popup into the whole dialog: the draft not applied yet and its undo kept.
#[test]
fn advanced_keeps_the_unsent_draft_and_its_undo() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Home);
    h.key(KeyCode::Left);
    h.keys("A");
    assert!(open(&h));
    assert_eq!(popup(&h), None);
    assert!(draft_where(&h).contains("n1x"));
    let screen = h.screen(120, 34);
    assert!(screen.contains("Filter: shop.users") || screen.contains("shop.users"), "{screen}");
    h.keys("u");
    assert_eq!(draft_where(&h), "name = 'n1'");
    assert!(runs(&mut h).is_empty());
    // The mouse: `⋯ Advanced` on the border.
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    let (x, y) = find(&mut h, 120, 34, "\u{22ef} Advanced \u{2500}\u{256f}").expect("drawn");
    click_at(&mut h, x + 1, y);
    assert!(open(&h));
    assert_eq!(popup(&h), None);
}

/// A function the app cannot check is asked about over the popup: a no keeps the popup as it
/// was and runs nothing; the popup takes clicks again only a moment after it is on top again.
#[test]
fn a_question_covers_the_popup_and_a_no_keeps_it() {
    let mut h = users_where("name = 'n1' AND my_fn(id) > 0");
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    assert!(runs(&mut h).is_empty());
    h.draw(120, 34);
    h.key(KeyCode::Esc);
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(runs(&mut h).is_empty());
    assert!(draft_where(&h).contains("n1x"));
    // Uncovered: a click right away does nothing, one a moment later acts.
    let (x, y) = find(&mut h, 120, 34, "\u{22ef} Advanced \u{2500}\u{256f}").expect("drawn");
    click_at(&mut h, x + 1, y);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    h.advance(Duration::from_millis(500));
    click_at(&mut h, x + 2, y);
    assert_eq!(popup(&h), None);
    // Asked again and confirmed: it runs, and the dialog closes once sent.
    h.key(KeyCode::Enter);
    h.keys("y");
    assert_eq!(runs(&mut h).len(), 1);
    assert!(!open(&h));
}

/// The popup's apply waits for the tab's session to open again: `Esc` cancels it, and the
/// session that comes runs nothing.
#[test]
fn an_apply_waiting_then_cancelled_never_runs() {
    let mut cfg = test_db_config();
    cfg.prefs.idle_disconnect = Some(Duration::from_secs(300));
    let mut h = users_where_in(&cfg, "name = 'n1'");
    // Unused for the idle time and more: the tab's session, then the profile's, close.
    h.advance(Duration::from_secs(300));
    h.advance(Duration::from_secs(300));
    assert!(h.session_closed(1), "the tab's session closed");
    h.app.transient = None;
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Enter);
    let tab = h.app.tab().id;
    assert!(runs(&mut h).is_empty());
    assert!(open(&h) && h.app.tab_busy(tab), "it waits for the connection, the popup open");
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    assert!(!h.app.tab_busy(tab));
    h.db(DbEvent::Connected);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "the run cancelled never goes");
}

/// A press on the popup, then the view changed by the keys, then the release: nothing acts.
#[test]
fn a_press_then_a_view_switch_then_the_release_does_nothing() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    let (x, y) = find(&mut h, 120, 34, "+ add condition").expect("drawn");
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 1, y);
    h.keys("A");
    h.mouse(MouseEventKind::Up(MouseButton::Left), x + 1, y);
    assert_eq!(popup(&h), None);
    assert_eq!(draft_where(&h), "name = 'n1'", "no condition added");
    assert_eq!(h.app.key_context(), Ctx::FilterDialog, "no value field focused");
}

/// The second click of a double click on a chip lands on the popup it opened (or outside it):
/// it does nothing.
#[test]
fn a_double_click_on_a_chip_leaves_the_popup_open() {
    let mut h = users_where("name = 'n1'");
    let r = hit_rect(&mut h, BarHit::Chip(0), 120, 34);
    h.advance(Duration::from_millis(500));
    click_at(&mut h, r.x, r.y);
    h.draw(120, 34);
    click_at(&mut h, r.x, r.y);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert_eq!(draft_where(&h), "name = 'n1'");
    assert!(runs(&mut h).is_empty());
}

/// Clear all, the SQL section and the history of filters are the whole dialog's.
#[test]
fn clear_all_sql_and_history_are_off_in_a_popup() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    h.keys("XQ");
    h.key_mod(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(h.app.overlays.top().map(|o| o.kind()), Some(OverlayKind::FilterDialog));
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert_eq!(draft_where(&h), "name = 'n1'");
    assert_eq!(h.app.filter_dialog_sql_fields(), None);
}

/// The values list says where it is when it scrolls.
#[test]
fn the_values_list_says_where_it_is() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    h.command("where id > 0");
    runs(&mut h);
    answer_n(&mut h, 40);
    h.app.focus = Focus::Results;
    open_chip(&mut h, 0);
    let screen = h.screen(120, 34);
    assert!(screen.contains("1-8 of "), "{screen}");
}

/// The conditions of the popup's column that do not fit say where they are on its border.
#[test]
fn conditions_that_do_not_fit_say_where_they_are() {
    let terms: Vec<String> = (1..=30).map(|i| format!("id > {i}")).collect();
    let mut h = users_where(&terms.join(" AND "));
    open_chip(&mut h, 0);
    let screen = h.screen(80, 24);
    let top = screen.lines().find(|l| l.contains(" id  int8 ")).unwrap_or_else(|| panic!("{screen}"));
    assert!(top.contains(" 1-") && top.contains("/31 "), "{screen}");
}

/// The condition popup's screens: on the grid with the explorer open at 80x24, and at 120x34;
/// the terminal theme. At 80x24 the bar counts its conditions: the popup opened at 120x34 stays
/// where its chip was, inside the pane.
#[test]
fn the_condition_popup_screens() {
    for (w, hh) in [(80u16, 24u16), (120, 34)] {
        let mut h = users_where("name = 'n1' AND id > 1");
        assert!(h.app.explorer_shown());
        open_chip(&mut h, 0);
        assert_screen!(format!("query_popup_condition_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
        assert_screen!(format!("query_popup_condition_terminal_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
}

/// Four conditions, more than the bar shows at 120x34: the last ones are counted (`+N`).
const MANY: &str = "name = 'n1' AND id > 1 AND id < 9 AND name <> 'a rather long text to leave out of the bar'";

/// A click on part `hit` of the bar at `w` x `hh`, then the popup it opened on screen long
/// enough to take clicks.
fn open_at(h: &mut Harness, hit: BarHit, w: u16, hh: u16) {
    let r = hit_rect(h, hit, w, hh);
    h.advance(Duration::from_millis(500));
    click_at(h, r.x, r.y);
    h.draw(w, hh);
    h.advance(Duration::from_millis(500));
}

/// `+` opens the column list of a new condition; `Enter` picks the column (nothing is sent),
/// the popup becomes that column's condition popup with a new condition waiting for its value;
/// `Enter` then runs it.
#[test]
fn adding_picks_a_column_then_enter_runs_it() {
    let mut h = users_where("name = 'n1'");
    let plus = hit_rect(&mut h, BarHit::Add, 120, 34);
    open_at(&mut h, BarHit::Add, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::AddPick));
    let t = h.draw(120, 34);
    let buf = t.backend().buffer();
    assert_eq!(buf[(plus.x, plus.y + 1)].symbol(), "\u{256d}", "under the `+`, from its left edge");
    let top = row_text(buf, plus.y + 1);
    assert!(top.contains(" WHERE \u{2026} new condition on "), "{top}");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Enter pick") && screen.contains("/ search"), "{screen}");
    // The first column, `id`: picked.
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "picking sends nothing");
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText, "the new condition's value has the keys");
    let top = row_text(h.draw(120, 34).backend().buffer(), plus.y + 1);
    assert!(top.contains(" id  int8 "), "{top}");
    h.type_text("2");
    h.key(KeyCode::Enter);
    let sent = runs(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0][0].contains("name = 'n1'") && sent[0][0].contains("id = 2"), "{sent:?}");
    assert!(!open(&h), "closed once sent");
}

/// The `WHERE` keyword opens the same list; a click on a column picks it on the release.
#[test]
fn where_opens_the_list_and_a_click_picks() {
    let mut h = users_where("name = 'n1'");
    open_at(&mut h, BarHit::Where, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::AddPick));
    let (x, y) = find(&mut h, 120, 34, "name  text").expect("listed");
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 1, y);
    assert_eq!(popup(&h), Some(PopupKindId::AddPick), "the press alone picks nothing");
    h.mouse(MouseEventKind::Up(MouseButton::Left), x + 1, y);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(runs(&mut h).is_empty());
    assert!(draft_where(&h).starts_with("name = 'n1'"), "{}", draft_where(&h));
    let screen = h.screen(120, 34);
    assert!(screen.contains(" name  text "), "{screen}");
}

/// `/` searches the columns: what is typed narrows the list (the count says how many are
/// left), `Esc` closes the search and keeps the popup, `Enter` picks the column found.
#[test]
fn slash_filters_the_columns() {
    let mut h = users_where("name = 'n1'");
    open_at(&mut h, BarHit::Add, 120, 34);
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    h.type_text("am");
    let screen = h.screen(120, 34);
    assert!(screen.contains("1 of 2") && !screen.contains("id    int8"), "{screen}");
    assert!(screen.contains("Enter pick") && screen.contains("Esc close"), "{screen}");
    h.key(KeyCode::Esc);
    assert_eq!(popup(&h), Some(PopupKindId::AddPick), "the search closed, the popup kept");
    h.keys("/");
    h.type_text("nam");
    h.key(KeyCode::Enter);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(runs(&mut h).is_empty());
    let screen = h.screen(120, 34);
    assert!(screen.contains(" name  text "), "{screen}");
    // Typed in the search, `j`, `k` and `g` are letters.
    let mut h = users_where("name = 'n1'");
    open_at(&mut h, BarHit::Add, 120, 34);
    h.keys("/jkg");
    assert!(h.screen(120, 34).contains("/ jkg"), "{}", h.screen(120, 34));
}

/// The count of conditions left out of the bar (`+N`) opens the WHERE list; `x` on it in the
/// bar's keys opens the list too and takes nothing out.
#[test]
fn plus_n_opens_the_where_list() {
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    let screen = h.screen(120, 34);
    assert!(screen.contains(" WHERE  4 conditions "), "{screen}");
    assert!(screen.contains("+ add condition") && screen.contains("Enter open"), "{screen}");
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    // The bar's keys: on to the count, then `x`.
    h.draw(120, 34);
    h.keys(" rw");
    let items: Vec<BarHit> = h.app.layout.qbar.iter().map(|(_, b)| *b).filter(|b| b.in_where()).collect();
    let at = items.iter().position(|b| *b == BarHit::More).expect("counted");
    for _ in 0..at {
        h.key(KeyCode::Right);
    }
    h.key(KeyCode::Char('x'));
    assert!(runs(&mut h).is_empty(), "nothing taken out");
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
}

/// In the WHERE list `x` takes the focused condition out and runs the query as the bar's `×`
/// does: the other conditions sent byte for byte, the list closed once the statement is sent.
/// `Enter` opens a condition in its column's popup, which applies.
#[test]
fn the_where_list_removes_and_opens() {
    let spaced = "name = 'n1'  AND id>1 AND (id < 9 OR  id = 20) AND name <> 'a rather long text to leave out'";
    let mut h = users_where(spaced);
    open_at(&mut h, BarHit::More, 120, 34);
    h.key(KeyCode::Down);
    h.keys("x");
    assert_eq!(
        runs(&mut h),
        vec![vec![format!(
            "{ALL} WHERE (name = 'n1' AND (id < 9 OR  id = 20) AND name <> 'a rather long text to leave out')"
        )]],
        "the other terms as written"
    );
    assert!(!open(&h), "closed once sent");
    answer(&mut h);
    // The same through `Delete` and through the line's `×`, the bar's `×` sending the same.
    let mut bar = users_where(spaced);
    let r = hit_rect(&mut bar, BarHit::Remove(0), 120, 34);
    bar.advance(Duration::from_millis(500));
    click_at(&mut bar, r.x, r.y);
    let by_bar = runs(&mut bar);
    assert_eq!(by_bar.len(), 1);
    let mut h = users_where(spaced);
    open_at(&mut h, BarHit::More, 120, 34);
    h.key(KeyCode::Delete);
    assert_eq!(runs(&mut h), by_bar);
    let mut h = users_where(spaced);
    let more = hit_rect(&mut h, BarHit::More, 120, 34);
    open_at(&mut h, BarHit::More, 120, 34);
    // The list's first line, under its top border.
    let y = more.y + 2;
    let line = row_text(h.draw(120, 34).backend().buffer(), y);
    assert!(line.contains("name = 'n1'"), "{line}");
    let cross = line[..line.rfind('\u{00d7}').expect("its cross")].chars().count() as u16;
    click_at(&mut h, cross, y);
    assert_eq!(runs(&mut h), by_bar);
    // A busy tab refuses it: nothing runs, the list and its draft stay.
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    h.app.dispatch(datarig_tui::app::action::Action::Table(datarig_tui::app::table_filter::TableAction::Clear));
    assert_eq!(runs(&mut h).len(), 1, "a run on its way");
    h.keys("x");
    assert!(runs(&mut h).is_empty());
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    assert_eq!(draft_where(&h), MANY);
    // The focus on the next one (`id < 9`): opened, in the popup of `id`.
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "opening sends nothing");
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    let screen = h.screen(120, 34);
    assert!(screen.contains(" id  int8 ") && screen.contains("Space operator"), "{screen}");
    h.key(KeyCode::Enter);
    let sent = runs(&mut h);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent, vec![vec![format!("{ALL} WHERE ({MANY})")]]);
    // `+` turns the list into the add popup; a click on a condition's line opens it.
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    h.keys("+");
    assert_eq!(popup(&h), Some(PopupKindId::AddPick));
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    let (x, y) = find(&mut h, 120, 34, "id > 1").expect("listed");
    click_at(&mut h, x, y);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(runs(&mut h).is_empty());
}

/// An expression in the WHERE list opens the whole dialog on its chip.
#[test]
fn an_expression_in_the_where_list_opens_advanced() {
    let mut h = users_where(&format!("(name = 'n1' OR id > 1) AND {MANY}"));
    open_at(&mut h, BarHit::More, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    let screen = h.screen(120, 34);
    assert!(screen.contains("expr (name = 'n1' OR id > 1)"), "{screen}");
    h.key(KeyCode::Enter);
    assert!(open(&h));
    assert_eq!(popup(&h), None, "the whole dialog");
    assert!(runs(&mut h).is_empty());
    let screen = h.screen(120, 34);
    assert!(screen.contains("go to its column"), "the keys on the expression's chip: {screen}");
}

/// The list's steps are one session: `+`, a column picked, a condition added, then an undo
/// gives back the draft the list opened with; `A` keeps it too.
#[test]
fn from_the_list_to_a_condition_keeps_the_draft_and_its_undo() {
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    h.keys("+");
    h.key(KeyCode::Enter);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(draft_where(&h).starts_with(MANY), "{}", draft_where(&h));
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(draft_where(&h), MANY);
    // `A` keeps it too.
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    h.keys("+A");
    assert_eq!(popup(&h), None);
    assert!(open(&h));
    assert!(runs(&mut h).is_empty());
}

/// The add popup and the WHERE list have no count of values.
#[test]
fn the_list_popups_count_no_values() {
    let mut h = users_where(MANY);
    open_at(&mut h, BarHit::More, 120, 34);
    h.keys("#");
    open_at(&mut h, BarHit::Add, 120, 34);
    h.keys("#");
    let sent = h.sent();
    assert!(sent.iter().all(|c| !matches!(c, DbCommand::Execute { .. })), "{sent:?}");
}

/// The add popup's screens (its column list, then the condition popup of the column picked)
/// and the WHERE list's: on the grid with the explorer open at 80x24, and at 120x34; the add
/// popup in the terminal theme.
#[test]
fn the_add_and_where_list_screens() {
    for (w, hh) in [(80u16, 24u16), (120, 34)] {
        let mut h = users_where("name = 'n1' AND id > 1");
        assert!(h.app.explorer_shown());
        open_at(&mut h, BarHit::Add, w, hh);
        assert_screen!(format!("query_popup_add_pick_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        let theme = h.app.theme.clone();
        h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
        assert_screen!(format!("query_popup_add_pick_terminal_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        h.app.theme = theme;
        h.key(KeyCode::Down);
        h.key(KeyCode::Enter);
        assert_screen!(format!("query_popup_add_condition_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        let mut h = users_where(MANY);
        open_at(&mut h, BarHit::More, w, hh);
        h.key(KeyCode::Down);
        assert_screen!(format!("query_popup_where_list_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
}

/// The order popup's keys: `Space` puts a column in the order (last, ascending) and turns a
/// key's direction, `J`/`K` move a key, `x` takes it out, each one undo step; `Enter` runs the
/// order and the popup closes once it is sent.
#[test]
fn the_order_popup_keys() {
    let mut h = users_where("name = 'n1'");
    h.keys(" ro");
    assert_eq!(popup(&h), Some(PopupKindId::Order));
    let order = |h: &Harness| h.app.filter_dialog_draft().map(|d| d.1).unwrap_or_default();
    let screen = h.screen(120, 34);
    assert!(screen.contains(" ORDER BY ") && screen.contains("ORDER BY none"), "{screen}");
    assert!(screen.contains("Space sort"), "{screen}");
    // On `id`: into the order; then `name`, turned descending.
    h.keys(" ");
    assert_eq!(order(&h), "id ASC");
    h.keys("j ");
    assert_eq!(order(&h), "id ASC, name ASC");
    h.keys(" ");
    assert_eq!(order(&h), "id ASC, name DESC");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Space direction") && screen.contains("J/K move") && screen.contains("x drop"), "{screen}");
    // `K`: `name` first, the keys on it still.
    h.keys("K");
    assert_eq!(order(&h), "name DESC, id ASC");
    h.keys(" ");
    assert_eq!(order(&h), "name ASC, id ASC", "the keys followed it");
    h.keys("J");
    assert_eq!(order(&h), "id ASC, name ASC");
    // `x` on `name` (now second): out; `u` brings it back.
    h.keys("x");
    assert_eq!(order(&h), "id ASC");
    h.keys("u");
    assert_eq!(order(&h), "id ASC, name ASC");
    assert!(runs(&mut h).is_empty(), "nothing runs before Enter");
    h.key(KeyCode::Enter);
    let sent = runs(&mut h);
    assert_eq!(sent, vec![vec![format!(r#"{ALL} WHERE (name = 'n1') ORDER BY "id" ASC, "name" ASC"#)]]);
    assert!(!open(&h), "closed once sent");
    // `x` and `J` on a column not in the order do nothing; `Esc` drops the draft.
    let mut h = users_where("name = 'n1'");
    h.keys(" roxJK");
    assert_eq!(order(&h), "");
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    assert!(runs(&mut h).is_empty());
}

/// `ORDER BY` of the bar opens the same popup under it; its last line is the clause sent, the
/// key the app adds to make the order total dimmed; `A` turns it into the whole dialog on the
/// key (another column: its sort line), the draft kept.
#[test]
fn the_order_popup_shows_the_clause_and_turns_into_advanced() {
    let th = datarig_tui::theme::cur();
    let mut h = users_keyed("name = 'n1'");
    h.command("orderby name DESC");
    runs(&mut h);
    answer(&mut h);
    h.app.focus = Focus::Results;
    let seg = hit_rect(&mut h, BarHit::Order, 120, 34);
    open_at(&mut h, BarHit::Order, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::Order));
    let t = h.draw(120, 34);
    let buf = t.backend().buffer();
    assert_eq!(buf[(seg.x, seg.y + 1)].symbol(), "\u{256d}", "under `ORDER BY`, from its left edge");
    let (x, y) = find(&mut h, 120, 34, r#"ORDER BY "name" DESC, "id" ASC"#).expect("the clause");
    let t = h.draw(120, 34);
    let buf = t.backend().buffer();
    let tie = x + r#"ORDER BY "name" DESC, "#.len() as u16;
    assert_eq!(buf[(tie, y)].fg, th.fg_dim, "the tie-breaker dimmed");
    assert_ne!(buf[(x, y)].fg, th.fg_dim, "the keyword coloured");
    let screen = h.screen(120, 34);
    assert!(screen.contains(" 1 \u{2193}") && screen.contains("name"), "{screen}");
    // On the key: the whole dialog on it.
    h.keys("A");
    assert_eq!(popup(&h), None);
    assert!(open(&h));
    let screen = h.screen(120, 34);
    assert!(screen.contains("move in the order"), "the keys on the key: {screen}");
    // Another column: its sort line; a change is kept.
    let mut h = users_where("name = 'n1'");
    h.keys(" roj");
    h.keys("A");
    let screen = h.screen(120, 34);
    assert!(screen.contains("none / asc / desc"), "the keys on the sort line: {screen}");
    let mut h = users_where("name = 'n1'");
    h.keys(" ro ");
    h.keys("A");
    assert_eq!(h.app.filter_dialog_draft().map(|d| d.1).as_deref(), Some("id ASC"));
    assert!(runs(&mut h).is_empty());
}

/// The columns popup: `Space` hides a column from the grid or shows it again, `a` shows them
/// all; a hidden key column is said to be still read. Only the columns changed: `Enter` applies
/// without a run.
#[test]
fn the_columns_popup_hides_restores_and_shows_all() {
    let th = datarig_tui::theme::cur();
    let mut h = users_keyed("name = 'n1'");
    let seg = hit_rect(&mut h, BarHit::Cols, 120, 34);
    open_at(&mut h, BarHit::Cols, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::Columns));
    let top = row_text(h.draw(120, 34).backend().buffer(), seg.y + 1);
    assert!(top.contains("\u{256d}\u{2500} columns 2/2 "), "under the count: {top}");
    let screen = h.screen(120, 34);
    assert!(
        screen.contains(" columns 2/2 ") && screen.contains("Space hide/show") && screen.contains("a all"),
        "{screen}"
    );
    // `id`, the key: hidden, crossed out, said still read.
    h.keys(" ");
    let screen = h.screen(120, 34);
    assert!(
        screen.contains(" columns 1/2 ") && screen.contains("hidden from the grid, still read (row key)"),
        "{screen}"
    );
    assert!(screen.contains("\u{25cb}") && screen.contains("\u{25c9}"), "{screen}");
    let (x, y) = find(&mut h, 120, 34, "\u{25cb}").expect("the hidden mark");
    let buf = h.draw(120, 34).backend().buffer().clone();
    let name = (x..x + 12).find(|&c| buf[(c, y)].symbol() == "i").expect("its name");
    assert!(buf[(name, y)].modifier.contains(Modifier::CROSSED_OUT), "crossed out");
    assert_eq!(buf[(name, y)].fg, th.fg_muted);
    // `name` too, then `a` shows both again.
    h.keys("j ");
    assert!(h.screen(120, 34).contains(" columns 0/2 "));
    h.keys("a");
    assert!(h.screen(120, 34).contains(" columns 2/2 "));
    // `name` hidden and applied: no run, the grid without it.
    h.keys(" ");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "the columns alone: no run");
    assert!(!open(&h));
    assert!(bar_text(&mut h).contains("1/2 cols"), "{}", bar_text(&mut h));
    // Opened again: `a` and `Enter` show it.
    h.keys(" rc");
    assert!(h.screen(120, 34).contains(" columns 1/2 "));
    h.keys("a");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(bar_text(&mut h).contains("2 cols"), "{}", bar_text(&mut h));
    // A click on a column shows or hides it; `A` turns the popup into the whole dialog on the
    // column list.
    let mut h = users_where("name = 'n1'");
    open_at(&mut h, BarHit::Cols, 120, 34);
    let (x, y) = find(&mut h, 120, 34, "name  text").expect("listed");
    click_at(&mut h, x, y);
    assert!(h.screen(120, 34).contains(" columns 1/2 "));
    h.keys("A");
    assert_eq!(popup(&h), None);
    assert!(h.screen(120, 34).contains("Columns 2 \u{b7} 1 hidden"), "{}", h.screen(120, 34));
}

/// The bar's row (the query bar).
fn bar_text(h: &mut Harness) -> String {
    let t = h.draw(120, 34);
    row_text(t.backend().buffer(), h.app.layout.results.y + 1)
}

/// Answer the active tab's run with a full first page of `shop.users` and more past it,
/// nothing held on the server.
fn page_more(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string()), Some(format!("n{}", i % 5))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
}

/// `shop.users` with a full first page and more past it (`filter`: filtered, answered the
/// same), the grid focused, nothing sent since.
fn users_paged(filter: Option<&str>) -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    page_more(&mut h);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    if let Some(f) = filter {
        h.command(&format!("where {f}"));
        page_more(&mut h);
    }
    h.app.focus = Focus::Results;
    h.sent();
    h
}

/// The counts and estimates sent since the last look, and whether a count of values was.
fn counts(h: &mut Harness) -> (Vec<String>, Vec<String>, bool) {
    let sent = h.sent();
    let counts =
        sent.iter().filter_map(|c| if let DbCommand::Count { sql, .. } = c { Some(sql.clone()) } else { None });
    let estimates =
        sent.iter().filter_map(|c| if let DbCommand::Estimate { sql, .. } = c { Some(sql.clone()) } else { None });
    let grouped = sent.iter().any(|c| matches!(c, DbCommand::GroupCount { .. }));
    (counts.collect(), estimates.collect(), grouped)
}

/// The rows popup says the page shown of the rows shown, the paging's state in full and the
/// last count; its items do what the bar's arrows and `#` do (the exact count with its time
/// limit, never a count of values) and it stays open, showing what changed.
#[test]
fn the_rows_popup_pages_and_counts_as_the_bar_does() {
    let mut h = users_paged(None);
    let seg = hit_rect(&mut h, BarHit::Rows, 120, 34);
    open_at(&mut h, BarHit::Rows, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::Rows));
    let buf = h.draw(120, 34).backend().buffer().clone();
    let top = row_text(&buf, seg.y + 1);
    assert!(top.contains(" rows 1\u{2013}500 \u{b7} page 1 of ? "), "{top}");
    let screen = h.screen(120, 34);
    assert!(screen.contains("count: not known, 500 read"), "{screen}");
    assert!(screen.contains("nothing is held open on the server"), "the paging state in full: {screen}");
    assert!(screen.contains("Estimate (planner)  only with a filter"), "{screen}");
    assert!(screen.contains("Enter run") && screen.contains("Esc close"), "{screen}");
    // The exact count: the bar's `#`, its statement capped and timed as `#` sends it.
    h.keys("jj");
    h.key(KeyCode::Enter);
    let (sent, estimates, grouped) = counts(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("count(*)") && sent[0].contains(ALL), "{sent:?}");
    assert!(estimates.is_empty() && !grouped);
    assert_eq!(popup(&h), Some(PopupKindId::Rows), "it stays open");
    let screen = h.screen(120, 34);
    assert!(screen.contains("counting\u{2026}") && screen.contains("a query runs in the tab"), "{screen}");
    // Counted (more than the rows read, apart from their pages): said, the pages known.
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Counted { id, result: Ok(1234), snapshot: false });
    let screen = h.screen(120, 34);
    assert!(
        screen.contains("count: 1,234, exact, counted apart from the") && screen.contains("pages read"),
        "{screen}"
    );
    assert!(screen.contains(" rows 1\u{2013}500 \u{b7} page 1 of 3 "), "{screen}");
    // The next page: the bar's arrow, which runs the statement again past the rows read.
    h.keys("kk");
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Resume { .. })), "{sent:?}");
    assert_eq!(popup(&h), Some(PopupKindId::Rows));
    // The previous page on the first: said, nothing sent. A click on an item does it.
    let (x, y) = find(&mut h, 120, 34, "previous page").expect("listed");
    h.advance(Duration::from_millis(500));
    click_at(&mut h, x, y);
    assert!(h.sent().is_empty());
}

/// The rows popup's two count items: dimmed with why where `#` would refuse them now (every
/// row read, a run on its way, no filter for the planner), the estimate sent on filtered rows,
/// its answer said as the planner's and, with a cap, within it.
#[test]
fn the_rows_popup_count_items_say_why_they_are_dimmed() {
    let th = datarig_tui::theme::cur();
    // Every row read: both dimmed, said; `Enter` says so too, nothing sent.
    let mut h = users_where("name = 'n1'");
    h.keys(" rr");
    let screen = h.screen(120, 34);
    assert_eq!(screen.matches("every row is read").count(), 2, "{screen}");
    assert!(screen.contains(" rows 1\u{2013}3 \u{b7} page 1 of 1 ") && screen.contains("count: 3, exact"), "{screen}");
    let (x, y) = find(&mut h, 120, 34, "Exact count").expect("listed");
    assert_eq!(h.draw(120, 34).backend().buffer()[(x, y)].fg, th.fg_dim);
    h.keys("jj");
    h.key(KeyCode::Enter);
    assert!(h.status(120, 34).contains("Every row is fetched already"), "{}", h.status(120, 34));
    assert_eq!(counts(&mut h), (Vec::new(), Vec::new(), false));
    // Filtered, a page and more: the planner's estimate, then its answer within the cap.
    let mut h = users_paged(Some("id > 1"));
    h.keys(" rl");
    h.type_text("1000");
    h.key(KeyCode::Enter);
    runs(&mut h);
    page_more(&mut h);
    h.app.focus = Focus::Results;
    h.sent();
    h.keys(" rr");
    let screen = h.screen(120, 34);
    assert!(screen.contains("nothing is read") && screen.contains("stops after 30s"), "{screen}");
    h.keys("j");
    h.key(KeyCode::Enter);
    let (sent, estimates, grouped) = counts(&mut h);
    assert!(sent.is_empty() && !grouped);
    assert_eq!(estimates.len(), 1, "{estimates:?}");
    assert!(estimates[0].contains("id > 1") && estimates[0].contains("LIMIT 1000"), "{estimates:?}");
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Estimated { id, result: Ok(50_000) });
    let screen = h.screen(120, 34);
    assert!(screen.contains("count: ~1,000, the planner's estimate"), "capped: {screen}");
    assert!(screen.contains(" rows 1\u{2013}500 \u{b7} page 1 of ~2 "), "{screen}");
}

/// The rows popup describes the rows shown within the cap: a cap below the page, equal to it,
/// above it.
#[test]
fn the_rows_popup_follows_the_cap() {
    // Below: two rows, the cap reached.
    let mut h = users_where("name = 'n1'");
    h.keys(" rl");
    h.type_text("2");
    h.key(KeyCode::Enter);
    runs(&mut h);
    answer_n(&mut h, 2);
    h.app.focus = Focus::Results;
    h.keys(" rr");
    let screen = h.screen(120, 34);
    assert!(screen.contains(" rows 1\u{2013}2 \u{b7} page 1 of 1 ") && screen.contains("limit 2"), "{screen}");
    // Equal: a page of the cap's rows, none past it.
    let mut h = users_where("name = 'n1'");
    let size = h.app.page_size;
    h.keys(" rl");
    h.type_text(&size.to_string());
    h.key(KeyCode::Enter);
    runs(&mut h);
    answer_n(&mut h, size);
    h.app.focus = Focus::Results;
    h.keys(" rr");
    let screen = h.screen(120, 34);
    assert!(screen.contains(&format!(" rows 1\u{2013}{size} \u{b7} page 1 of 1 ")), "{screen}");
    assert!(screen.contains(&format!("limit {size}")) && screen.contains("next page"), "{screen}");
    // Above: a page and more, the pages counted within the cap.
    let mut h = users_paged(None);
    h.keys(" rl");
    h.type_text("1200");
    h.key(KeyCode::Enter);
    runs(&mut h);
    page_more(&mut h);
    h.app.focus = Focus::Results;
    h.sent();
    h.keys(" rrjj");
    h.key(KeyCode::Enter);
    let (sent, _, _) = counts(&mut h);
    assert!(sent.len() == 1 && sent[0].contains("LIMIT 1200"), "{sent:?}");
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Counted { id, result: Ok(1200), snapshot: false });
    let screen = h.screen(120, 34);
    assert!(screen.contains(" rows 1\u{2013}500 \u{b7} page 1 of 3 ") && screen.contains("count: 1,200"), "{screen}");
}

/// The order, columns and rows popups' screens: on the grid with the explorer open at 80x24,
/// and at 120x34; the order and columns popups in the terminal theme.
#[test]
fn the_order_columns_and_rows_screens() {
    for (w, hh) in [(80u16, 24u16), (120, 34)] {
        let mut h = users_keyed("name = 'n1'");
        h.command("orderby name DESC");
        runs(&mut h);
        answer(&mut h);
        h.app.focus = Focus::Results;
        assert!(h.app.explorer_shown());
        open_at(&mut h, BarHit::Order, w, hh);
        assert_eq!(popup(&h), Some(PopupKindId::Order));
        assert_screen!(format!("query_popup_order_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        let theme = h.app.theme.clone();
        h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
        assert_screen!(format!("query_popup_order_terminal_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        h.app.theme = theme.clone();
        h.key(KeyCode::Esc);
        h.keys(" rc");
        assert_eq!(popup(&h), Some(PopupKindId::Columns));
        h.keys(" ");
        assert_screen!(format!("query_popup_columns_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
        assert_screen!(format!("query_popup_columns_terminal_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        h.app.theme = theme;
        let mut h = users_paged(None);
        assert!(h.app.explorer_shown());
        h.keys(" rr");
        assert_eq!(popup(&h), Some(PopupKindId::Rows));
        assert_screen!(format!("query_popup_rows_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
}

/// A popup opened again from the command line while an apply of the session waits for the
/// connection: the same session in another view, its draft, its undo and the apply kept; `Esc`
/// still cancels the apply and the session that comes runs nothing. The whole dialog does not
/// turn into a popup: it stays as it was.
#[test]
fn a_popup_opened_again_keeps_the_session_and_its_waiting_apply() {
    let mut cfg = test_db_config();
    cfg.prefs.idle_disconnect = Some(Duration::from_secs(300));
    let mut h = users_where_in(&cfg, "name = 'n1'");
    h.advance(Duration::from_secs(300));
    h.advance(Duration::from_secs(300));
    assert!(h.session_closed(1), "the tab's session closed");
    h.app.transient = None;
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Enter);
    let tab = h.app.tab().id;
    assert!(runs(&mut h).is_empty());
    assert!(h.app.tab_busy(tab), "it waits for the connection");
    // `Space r c` from the command line, over the popup.
    h.ctrl('k');
    h.type_text("columns shown and hidden");
    h.key(KeyCode::Enter);
    assert_eq!(popup(&h), Some(PopupKindId::Columns), "the same session as the columns popup");
    assert!(draft_where(&h).contains("n1x"), "the draft kept: {}", draft_where(&h));
    assert!(h.app.tab_busy(tab), "the apply still waits");
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    assert!(!h.app.tab_busy(tab), "cancelled with the session");
    h.db(DbEvent::Connected);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "the run cancelled never goes");
    // The whole dialog, edited: the columns popup asked for keeps it whole, the draft kept.
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    h.keys("A");
    type_value(&mut h, "y");
    h.ctrl('k');
    h.type_text("columns shown and hidden");
    h.key(KeyCode::Enter);
    assert!(open(&h));
    assert_eq!(popup(&h), None, "the whole dialog stays whole");
    assert!(draft_where(&h).contains("n1y"), "{}", draft_where(&h));
}

/// Open `shop.orders` from the explorer too: a second table tab, answered, filtered with
/// `filter` and answered; nothing sent since.
fn open_orders_where(h: &mut Harness, filter: &str) {
    h.explore("local-pg");
    h.goto("shop");
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".into(), "users".into()], Vec::new()).into()),
    });
    h.goto("orders");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
    answer(h);
    h.key(KeyCode::Tab);
    h.command(&format!("where {filter}"));
    runs(h);
    answer(h);
    h.app.focus = Focus::Results;
}

/// The WHERE list of one tab is closed when another tab becomes active; and whatever it does
/// is its own tab's: `x` never runs the active tab's query.
#[test]
fn the_where_list_closes_on_a_tab_switch_and_acts_on_its_own_tab() {
    let mut h = users_where(MANY);
    open_orders_where(&mut h, "id > 7 AND id < 70 AND id <> 9 AND id <> 11 AND id <> 13 AND id <> 15");
    let orders = h.app.tab().id;
    // Back on `users`: its WHERE list, then the next tab from the command line.
    h.ctrl('k');
    h.type_text("tab.prev");
    h.key(KeyCode::Enter);
    assert_ne!(h.app.tab().id, orders);
    open_at(&mut h, BarHit::More, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    h.ctrl('k');
    h.type_text("tab.next");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().id, orders);
    assert!(!open(&h), "the list closed with its tab");
    // Still open on `users` while `orders` is active (the tab switched by no action): `x`
    // takes the condition out of `users`' query, never out of `orders`'.
    h.ctrl('k');
    h.type_text("tab.prev");
    h.key(KeyCode::Enter);
    open_at(&mut h, BarHit::More, 120, 34);
    h.key(KeyCode::Down);
    h.app.tabs.cycle(1);
    assert_eq!(h.app.tab().id, orders);
    h.keys("x");
    let sent = runs(&mut h);
    assert!(sent.iter().all(|s| s[0].contains(r#""shop"."users""#)), "{sent:?}");
    let applied = h.app.tabs.get(orders).and_then(|t| t.doc.table_query.as_ref()).map(|q| q.applied().clone());
    assert!(format!("{applied:?}").contains("id <> 15"), "orders keeps its conditions: {applied:?}");
}

/// The values list a popup has no room to draw (many conditions on its column at 80x24) is
/// not reached by the keys: nothing unseen is ticked.
#[test]
fn values_not_drawn_are_not_reached() {
    let terms: Vec<String> = (1..=15).map(|i| format!("id > {i}")).collect();
    let all = terms.join(" AND ");
    for advanced in [false, true] {
        let mut h = users_where(&all);
        open_chip(&mut h, 0);
        if advanced {
            h.keys("A");
        }
        // To the last condition, then past `+ add condition`.
        for _ in 0..16 {
            h.draw(80, 24);
            h.key(KeyCode::Down);
        }
        h.draw(80, 24);
        h.key(KeyCode::Char(' '));
        assert_eq!(draft_where(&h), all, "advanced: {advanced}: nothing unseen ticked");
    }
}

/// An incomplete `between` in a narrow popup (the explorer widened, 80x24) keeps room for each
/// of its two values: the one typed stays in view.
#[test]
fn a_narrow_between_keeps_room_for_both_values() {
    let mut h = users_where("id BETWEEN 1234 AND 5678");
    open_chip(&mut h, 0);
    h.app.explorer.width = Some(36);
    h.draw(80, 24);
    // The second value emptied: incomplete.
    h.key(KeyCode::Right);
    h.key(KeyCode::End);
    h.key(KeyCode::Right);
    h.key(KeyCode::End);
    for _ in 0..4 {
        h.key(KeyCode::Backspace);
    }
    assert!(draft_where(&h).is_empty() || !draft_where(&h).contains("5678"), "{}", draft_where(&h));
    let screen = h.screen(80, 24);
    let line = screen.lines().find(|l| l.contains("between")).unwrap_or_else(|| panic!("{screen}"));
    assert!(line.contains("1234"), "the first value in view: {line}\n{screen}");
}

/// `x` in the WHERE list takes the condition out of the draft the list shows (here the query
/// before the last change, undone to) and applies that draft: the query applied last is not
/// what it starts from.
#[test]
fn the_where_list_removes_from_the_draft_it_shows() {
    let long = "name <> 'a rather long text to leave out' AND (id < 9 OR  id = 20)";
    let mut h = users_where(&format!("id > 0 AND name = 'a' AND {long}"));
    h.command(&format!("where id > 0 AND name = 'b' AND {long}"));
    runs(&mut h);
    answer(&mut h);
    h.app.focus = Focus::Results;
    open_at(&mut h, BarHit::More, 120, 34);
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    h.keys("u");
    assert_eq!(draft_where(&h), format!("id > 0 AND name = 'a' AND {long}"));
    h.keys("x");
    let sent = runs(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let s = &sent[0][0];
    assert!(s.contains("name = 'a'") && !s.contains("name = 'b'") && !s.contains("id > 0"), "{s}");
}

/// A value of the list focused at 120x50, then the terminal made 80x24, where the column's
/// conditions leave no room for the list (no line for a value, with 12; none at all, with 14):
/// the keys leave it and `Space` ticks nothing unseen, in the popup and in the whole dialog.
#[test]
fn values_hidden_by_a_resize_are_left() {
    for (n, advanced) in [(12, false), (14, false), (12, true), (14, true)] {
        let terms: Vec<String> = (1..=n).map(|i| format!("id > {i}")).collect();
        let all = terms.join(" AND ");
        let mut h = users_where(&all);
        open_chip(&mut h, 0);
        if advanced {
            h.keys("A");
        }
        // Past the conditions and `+ add condition`, onto the list's first value.
        for _ in 0..=n {
            h.draw(120, 50);
            h.key(KeyCode::Down);
        }
        let screen = h.screen(120, 50);
        assert!(screen.contains("\u{25b8} [ ] 0"), "on the list's first value: {screen}");
        h.draw(80, 24);
        h.key(KeyCode::Char(' '));
        assert_eq!(draft_where(&h), all, "{n}, advanced: {advanced}: nothing unseen ticked");
    }
}

/// A switch to the tab already active (`tab.goto.N` from the command line) leaves it as it is: its popup, the draft
/// and its undo stay.
#[test]
fn a_tab_switch_that_stays_keeps_the_popup() {
    let mut h = users_where("name = 'n1'");
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    let n = h.app.tabs.active_index() + 1;
    h.ctrl('k');
    h.type_text(&format!("tab.goto.{n}"));
    h.key(KeyCode::Enter);
    assert_eq!(popup(&h), Some(PopupKindId::Condition), "the popup stays");
    assert!(draft_where(&h).contains("n1x"), "{}", draft_where(&h));
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(draft_where(&h), "name = 'n1'", "its undo kept");
}
