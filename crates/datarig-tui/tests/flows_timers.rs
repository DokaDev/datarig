//! Timers and frames: the event loop wakes only when something waits for
//! time ([`App::next_tick`]), a paging countdown wakes it once a second, a mouse move draws no
//! frame, and a large text is autosaved when typing pauses instead of every second.

mod common;

use common::*;
use datarig_core::driver::{DbEvent, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_tui::app::{AUTOSAVE, AUTOSAVE_LARGE, AppEvent, CANCEL_GRACE, EventTarget, FINE_TICK, Paging, Results};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::Duration;

/// Run the statement under the cursor and answer with a first page with more rows to come.
fn open_portal(h: &mut Harness) {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..500).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(0, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(1) });
    h.app.transient = None;
}

#[test]
fn nothing_waiting_means_no_timer() {
    let mut h = Harness::connected(Lang::En);
    h.app.transient = None;
    assert_eq!(h.app.next_tick(h.clock.now()), None);
    // A running statement counts its time in tenths.
    h.ctrl('e');
    h.app.transient = None;
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), Some(now + FINE_TICK));
}

#[test]
fn a_countdown_wakes_the_loop_once_a_second_until_the_portal_closes() {
    let mut h = Harness::connected(Lang::En);
    open_portal(&mut h);
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { .. }));
    let mut wakes = 0;
    while matches!(h.app.tab().exec.paging, Paging::Open { .. }) {
        let now = h.clock.now();
        let next = h.app.next_tick(now).expect("the countdown needs a timer");
        let wait = next - now;
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(1), "{wait:?}");
        h.clock.advance(wait);
        h.app.on_tick(h.clock.now());
        wakes += 1;
        assert!(wakes < 40, "about one wake per second of the 30s countdown");
    }
    assert!((29..=32).contains(&wakes), "{wakes}");
    assert_eq!(h.app.tab().exec.paging, Paging::ClosedIdle);
    // A countdown that is already under way wakes at its next whole second.
    let mut h = Harness::connected(Lang::En);
    open_portal(&mut h);
    h.clock.advance(Duration::from_millis(400));
    let now = h.clock.now();
    assert_eq!(h.app.next_tick(now), Some(now + Duration::from_millis(600)));
}

#[test]
fn a_mouse_move_draws_no_frame() {
    let mut h = Harness::connected(Lang::En);
    let before = h.screen(160, 45);
    let at = |kind| Event::Mouse(MouseEvent { kind, column: 80, row: 20, modifiers: KeyModifiers::NONE });
    h.app.handle_event(at(MouseEventKind::Moved));
    assert!(h.app.take_idle_event(), "nothing to draw");
    assert_eq!(h.screen(160, 45), before);
    h.app.handle_event(at(MouseEventKind::Down(MouseButton::Left)));
    assert!(!h.app.take_idle_event(), "a click may change the screen");
}

#[test]
fn a_large_text_is_saved_when_typing_pauses_and_at_most_five_seconds_late() {
    let state = std::env::temp_dir().join(format!("datarig-flows-timers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let mut h = Harness::connected(Lang::En);
    h.app.set_paths(Paths { data: None, state: Some(state.clone()) });
    let big: String = (0..40_000).map(|i| format!("SELECT {i} FROM t WHERE x = 'a long enough line';\n")).collect();
    assert!(big.len() > datarig_tui::app::LARGE_TEXT);
    h.app.tab_mut().editor = Editor::new(&big);
    h.key(KeyCode::Char('i'));
    // Typing every 300 ms: the save waits for a pause, but not longer than AUTOSAVE_LARGE.
    let start = h.clock.now();
    let mut saved_at = None;
    for _ in 0..40 {
        h.key(KeyCode::Char('x'));
        h.advance(Duration::from_millis(300));
        if saved_at.is_none() && h.app.tab().doc.save_due.is_none() {
            saved_at = Some(h.clock.now() - start);
        }
    }
    let saved_at = saved_at.expect("saved while typing went on");
    assert!(saved_at > AUTOSAVE * 2 && saved_at <= AUTOSAVE_LARGE + Duration::from_millis(300), "{saved_at:?}");
    // A pause: saved within AUTOSAVE.
    h.key(KeyCode::Esc);
    h.advance(AUTOSAVE);
    assert!(h.app.tab().doc.save_due.is_none());
    assert!(!h.app.tab().dirty());
    // A small text keeps saving at most AUTOSAVE after an edit.
    h.app.tab_mut().editor = Editor::new("SELECT 1;");
    h.key(KeyCode::Char('i'));
    h.key(KeyCode::Char('x'));
    let due = h.app.tab().doc.save_due.expect("due");
    h.advance(Duration::from_millis(300));
    h.key(KeyCode::Char('y'));
    assert_eq!(h.app.tab().doc.save_due, Some(due), "later edits do not push it back");
    let _ = std::fs::remove_dir_all(&state);
}

/// Cancel always ends a running tab, even when its session never answers (a wedged driver or
/// connection): after `CANCEL_GRACE` the run is marked cancelled, the session is closed, its
/// late events are ignored, and the next run opens a new connection.
#[test]
fn a_cancel_nobody_answers_still_ends_the_run() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let query = h.roles().iter().position(|r| *r == SessionRole::Query).expect("the tab's session");
    let (id, qid, generation) = {
        let t = h.app.tab();
        (t.id, t.exec.query_id, t.exec.generation)
    };
    h.ctrl('c');
    assert!(h.session_cancelled(query));
    h.advance(CANCEL_GRACE - Duration::from_millis(100));
    assert!(h.app.tab().exec.running.is_some(), "still waiting for the session");
    assert!(!h.session_closed(query));
    h.advance(Duration::from_millis(100));
    assert!(h.app.tab().exec.running.is_none(), "the tab no longer runs");
    assert!(matches!(h.app.tab().results, Results::Cancelled));
    assert!(h.session_closed(query), "the unanswering session is closed");
    assert!(h.status(120, 30).contains("the connection did not answer"), "{}", h.status(120, 30));
    // What the old session says late changes nothing.
    let late = DbEvent::Done {
        id: qid,
        outcome: datarig_core::driver::Outcome::Command("SELECT".into()),
        elapsed: Duration::ZERO,
    };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(id), generation, ev: late });
    assert!(matches!(h.app.tab().results, Results::Cancelled));
    // The next run connects again.
    let before = h.roles().len();
    h.ctrl('e');
    assert!(h.app.tab().exec.running.is_some());
    assert_eq!(h.roles().len(), before + 1, "a new query session");
    // A cancel that is answered in time closes nothing.
    let query = h.roles().len() - 1;
    h.tab_db(0, DbEvent::Connected);
    h.ctrl('c');
    let qid = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Failed { id: qid, error: "canceling statement".into(), cancelled: true });
    h.advance(CANCEL_GRACE * 2);
    assert!(!h.session_closed(query));
    assert!(h.app.tab().exec.session.is_some());
}
