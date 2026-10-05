//! The action menu: `Space Space`, `Shift+F10` and the Menu key open the same menu a right click
//! does, next to the selection (the explorer's row, the grid's cell, the editor's cursor; `Space
//! t m` the active tab's). Typing filters it, prefix matches first; when nothing in it matches,
//! every action is searched. An item never runs on something other than what the menu was
//! opened on.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::{Label, Lang};
use datarig_tui::app::Focus;
use datarig_tui::app::action;
use datarig_tui::app::menu::{Heading, MenuItem, MenuLine};
use datarig_tui::app::overlay::OverlayKind;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use std::time::Duration;

/// The open menu's actions, as opened (ids).
fn ids(h: &Harness) -> Vec<&'static str> {
    h.app.overlays.menu().expect("a menu").actions().iter().map(|a| action::spec(*a).id).collect()
}

/// The open menu's lines as shown: headings as `# text`, items as `label | keys`.
fn lines(h: &Harness) -> Vec<String> {
    h.app
        .menu_lines()
        .into_iter()
        .map(|(heading, label, keys)| if heading { format!("# {label}") } else { format!("{label} | {keys}") })
        .collect()
}

/// The label of the selected line.
fn selected(h: &Harness) -> String {
    let m = h.app.overlays.menu().expect("a menu");
    h.app.menu_lines()[m.selected].1.to_string()
}

fn menu_open(h: &Harness) -> bool {
    h.overlay_kind() == Some(OverlayKind::ContextMenu)
}

fn shift_f10(h: &mut Harness) {
    h.key_mod(KeyCode::F(10), KeyModifiers::SHIFT);
}

/// Right click on the explorer row whose text is `text` (drawn at 120x30).
fn right_click_row(h: &mut Harness, text: &str) {
    h.draw(120, 30);
    let i = h.rows().iter().position(|r| r.trim() == text).unwrap_or_else(|| panic!("{text}: {:?}", h.rows()));
    let area = h.app.explorer.area;
    h.mouse(MouseEventKind::Down(MouseButton::Right), area.x + 4, area.y + i as u16);
}

#[test]
fn every_key_opens_the_explorers_menu_next_to_the_row_with_the_right_clicks_items() {
    let mut h = Harness::connected(Lang::En);
    h.draw(120, 30);
    h.explore("local-pg");
    let row = h.rows().iter().position(|r| r == "local-pg").unwrap() as u16;
    h.keys("  ");
    assert!(menu_open(&h), "Space Space");
    let by_key = ids(&h);
    let area = h.app.explorer.area;
    assert_eq!(h.app.overlays.menu().unwrap().at.1, area.y + row, "next to the cursor's row");
    let shown = lines(&h);
    assert!(shown[0].starts_with("# ") && shown[0].ends_with("local-pg"), "the node first: {shown:?}");
    assert!(shown.contains(&"# Explorer".to_string()), "then the pane: {shown:?}");
    assert!(shown.contains(&"Edit connection profile | e".to_string()), "each with its key: {shown:?}");
    h.key(KeyCode::Esc);
    assert!(!menu_open(&h));
    // The right click: the same items.
    right_click_row(&mut h, "local-pg");
    assert_eq!(ids(&h), by_key);
    assert_eq!(lines(&h), shown, "one menu, one look");
    h.key(KeyCode::Esc);
    // Shift+F10, the Menu key and the action from the command line.
    h.explore("local-pg");
    shift_f10(&mut h);
    assert_eq!(ids(&h), by_key, "Shift+F10");
    h.key(KeyCode::Esc);
    h.key(KeyCode::Menu);
    assert_eq!(ids(&h), by_key, "the Menu key");
    h.key(KeyCode::Esc);
    h.command("menu.open");
    assert_eq!(ids(&h), by_key, "menu.open from the command line");
    h.key(KeyCode::Esc);
    // `explorer.context_menu` (kept for keymaps that bound it) opens the same.
    h.command("explorer.context_menu");
    assert_eq!(ids(&h), by_key);
    // Enter runs the selected item: the first, open or toggle.
    assert_eq!(selected(&h), "Explorer: open or toggle");
}

