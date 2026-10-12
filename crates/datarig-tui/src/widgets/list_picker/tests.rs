use super::*;
use std::time::Duration;

fn entry(value: u8, label: &str, hint: &str) -> Entry<u8> {
    Entry::new(value, label.into(), hint.into())
}

/// Four entries and a new one, the second selected.
fn picker() -> ListPicker<u8> {
    let entries = vec![
        entry(0, "off", "connects to the database directly"),
        entry(1, "office", "ec2-user@bastion.example.com"),
        entry(2, "lab", "tunnel@lab-bastion.internal:2222"),
        entry(3, "this profile only", ""),
    ];
    ListPicker::new(entries, Some("New SSH tunnel…".into()), Some(&1))
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn typ(p: &mut ListPicker<u8>, s: &str) {
    for c in s.chars() {
        assert_eq!(p.key(&k(KeyCode::Char(c)), false), Outcome::Pending);
    }
}

fn labels(p: &ListPicker<u8>) -> Vec<&str> {
    p.rows().iter().map(|r| p.label(*r)).collect()
}

#[test]
fn every_entry_in_order_then_the_new_one_the_current_value_selected() {
    let p = picker();
    assert_eq!(labels(&p), ["off", "office", "lab", "this profile only", "New SSH tunnel…"]);
    assert_eq!(p.selected(), Some(Row::Entry(1)));
    // A value that is not listed: the first row.
    let p = ListPicker::new(vec![entry(0, "a", ""), entry(1, "b", "")], None, Some(&9));
    assert_eq!(p.selected(), Some(Row::Entry(0)));
    assert_eq!(p.rows(), [Row::Entry(0), Row::Entry(1)], "no new entry asked for");
}

#[test]
fn typing_filters_as_the_action_menu_ranks_and_the_best_match_is_selected() {
    let mut p = picker();
    // `o`: the labels that start with it, in order, then a word of a label ("only"), then the
    // hints that have it.
    typ(&mut p, "o");
    assert_eq!(labels(&p), ["off", "office", "this profile only", "lab", "New SSH tunnel…"]);
    assert_eq!(p.selected(), Some(Row::Entry(0)), "the best match");
    typ(&mut p, "ff");
    assert_eq!(labels(&p), ["off", "office", "New SSH tunnel…"]);
    // Letters in order, not together: below a prefix.
    p.filter.set("");
    typ(&mut p, "ofc");
    assert_eq!(labels(&p), ["office", "New SSH tunnel…"]);
    // A part of a hint finds its entry, case ignored; its letters spread out do not.
    p.filter.set("");
    typ(&mut p, "dbd");
    assert_eq!(labels(&p), ["New SSH tunnel…"], "d…b…d of \"database directly\"");
    // The bastion in the hint finds its preset.
    p.filter.set("");
    typ(&mut p, "LAB-B");
    assert_eq!(labels(&p), ["lab", "New SSH tunnel…"]);
    // Backspace widens it again.
    for _ in 0..5 {
        p.key(&k(KeyCode::Backspace), false);
    }
    assert_eq!(p.rows().len(), 5);
}

#[test]
fn j_and_k_are_typed_into_the_filter_the_arrows_move() {
    let mut p = picker();
    typ(&mut p, "k");
    assert_eq!(p.filter.text(), "k");
    assert_eq!(labels(&p), ["New SSH tunnel…"], "no entry has a k");
    p.key(&k(KeyCode::Backspace), false);
    typ(&mut p, "j");
    assert_eq!(p.filter.text(), "j");
}

#[test]
fn the_selection_moves_around_the_ends() {
    let mut p = picker();
    p.key(&k(KeyCode::Down), false);
    p.key(&k(KeyCode::Down), false);
    p.key(&k(KeyCode::Down), false);
    assert_eq!(p.selected(), Some(Row::New), "the new entry is reached by the arrows");
    p.key(&k(KeyCode::Down), false);
    assert_eq!(p.selected(), Some(Row::Entry(0)), "past the last: the first");
    p.key(&k(KeyCode::Up), false);
    assert_eq!(p.selected(), Some(Row::New), "before the first: the last");
    p.key(&k(KeyCode::Tab), false);
    assert_eq!(p.selected(), Some(Row::Entry(0)));
    p.key(&ctrl('n'), false);
    assert_eq!(p.selected(), Some(Row::Entry(1)));
    p.key(&ctrl('p'), false);
    p.key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), false);
    assert_eq!(p.selected(), Some(Row::New));
    // One row: it stays there.
    typ(&mut p, "zz");
    p.key(&k(KeyCode::Down), false);
    p.key(&k(KeyCode::Up), false);
    assert_eq!(p.selected(), Some(Row::New));
}

