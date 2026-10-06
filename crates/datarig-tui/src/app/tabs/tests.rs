use super::*;

fn ed(text: &str) -> Editor {
    Editor::new(text)
}

fn texts(m: &TabManager) -> Vec<String> {
    m.iter().map(|t| t.editor.text()).collect()
}

#[test]
fn opens_after_the_active_tab_and_activates_it() {
    let p = ProfileId::new();
    let mut m = TabManager::new(Some(p), ed("a"));
    let b = m.open(TabKind::Console, Some(p), ed("b"));
    assert_eq!(m.active().id, b);
    m.activate(0);
    m.open(TabKind::Console, Some(p), ed("c"));
    assert_eq!(texts(&m), ["a", "c", "b"]);
    assert_eq!(m.active_index(), 1);
    let ids: Vec<TabId> = m.iter().map(|t| t.id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "ids are unique");
}

#[test]
fn cycling_and_numbers_wrap_and_stay_in_range() {
    let mut m = TabManager::new(None, ed("a"));
    m.open(TabKind::Console, None, ed("b"));
    m.open(TabKind::Console, None, ed("c"));
    assert_eq!(m.active_index(), 2);
    m.cycle(1);
    assert_eq!(m.active_index(), 0);
    m.cycle(-1);
    assert_eq!(m.active_index(), 2);
    assert!(!m.activate(3), "no fourth tab");
    assert_eq!(m.active_index(), 2);
    assert!(m.activate(1));
    assert_eq!(m.active().editor.text(), "b");
}

#[test]
fn closing_fixes_the_active_tab_down_to_no_tab() {
    let mut m = TabManager::new(None, ed("a"));
    let b = m.open(TabKind::Console, None, ed("b"));
    let c = m.open(TabKind::Console, None, ed("c"));
    // Closing the active last tab activates the new last one.
    assert!(m.close(c).is_some());
    assert_eq!(m.active().id, b);
    // Closing a tab left of the active one keeps the same tab active.
    let d = m.open(TabKind::Console, None, ed("d"));
    let first = m.iter().next().unwrap().id;
    m.close(first);
    assert_eq!(m.active().id, d);
    assert_eq!(texts(&m), ["b", "d"]);
    // Closing the active middle tab activates its right neighbour.
    m.activate(0);
    m.close(b);
    assert_eq!(m.active().id, d);
    // The last tab closes too: no tab, and the active one is a blank stand-in.
    assert!(m.close(d).is_some());
    assert!(m.is_empty());
    assert_eq!((m.active().id, m.active().profile), (TabId(0), None));
    m.cycle(1);
    assert!(!m.activate(0));
    assert_eq!(m.closed_count(), 4);
    assert!(m.reopen().is_some(), "and comes back");
    assert_eq!(texts(&m), ["d"]);
}

#[test]
fn an_empty_manager_has_a_blank_stand_in_that_keeps_nothing() {
    let mut m = TabManager::default();
    assert!(m.is_empty());
    m.active_mut().editor = ed("typed into nothing");
    m.active_mut().profile = Some(ProfileId::new());
    assert_eq!(m.active_mut().editor.text(), "", "a fresh stand-in every time");
    assert_eq!(m.active_mut().profile, None);
    assert!(m.get(TabId(0)).is_none(), "nothing finds it by its id");
    let id = m.open(TabKind::Console, None, ed("a"));
    assert_eq!(m.active().id, id);
}

#[test]
fn reopen_brings_back_text_cursor_and_place_newest_first() {
    let p = ProfileId::new();
    let mut m = TabManager::new(Some(p), ed("a"));
    let mut e = ed("select 1;\nselect 2;");
    e.row = 1;
    e.col = 3;
    let b = m.open(TabKind::Console, Some(p), e);
    let c = m.open(TabKind::Console, Some(p), ed("c"));
    m.close(b);
    m.close(c);
    assert_eq!(m.closed_count(), 2);
    let back = m.reopen().unwrap();
    assert!(back != c, "a new id");
    assert_eq!(texts(&m), ["a", "c"]);
    m.reopen();
    assert_eq!(texts(&m), ["a", "select 1;\nselect 2;", "c"]);
    let t = m.active();
    assert_eq!((t.editor.row, t.editor.col, t.profile), (1, 3, Some(p)));
    assert!(t.exec.session.is_none() && t.exec.state == SessionState::Idle);
    assert!(m.reopen().is_none());
}