#[test]
fn the_grids_menu_opens_at_the_cell_and_by_right_click_with_the_same_items() {
    let mut h = with_edge_results(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h.keys("jl");
    h.draw(160, 45);
    h.keys("  ");
    assert!(menu_open(&h));
    let g = h.app.tab().grid.clone();
    let col = *g.hit_cols.iter().find(|c| c.2 == 1).unwrap();
    assert_eq!(h.app.overlays.menu().unwrap().at, (col.0 + 1, g.data_y + 1), "at the selected cell");
    let by_key = lines(&h);
    assert!(by_key[0].starts_with("# Cell ") && by_key[0].ends_with(", row 2"), "{by_key:?}");
    assert!(by_key.contains(&"# Results".to_string()), "{by_key:?}");
    assert!(by_key.contains(&"Copy selection ▸ | ".to_string()), "{by_key:?}");
    h.key(KeyCode::Esc);
    let (x, y) = (col.0 + 2, h.app.tab().grid.data_y + 1);
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, y);
    assert_eq!(lines(&h), by_key, "the right click on that cell");
    // Typing finds an item; Enter runs it on that cell.
    h.menu_pick("Copy the cell (a selected range as TSV)");
    assert!(!menu_open(&h));
    let cell = clip.last().expect("copied");
    // A range selected with `v`: the menu is the selection's.
    h.keys("vj");
    h.keys("  ");
    assert_eq!(lines(&h)[0], "# Selection");
    h.menu_pick("Copy the cell (a selected range as TSV)");
    assert_ne!(clip.last().as_deref(), Some(cell.as_str()), "the range, not the cell");
    // From the inspector: the grid's menu.
    h.app.focus = Focus::Inspector;
    shift_f10(&mut h);
    assert!(menu_open(&h) && h.app.overlays.menu().unwrap().ctx == datarig_tui::keymap::Ctx::Grid);
}

#[test]
fn the_tabs_menu_by_space_t_m_and_by_right_click() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('t');
    assert_eq!(h.app.tabs.len(), 2);
    h.draw(120, 30);
    h.keys(" tm");
    assert!(menu_open(&h), "Space t m");
    let by_key = ids(&h);
    assert!(by_key.starts_with(&["tab.close"]), "{by_key:?}");
    assert!(by_key.contains(&"tab.new_console") && by_key.contains(&"tab.reopen_closed"), "{by_key:?}");
    assert!(by_key.contains(&"conn.disconnect_current"), "its profile is connected");
    assert_eq!(h.app.overlays.menu().unwrap().at.1, h.app.layout.tab_bar.y, "under the tab");
    let shown = lines(&h);
    assert!(shown.contains(&"# Tabs".to_string()), "{shown:?}");
    assert!(shown.contains(&"Close tab | Ctrl+W".to_string()), "{shown:?}");
    h.key(KeyCode::Esc);
    // Right click on the active tab: the same menu.
    let active = h.app.tabs.active_index();
    let hit = |h: &Harness, i: usize| {
        h.app.tab_hits().into_iter().find(|t| t.2 == datarig_tui::widgets::tabbar::TabHit::Tab(i)).unwrap()
    };
    let (x0, _, _) = hit(&h, active);
    h.mouse(MouseEventKind::Down(MouseButton::Right), x0 + 1, h.app.layout.tab_bar.y);
    assert_eq!(ids(&h), by_key);
    // A shown key that is not a character runs its item while the menu is open: Ctrl+W.
    h.ctrl('w');
    assert!(!menu_open(&h));
    assert_eq!(h.app.tabs.len(), 1, "the tab closed");
    // Right click on another tab: it becomes the active one, the menu is its.
    h.ctrl('t');
    h.draw(120, 30);
    let (x0, _, _) = hit(&h, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Right), x0 + 1, h.app.layout.tab_bar.y);
    assert_eq!(h.app.tabs.active_index(), 0);
    assert!(menu_open(&h));
}

