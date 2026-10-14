//! Moving tabs through the real `App` event path: a tab dragged by the mouse (raw press,
//! drag and release events, a frame drawn after each as the event loop does, the clock moved
//! for the frames of a slide), the keys and `:tabmove`. The dragged tab is drawn where the
//! pointer holds it, over its neighbours; a neighbour it went past the middle of slides into
//! the gap; the bar's numbers are places and follow the order shown; a console's own number
//! does not; the order is written to the workspace state and comes back on restart.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::tab_drag::{SLIDE, SLIDE_FRAME};
use datarig_tui::app::{Focus, Startup};
use datarig_tui::widgets::tabbar::TabHit;
use ratatui::buffer::Cell;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;

const W: u16 = 120;
const H: u16 = 30;

/// Connected, `n` console tabs with texts `0`, `1`, … (the first one active), drawn `w` wide.
fn tabs_in(lang: Lang, n: usize, w: u16) -> Harness {
    let mut h = Harness::connected(lang);
    set_text(&mut h, "0");
    for i in 1..n {
        h.ctrl('t');
        set_text(&mut h, &i.to_string());
    }
    h.keys(" 1");
    assert_eq!(h.app.tabs.active_index(), 0);
    h.draw(w, H);
    h
}

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

/// The columns `[x0, x1)` tab `i` was drawn on (body and `×`; what another tab covers is not
/// its).
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

/// The frames of a slide, `w` wide, until it is over.
fn slide_out(h: &mut Harness, w: u16) {
    for _ in 0..SLIDE.as_millis() / SLIDE_FRAME.as_millis() {
        h.advance(SLIDE_FRAME);
        h.draw(w, H);
    }
}

/// The tab bar's line as drawn now.
fn bar(h: &mut Harness, w: u16) -> String {
    h.screen(w, H).lines().nth(usize::from(bar_y(h))).unwrap().to_string()
}

/// The columns `[x0, x1)` drawn lifted (the theme's selection background; the second column
/// of a wide character is the blank the terminal draws it over), checked to be one stretch.
fn lifted(h: &mut Harness, w: u16) -> Option<(u16, u16)> {
    let t = h.draw(w, H);
    let buf = t.backend().buffer();
    let y = bar_y(h);
    let sel = datarig_tui::theme::DARK.selection.bg;
    let xs: Vec<u16> = (0..w).filter(|x| buf[(*x, y)].bg == sel.unwrap()).collect();
    let (&x0, &last) = (xs.first()?, xs.last()?);
    let x1 = last + buf[(last, y)].symbol().width() as u16;
    for x in x0..x1 {
        let after_wide = x > 0 && buf[(x - 1, y)].symbol().width() == 2;
        assert!(xs.contains(&x) || after_wide, "one stretch {x0}..{x1}, not {x}: {}", bar(h, w));
    }
    Some((x0, x1))
}

/// The column right of the last tab drawn.
fn tabs_end(h: &Harness) -> u16 {
    h.app.tab_hits().iter().filter(|t| !matches!(t.2, TabHit::More(_))).map(|t| t.1).max().unwrap()
}

/// No wide character of the bar is drawn by halves: each is followed by the blank the terminal
/// draws it over, never by another tab's column.
fn whole(h: &mut Harness, w: u16) {
    let t = h.draw(w, H);
    let buf = t.backend().buffer();
    let y = bar_y(h);
    for x in 0..w {
        if buf[(x, y)].symbol().width() == 2 {
            assert!(x + 1 < w && buf[(x + 1, y)] == Cell::default(), "half a character at {x}: {}", bar(h, w));
        }
    }
}

