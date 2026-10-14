//! Moving tabs through the real `App` event path: a tab dragged by the mouse (raw press,
//! drag and release events, a frame drawn after each as the event loop does), the keys and
//! `:tabmove`. The tab bar's numbers are places and follow the new order; a console's own
//! number does not; the order is written to the workspace state and comes back on restart.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{Focus, Startup};
use datarig_tui::widgets::tabbar::{DROP, TabHit};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;

const W: u16 = 120;
const H: u16 = 30;

/// Connected, three console tabs with texts `a`, `b`, `c` (the first one active), drawn.
fn three_tabs() -> Harness {
    let mut h = Harness::connected(Lang::En);
    set_text(&mut h, "a");
    for t in ["b", "c"] {
        h.ctrl('t');
        set_text(&mut h, t);
    }
    h.keys(" 1");
    assert_eq!(h.app.tabs.active_index(), 0);
    h.draw(W, H);
    h
}

/// Replace the active tab's text (Normal mode before and after).
fn set_text(h: &mut Harness, text: &str) {
    h.keys("ggdGi");
    h.type_text(text);
    h.key(KeyCode::Esc);
}

fn texts(h: &Harness) -> Vec<String> {
    h.app.tabs.iter().map(|t| t.editor.text()).collect()
}

fn console_nos(h: &Harness) -> Vec<u32> {
    h.app.tabs.iter().map(|t| t.doc.console_no).collect()
}

/// The columns `[x0, x1)` tab `i` was drawn on (body and `×`).
fn span(h: &Harness, i: usize) -> (u16, u16) {
    let of = |t: &(u16, u16, TabHit)| matches!(t.2, TabHit::Tab(j) | TabHit::Close(j) if j == i);
    let hits = h.app.tab_hits();
    let x0 = hits.iter().filter(|t| of(t)).map(|t| t.0).min().unwrap_or_else(|| panic!("tab {i} drawn"));
    let x1 = hits.iter().filter(|t| of(t)).map(|t| t.1).max().unwrap();
    (x0, x1)
}

fn bar_y(h: &Harness) -> u16 {
    h.app.layout.tab_bar.y
}

fn press(h: &mut Harness, x: u16, y: u16) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.draw(W, H);
}

/// The left button held, moved to (x, y); a frame follows.
fn drag(h: &mut Harness, x: u16, y: u16) {
    h.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    h.draw(W, H);
}

fn release(h: &mut Harness, x: u16, y: u16) {
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.draw(W, H);
}

/// The tab bar's line as drawn now.
fn bar(h: &mut Harness, w: u16) -> String {
    h.screen(w, H).lines().nth(usize::from(bar_y(h))).unwrap().to_string()
}