#[test]
fn the_editors_menu_in_normal_and_visual_mode_not_while_typing() {
    let mut h = Harness::connected(Lang::En);
    assert_eq!(h.app.focus, Focus::Editor);
    h.draw(120, 30);
    let cursor = h.app.layout.editor_cursor;
    h.keys("  ");
    assert!(menu_open(&h));
    assert_eq!(h.app.overlays.menu().unwrap().at, cursor, "at the cursor");
    let shown = lines(&h);
    assert_eq!(shown[0], "# Statement under the cursor");
    assert_eq!(&ids(&h)[..3], ["query.execute_current", "editor.format", "editor.comment_toggle"]);
    assert!(shown.contains(&"# Query".to_string()), "{shown:?}");
    assert!(shown.iter().any(|l| l.starts_with("Format the statement") && l.ends_with("Space e f")), "{shown:?}");
    // Enter on the first item runs the statement under the cursor.
    h.sent();
    h.key(KeyCode::Enter);
    assert!(matches!(&h.sent()[..], [DbCommand::Execute { .. }]), "run");
    h.db(DbEvent::Page {
        id: h.app.tab().exec.query_id,
        columns: None,
        rows: Vec::new(),
        more: false,
        elapsed: Duration::ZERO,
    });
    // Visual mode: the selection's menu.
    h.app.focus = Focus::Editor;
    h.keys("V");
    h.keys("  ");
    assert_eq!(lines(&h)[0], "# Selection");
    assert_eq!(h.app.overlays.menu().unwrap().ctx, datarig_tui::keymap::Ctx::VimVisual);
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    // Insert mode: Space is typed.
    h.keys("i");
    let before = h.app.tab().editor.text();
    h.keys("  ");
    assert!(!menu_open(&h));
    assert_eq!(h.app.tab().editor.text().len(), before.len() + 2, "typed");
    h.key(KeyCode::Esc);
    // Right click on another line: the cursor goes there, the menu is for that statement.
    h.draw(120, 30);
    let t = h.app.layout.editor_text;
    h.mouse(MouseEventKind::Down(MouseButton::Right), t.x + 8, t.y + 3);
    assert!(menu_open(&h));
    assert_eq!(h.app.tab().editor.row, 3);
    assert_eq!(&ids(&h)[..3], ["query.execute_current", "editor.format", "editor.comment_toggle"]);
}

#[test]
fn a_right_click_in_a_visual_selection_keeps_it() {
    let mut h = Harness::connected(Lang::En);
    h.keys("Vj");
    h.draw(120, 30);
    let t = h.app.layout.editor_text;
    let row = h.app.tab().editor.row as u16 - h.app.tab().editor.top as u16;
    h.mouse(MouseEventKind::Down(MouseButton::Right), t.x + 8, t.y + row);
    assert!(menu_open(&h));
    assert_eq!(lines(&h)[0], "# Selection", "the selection stays");
}

#[test]
fn typing_filters_prefix_first_then_searches_every_action() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.keys("  ");
    // `d`: the names that start with it first, in the menu's order; the best is selected.
    h.type_text("d");
    let shown = lines(&h);
    let items: Vec<&str> =
        shown.iter().filter(|l| !l.starts_with('#')).map(|l| l.split(" | ").next().unwrap()).collect();
    assert_eq!(&items[..3], ["Duplicate connection profile", "Delete connection profile", "Disconnect"], "{shown:?}");
    assert!(items.contains(&"Edit connection profile"), "letters in order come after: {shown:?}");
    assert!(!items.contains(&"Test connection"), "{shown:?}");
    assert_eq!(selected(&h), "Duplicate connection profile");
    // Letters are text here: `j` and `k` filter, the arrows move.
    h.key(KeyCode::Down);
    assert_eq!(selected(&h), "Delete connection profile");
    h.ctrl('p');
    assert_eq!(selected(&h), "Duplicate connection profile");
    // Backspace to an empty filter: every item again; once more keeps the menu open.
    h.key(KeyCode::Backspace);
    assert_eq!(h.app.overlays.menu().unwrap().filter.text(), "");
    assert_eq!(selected(&h), "Explorer: open or toggle");
    h.key(KeyCode::Backspace);
    assert!(menu_open(&h), "Backspace on an empty filter does not close");
    // Nothing in the menu matches: every action, under its own heading.
    h.type_text("settings");
    let shown = lines(&h);
    assert_eq!(shown[0], "# All actions", "{shown:?}");
    assert_eq!(selected(&h), "Settings");
    assert!(!shown.iter().any(|l| l.starts_with("Action menu")), "the menu itself is not offered");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Settings));
    h.key(KeyCode::Esc);
    // Nothing at all: said; Enter does nothing; Esc closes.
    h.explore("local-pg");
    h.keys("  ");
    h.type_text("zzqx");
    assert_eq!(h.app.overlays.menu().unwrap().lines, [MenuLine::NoMatch]);
    assert!(h.screen(120, 30).contains("No action matches"));
    h.key(KeyCode::Enter);
    assert!(menu_open(&h));
    h.key(KeyCode::Esc);
    assert!(!menu_open(&h));
}

