//! Key event filtering and normalization (kitty keyboard protocol vs legacy terminals).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Terminal key event normalized for the app. With the kitty keyboard protocol some terminals
/// report Shift+Tab as `Tab` + SHIFT; legacy terminals send `BackTab`.
pub(crate) fn normalize(mut k: KeyEvent) -> KeyEvent {
    if k.code == KeyCode::Tab && k.modifiers.contains(KeyModifiers::SHIFT) {
        k.code = KeyCode::BackTab;
    }
    k
}

/// How a key event may be used. Release events (kitty `REPORT_EVENT_TYPES`) never act;
/// auto-repeat may move the cursor or type, but never re-triggers an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyUse {
    Ignore,
    Press,
    Repeat,
}

pub(crate) fn key_use(k: &KeyEvent) -> KeyUse {
    match k.kind {
        KeyEventKind::Press => KeyUse::Press,
        KeyEventKind::Repeat => KeyUse::Repeat,
        KeyEventKind::Release => KeyUse::Ignore,
    }
}

#[cfg(test)]
mod tests;
