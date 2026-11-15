//! The query bar's condition popup: a click on a column's condition opens the filter dialog as
//! a small popup under it, on that column alone. It edits the dialog's one draft (undo, the
//! apply waiting, the checks and questions of a run all as the whole dialog's), `Enter` applies
//! through the tab's run and it closes once the statement is sent; `Esc` or a press outside
//! closes it without applying; `A` or `⋯ Advanced` turns it into the whole dialog.

use crate::common;

use common::*;
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