#[test]
fn a_paste_goes_to_the_filter() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.keys("  ");
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("test con".into()));
    assert_eq!(h.app.overlays.menu().unwrap().filter.text(), "test con");
    assert_eq!(selected(&h), "Test connection");
}

#[test]
fn korean_is_typed_into_the_filter() {
    let mut h = Harness::connected(Lang::Ko);
    h.explore("local-pg");
    h.keys("  ");
    let label = ko(Label::ActionConnTest);
    h.type_text(label);
    assert_eq!(h.app.overlays.menu().unwrap().filter.text(), label, "taken as typed, not as QWERTY keys");
    assert_eq!(selected(&h), label);
    // English words find items while the UI is in Korean.
    h.key_mod(KeyCode::Char('u'), KeyModifiers::CONTROL);
    h.type_text("test con");
    assert_eq!(selected(&h), label);
}

/// A menu opened on a cell whose result was replaced since (a run that was going on delivered
/// its rows): its items run on nothing, and say so.
#[test]
fn a_menu_whose_cell_was_replaced_runs_nothing() {
    let mut h = with_edge_results(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.key(KeyCode::Tab);
    h.keys("jjl");
    h.keys("  ");
    assert!(menu_open(&h));
    let (cols, rows) = edge_rows();
    h.db(DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 0), "the new result starts at its first cell");
    h.menu_pick("Copy the cell (a selected range as TSV)");
    assert!(!menu_open(&h));
    assert!(clip.last().is_none(), "nothing copied");
    assert!(h.status(160, 45).contains("Not run: what the menu was opened on has changed"), "{}", h.status(160, 45));
}

/// A menu opened on a schema whose list was read again meanwhile (another schema is now where
/// it was): its console item does not open on the other schema.
#[test]
fn a_menu_whose_node_moved_runs_nothing() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.goto("shop");
    h.keys("  ");
    assert!(lines(&h)[0].ends_with("shop"), "{:?}", lines(&h));
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "reporting".into(), "shop".into()])));
    let tabs = h.app.tabs.len();
    h.menu_pick("New console in shop");
    assert_eq!(h.app.tabs.len(), tabs, "no console opened");
    assert!(h.status(120, 30).contains("Not run"), "{}", h.status(120, 30));
    // Opened again: it is right again.
    h.goto("shop");
    h.keys("  ");
    h.menu_pick("New console in shop");
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert_eq!(h.app.tab().context.schema.as_deref(), Some("shop"));
}

#[test]
fn the_welcome_panel_has_a_menu() {
    let mut h = Harness::with_config(&datarig_core::config::Config::default(), Lang::En);
    assert!(h.app.profiles.is_empty());
    h.draw(100, 30);
    h.key(KeyCode::Tab);
    h.keys("  ");
    assert!(menu_open(&h));
    assert_eq!(ids(&h), ["conn.new", "settings.open", "help.context"]);
    h.key(KeyCode::Enter);
    assert!(h.form_open());
}

#[test]
fn which_key_and_the_help_show_the_menu() {
    let mut h = Harness::connected(Lang::En);
    h.app.focus = Focus::Tree;
    h.keys(" ");
    h.advance(Duration::from_secs(2));
    let screen = h.screen(120, 30);
    assert!(screen.contains("Space") && screen.contains("Action menu"), "{screen}");
    h.key(KeyCode::Esc);
    h.key(KeyCode::F(1));
    h.keys("/");
    h.type_text("action menu");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Space Space / Shift+F10 / Menu"), "{screen}");
}

#[test]
fn the_menus_items_are_in_sections_with_headings_that_select_nothing() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.keys("  ");
    let m = h.app.overlays.menu().unwrap();
    assert!(matches!(m.lines[0], MenuLine::Heading(Heading::Target)));
    assert!(m.lines.contains(&MenuLine::Heading(Heading::Pane)));
    assert_eq!(m.selected, 1);
    // Up from the first item goes around to the last; headings are skipped.
    h.key(KeyCode::Up);
    let m = h.app.overlays.menu().unwrap();
    assert_eq!(m.selected, m.lines.len() - 1);
    assert!(matches!(m.lines[m.selected], MenuLine::Item(MenuItem::Action(_))));
    h.key(KeyCode::Down);
    assert_eq!(h.app.overlays.menu().unwrap().selected, 1);
    // Insta: the menu as drawn, by key, in the explorer.
    insta::assert_snapshot!("action_menu_explorer_en_100x30", h.draw(100, 30).backend());
}

