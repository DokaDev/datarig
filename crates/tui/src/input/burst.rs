//! Bursts of input events drawn as one frame. A terminal sends pointer motions in bursts (over
//! SSH), a trackpad wheel events in bursts (dozens a swipe): the event loop hands the events
//! already read of a burst to the app one by one and draws once, after the last.

use ratatui::crossterm::event::{Event, KeyModifiers, MouseEventKind};

/// The most events of a burst handled before a frame is drawn.
pub const PER_FRAME: usize = 64;

/// What a burst is made of: events of one burst are handled together, any other event ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Burst {
    /// Pointer motions: drags and moves without a button.
    Motion,
    /// Wheel events of one direction, with the same modifier keys.
    Wheel(MouseEventKind, KeyModifiers),
}

/// The burst event `ev` starts or goes on with, if it is one that comes in bursts.
pub fn burst(ev: &Event) -> Option<Burst> {
    let Event::Mouse(m) = ev else { return None };
    match m.kind {
        MouseEventKind::Drag(_) | MouseEventKind::Moved => Some(Burst::Motion),
        MouseEventKind::ScrollUp
        | MouseEventKind::ScrollDown
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => Some(Burst::Wheel(m.kind, m.modifiers)),
        _ => None,
    }
}

/// How many frames the event loop draws for `events`, all read at once: one per burst of at
/// most [`PER_FRAME`] events, one per other event.
pub fn frames(events: &[Event]) -> usize {
    let mut frames = 0;
    let mut i = 0;
    while i < events.len() {
        let kind = burst(&events[i]);
        let mut n = 1;
        while kind.is_some() && n < PER_FRAME && events.get(i + n).is_some_and(|e| burst(e) == kind) {
            n += 1;
        }
        frames += 1;
        i += n;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent};

    fn mouse(kind: MouseEventKind, modifiers: KeyModifiers) -> Event {
        Event::Mouse(MouseEvent { kind, column: 10, row: 10, modifiers })
    }

    #[test]
    fn wheel_events_of_one_kind_make_one_burst() {
        let right = mouse(MouseEventKind::ScrollRight, KeyModifiers::NONE);
        let left = mouse(MouseEventKind::ScrollLeft, KeyModifiers::NONE);
        let shift_down = mouse(MouseEventKind::ScrollDown, KeyModifiers::SHIFT);
        let down = mouse(MouseEventKind::ScrollDown, KeyModifiers::NONE);
        let key = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(burst(&right), Some(Burst::Wheel(MouseEventKind::ScrollRight, KeyModifiers::NONE)));
        assert_eq!(burst(&key), None);
        assert_eq!(burst(&mouse(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE)), None);
        assert_eq!(burst(&mouse(MouseEventKind::Moved, KeyModifiers::NONE)), Some(Burst::Motion));
        let many = |n| std::iter::repeat_n(right.clone(), n).collect::<Vec<_>>();
        assert_eq!(frames(&many(30)), 1);
        // Bounded: a long burst is drawn every PER_FRAME events.
        assert_eq!(frames(&many(PER_FRAME * 2 + 1)), 3);
        // Another direction, other modifiers or another event end a burst.
        assert_eq!(frames(&[right.clone(), right.clone(), left.clone(), left]), 2);
        assert_eq!(frames(&[shift_down.clone(), shift_down, down.clone(), down]), 2);
        assert_eq!(frames(&[right.clone(), key.clone(), key, right]), 4);
    }
}
