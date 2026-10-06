//! The tab list (`Space t t`, `:tabs`, `:ls`, `:buffers`, the action menu): the open tabs by
//! when they were last active, then the tabs closed in this run; typing filters it, `Enter`
//! goes to a tab or brings a closed one back, `Ctrl+D` closes the selected tab with the usual
//! confirmations and the list follows; the mouse works as on the other lists.

mod common;

use common::*;
use datarig_core::driver::{DbEvent, Outcome};
use datarig_core::i18n::Lang;
use datarig_tui::app::Focus;
use datarig_tui::app::overlay::OverlayKind;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::time::Duration;

const W: u16 = 120;
const H: u16 = 34;

/// Connected, with four consoles (`console 1` to `console 4`), the last one active.
fn four_tabs() -> Harness {
    let mut h = Harness::connected(Lang::En);
    for _ in 0..3 {
        h.ctrl('t');
    }
    assert_eq!(h.app.tabs.len(), 4);
    h
}

/// The active tab's name as the tab bar shows it.
fn active(h: &Harness) -> String {
    h.app.tabs.iter().nth(h.app.tabs.active_index()).map(|t| format!("console {}", t.doc.console_no)).unwrap()
}

fn names(h: &Harness) -> Vec<String> {
    h.app.tab_list_names()
}

fn selected(h: &Harness) -> String {
    let l = h.app.overlays.tab_list().expect("the tab list is open");
    names(h)[l.selected].clone()
}

fn open_list(h: &mut Harness) {
    h.keys(" tt");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "Space t t opens it");
}

/// Type `sql` into the active tab (Insert mode) and go back to Normal mode.
fn type_sql(h: &mut Harness, sql: &str) {
    h.keys("i");
    h.type_text(sql);
    h.key(KeyCode::Esc);
}

/// The active tab (index `tab`) ran a `BEGIN`: the user's transaction is open there.
fn open_transaction(h: &mut Harness, tab: usize) {
    h.ctrl('e');
    let id = h.app.tabs.iter().nth(tab).unwrap().exec.query_id;
    let done = DbEvent::Done { id, outcome: Outcome::Command("BEGIN".into()), elapsed: Duration::from_millis(1) };
    h.tab_db(tab, done);
    h.tab_db(tab, DbEvent::Block(true));
    h.tab_db(tab, DbEvent::TxOpen(true));
}

/// From the editor, open `shop.users` in the explorer (the schema's objects answered).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".to_string(), "users".to_string()], Vec::new()).into()),
    });
    h.keys("jjj"); // Tables, orders, users
    h.key(KeyCode::Enter);
}

/// The screen row of entry `name` and its rect, as last drawn.
fn row_of(h: &mut Harness, name: &str) -> (u16, u16) {
    h.draw(W, H);
    let i = names(h).iter().position(|n| n == name).unwrap_or_else(|| panic!("{name} listed"));
    let l = h.app.overlays.tab_list().unwrap();
    let r = l.rows.iter().find(|(_, e)| *e == i).map(|r| r.0).unwrap_or_else(|| panic!("{name} drawn"));
    (r.x + 4, r.y)
}

fn press(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
}

fn release(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

#[test]
fn every_entry_point_opens_the_list() {
    // Space t t from the editor and from the explorer.
    let mut h = four_tabs();
    open_list(&mut h);
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None, "Esc closes it");
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    open_list(&mut h);
    h.key(KeyCode::Esc);
    // The command line: `:tabs` and its vim names.
    for name in ["tabs", "ls", "buffers"] {
        h.command(name);
        assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), ":{name}");
        h.key(KeyCode::Esc);
    }
    // The action menu of the editor and of the tab bar list it.
    h.app.focus = Focus::Editor;
    h.draw(W, H);
    h.keys("  ");
    assert!(h.menu_labels().contains(&"List tabs".to_string()), "{:?}", h.menu_labels());
    h.menu_pick("List tabs");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "from the editor's action menu");
    h.key(KeyCode::Esc);
    h.draw(W, H);
    h.keys(" tm");
    h.menu_pick("List tabs");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "from the tab menu");
    // The command line's search finds the action by its name too.
    h.key(KeyCode::Esc);
    h.command("list tabs");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "by the action's name");
}

