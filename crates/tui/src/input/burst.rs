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

/// The rest of a burst of `kind` whose first event was handled: the events `next` has ready
/// (`None`: nothing more for now) are handed to `handle` while they go on with the burst, at
/// most [`PER_FRAME`] events in all with the first. Returns the first event read that does not
/// (it is the next turn's). The event loop draws once after this.
pub fn take_burst(
    kind: Burst,
    mut next: impl FnMut() -> Option<Event>,
    mut handle: impl FnMut(Event),
) -> Option<Event> {
    for _ in 1..PER_FRAME {
        let ev = next()?;
        if burst(&ev) != Some(kind) {
            return Some(ev);
        }
        handle(ev);
    }
    None
}

/// How many frames the event loop draws for `events`, all read at once ([`take_burst`]): one
/// per burst of at most [`PER_FRAME`] events, one per other event.
pub fn frames(events: &[Event]) -> usize {
    let mut it = events.iter().cloned();
    let (mut frames, mut pending) = (0, None);
    while let Some(ev) = pending.take().or_else(|| it.next()) {
        frames += 1;
        if let Some(kind) = burst(&ev) {
            pending = take_burst(kind, || it.next(), |_| {});
        }
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

    /// The event loop's grouping: the wheel events of a burst are handed over, the first other
    /// event comes back for the next turn, nothing more is read once none is ready.
    #[test]
    fn take_burst_hands_over_the_burst_and_returns_what_ends_it() {
        let right = mouse(MouseEventKind::ScrollRight, KeyModifiers::NONE);
        let shift_down = mouse(MouseEventKind::ScrollDown, KeyModifiers::SHIFT);
        let moved = mouse(MouseEventKind::Moved, KeyModifiers::NONE);
        let mut queue = vec![right.clone(), right.clone(), shift_down.clone(), right.clone()].into_iter();
        let mut handled = Vec::new();
        let rest = take_burst(
            Burst::Wheel(MouseEventKind::ScrollRight, KeyModifiers::NONE),
            || queue.next(),
            |e| handled.push(e),
        );
        assert_eq!(handled, [right.clone(), right.clone()]);
        assert_eq!(rest, Some(shift_down.clone()));
        assert_eq!(queue.next(), Some(right.clone()), "nothing read past the event that ended it");
        // Nothing ready: the burst ends there.
        let mut handled = 0;
        assert_eq!(take_burst(Burst::Motion, || None, |_| handled += 1), None);
        assert_eq!(handled, 0);
        // Motions mix drags and moves; at most PER_FRAME events in all.
        let drag = mouse(MouseEventKind::Drag(MouseButton::Left), KeyModifiers::NONE);
        let mut motions = std::iter::repeat_n([drag, moved], PER_FRAME).flatten();
        let mut n = 1;
        assert_eq!(take_burst(Burst::Motion, || motions.next(), |_| n += 1), None);
        assert_eq!(n, PER_FRAME);
    }
}
