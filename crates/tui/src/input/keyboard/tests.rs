use super::*;
use ratatui::crossterm::event::KeyEventState;

fn ev(code: KeyCode, m: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
    KeyEvent { code, modifiers: m, kind, state: KeyEventState::NONE }
}

#[test]
fn enhanced_key_events_are_filtered_and_normalized() {
    let ctrl = KeyModifiers::CONTROL;
    assert_eq!(key_use(&ev(KeyCode::Enter, ctrl, KeyEventKind::Release)), KeyUse::Ignore);
    assert_eq!(key_use(&ev(KeyCode::Enter, ctrl, KeyEventKind::Repeat)), KeyUse::Repeat);
    assert_eq!(key_use(&ev(KeyCode::Char('\u{D55C}'), KeyModifiers::NONE, KeyEventKind::Press)), KeyUse::Press);
    let shift_tab = normalize(ev(KeyCode::Tab, KeyModifiers::SHIFT, KeyEventKind::Press));
    assert_eq!(shift_tab.code, KeyCode::BackTab);
    let tab = normalize(ev(KeyCode::Tab, KeyModifiers::NONE, KeyEventKind::Press));
    assert_eq!(tab.code, KeyCode::Tab);
}