#[test]
fn the_list_is_most_recently_active_first_and_selects_the_previous_tab() {
    let mut h = four_tabs();
    // Visit 1, then 3 (keys), then 2 (a click on its tab in the tab bar).
    h.keys(" 1");
    h.keys(" 3");
    let t = h.draw(W, H);
    let bar = row_text(t.backend().buffer(), 0);
    let x = datarig_tui::text::width(&bar[..bar.find("2 console 2").unwrap()]) as u16;
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 3, 0);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x + 3, 0);
    assert_eq!(active(&h), "console 2");
    open_list(&mut h);
    assert_eq!(names(&h), ["console 2", "console 3", "console 1", "console 4"]);
    assert_eq!(selected(&h), "console 3", "the tab before the current one, as Telescope's buffers");
    // Enter goes back to it: switching back and forth is Space t t Enter.
    h.key(KeyCode::Enter);
    assert_eq!((h.overlay_kind(), active(&h)), (None, "console 3".to_string()));
    assert_eq!(h.app.focus, Focus::Editor);
    open_list(&mut h);
    assert_eq!(names(&h), ["console 3", "console 2", "console 1", "console 4"]);
    h.key(KeyCode::Esc);
    // Closing the active tab makes its neighbour active: that one comes first.
    h.ctrl('w');
    assert_eq!(active(&h), "console 4");
    open_list(&mut h);
    assert_eq!(names(&h), ["console 4", "console 2", "console 1", "console 3"], "the closed one at the end");
    h.key(KeyCode::Esc);
    // A reopened tab is active, so it comes first.
    h.keys(" tu");
    open_list(&mut h);
    assert_eq!(names(&h)[..4], ["console 3", "console 4", "console 2", "console 1"]);
    // Alone, the current tab is selected.
    let mut h = Harness::connected(Lang::En);
    open_list(&mut h);
    assert_eq!((names(&h), selected(&h)), (vec!["console 1".to_string()], "console 1".to_string()));
}

#[test]
fn typing_filters_by_name_kind_connection_and_number() {
    let mut h = four_tabs();
    open_users(&mut h);
    h.keys(" 2");
    assert_eq!(active(&h), "console 2");
    open_list(&mut h);
    assert_eq!(names(&h).len(), 5);
    h.type_text("users");
    assert_eq!(names(&h), ["shop.users"], "the name");
    assert_eq!(selected(&h), "shop.users", "the best match is selected");
    h.ctrl('u');
    h.type_text("table");
    assert_eq!(names(&h), ["shop.users"], "the kind");
    h.ctrl('u');
    h.type_text("console 3");
    assert_eq!(names(&h), ["console 3"], "the name, words included");
    h.ctrl('u');
    h.type_text("local");
    assert_eq!(names(&h).len(), 5, "the profile");
    h.ctrl('u');
    h.type_text("datarig");
    assert_eq!(names(&h).len(), 5, "the database");
    h.ctrl('u');
    h.type_text("zzz");
    assert!(names(&h).is_empty());
    assert!(h.screen(W, H).contains("No matching tabs"));
    // Backspace on the filter keeps the list open, also once it is empty.
    for _ in 0..4 {
        h.key(KeyCode::Backspace);
    }
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList));
    assert_eq!(names(&h).len(), 5);
    // The arrows, Ctrl+N/Ctrl+P and Tab move, around the ends.
    let first = selected(&h);
    h.key(KeyCode::Up);
    h.key(KeyCode::Down);
    assert_eq!(selected(&h), first);
    h.ctrl('n');
    h.key(KeyCode::Tab);
    h.key_mod(KeyCode::BackTab, KeyModifiers::SHIFT);
    h.ctrl('p');
    assert_eq!(selected(&h), first);
    // The filter finds closed tabs too.
    h.key(KeyCode::Esc);
    h.ctrl('w');
    open_list(&mut h);
    h.type_text("console 2");
    assert_eq!(names(&h), ["console 2"], "a closed tab is found as an open one is");
    assert!(h.screen(W, H).contains("Recently closed"));
}