#[test]
fn snapshots_of_the_menus() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.keys("j");
    h.draw(120, 34);
    h.keys("  ");
    insta::assert_snapshot!("action_menu_grid_en_120x34", h.draw(120, 34).backend());
    h.type_text("cop");
    insta::assert_snapshot!("action_menu_grid_filtered_en_120x34", h.draw(120, 34).backend());
    h.key(KeyCode::Esc);
    h.app.focus = Focus::Editor;
    h.draw(120, 34);
    h.keys("  ");
    insta::assert_snapshot!("action_menu_editor_en_120x34", h.draw(120, 34).backend());
    h.type_text("new tab");
    insta::assert_snapshot!("action_menu_all_actions_en_120x34", h.draw(120, 34).backend());
    h.key(KeyCode::Esc);
    h.draw(120, 34);
    h.keys(" tm");
    insta::assert_snapshot!("action_menu_tab_en_120x34", h.draw(120, 34).backend());
}

/// A profile's menu while it connects: its mark and spinner change, the profile does not, so
/// its items run (Disconnect is how a hanging attempt is stopped).
#[test]
fn a_profiles_menu_runs_while_its_spinner_turns() {
    use datarig_tui::app::NodeState;
    let mut h = Harness::new(Lang::En);
    let id = h.app.profiles.iter().find(|p| p.name == "local-pg").unwrap().id;
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.conns.state(id), NodeState::Connecting);
    h.draw(120, 30);
    h.keys("  ");
    h.advance(Duration::from_millis(450));
    h.draw(120, 30);
    h.menu_pick("Disconnect");
    assert_eq!(h.app.conns.state(id), NodeState::Disconnected, "{}", h.status(120, 30));
    // A connect that finishes while the menu is open changes nothing either.
    h.key(KeyCode::Enter);
    h.keys("  ");
    h.db(DbEvent::Connected);
    h.menu_pick("Test connection");
    assert!(!h.status(120, 30).contains("Not run"), "{}", h.status(120, 30));
}

/// The results pane showing a run's Messages: the menu has no cell items (the grid is hidden),
/// and a right click there leaves the hidden grid's selection alone.
#[test]
fn the_messages_view_has_no_cell_items() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.keys("jl");
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, datarig_tui::app::tabs::ResultView::Messages);
    h.draw(120, 34);
    h.keys("  ");
    assert!(menu_open(&h));
    let shown = lines(&h);
    assert!(!ids(&h).contains(&"grid.view_cell") && !shown.iter().any(|l| l.starts_with("# Cell")), "{shown:?}");
    assert!(ids(&h).contains(&"results.tab.prev"), "{shown:?}");
    h.key(KeyCode::Esc);
    let r = h.app.layout.results;
    h.mouse(MouseEventKind::Down(MouseButton::Right), r.x + 10, r.y + 4);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (1, 1), "the hidden grid's cell stays");
}

/// `Ctrl+Enter` (the kitty keyboard protocol) is the key shown next to "Run statement": it runs
/// that, not the item selected.
#[test]
fn a_modified_enter_runs_the_item_whose_key_it_is() {
    let mut h = Harness::connected(Lang::En);
    h.app.enhanced_keys = true;
    h.draw(120, 30);
    h.keys("  ");
    assert!(lines(&h).iter().any(|l| l.starts_with("Run statement") && l.ends_with("Ctrl+Enter")));
    h.type_text("fo");
    assert!(selected(&h).starts_with("Format"));
    let text = h.app.tab().editor.text();
    h.sent();
    h.key_mod(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(matches!(&h.sent()[..], [DbCommand::Execute { .. }]), "run");
    assert_eq!(h.app.tab().editor.text(), text, "not formatted");
}

/// The copy formats' letters typed with a Korean input source are the QWERTY keys at their
/// places, as they are outside the menu.
#[test]
fn the_copy_formats_take_hangul_as_their_keys() {
    let mut h = with_edge_results(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.key(KeyCode::Tab);
    h.draw(160, 45);
    h.keys("  ");
    (0..4).for_each(|_| h.key(KeyCode::Down));
    assert_eq!(selected(&h), "Copy selection ▸");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.menu().unwrap().sub.is_some());
    // The jamo on the `j` key: JSON.
    h.type_text("\u{3153}");
    assert!(!menu_open(&h));
    assert!(clip.last().is_some_and(|t| t.starts_with('[')), "{:?}", clip.last());
}

/// From the inspector the menu is the grid's; closed without running anything, the focus is
/// the inspector's again.
#[test]
fn the_inspector_keeps_the_focus_after_the_menu() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.draw(160, 45);
    h.app.focus = Focus::Inspector;
    h.keys("  ");
    assert!(menu_open(&h));
    h.key(KeyCode::Esc);
    assert_eq!(h.app.focus, Focus::Inspector);
}