#[test]
fn a_dragged_tab_is_lifted_shows_where_it_lands_and_lands_there() {
    let mut h = three_tabs();
    let focus = h.app.focus;
    assert_eq!(console_nos(&h), [1, 2, 3]);
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    let (c0, c1) = span(&h, 2);
    press(&mut h, a0 + 3, y);
    assert_eq!(h.app.tab_drag_view(), None, "a press alone lifts nothing");
    // Sideways past the threshold: lifted, drawn in the selection style.
    drag(&mut h, a0 + 5, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(0))), "still over its own left half");
    let t = h.draw(W, H);
    let sel = datarig_tui::theme::DARK.selection.bg.unwrap();
    assert_eq!(t.backend().buffer()[(a0 + 1, y)].bg, sel, "the dragged tab is lifted");
    assert!(!bar(&mut h, W).contains(DROP), "no marker where it would stay: {}", bar(&mut h, W));
    // Over the right half of the last tab: after it, the marker on its last blank.
    drag(&mut h, c1 - 2, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(3))));
    let t = h.draw(W, H);
    assert_eq!(t.backend().buffer()[(c1 - 1, y)].symbol(), DROP, "{}", bar(&mut h, W));
    assert_eq!(t.backend().buffer()[(c1 - 1, y)].fg, datarig_tui::theme::DARK.accent);
    assert_eq!(texts(&h), ["a", "b", "c"], "nothing moved before the release");
    // Over the left half of the last tab: before it, the marker on its first blank.
    drag(&mut h, c0 + 1, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(2))));
    assert_eq!(h.draw(W, H).backend().buffer()[(c0 - 1, y)].symbol(), DROP, "on the middle tab's last blank");
    release(&mut h, c0 + 1, y);
    assert_eq!(texts(&h), ["b", "a", "c"]);
    assert_eq!(h.app.tab_drag_view(), None);
    // The moved tab stays active, its console number (its name) goes with it, the bar's
    // numbers are places; the focus did not move.
    assert_eq!(h.app.tabs.active_index(), 1);
    assert_eq!(h.app.tab().editor.text(), "a");
    assert_eq!(console_nos(&h), [2, 1, 3]);
    assert_eq!(h.app.focus, focus);
    let line = bar(&mut h, W);
    assert!(
        line.contains(" 1 console 2 ") && line.contains(" 2 console 1 ") && line.contains(" 3 console 3 "),
        "{line}"
    );
    assert!(!line.contains(DROP));
    // `Space 1` goes to the first place.
    h.keys(" 1");
    assert_eq!(h.app.tab().editor.text(), "b");
}

#[test]
fn a_tab_dragged_left_lands_before_the_tab_under_the_pointer() {
    let mut h = three_tabs();
    h.keys(" 3");
    h.draw(W, H);
    let y = bar_y(&h);
    let (c0, _) = span(&h, 2);
    let (a0, _) = span(&h, 0);
    press(&mut h, c0 + 4, y);
    assert_eq!(h.app.tabs.active_index(), 2);
    drag(&mut h, c0 + 1, y);
    drag(&mut h, a0 + 1, y);
    assert_eq!(h.app.tab_drag_view(), Some((2, Some(0))));
    assert_eq!(h.draw(W, H).backend().buffer()[(a0, y)].symbol(), DROP, "on the first tab's first blank");
    release(&mut h, a0 + 1, y);
    assert_eq!(texts(&h), ["c", "a", "b"]);
    assert_eq!(h.app.tabs.active_index(), 0);
}

#[test]
fn a_press_and_release_that_did_not_go_far_is_a_click() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (b0, _) = span(&h, 1);
    press(&mut h, b0 + 3, y);
    assert_eq!(h.app.tabs.active_index(), 1, "the press selects, as before");
    drag(&mut h, b0 + 4, y);
    drag(&mut h, b0 + 3, y + 1);
    assert_eq!(h.app.tab_drag_view(), None, "one column sideways (or down) is no drag");
    release(&mut h, b0 + 4, y + 1);
    assert_eq!(texts(&h), ["a", "b", "c"]);
    assert_eq!(h.app.tabs.active_index(), 1);
    assert_eq!(h.overlay_kind(), None);
}

#[test]
fn the_close_button_still_closes_and_drags_nothing() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let x = h.app.tab_hits().iter().find(|t| t.2 == TabHit::Close(1)).unwrap().0;
    press(&mut h, x, y);
    assert_eq!(texts(&h), ["a", "c"], "closed on the press, as before");
    let (_, c1) = span(&h, 1);
    drag(&mut h, c1 - 1, y);
    assert_eq!(h.app.tab_drag_view(), None);
    release(&mut h, c1 - 1, y);
    assert_eq!(texts(&h), ["a", "c"]);
    // A middle press closes too and drags nothing.
    let (a0, _) = span(&h, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Middle), a0 + 2, y);
    h.draw(W, H);
    assert_eq!(texts(&h), ["c"]);
    drag(&mut h, a0 + 9, y);
    assert_eq!(h.app.tab_drag_view(), None);
}