#[test]
fn a_dragged_tab_follows_the_pointer_over_its_neighbour() {
    let mut h = three_tabs();
    let focus = h.app.focus;
    let y = bar_y(&h);
    let (a0, a1) = span(&h, 0);
    let (b0, b1) = span(&h, 1);
    assert_eq!(b0, a1);
    press(&mut h, a0 + 3, y);
    assert_eq!(h.app.tab_drag_view(), None, "a press alone lifts nothing");
    assert_eq!(lifted(&mut h, W), None);
    // Sideways past the threshold: lifted, held where it was pressed.
    drag(&mut h, a0 + 5, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(0))));
    assert_eq!(lifted(&mut h, W), Some((a0 + 2, a1 + 2)), "it moved with the pointer");
    // Five columns over the next tab: drawn over it; the gap it left is blank; nothing moved.
    drag(&mut h, a0 + 8, y);
    assert_eq!(lifted(&mut h, W), Some((a0 + 5, a1 + 5)));
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(0))), "not past the middle of the next tab yet");
    let line = bar(&mut h, W);
    let cols: Vec<char> = line.chars().collect();
    assert!(cols[usize::from(a0)..usize::from(a0 + 5)].iter().all(|c| *c == ' '), "the gap: {line}");
    assert_eq!(span(&h, 1), (a1 + 5, b1), "what shows of the next tab");
    assert!(line.contains(" 1 console 1 ") && !line.contains(" 2 console 2 "), "{line}");
    assert_eq!(texts(&h), ["a", "b", "c"], "nothing moved before the release");
    assert_eq!(h.app.focus, focus);
}

#[test]
fn past_the_middle_of_a_neighbour_it_slides_into_the_gap_and_the_release_keeps_the_order() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, a1) = span(&h, 0);
    let (b0, b1) = span(&h, 1);
    let (w, wb) = (a1 - a0, b1 - b0);
    press(&mut h, a0 + 3, y);
    drag(&mut h, a0 + 5, y);
    // Just short of half of the next tab, then just past it.
    let half = wb / 2;
    drag(&mut h, a0 + 3 + half, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(0))));
    drag(&mut h, a0 + 3 + half + 1, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(1))));
    // The next tab slides over, a few columns a frame, faster first; then the timer stops.
    let now = h.clock.now();
    assert!(h.app.next_tick(now).is_some_and(|t| t <= now + SLIDE_FRAME), "a frame soon");
    let mut xs = vec![h.app.tab_x(1).unwrap()];
    for _ in 0..5 {
        h.advance(SLIDE_FRAME);
        h.draw(W, H);
        xs.push(h.app.tab_x(1).unwrap());
    }
    assert_eq!(xs.first(), Some(&i32::from(b0)), "it starts where it was: {xs:?}");
    assert_eq!(xs.last(), Some(&i32::from(a0)), "it ends in the gap: {xs:?}");
    assert!(xs.windows(2).all(|p| p[1] <= p[0]) && xs.windows(3).any(|p| p[0] > p[1] && p[1] > p[2]), "{xs:?}");
    let steps: Vec<i32> = xs.windows(2).map(|p| p[0] - p[1]).collect();
    assert!(steps[0] >= steps[steps.len() - 1], "faster first: {steps:?}");
    let now = h.clock.now();
    assert!(h.app.next_tick(now).is_none_or(|t| t >= now + std::time::Duration::from_secs(1)), "no more frames");
    // The numbers are places: of the order shown.
    let line = bar(&mut h, W);
    assert!(line.contains(" 1 conso") && line.contains(" 2 console 1 \u{d7} "), "{line}");
    assert_eq!(texts(&h), ["a", "b", "c"], "nothing moved before the release");
    release(&mut h, a0 + 3 + half + 1, y);
    assert_eq!(texts(&h), ["b", "a", "c"]);
    assert_eq!(h.app.tab_drag_view(), None);
    // The moved tab stays active, its console number (its name) goes with it; it settles
    // into its place from where it was let go.
    assert_eq!(h.app.tabs.active_index(), 1);
    assert_eq!(console_nos(&h), [2, 1, 3]);
    assert_eq!(h.app.tab_x(1), Some(i32::from(a0 + half + 1)), "drawn where it was let go");
    slide_out(&mut h, W);
    assert_eq!(span(&h, 1), (a0 + wb, a0 + wb + w));
    assert_eq!(lifted(&mut h, W), None);
    let line = bar(&mut h, W);
    assert!(
        line.contains(" 1 console 2 ") && line.contains(" 2 console 1 ") && line.contains(" 3 console 3 "),
        "{line}"
    );
    // `Space 1` goes to the first place.
    h.keys(" 1");
    assert_eq!(h.app.tab().editor.text(), "b");
}