/// What the filter matches in the label shown ranks above what it matches only in an action's
/// id: `f` selects the first label with a word that starts with it, not "New folder" (whose id,
/// `folder.new`, starts with it).
#[test]
fn the_label_shown_ranks_above_the_id() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.keys("  ");
    h.type_text("f");
    assert_eq!(selected(&h), "Move connection profile to a folder");
}

/// Scrolled back to a section's first item, its heading is on screen too.
#[test]
fn a_scrolled_menu_shows_the_heading_of_the_first_item() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.draw(120, 24);
    h.keys("  ");
    // No item of the grid's menu matches: every copy action, more than fit.
    h.type_text("results.copy");
    h.draw(120, 24);
    assert_eq!(lines(&h)[0], "# All actions");
    h.key(KeyCode::Up);
    h.draw(120, 24);
    assert!(h.app.overlays.menu().unwrap().scroll > 0, "the last item is far down");
    h.key(KeyCode::Down);
    let screen = h.screen(120, 24);
    assert_eq!(h.app.overlays.menu().unwrap().scroll, 0);
    assert!(screen.contains("All actions"), "{screen}");
}

/// A click in the box that is not on an item (the filter, a heading, the border) keeps the menu.
#[test]
fn a_click_on_the_filter_keeps_the_menu() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.draw(120, 30);
    h.keys("  ");
    h.draw(120, 30);
    let list = h.app.overlays.menu().unwrap().list;
    // The filter's row, the left border, the top border, a heading.
    for (x, y) in [(list.x + 3, list.y - 1), (list.x - 1, list.y + 2), (list.x + 3, list.y - 2), (list.x + 3, list.y)] {
        h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        assert!(menu_open(&h), "({x}, {y})");
    }
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + list.width + 3, list.y + 1);
    assert!(!menu_open(&h), "outside: closed");
}

/// The copy formats of a menu whose result was replaced since copy nothing.
#[test]
fn the_formats_of_a_stale_menu_copy_nothing() {
    let mut h = with_edge_results(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.key(KeyCode::Tab);
    h.keys("jjl");
    h.keys("  ");
    (0..4).for_each(|_| h.key(KeyCode::Down));
    h.key(KeyCode::Enter);
    let (cols, rows) = edge_rows();
    h.db(DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
    h.keys("c");
    assert!(clip.last().is_none(), "nothing copied");
    assert!(!menu_open(&h));
    assert!(h.status(160, 45).contains("Not run"));
}

/// An editor menu whose text changed since runs nothing; a key bound in the pane but not shown
/// in the menu does nothing.
#[test]
fn an_editor_menu_whose_text_changed_runs_nothing() {
    let mut h = Harness::connected(Lang::En);
    h.keys("  ");
    // Ctrl+O (quick connect) is not in the menu: nothing.
    h.ctrl('o');
    assert!(menu_open(&h));
    assert!(h.app.tab_mut().editor.replace_text("SELECT 2;"));
    h.sent();
    h.key(KeyCode::Enter);
    assert!(h.sent().is_empty(), "not run");
    assert!(h.status(120, 30).contains("Not run"));
}

/// A tab menu whose tab is not the active one any more runs nothing on the active one.
#[test]
fn a_tab_menu_of_another_tab_closes_nothing() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('t');
    h.draw(120, 30);
    h.keys(" tm");
    h.app.dispatch(datarig_tui::app::action::Action::PrevTab);
    let tabs = h.app.tabs.len();
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), tabs, "no tab closed");
}