#[test]
fn ctrl_d_closes_the_selected_tab_and_the_list_follows() {
    let mut h = four_tabs();
    open_list(&mut h);
    assert_eq!(selected(&h), "console 3");
    h.ctrl('d');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "the list stays open");
    assert_eq!(h.app.tabs.len(), 3);
    assert_eq!(active(&h), "console 4", "another tab than the active one closes; the active one stays");
    assert_eq!(names(&h), ["console 4", "console 2", "console 1", "console 3"]);
    assert_eq!(selected(&h), "console 2", "the next tab in its place");
    assert!(h.screen(W, H).contains("Recently closed"));
    // The last open entry: the selection stays among the open tabs.
    h.key(KeyCode::Down);
    assert_eq!(selected(&h), "console 1");
    h.ctrl('d');
    assert_eq!(selected(&h), "console 2");
    // Ctrl+D on a closed tab does nothing.
    h.key(KeyCode::Down);
    assert_eq!(selected(&h), "console 1");
    h.ctrl('d');
    assert_eq!(names(&h), ["console 4", "console 2", "console 1", "console 3"]);
}

#[test]
fn ctrl_d_asks_first_when_something_would_be_lost() {
    let mut h = Harness::connected(Lang::En);
    open_transaction(&mut h, 0);
    h.ctrl('t');
    open_list(&mut h);
    assert_eq!(selected(&h), "console 1");
    assert!(h.screen(W, H).contains("1 ◆  console 1"), "the tab bar's mark:\n{}", h.screen(W, H));
    h.ctrl('d');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "the close confirmation, as Ctrl+W asks");
    let screen = h.screen(W, H);
    assert!(screen.contains("Close this tab?") && screen.contains("A transaction is open in this tab"), "{screen}");
    // No (Enter keeps): the list is back, nothing closed.
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList));
    assert_eq!(h.app.tabs.len(), 2);
    assert!(!h.session_closed(1));
    // Yes: the tab and its session close, the list follows.
    h.ctrl('d');
    h.keys("y");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList));
    assert_eq!(h.app.tabs.len(), 1);
    assert!(h.session_closed(1), "its session closed (the server rolls back)");
    assert_eq!(names(&h), ["console 2", "console 1"]);
    assert_eq!(selected(&h), "console 2");
}

#[test]
fn a_recently_closed_tab_comes_back_by_name() {
    let mut h = four_tabs();
    h.keys(" 2");
    type_sql(&mut h, "select 2");
    h.ctrl('w');
    h.keys(" 3"); // console 4 is the third tab now
    h.keys(" 1");
    type_sql(&mut h, "select 1");
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), 2);
    open_list(&mut h);
    assert_eq!(names(&h), ["console 3", "console 4", "console 1", "console 2"], "the newest closed first");
    let screen = h.screen(W, H);
    let heading = screen.lines().position(|l| l.contains("Recently closed")).expect("heading");
    let older = screen.lines().position(|l| l.contains("console 2")).unwrap();
    assert!(older > heading, "under the heading:\n{screen}");
    // Not the newest (Space t u would bring console 1): the one picked.
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    assert_eq!(selected(&h), "console 2");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert!(h.app.tab().editor.text().contains("select 2"), "its text came back");
    assert_eq!(h.app.tabs.len(), 3);
    open_list(&mut h);
    assert_eq!(names(&h).last().map(String::as_str), Some("console 1"), "the other one stays closed");
    assert_eq!(names(&h).len(), 4);
    h.key(KeyCode::Esc);
    h.keys(" tu");
    assert!(h.app.tab().editor.text().contains("select 1"), "Space t u brings the next one");
}