#[test]
fn esc_a_dialog_or_a_lost_release_put_the_tab_back() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    let (_, c1) = span(&h, 2);
    let mode = h.app.tab().editor.mode;
    // Esc: back where it was; the Esc goes nowhere else.
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y);
    assert!(h.app.tab_drag_view().is_some());
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab_drag_view(), None);
    assert!(!bar(&mut h, W).contains(DROP));
    drag(&mut h, c1 - 2, y);
    release(&mut h, c1 - 2, y);
    assert_eq!(texts(&h), ["a", "b", "c"], "the rest of the drag moves nothing");
    assert_eq!(h.app.tab().editor.mode, mode);
    // A dialog coming on top (`:`) ends the drag.
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y);
    h.keys(":");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands));
    assert_eq!(h.app.tab_drag_view(), None, "not drawn lifted under the dialog");
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None, "the Esc is the dialog's");
    release(&mut h, c1 - 1, y);
    drag(&mut h, c1 - 2, y);
    release(&mut h, c1 - 2, y);
    assert_eq!(texts(&h), ["a", "b", "c"]);
    // Let go outside the window: the next move has no button; it needs a frame.
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y);
    h.mouse(MouseEventKind::Moved, c1 - 1, y + 10);
    assert!(!h.app.take_idle_event(), "the marker goes: a frame");
    assert_eq!(h.app.tab_drag_view(), None);
    release(&mut h, c1 - 1, y);
    assert_eq!(texts(&h), ["a", "b", "c"]);
}

#[test]
fn released_far_from_the_bar_it_stays_near_it_it_lands() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    let (_, c1) = span(&h, 2);
    // Far below the bar: nowhere to land, no marker; the release moves nothing.
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y + 3);
    assert_eq!(h.app.tab_drag_view(), Some((0, None)));
    assert!(!bar(&mut h, W).contains(DROP));
    release(&mut h, c1 - 1, y + 3);
    assert_eq!(texts(&h), ["a", "b", "c"]);
    // Back near the bar it goes on; two rows below and right of every tab: the end.
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y + 5);
    drag(&mut h, W - 2, y + 2);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(3))));
    release(&mut h, W - 2, y + 2);
    assert_eq!(texts(&h), ["b", "c", "a"]);
    // Left of every tab (over the explorer): the start.
    let x = span(&h, 2).0 + 2;
    press(&mut h, x, y);
    drag(&mut h, 1, y);
    assert_eq!(h.app.tab_drag_view(), Some((2, Some(0))));
    release(&mut h, 1, y);
    assert_eq!(texts(&h), ["a", "b", "c"]);
}