#[test]
fn a_pointer_that_jumps_passes_every_tab_on_the_way() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    let (_, c1) = span(&h, 2);
    press(&mut h, a0 + 3, y);
    drag(&mut h, a0 + 5, y);
    // One move from the first tab to the end (a remote terminal reports few moves).
    drag(&mut h, c1 - 1, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(2))));
    // Both slide, at once.
    slide_out(&mut h, W);
    let line = bar(&mut h, W);
    assert!(line.contains(" 1 console 2 ") && line.contains(" 2 console 3 "), "{line}");
    assert_eq!(lifted(&mut h, W).map(|l| l.1), Some(c1), "held at the end of the tabs, not past it");
    // And back in one move: before the first.
    drag(&mut h, 0, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(0))));
    drag(&mut h, c1 - 1, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), c1 - 1, y);
    assert_eq!(texts(&h), ["b", "c", "a"]);
    // A release where no move was reported lands there too.
    let mut h = three_tabs();
    press(&mut h, a0 + 3, y);
    drag(&mut h, a0 + 5, y);
    release(&mut h, c1 - 1, y);
    assert_eq!(texts(&h), ["b", "c", "a"]);
}

#[test]
fn a_tab_dragged_left_lands_before_the_tab_it_went_past() {
    let mut h = three_tabs();
    h.keys(" 3");
    h.draw(W, H);
    let y = bar_y(&h);
    let (c0, _) = span(&h, 2);
    let (a0, _) = span(&h, 0);
    press(&mut h, c0 + 4, y);
    assert_eq!(h.app.tabs.active_index(), 2);
    drag(&mut h, c0 + 1, y);
    drag(&mut h, a0 + 4, y);
    assert_eq!(h.app.tab_drag_view(), Some((2, Some(0))));
    assert_eq!(lifted(&mut h, W).map(|l| l.0), Some(a0), "not left of the first tab");
    release(&mut h, a0 + 4, y);
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
    assert_eq!(lifted(&mut h, W), None);
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

/// A tab dragged past the next one (`b` slid into its place), from the first tab's column 3.
fn dragged_past_b(h: &mut Harness) -> u16 {
    let y = bar_y(h);
    let (a0, _) = span(h, 0);
    let (b0, b1) = span(h, 1);
    press(h, a0 + 3, y);
    let x = a0 + 3 + (b1 - b0) / 2 + 2;
    drag(h, x, y);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(1))));
    slide_out(h, W);
    x
}

/// Every tab is back where it was, drawn as before the drag.
fn all_back(h: &mut Harness, before: &str, spans: &[(u16, u16)]) {
    assert_eq!(texts(h), ["a", "b", "c"]);
    assert_eq!(h.app.tab_drag_view(), None);
    slide_out(h, W);
    assert_eq!(bar(h, W), before);
    assert_eq!(lifted(h, W), None);
    assert_eq!((0..3).map(|i| span(h, i)).collect::<Vec<_>>(), spans);
    let now = h.clock.now();
    assert!(h.app.next_tick(now).is_none_or(|t| t >= now + std::time::Duration::from_secs(1)), "no more frames");
}

#[test]
fn esc_a_dialog_another_button_or_a_lost_release_put_every_tab_back() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let before = bar(&mut h, W);
    let spans: Vec<(u16, u16)> = (0..3).map(|i| span(&h, i)).collect();
    let mode = h.app.tab().editor.mode;
    // Esc: back where they were; the Esc goes nowhere else; the rest of the drag moves nothing.
    let x = dragged_past_b(&mut h);
    h.key(KeyCode::Esc);
    h.draw(W, H);
    assert!(lifted(&mut h, W).is_none());
    assert_eq!(h.app.tab_x(1), Some(i32::from(spans[0].0)), "b slides back from where it was drawn");
    all_back(&mut h, &before, &spans);
    drag(&mut h, x + 10, y);
    release(&mut h, x + 10, y);
    all_back(&mut h, &before, &spans);
    assert_eq!(h.app.tab().editor.mode, mode);
    // A dialog coming on top (`:`) ends the drag; its Esc is the dialog's.
    let x = dragged_past_b(&mut h);
    h.keys(":");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands));
    assert_eq!(h.app.tab_drag_view(), None, "not drawn lifted under the dialog");
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None, "the Esc is the dialog's");
    release(&mut h, x, y);
    all_back(&mut h, &before, &spans);
    // Let go outside the window: the next move has no button; it needs a frame.
    let x = dragged_past_b(&mut h);
    h.mouse(MouseEventKind::Moved, x, y + 10);
    assert!(!h.app.take_idle_event(), "the tabs go back: a frame");
    h.draw(W, H);
    release(&mut h, x, y);
    all_back(&mut h, &before, &spans);
    // A press of another button (the right one opens the tab's menu).
    let x = dragged_past_b(&mut h);
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, y + 5);
    h.draw(W, H);
    h.key(KeyCode::Esc);
    release(&mut h, x, y);
    all_back(&mut h, &before, &spans);
}

