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