#[test]
fn enter_picks_esc_cancels_and_held_keys_do_neither() {
    let mut p = picker();
    p.key(&k(KeyCode::Down), false);
    assert_eq!(p.key(&k(KeyCode::Enter), true), Outcome::Pending, "a held Enter");
    assert_eq!(p.key(&k(KeyCode::Esc), true), Outcome::Pending, "a held Esc");
    assert_eq!(p.key(&k(KeyCode::Enter), false), Outcome::Picked(2));
    assert_eq!(p.key(&k(KeyCode::Esc), false), Outcome::Cancelled);
    typ(&mut p, "new");
    assert_eq!(p.selected(), Some(Row::New), "only the new entry is left");
    assert_eq!(p.key(&k(KeyCode::Enter), false), Outcome::New);
}

#[test]
fn nothing_matching_leaves_the_new_entry_or_nothing_to_pick() {
    let mut p = picker();
    typ(&mut p, "nothing like it");
    assert_eq!(p.rows(), [Row::New]);
    assert_eq!(p.key(&k(KeyCode::Enter), false), Outcome::New);
    let mut p = ListPicker::new(vec![entry(0, "off", "")], None, None);
    typ(&mut p, "x");
    assert!(p.rows().is_empty() && p.selected().is_none());
    assert_eq!(p.key(&k(KeyCode::Down), false), Outcome::Pending);
    assert_eq!(p.key(&k(KeyCode::Enter), false), Outcome::Pending, "nothing to pick");
    assert_eq!(p.key(&k(KeyCode::Esc), false), Outcome::Cancelled);
}

#[test]
fn a_paste_goes_to_the_filter() {
    let mut p = picker();
    p.paste("lab");
    assert_eq!(labels(&p), ["lab", "New SSH tunnel…"]);
}