#[test]
fn reopen_keeps_the_last_twenty() {
    let mut m = TabManager::new(None, ed("keep"));
    for i in 0..25 {
        let id = m.open(TabKind::Console, None, ed(&i.to_string()));
        m.close(id);
    }
    assert_eq!(m.closed_count(), REOPEN_LIMIT);
    m.reopen();
    assert_eq!(m.active().editor.text(), "24");
}

#[test]
fn recent_tab_per_profile() {
    let (p, q) = (ProfileId::new(), ProfileId::new());
    let mut m = TabManager::new(Some(p), ed("p1"));
    let p2 = m.open(TabKind::Console, Some(p), ed("p2"));
    let q1 = m.open(TabKind::Console, Some(q), ed("q1"));
    assert_eq!(m.recent_for(q), Some(q1));
    assert_eq!(m.recent_for(p), Some(p2), "the one used last");
    m.activate(0);
    m.activate(2);
    assert_eq!(m.recent_for(p), Some(m.iter().next().unwrap().id));
    m.close(q1);
    assert_eq!(m.recent_for(q), None);
    assert_eq!(m.recent_for(ProfileId::new()), None);
}

#[test]
fn generations_are_never_reused() {
    let mut m = TabManager::new(None, ed(""));
    let a = m.next_generation();
    let b = m.next_generation();
    assert!(b > a);
}

#[test]
fn tabs_by_recent_use_put_the_active_one_first_and_restored_ones_in_bar_order() {
    let mut m = TabManager::default();
    // Restored tabs are not made active one by one: the bar's order is the seed.
    let ids: Vec<TabId> = ["a", "b", "c", "d"]
        .iter()
        .map(|t| m.insert_tab(usize::MAX, Tab::new(TabId(0), TabKind::Console, None, ed(t))))
        .collect();
    m.activate(1);
    assert_eq!(m.by_recent(), [ids[1], ids[0], ids[2], ids[3]]);
    m.activate(3);
    m.cycle(1); // wraps to the first
    assert_eq!(m.by_recent(), [ids[0], ids[3], ids[1], ids[2]]);
    // Closing the active tab: its neighbour is active, the closed one is gone.
    m.close(ids[0]);
    assert_eq!(m.active().id, ids[1]);
    assert_eq!(m.by_recent(), [ids[1], ids[3], ids[2]]);
    // A tab that comes back is active.
    let back = m.reopen().unwrap();
    assert_eq!(m.by_recent(), [back, ids[1], ids[3], ids[2]]);
    assert_eq!(m.active().doc.console_no, 1, "a console comes back with its number");
    // The closed tabs, newest first, each found by its serial.
    m.close(ids[2]);
    m.close(ids[3]);
    let closed: Vec<u64> = m.closed().map(|c| c.serial).collect();
    assert_eq!(closed.len(), 2);
    let older = m.take_closed_serial(closed[1]).unwrap();
    assert_eq!(older.text, "c");
    assert_eq!(m.closed().map(|c| c.text.clone()).collect::<Vec<_>>(), ["d"]);
    assert!(m.take_closed_serial(closed[1]).is_none(), "taken once");
    // A new console skips the numbers closed consoles hold (4, "d"'s); "c"'s 3 was given back
    // when it left the list, so the new one takes it and "c" gets the lowest free one.
    let new = m.open(TabKind::Console, None, ed("e"));
    assert_eq!(m.get(new).unwrap().doc.console_no, 3);
    let back = m.reopen_closed(older);
    assert_eq!(m.get(back).unwrap().doc.console_no, 5);
}