#[test]
fn far_from_the_bar_the_tab_goes_back_to_its_place_and_near_it_follows_again() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let before = bar(&mut h, W);
    let spans: Vec<(u16, u16)> = (0..3).map(|i| span(&h, i)).collect();
    let (a0, a1) = spans[0];
    let (_, c1) = spans[2];
    // Far below the bar: nowhere to land; back at its place, still lifted; the release moves
    // nothing.
    let x = dragged_past_b(&mut h);
    drag(&mut h, x, y + 3);
    assert_eq!(h.app.tab_drag_view(), Some((0, None)));
    slide_out(&mut h, W);
    assert_eq!(lifted(&mut h, W), Some((a0, a1)), "at its own place");
    assert_eq!(span(&h, 1), spans[1]);
    release(&mut h, x, y + 3);
    all_back(&mut h, &before, &spans);
    // Back near the bar it follows again; two rows below and right of every tab: the end.
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y + 5);
    assert_eq!(h.app.tab_drag_view(), Some((0, None)));
    drag(&mut h, W - 2, y + 2);
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(2))));
    release(&mut h, W - 2, y + 2);
    assert_eq!(texts(&h), ["b", "c", "a"]);
    // Left of every tab (over the explorer): the start.
    slide_out(&mut h, W);
    let x = span(&h, 2).0 + 2;
    press(&mut h, x, y);
    drag(&mut h, 1, y);
    assert_eq!(h.app.tab_drag_view(), Some((2, Some(0))));
    release(&mut h, 1, y);
    assert_eq!(texts(&h), ["a", "b", "c"]);
}

#[test]
fn wide_tab_names_are_covered_and_uncovered_by_whole_characters() {
    // Korean console names: two columns a character.
    let mut h = tabs_in(Lang::Ko, 3, W);
    let y = bar_y(&h);
    let (a0, a1) = span(&h, 0);
    let (_, c1) = span(&h, 2);
    let w = a1 - a0;
    press(&mut h, a0 + 3, y);
    // Column by column to the end and back, each frame and the frames of each slide checked.
    let path: Vec<u16> = (a0 + 5..c1 + 3).chain((0..c1 + 3).rev()).collect();
    for x in path {
        drag(&mut h, x, y);
        whole(&mut h, W);
        let (l0, l1) = lifted(&mut h, W).unwrap_or_else(|| panic!("lifted at {x}: {}", bar(&mut h, W)));
        assert_eq!(l1 - l0, w, "drawn whole at {x}: {}", bar(&mut h, W));
        h.advance(SLIDE_FRAME / 2);
        whole(&mut h, W);
    }
    assert_eq!(h.app.tab_drag_view(), Some((0, Some(0))));
    drag(&mut h, c1 - 4, y);
    release(&mut h, c1 - 4, y);
    for _ in 0..6 {
        whole(&mut h, W);
        h.advance(SLIDE_FRAME);
    }
    assert_eq!(texts(&h), ["1", "2", "0"]);
}