/// Draw `p` on a 80x24 screen at `now`; the screen's text.
fn draw(p: &mut ListPicker<u8>, now: Instant) -> String {
    let area = Rect::new(0, 0, 80, 24);
    let mut buf = Buffer::empty(area);
    let (title, footer) = (Localized::verbatim("SSH tunnel"), Localized::verbatim("Enter pick"));
    p.draw(area, &mut buf, (&title, &footer), ("type to filter", "Nothing matches"), true, now);
    (0..24)
        .map(|y| (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }
}

#[test]
fn it_draws_the_filter_the_rows_and_says_when_nothing_matches() {
    let mut p = picker();
    let now = Instant::now();
    let screen = draw(&mut p, now);
    // The hints in a column after the widest label.
    let office = format!("{:<17}  ec2-user@bastion.example.com", "office");
    let off = format!("{:<17}  connects to the database directly", "off");
    for text in ["SSH tunnel", "› type to filter", &office, &off, "New SSH tunnel…"] {
        assert!(screen.contains(text), "{text}:\n{screen}");
    }
    assert_eq!(p.list_area().height, 5);
    typ(&mut p, "zz");
    let screen = draw(&mut p, now);
    assert!(screen.contains("Nothing matches") && screen.contains("New SSH tunnel…"), "{screen}");
    let list = p.list_area();
    assert_eq!(list.height, 1, "the line that says so is not a row");
    assert!(screen.lines().nth(list.y as usize).unwrap().contains("New SSH tunnel…"), "{screen}");
}

#[test]
fn a_click_picks_on_the_release_over_the_row_pressed_but_not_right_after_it_came_on_top() {
    let mut p = picker();
    let shown = Instant::now();
    draw(&mut p, shown);
    let list = p.list_area();
    let (x, lab) = (list.x + 2, list.y + 2);
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    // Too soon: the press is not armed.
    assert_eq!(p.mouse(&mouse(down, x, lab), shown + Duration::from_millis(100)), Outcome::Pending);
    assert_eq!(p.mouse(&mouse(up, x, lab), shown + Duration::from_millis(150)), Outcome::Pending);
    let later = shown + Duration::from_millis(500);
    // Released over another row: nothing.
    p.mouse(&mouse(down, x, lab), later);
    assert_eq!(p.mouse(&mouse(up, x, lab + 1), later), Outcome::Pending);
    assert_eq!(p.selected(), Some(Row::Entry(1)), "the selection stays");
    // Outside the list: nothing, not even the filter.
    p.mouse(&mouse(down, 0, 0), later);
    assert_eq!(p.mouse(&mouse(up, 0, 0), later), Outcome::Pending);
    assert_eq!((p.filter.text(), p.selected()), ("", Some(Row::Entry(1))));
    p.mouse(&mouse(down, x, lab), later);
    assert_eq!(p.mouse(&mouse(up, x, lab), later), Outcome::Picked(2));
    assert_eq!(p.selected(), Some(Row::Entry(2)));
    // The new entry, by a click.
    p.mouse(&mouse(down, x, list.y + 4), later);
    assert_eq!(p.mouse(&mouse(up, x, list.y + 4), later), Outcome::New);
    // Covered and back on top: the clock starts again.
    p.rearm();
    draw(&mut p, later);
    p.mouse(&mouse(down, x, lab), later + Duration::from_millis(10));
    assert_eq!(p.mouse(&mouse(up, x, lab), later + Duration::from_millis(20)), Outcome::Pending);
}

#[test]
fn the_pointer_only_highlights_and_the_wheel_moves_the_selection() {
    let mut p = picker();
    let now = Instant::now();
    draw(&mut p, now);
    let list = p.list_area();
    assert!(p.pointer(list.x + 2, list.y + 3));
    assert_eq!((p.hover(), p.selected()), (Some(3), Some(Row::Entry(1))), "highlighted, not selected");
    assert!(!p.pointer(list.x + 5, list.y + 3), "the same row: no change");
    assert!(p.pointer(list.x + 2, list.y + 1), "over the selected row: no highlight");
    assert_eq!(p.hover(), None);
    p.pointer(list.x + 2, list.y + 3);
    // A move with no button held drops an armed press (its release was lost).
    let later = now + Duration::from_millis(500);
    p.mouse(&mouse(MouseEventKind::Down(MouseButton::Left), list.x + 2, list.y + 3), later);
    p.pointer(list.x + 2, list.y + 3);
    assert_eq!(p.mouse(&mouse(MouseEventKind::Up(MouseButton::Left), list.x + 2, list.y + 3), later), Outcome::Pending);
    assert_eq!(p.mouse(&mouse(MouseEventKind::ScrollDown, list.x, list.y), later), Outcome::Pending);
    assert_eq!(p.selected(), Some(Row::Entry(2)));
    assert_eq!(p.hover(), None, "the rows moved under it");
    p.mouse(&mouse(MouseEventKind::ScrollUp, list.x, list.y), later);
    p.mouse(&mouse(MouseEventKind::ScrollUp, list.x, list.y), later);
    assert_eq!(p.selected(), Some(Row::Entry(0)));
}

#[test]
fn a_long_list_scrolls_to_keep_the_selection_in_sight() {
    let entries = (0..40).map(|i| entry(i, &format!("preset-{i:02}"), "")).collect();
    let mut p = ListPicker::new(entries, Some("New SSH tunnel…".into()), Some(&30));
    let screen = draw(&mut p, Instant::now());
    assert!(screen.contains("preset-30") && !screen.contains("preset-00"), "{screen}");
    for _ in 0..30 {
        p.key(&k(KeyCode::Up), false);
    }
    let screen = draw(&mut p, Instant::now());
    assert!(screen.contains("preset-00") && !screen.contains("New SSH tunnel…"), "{screen}");
    p.key(&k(KeyCode::Up), false);
    let screen = draw(&mut p, Instant::now());
    assert!(screen.contains("New SSH tunnel…") && !screen.contains("preset-00"), "{screen}");
}

#[test]
fn an_emptied_filter_selects_the_current_value_again() {
    let mut p = picker();
    typ(&mut p, "o");
    assert_eq!(p.selected(), Some(Row::Entry(0)), "the best match");
    p.key(&k(KeyCode::Backspace), false);
    assert_eq!(p.selected(), Some(Row::Entry(1)), "the current one, as when it opened");
}

#[test]
fn a_release_picks_the_entry_pressed_not_the_one_typing_moved_under_it() {
    let mut p = picker();
    let shown = Instant::now();
    draw(&mut p, shown);
    let list = p.list_area();
    let (x, y) = (list.x + 2, list.y + 2);
    let later = shown + Duration::from_millis(500);
    p.mouse(&mouse(MouseEventKind::Down(MouseButton::Left), x, y), later);
    assert_eq!(p.rows()[2], Row::Entry(2), "lab pressed");
    // Typed meanwhile: another entry is drawn there now.
    typ(&mut p, "o");
    assert_eq!(p.rows()[2], Row::Entry(3));
    assert_eq!(p.mouse(&mouse(MouseEventKind::Up(MouseButton::Left), x, y), later), Outcome::Pending);
    assert_eq!(p.selected(), Some(Row::Entry(0)), "the selection stays");
}