#[test]
fn the_mouse_picks_on_release_hovers_without_selecting_and_waits_for_the_list() {
    let mut h = four_tabs();
    open_list(&mut h);
    let c1 = row_of(&mut h, "console 1");
    // Right after it came up a press is ignored.
    press(&mut h, c1);
    release(&mut h, c1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "too early");
    h.advance(Duration::from_millis(500));
    // The pointer highlights a row; the selection stays where the keys put it.
    h.mouse(MouseEventKind::Moved, c1.0, c1.1);
    let l = h.app.overlays.tab_list().unwrap();
    assert_eq!((selected(&h), l.hover), ("console 3".to_string(), Some(3)));
    assert!(!h.app.take_idle_event(), "a frame for the highlight");
    h.mouse(MouseEventKind::Moved, c1.0 + 1, c1.1);
    assert!(h.app.take_idle_event(), "the same row: no frame");
    h.key(KeyCode::Enter);
    assert_eq!(active(&h), "console 3", "Enter after hovering picks the selection");
    // Pressed on one row, released on another: nothing.
    open_list(&mut h);
    h.draw(W, H);
    h.advance(Duration::from_millis(500));
    let (c1, c2) = (row_of(&mut h, "console 1"), row_of(&mut h, "console 2"));
    press(&mut h, c1);
    release(&mut h, c2);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList));
    // Outside the box: nothing.
    press(&mut h, (0, H - 2));
    release(&mut h, (0, H - 2));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "an outside click is ignored");
    // The wheel moves the selection.
    h.mouse(MouseEventKind::ScrollDown, c1.0, c1.1);
    assert_eq!(selected(&h), "console 2");
    h.mouse(MouseEventKind::ScrollUp, c1.0, c1.1);
    assert_eq!(selected(&h), "console 4");
    // A click on the filter line puts the cursor there; a click on a row goes to its tab.
    h.type_text("console");
    let input = h.app.overlays.tab_list().unwrap().rows[0].0;
    press(&mut h, (input.x + 6, input.y - 2));
    release(&mut h, (input.x + 6, input.y - 2));
    assert_eq!(h.app.overlays.tab_list().unwrap().filter.cursor(), 3);
    h.key(KeyCode::Char('x'));
    assert!(names(&h).is_empty(), "typed where the click put the cursor");
    h.key(KeyCode::Backspace);
    let c2 = row_of(&mut h, "console 2");
    press(&mut h, c2);
    release(&mut h, c2);
    assert_eq!((h.overlay_kind(), active(&h)), (None, "console 2".to_string()));
    // A press arms the entry, not the row: when the entries change before the release (as if
    // the tab under the pointer went), the row's new entry is not picked.
    open_list(&mut h);
    h.draw(W, H);
    h.advance(Duration::from_millis(500));
    let c3 = row_of(&mut h, "console 3");
    press(&mut h, c3);
    let id = h.app.tabs.iter().find(|t| t.doc.console_no == 3).unwrap().id;
    let gone = datarig_tui::app::tab_list::TabEntry::Open(id);
    h.app.overlays.tab_list_mut().unwrap().entries.retain(|e| *e != gone);
    release(&mut h, c3);
    assert_eq!((h.overlay_kind(), active(&h)), (Some(OverlayKind::TabList), "console 2".to_string()));
}

#[test]
fn a_covered_list_waits_again_when_it_comes_back_on_top() {
    let mut h = Harness::connected(Lang::En);
    open_transaction(&mut h, 0);
    h.ctrl('t');
    h.ctrl('t');
    open_list(&mut h);
    h.draw(W, H);
    h.advance(Duration::from_millis(500));
    h.key(KeyCode::Down); // console 1, with the open transaction
    h.ctrl('d');
    h.draw(W, H);
    h.key(KeyCode::Char('n'));
    let c = row_of(&mut h, "console 2");
    press(&mut h, c);
    release(&mut h, c);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "uncovered just now: the press is ignored");
    h.advance(Duration::from_millis(500));
    press(&mut h, c);
    release(&mut h, c);
    assert_eq!((h.overlay_kind(), active(&h)), (None, "console 2".to_string()));
}

#[test]
fn a_small_screen_keeps_the_selection_in_view() {
    let mut h = Harness::connected(Lang::En);
    for _ in 0..24 {
        h.ctrl('t');
    }
    h.keys(" tt");
    let screen = h.screen(80, 24);
    assert!(screen.contains("console 24"), "{screen}");
    assert!(screen.contains("Enter go"), "the footer fits:\n{screen}");
    // The bottom of the list is scrolled to.
    for _ in 0..23 {
        h.key(KeyCode::Down);
    }
    assert_eq!(selected(&h), "console 1");
    h.draw(80, 24);
    let rows = &h.app.overlays.tab_list().unwrap().rows;
    let (_, last) = rows.last().copied().unwrap();
    assert_eq!(last, 24, "the selected entry is drawn");
    // Too small to draw: it does not open, and an open one takes no mouse.
    h.key(KeyCode::Esc);
    h.draw(60, 20);
    h.keys(" tt");
    assert_eq!(h.overlay_kind(), None, "nothing opens on a screen too small");
    h.draw(80, 24);
    h.keys(" tt");
    h.draw(60, 20);
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), 10, 5);
    h.mouse(MouseEventKind::Up(MouseButton::Left), 10, 5);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList), "nothing is hit when nothing is drawn");
}