#[test]
fn dragging_past_the_end_of_an_overflowing_bar_scrolls_it() {
    let w = 80;
    let mut h = Harness::connected(Lang::En);
    set_text(&mut h, "0");
    for i in 1..14 {
        h.ctrl('t');
        set_text(&mut h, &i.to_string());
    }
    h.keys(" 1");
    h.draw(w, H);
    let y = bar_y(&h);
    let hits = h.app.tab_hits();
    let right = hits
        .iter()
        .find(|t| matches!(t.2, TabHit::More(_)))
        .unwrap_or_else(|| panic!("a scroll mark: {:?}\n{}", h.app.tabs.len(), h.screen(w, H)));
    let TabHit::More(hidden) = right.2 else { unreachable!() };
    let last_shown = hidden - 1;
    assert!(last_shown < 12, "tabs hidden on the right: {}", bar(&mut h, w));
    let (a0, _) = span(&h, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Left), a0 + 2, y);
    h.draw(w, H);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), a0 + 6, y);
    h.draw(w, H);
    // Each move over `›` goes past the next hidden tab; the bar follows.
    let mut slots = Vec::new();
    for k in 0..30 {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), w - 1 - (k % 2), y);
        h.draw(w, H);
        let v = h.app.tab_drag_view();
        slots.push(
            v.and_then(|v| v.1).unwrap_or_else(|| panic!("{k} {v:?} {:?}\n{}", h.app.tab_hits(), bar(&mut h, w))),
        );
    }
    assert!(slots.windows(2).all(|s| s[1] == s[0] || s[1] == s[0] + 1), "one tab at a time: {slots:?}");
    assert_eq!(*slots.last().unwrap(), 14, "after the last tab: {slots:?}");
    assert!(h.app.tab_hits().iter().any(|t| t.2 == TabHit::Tab(13)), "the last tab came into view");
    assert!(bar(&mut h, w).contains(DROP), "{}", bar(&mut h, w));
    h.mouse(MouseEventKind::Up(MouseButton::Left), w - 1, y);
    let order: Vec<String> = texts(&h);
    let want: Vec<String> = (1..14).chain([0]).map(|i| i.to_string()).collect();
    assert_eq!(order, want);
    assert_eq!(h.app.tabs.active_index(), 13);
    h.draw(w, H);
    assert!(h.app.tab_hits().iter().any(|t| t.2 == TabHit::Tab(13)), "the active tab is in view");
    // Back over `‹`: one tab at a time to the start.
    let (x0, _) = span(&h, 13);
    let left = h.app.layout.tab_bar.x;
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0 + 2, y);
    h.draw(w, H);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), x0, y);
    h.draw(w, H);
    let mut slots = Vec::new();
    for _ in 0..16 {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), left, y);
        h.draw(w, H);
        slots.push(h.app.tab_drag_view().and_then(|v| v.1).unwrap());
    }
    assert!(slots.windows(2).all(|s| s[1] == s[0] || s[1] + 1 == s[0]), "one tab at a time: {slots:?}");
    assert_eq!(*slots.last().unwrap(), 0, "{slots:?}");
    h.mouse(MouseEventKind::Up(MouseButton::Left), left, y);
    assert_eq!(texts(&h)[0], "0");
    assert_eq!(h.app.tabs.active_index(), 0);
}

/// Fourteen tabs at 80 columns, the first one active: the bar hides tabs on its right.
fn many_tabs(w: u16) -> Harness {
    let mut h = Harness::connected(Lang::En);
    set_text(&mut h, "0");
    for i in 1..14 {
        h.ctrl('t');
        set_text(&mut h, &i.to_string());
    }
    h.keys(" 1");
    h.draw(w, H);
    h
}

/// Released over a scroll mark while more tabs are hidden on its side: the tab lands where the
/// marker showed, not past the tab the bar scrolled to after the marker was drawn.
#[test]
fn released_over_a_scroll_mark_a_tab_lands_where_the_marker_showed() {
    let w = 80;
    let mut h = many_tabs(w);
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Left), a0 + 2, y);
    h.draw(w, H);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), a0 + 6, y);
    h.draw(w, H);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), w - 1, y);
    h.draw(w, H);
    let shown = h.app.tab_drag_view().unwrap().1.unwrap();
    assert!(h.app.tab_hits().iter().any(|t| t.2 == TabHit::More(shown)), "more tabs hidden: {}", bar(&mut h, w));
    h.mouse(MouseEventKind::Up(MouseButton::Left), w - 1, y);
    assert_eq!(texts(&h).iter().position(|t| t == "0"), Some(shown - 1), "the marker said place {shown}");
    // And back over `‹`.
    h.draw(w, H);
    let (x0, _) = span(&h, shown - 1);
    let left = h.app.layout.tab_bar.x;
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0 + 2, y);
    h.draw(w, H);
    for x in [x0, left, left] {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
        h.draw(w, H);
    }
    let (_, Some(shown)) = h.app.tab_drag_view().unwrap() else { panic!("a place") };
    assert!(shown > 0 && h.app.tab_hits().first().is_some_and(|t| t.2 == TabHit::More(shown - 1)));
    h.mouse(MouseEventKind::Up(MouseButton::Left), left, y);
    assert_eq!(texts(&h).iter().position(|t| t == "0"), Some(shown));
}