#[test]
fn a_narrow_bar_keeps_the_dragged_tab_whole_and_lands_it_where_shown() {
    // The smallest screen, five tabs; the explorer widened (`Space >`) until the bar has room
    // for four of them, three, then less than three.
    let w = 80;
    for widen in [0, 3, 5] {
        let mut h = tabs_in(Lang::Ko, 5, w);
        for _ in 0..widen {
            h.keys(" >");
        }
        h.draw(w, H);
        let y = bar_y(&h);
        assert!(h.app.tab_hits().iter().any(|t| matches!(t.2, TabHit::More(_))), "{widen}: {}", bar(&mut h, w));
        let (a0, _) = span(&h, 0);
        h.mouse(MouseEventKind::Down(MouseButton::Left), a0 + 2, y);
        h.draw(w, H);
        let mut shown = None;
        for x in (a0 + 4..w).chain((0..w).rev()).chain(a0..w) {
            h.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
            h.draw(w, H);
            whole(&mut h, w);
            assert!(lifted(&mut h, w).is_some(), "{widen} {x}: {}", bar(&mut h, w));
            shown = h.app.tab_drag_view().and_then(|v| v.1);
        }
        let shown = shown.expect("a place");
        h.mouse(MouseEventKind::Up(MouseButton::Left), w - 1, y);
        assert_eq!(texts(&h).iter().position(|t| t == "0"), Some(shown), "{widen}");
    }
}

/// Fourteen tabs at 80 columns, the first one active: the bar hides tabs on its right.
fn many_tabs(w: u16) -> Harness {
    tabs_in(Lang::En, 14, w)
}

#[test]
fn dragging_past_the_end_of_an_overflowing_bar_scrolls_it_one_tab_at_a_time() {
    let w = 80;
    let mut h = many_tabs(w);
    let y = bar_y(&h);
    assert!(h.app.tab_hits().iter().any(|t| matches!(t.2, TabHit::More(_))), "tabs hidden: {}", bar(&mut h, w));
    let (a0, _) = span(&h, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Left), a0 + 2, y);
    h.draw(w, H);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), a0 + 6, y);
    h.draw(w, H);
    // Each move over `›` goes past the next hidden tab; the bar follows; the tab stays at the
    // pointer, next to the mark.
    let mut slots = Vec::new();
    for _ in 0..30 {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), w - 1, y);
        h.draw(w, H);
        let v = h.app.tab_drag_view();
        slots.push(v.and_then(|v| v.1).unwrap_or_else(|| panic!("{v:?} {}", bar(&mut h, w))));
        assert_eq!(lifted(&mut h, w).map(|l| l.1), Some(tabs_end(&h)), "at the end of the tabs: {}", bar(&mut h, w));
        whole(&mut h, w);
    }
    assert!(slots.windows(2).skip(1).all(|s| s[1] == s[0] || s[1] == s[0] + 1), "one tab at a time: {slots:?}");
    assert_eq!(*slots.last().unwrap(), 13, "the last place: {slots:?}");
    h.mouse(MouseEventKind::Up(MouseButton::Left), w - 1, y);
    let want: Vec<String> = (1..14).chain([0]).map(|i| i.to_string()).collect();
    assert_eq!(texts(&h), want);
    assert_eq!(h.app.tabs.active_index(), 13);
    h.draw(w, H);
    slide_out(&mut h, w);
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
        assert_eq!(lifted(&mut h, w).map(|l| l.0), Some(left + u16::from(slots.last() != Some(&0))));
    }
    assert!(slots.windows(2).skip(1).all(|s| s[1] == s[0] || s[1] + 1 == s[0]), "one tab at a time: {slots:?}");
    assert_eq!(*slots.last().unwrap(), 0, "{slots:?}");
    h.mouse(MouseEventKind::Up(MouseButton::Left), left, y);
    assert_eq!(texts(&h)[0], "0");
    assert_eq!(h.app.tabs.active_index(), 0);
}

/// Released over a scroll mark while more tabs are hidden on its side: the tab lands where it
/// was shown.
#[test]
fn released_over_a_scroll_mark_a_tab_lands_where_it_was_shown() {
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
    assert!(h.app.tab_hits().iter().any(|t| matches!(t.2, TabHit::More(_))), "more tabs hidden: {}", bar(&mut h, w));
    // The release reports the same place as the last move: no step further.
    h.mouse(MouseEventKind::Up(MouseButton::Left), w - 1, y);
    assert_eq!(texts(&h).iter().position(|t| t == "0"), Some(shown));
}