/// Tabs 1 (an open transaction) and 2, a table tab, console 3 closed; console 2 active.
fn snapshot_scene(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    open_transaction(&mut h, 0);
    h.ctrl('t');
    open_users(&mut h);
    h.ctrl('t');
    h.ctrl('w');
    h.keys(" 2");
    open_list(&mut h);
    h
}

#[test]
fn snapshot_of_the_tab_list() {
    for (lang, code) in [(Lang::En, "en"), (Lang::Ko, "ko")] {
        let mut h = snapshot_scene(lang);
        assert_screen!(format!("tab_list_{code}_120x34"), lang, h.draw(W, H));
    }
    let mut h = snapshot_scene(Lang::En);
    assert_eq!(names(&h), ["console 2", "shop.users", "console 1", "console 3"]);
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    let th = h.app.theme.clone();
    let row = |text: &str| -> u16 {
        (0..H).find(|y| row_text(buf, *y).contains(text)).unwrap_or_else(|| panic!("{text} on screen"))
    };
    let col = |y: u16, text: &str| -> u16 {
        let r = row_text(buf, y);
        datarig_tui::text::width(&r[..r.find(text).unwrap()]) as u16
    };
    // The active tab first, its name bold; the one active before it selected.
    let y = row("console 2  ");
    assert!(buf[(col(y, "console 2"), y)].modifier.contains(Modifier::BOLD), "the active tab is bold");
    assert_eq!(selected(&h), "shop.users");
    let y = row("shop.users  ");
    let cell = &buf[(col(y, "shop.users"), y)];
    if let Some(bg) = th.selection.bg {
        assert_eq!(cell.bg, bg, "the selected row has the selection's background");
    }
    assert!(cell.modifier.contains(th.selection.add_modifier), "and its modifiers");
    assert!(!cell.modifier.contains(Modifier::BOLD), "not the active tab");
    // The open transaction's mark in the warning color, as in the tab bar; the row on the
    // dialog's surface (neither selected nor hovered).
    let y = row("console 1  ");
    let x = col(y, "\u{25c6}");
    assert_eq!(buf[(x, y)].fg, th.warning);
    assert_eq!(buf[(col(y, "console 1"), y)].bg, th.surface);
    // The closed tabs' heading.
    let y = row("Recently closed");
    assert!(buf[(col(y, "Recently closed"), y)].modifier.contains(Modifier::BOLD));
}

/// A console closed in this session keeps its number: a new console takes another one, so the
/// list never shows two entries of the same name and the closed one comes back as listed.
#[test]
fn a_new_console_does_not_take_the_number_of_a_closed_one() {
    let mut h = four_tabs();
    h.keys(" 2");
    h.ctrl('w'); // console 2 closed
    h.ctrl('t');
    assert_eq!(active(&h), "console 5", "the closed console's number is held");
    open_list(&mut h);
    let all = names(&h);
    let mut unique = all.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), all.len(), "no two entries of the same name: {all:?}");
    while selected(&h) != "console 2" {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Enter);
    assert_eq!(active(&h), "console 2", "it comes back as it was listed");
}

/// The best match is selected whichever section it is in.
#[test]
fn the_best_match_is_selected_across_both_sections() {
    let mut h = four_tabs();
    h.keys(" 3");
    h.ctrl('w'); // console 3 closed; console 4 is tab 3 now
    open_list(&mut h);
    h.type_text("3");
    assert_eq!(names(&h)[0], "console 4", "the open ones still come first");
    assert_eq!(selected(&h), "console 3", "a name beats a tab number");
}