/// A release while the screen is too small to draw the workspace still ends the drag.
#[test]
fn a_release_on_a_screen_too_small_ends_the_drag() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    press(&mut h, a0 + 2, y);
    drag(&mut h, a0 + 30, y);
    assert!(h.app.tab_drag_view().is_some());
    h.draw(20, 5);
    h.mouse(MouseEventKind::Up(MouseButton::Left), 3, 0);
    h.draw(W, H);
    assert_eq!(h.app.tab_drag_view(), None);
    assert!(!bar(&mut h, W).contains(DROP));
    assert_eq!(texts(&h), ["a", "b", "c"]);
}

/// A press of another button ends a drag (a middle press on another tab closes that tab).
#[test]
fn another_buttons_press_ends_the_drag() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    let (c0, c1) = span(&h, 2);
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y);
    h.mouse(MouseEventKind::Down(MouseButton::Middle), c0 + 2, y);
    h.draw(W, H);
    assert_eq!(texts(&h), ["a", "b"], "the middle press closed its tab");
    assert_eq!(h.app.tab_drag_view(), None);
    drag(&mut h, a0 + 20, y);
    release(&mut h, a0 + 20, y);
    assert_eq!(texts(&h), ["a", "b"], "the rest of the drag moves nothing");
}

#[test]
fn keys_move_the_active_tab_one_place_and_stop_at_the_ends() {
    let mut h = three_tabs();
    h.keys(" t>");
    assert_eq!(texts(&h), ["b", "a", "c"]);
    assert_eq!(h.app.tab().editor.text(), "a");
    h.keys(" t>");
    h.keys(" t>");
    assert_eq!(texts(&h), ["b", "c", "a"], "not past the end, no wrap");
    assert_eq!(h.app.tabs.active_index(), 2);
    h.keys(" t<");
    h.keys(" t<");
    h.keys(" t<");
    assert_eq!(texts(&h), ["a", "b", "c"]);
    assert_eq!(h.overlay_kind(), None, "nothing said at an end");
    // Held (terminal auto-repeat), as `Space <` / `Space >` are.
    let spec = |id: &str| datarig_tui::app::action::by_id(id).unwrap();
    assert!(spec("tab.move_left").repeatable && spec("tab.move_right").repeatable);
    // From the explorer too.
    h.app.focus = Focus::Tree;
    h.keys(" t>");
    assert_eq!(texts(&h), ["b", "a", "c"]);
    assert_eq!(h.app.focus, Focus::Tree);
}

#[test]
fn tabmove_moves_the_active_tab_as_vim_does() {
    let mut h = Harness::connected(Lang::En);
    set_text(&mut h, "a");
    for t in ["b", "c", "d"] {
        h.ctrl('t');
        set_text(&mut h, t);
    }
    h.keys(" 2");
    h.command("tabmove");
    assert_eq!(texts(&h), ["a", "c", "d", "b"], "alone: last");
    h.command("tabmove 0");
    assert_eq!(texts(&h), ["b", "a", "c", "d"], "0: first");
    h.command("tabm 2");
    assert_eq!(texts(&h), ["a", "b", "c", "d"], "after tab 2");
    h.command("tabmove +2");
    assert_eq!(texts(&h), ["a", "c", "d", "b"]);
    h.command("tabmove -1");
    assert_eq!(texts(&h), ["a", "c", "b", "d"]);
    h.command("tabmove -9");
    assert_eq!(texts(&h), ["b", "a", "c", "d"], "not past the start");
    assert_eq!(h.app.tab().editor.text(), "b", "the moved tab stays active");
    // A place that is not one: the line says how, nothing moves.
    h.ctrl('k');
    h.type_text("tabmove x");
    h.key(KeyCode::Enter);
    let error = h.cmdline().and_then(|c| c.error.clone()).expect("an error");
    assert_eq!(
        error.render(&h.app.i18n).to_string(),
        "Usage: :tabmove [N|+N|-N|$] (N: after tab N, 0: first; +N / -N: N places right / left; alone or $: last)"
    );
    assert_eq!(
        error.render(&datarig_core::i18n::I18n::new(Lang::Ko)).to_string(),
        ko(datarig_core::i18n::Label::CommandsErrorTabmoveUsage)
    );
    h.key(KeyCode::Esc);
    assert_eq!(texts(&h), ["b", "a", "c", "d"]);
}