/// Released next to a scroll mark where no move was reported (a row lower): the tab lands where
/// it was shown, not past one more hidden tab.
#[test]
fn released_off_the_last_move_next_to_a_scroll_mark_a_tab_lands_where_it_was_shown() {
    let w = 80;
    let mut h = many_tabs(w);
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Left), a0 + 2, y);
    h.draw(w, H);
    for x in [a0 + 6, w - 1] {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
        h.draw(w, H);
    }
    let shown = h.app.tab_drag_view().unwrap().1.unwrap();
    assert!(h.app.tab_hits().iter().any(|t| matches!(t.2, TabHit::More(_))), "more tabs hidden: {}", bar(&mut h, w));
    h.mouse(MouseEventKind::Up(MouseButton::Left), w - 1, y + 1);
    assert_eq!(texts(&h).iter().position(|t| t == "0"), Some(shown));
}

/// A tab pressed while it slides is held where it was drawn, not where it is going.
#[test]
fn a_tab_pressed_while_it_slides_is_held_where_it_was_drawn() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, a1) = span(&h, 0);
    let (b0, b1) = span(&h, 1);
    let x = dragged_past_b(&mut h);
    release(&mut h, x, y);
    assert_eq!(texts(&h), ["b", "a", "c"]);
    h.advance(SLIDE_FRAME);
    h.draw(W, H);
    // `a` belongs after `b` now, at a0 + b's width.
    let (place, drawn) = (i32::from(a0 + b1 - b0), h.app.tab_x(1).unwrap());
    assert!(drawn < place, "still sliding to its place at {place}: {drawn}");
    let at = u16::try_from(drawn).unwrap() + 3;
    press(&mut h, at, y);
    drag(&mut h, at + 2, y);
    assert_eq!(lifted(&mut h, W).map(|l| l.0), Some(at + 2 - 3), "held by its fourth column");
    assert_eq!(lifted(&mut h, W).map(|l| l.1 - l.0), Some(a1 - a0));
}

/// Other tabs or another order (a key opened or moved one) end the drag: every tab goes back,
/// the rest of the drag moves nothing.
#[test]
fn a_key_that_opens_or_moves_a_tab_ends_the_drag() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let before = bar(&mut h, W);
    let spans: Vec<(u16, u16)> = (0..3).map(|i| span(&h, i)).collect();
    let x = dragged_past_b(&mut h);
    h.keys(" t>");
    h.draw(W, H);
    assert_eq!(texts(&h), ["b", "a", "c"], "the key moved the tab");
    assert_eq!(h.app.tab_drag_view(), None);
    let now = h.clock.now();
    assert!(h.app.next_tick(now).is_some_and(|t| t <= now + SLIDE_FRAME), "the drag ended with the key: a slide");
    slide_out(&mut h, W);
    assert_eq!(lifted(&mut h, W), None);
    drag(&mut h, x + 20, y);
    release(&mut h, x + 20, y);
    assert_eq!(texts(&h), ["b", "a", "c"], "the rest of the drag moves nothing");
    h.keys(" t<");
    h.draw(W, H);
    slide_out(&mut h, W);
    assert_eq!(bar(&mut h, W), before);
    let x = dragged_past_b(&mut h);
    h.ctrl('t');
    h.draw(W, H);
    assert_eq!(h.app.tab_drag_view(), None);
    release(&mut h, x, y);
    let kept: Vec<String> = texts(&h).into_iter().filter(|t| !t.is_empty()).collect();
    assert_eq!(kept, ["a", "b", "c"], "the new tab is the only change");
    slide_out(&mut h, W);
    assert_eq!(span(&h, 0), spans[0]);
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
    slide_out(&mut h, W);
    assert_eq!(lifted(&mut h, W), None);
    assert_eq!(texts(&h), ["a", "b", "c"]);
}

/// A press of another button ends a drag (a middle press on another tab closes that tab).
#[test]
fn another_buttons_press_ends_the_drag() {
    let mut h = three_tabs();
    let y = bar_y(&h);
    let (a0, _) = span(&h, 0);
    let (_, c1) = span(&h, 2);
    press(&mut h, a0 + 2, y);
    drag(&mut h, c1 - 1, y);
    // The last frame drew `c` slid over: the middle press hits what is drawn there.
    slide_out(&mut h, W);
    h.mouse(MouseEventKind::Down(MouseButton::Middle), span(&h, 2).0 + 1, y);
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