/// On a theme whose alternate surface is its surface (the terminal theme), the row under the
/// pointer is underlined instead, in the tab list and the other lists.
#[test]
fn the_hover_shows_on_the_terminal_theme() {
    use datarig_tui::theme;
    let mut h = four_tabs();
    h.app.theme = std::sync::Arc::new(theme::TERMINAL.clone());
    assert_eq!(theme::TERMINAL.surface_alt, theme::TERMINAL.surface);
    open_list(&mut h);
    let (x, y) = row_of(&mut h, "console 1");
    h.mouse(MouseEventKind::Moved, x, y);
    let t = h.draw(W, H);
    assert!(t.backend().buffer()[(x, y)].modifier.contains(Modifier::UNDERLINED), "the tab list's hovered row");
    // Quick connect (three profiles: the second row is not the selected one).
    let mut h = Harness::with_config(&sample_config(None), Lang::En);
    h.app.theme = std::sync::Arc::new(theme::TERMINAL.clone());
    h.ctrl('o');
    h.draw(W, H);
    let q = h.app.overlays.quick().unwrap().list;
    h.mouse(MouseEventKind::Moved, q.x + 3, q.y + 1);
    let t = h.draw(W, H);
    assert!(t.backend().buffer()[(q.x + 3, q.y + 1)].modifier.contains(Modifier::UNDERLINED), "quick connect");
    // A theme with an alternate surface keeps its background and no underline.
    h.app.theme = std::sync::Arc::new(theme::DARK.clone());
    let t = h.draw(W, H);
    let cell = &t.backend().buffer()[(q.x + 3, q.y + 1)];
    assert_eq!(cell.bg, theme::DARK.surface_alt);
    assert!(!cell.modifier.contains(Modifier::UNDERLINED));
}

/// A table tab in another database than its profile's own keeps its `database.` prefix once
/// closed, as the tab bar showed it.
#[test]
fn a_closed_tab_of_another_database_keeps_its_database_in_its_name() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let id = h.app.tab().id;
    h.app.tabs.get_mut(id).unwrap().context.database = Some("lab".into());
    open_list(&mut h);
    assert_eq!(names(&h)[0], "lab.shop.users");
    h.key(KeyCode::Esc);
    h.ctrl('w');
    // Its first page is still loading: closing asks first.
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.keys("y");
    open_list(&mut h);
    assert_eq!(names(&h).last().map(String::as_str), Some("lab.shop.users"), "{:?}", names(&h));
}

/// Ctrl+D on a tab that is not the active one leaves the status line alone.
#[test]
fn closing_another_tab_keeps_the_status_line() {
    use datarig_tui::app::{Level, Notice};
    let mut h = four_tabs();
    open_list(&mut h);
    h.app.status = Some(Notice::new(datarig_core::i18n::Label::TabReopenNone, Level::Info));
    assert_eq!(selected(&h), "console 3");
    h.ctrl('d');
    assert_eq!(h.app.tabs.len(), 3);
    assert!(h.status(W, H).contains("No closed tab to reopen"), "{}", h.status(W, H));
}

/// A closed saved query whose file is open again in another tab is not listed twice: picking
/// its closed row would only go to that tab.
#[test]
fn a_closed_saved_query_open_again_is_not_listed_as_closed() {
    let root = std::env::temp_dir().join(format!("datarig-tab-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (mut app, clock) = new_app_with_clock(&test_db_config(), Lang::En);
    let store = std::sync::Arc::new(datarig_core::secret::MemoryStore::new());
    app.set_secret_store(store.clone() as std::sync::Arc<dyn datarig_core::secret::SecretStore>);
    app.set_paths(datarig_core::paths::Paths { data: Some(root.join("data")), state: Some(root.join("state")) });
    let driver = FakeDriver::default();
    let fake = driver.clone();
    app.set_drivers(std::sync::Arc::new(move |name: &str| {
        matches!(name, "postgres")
            .then(|| std::sync::Arc::new(fake.clone()) as std::sync::Arc<dyn datarig_core::driver::Driver>)
    }));
    app.launch(datarig_tui::app::Startup::Normal);
    let mut h = Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock };
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.meta_db("local-pg", DbEvent::Connected);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
    type_sql(&mut h, "select 42");
    h.command("w mine");
    assert_eq!(h.app.tab().script(), Some("mine.sql"), "saved");
    h.ctrl('w');
    h.command("e mine");
    assert_eq!(h.app.tab().script(), Some("mine.sql"), "open again");
    open_list(&mut h);
    let listed = names(&h).iter().filter(|n| n.as_str() == "mine").count();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(listed, 1, "{:?}", names(&h));
}