#[test]
fn the_tab_menu_moves_the_tab_and_offers_only_what_can_be_done() {
    let mut h = three_tabs();
    h.keys(" tm");
    let labels = h.menu_labels();
    assert!(labels.contains(&"Move tab right".to_string()), "{labels:?}");
    assert!(!labels.contains(&"Move tab left".to_string()), "the first tab: {labels:?}");
    h.menu_pick("Move tab right");
    assert_eq!(texts(&h), ["b", "a", "c"]);
    h.keys(" tm");
    let labels = h.menu_labels();
    assert!(labels.contains(&"Move tab left".to_string()) && labels.contains(&"Move tab right".to_string()));
    h.key(KeyCode::Esc);
}

#[test]
fn the_tab_list_and_recently_closed_follow_the_new_order() {
    let mut h = three_tabs();
    h.keys(" t>");
    h.keys(" t>");
    assert_eq!(texts(&h), ["b", "c", "a"]);
    // The tab list numbers are places; the MRU order is not changed by a move.
    h.keys(" tt");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList));
    let screen = h.screen(W, H);
    let row = |name: &str| screen.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}\n{screen}"));
    assert!(row("console 1").contains(" 3 "), "{screen}");
    assert!(row("console 2").contains(" 1 "), "{screen}");
    h.key(KeyCode::Esc);
    // Closed and reopened: where it was closed, with its number.
    h.ctrl('w');
    assert_eq!(texts(&h), ["b", "c"]);
    h.keys(" tu");
    assert_eq!(texts(&h), ["b", "c", "a"]);
    assert_eq!(console_nos(&h), [2, 3, 1]);
    h.keys(" t<");
    h.ctrl('w');
    h.keys(" tu");
    assert_eq!(texts(&h), ["b", "a", "c"], "at its new place");
    assert_eq!(console_nos(&h), [2, 1, 3]);
}

// ── persistence ──────────────────────────────────────────────────────────────

struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("datarig-tabdrag-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Dirs { root }
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn launch(dirs: &Dirs) -> Harness {
    let cfg = Config { connections: vec![ConnectionConfig::test_db()], ..Config::default() };
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: Some(dirs.root.join("data")), state: Some(dirs.root.join("state")) });
    let driver = FakeDriver::default();
    let fake = driver.clone();
    app.set_drivers(Arc::new(move |name: &str| {
        matches!(name, "postgres").then(|| Arc::new(fake.clone()) as Arc<dyn datarig_core::driver::Driver>)
    }));
    app.launch(Startup::Normal);
    Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
}

#[test]
fn the_new_order_is_written_at_once_and_comes_back_on_restart() {
    let dirs = Dirs::new("order");
    let mut h = launch(&dirs);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.meta_db("local-pg", datarig_core::driver::DbEvent::Connected);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
    set_text(&mut h, "a");
    for t in ["b", "c"] {
        h.ctrl('t');
        set_text(&mut h, t);
    }
    // The last tab dragged to the front.
    h.draw(W, H);
    let y = bar_y(&h);
    let (c0, _) = span(&h, 2);
    let (a0, _) = span(&h, 0);
    press(&mut h, c0 + 3, y);
    drag(&mut h, a0 + 1, y);
    release(&mut h, a0 + 1, y);
    assert_eq!(texts(&h), ["c", "a", "b"]);
    let ws = std::fs::read_to_string(dirs.root.join("state").join("workspace.toml")).unwrap();
    assert!(ws.contains("active = 0"), "{ws}");
    // A crash (no quit): the order and the console numbers come back, the same tab active.
    drop(h);
    let h = launch(&dirs);
    assert_eq!(texts(&h), ["c", "a", "b"]);
    assert_eq!(console_nos(&h), [3, 1, 2]);
    assert_eq!(h.app.tabs.active_index(), 0);
}
