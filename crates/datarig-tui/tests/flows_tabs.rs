//! Workspace tabs through the real `App` event path : every tab opens its own query session on its first statement, the profile's metadata
//! session is shared, transactions and cancel stay in their tab, closing a busy tab asks and then
//! cancels and closes its session, closed tabs come back, and events of closed or replaced
//! sessions are dropped. The fake driver records every session the app opens.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent, Outcome, SessionRole};
use datarig_core::i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{AppEvent, EventTarget, Focus, SessionState};
use datarig_tui::widgets::tabbar::State;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::time::Duration;

use SessionRole::{Meta, Query};

/// Type `sql` into the active tab (Insert mode) and go back to Normal mode.
fn type_sql(h: &mut Harness, sql: &str) {
    h.keys("i");
    h.type_text(sql);
    h.key(KeyCode::Esc);
}

fn executes(cmds: &[DbCommand]) -> Vec<Vec<String>> {
    cmds.iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements.clone()),
            _ => None,
        })
        .collect()
}

fn last_query_id(h: &Harness, tab: usize) -> u64 {
    h.app.tabs.iter().nth(tab).unwrap().exec.query_id
}

fn done(id: u64) -> DbEvent {
    DbEvent::Done { id, outcome: Outcome::Command("BEGIN".into()), elapsed: Duration::from_millis(1) }
}

fn tab_line(h: &mut Harness, w: u16, hh: u16) -> String {
    h.screen(w, hh).lines().next().unwrap().to_string()
}

#[test]
fn each_tab_opens_its_own_query_session_on_its_first_statement() {
    let mut h = Harness::connected(Lang::En);
    assert_eq!(h.roles(), [Meta], "connecting opens the metadata session only");
    h.ctrl('e');
    assert_eq!(h.roles(), [Meta, Query], "the first run opens the tab's session");
    assert_eq!(executes(&h.sent_to(1)), [["SELECT * FROM shop.users WHERE id <= 8"]]);
    h.tab_db(0, DbEvent::Connected);
    assert_eq!(h.app.tab().exec.state, SessionState::Ready);

    // A second console tab: no session until it runs; then its own.
    h.ctrl('t');
    assert_eq!(h.app.tabs.len(), 2);
    assert_eq!(h.app.tabs.active_index(), 1);
    assert_eq!(h.app.tab().editor.text(), "", "a new console is empty");
    assert_eq!(h.roles(), [Meta, Query]);
    type_sql(&mut h, "select 2");
    h.ctrl('e');
    assert_eq!(h.roles(), [Meta, Query, Query]);
    assert_eq!(executes(&h.sent_to(2)), [["select 2"]]);
    assert!(h.sent_to(1).is_empty(), "nothing went to the first tab's session");
    // Tree and catalog requests go to the one metadata session.
    h.key(KeyCode::Tab); // results
    h.key(KeyCode::Tab); // explorer
    h.keys("l");
    let meta = h.sent_to(0);
    assert!(
        meta.iter()
            .all(|c| matches!(c, DbCommand::LoadObjects { .. } | DbCommand::LoadSchemas | DbCommand::LoadCatalog))
    );
    assert_eq!(h.roles().iter().filter(|r| **r == Meta).count(), 1, "one metadata session per profile");
    // Every session is named after its role and this instance.
    let names: Vec<String> =
        h.driver.sessions.lock().unwrap().iter().map(|s| s.opts.application_name.clone()).collect();
    let pid = std::process::id();
    assert_eq!(names, [format!("datarig-meta-{pid}"), format!("datarig-q-{pid}"), format!("datarig-q-{pid}")]);
}

#[test]
fn transactions_and_cancel_stay_in_their_tab() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e'); // opens session 1 for tab 1
    h.tab_db(0, done(last_query_id(&h, 0)));
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    assert!(h.status(160, 45).contains("TX open"));
    // By shape first: `◆`, not the connected `●` in another color.
    assert!(tab_line(&mut h, 160, 45).contains("console 1 ◆ ×"), "the tab shows its open transaction");
    assert_eq!(tab_state(&h, 0), State::TxOpen, "in the warning color, not the connected accent");
    h.ctrl('t');
    assert!(!h.app.tab().exec.tx_open, "the new tab has no transaction");
    assert!(!h.status(160, 45).contains("TX open"));
    // A slow statement in each tab; Ctrl+C cancels the active tab's only.
    type_sql(&mut h, "select pg_sleep(60)");
    h.ctrl('e');
    h.keys("gt"); // back to tab 1 (wraps)
    assert_eq!(h.app.tabs.active_index(), 0);
    h.ctrl('e');
    let line = tab_line(&mut h, 160, 45);
    let spinner = datarig_tui::widgets::SPINNER[0];
    assert!(line.matches(spinner).count() == 2, "both tabs run: {line}");
    assert_eq!(
        (tab_state(&h, 0), tab_state(&h, 1)),
        (State::Running, State::Running),
        "running shows over the open transaction"
    );
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the active tab's statement");
    assert!(!h.session_cancelled(2), "not the other tab's");
    // Results of one tab never land in the other.
    let other = last_query_id(&h, 1);
    h.tab_db(1, DbEvent::Failed { id: other, error: "boom".into(), cancelled: false });
    assert!(h.app.tab().exec.running.is_some(), "tab 1 still waits for its own result");
    assert!(!h.status(160, 45).contains("boom"), "a background tab's outcome is not shown here");
    h.keys("gt");
    assert!(h.status(160, 45).contains("boom"), "it is shown with its tab");
}

#[test]
fn closing_a_tab_with_an_open_transaction_asks_then_rolls_back() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.tab_db(0, done(last_query_id(&h, 0)));
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.ctrl('t');
    h.keys("gT");
    // No: nothing happens.
    h.ctrl('w');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let screen = h.screen(160, 45);
    assert!(screen.contains("Close this tab?") && screen.contains("A transaction is open in this tab"), "{screen}");
    h.keys("n");
    assert_eq!(h.app.tabs.len(), 2);
    assert!(!h.session_closed(1));
    // Yes: the session closes (the server rolls back) and the tab goes.
    h.keys(" tc");
    h.keys("y");
    assert_eq!(h.app.tabs.len(), 1);
    assert!(h.session_closed(1), "teardown closes the session");
    assert!(!h.session_cancelled(1), "nothing was running");
    assert!(h.status(160, 45).contains("Tab closed; its open transaction was rolled back"));
}

#[test]
fn closing_a_tab_mid_query_cancels_then_closes_it() {
    let mut h = Harness::connected(Lang::Ko);
    h.ctrl('e');
    h.ctrl('t');
    h.keys("gt");
    h.ctrl('w');
    let screen = h.screen(80, 24);
    assert!(screen.contains(ko(datarig_core::i18n::Label::TabCloseTitle)), "{screen}");
    // Enter keeps the tab (its × is one click away); only `y` closes it.
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), 2);
    assert!(!h.session_cancelled(1) && !h.session_closed(1), "kept");
    h.ctrl('w');
    h.keys("y");
    assert!(h.session_cancelled(1) && h.session_closed(1), "cancel, then close");
    assert_eq!(h.app.tabs.len(), 1);
    // A late result of the closed tab's session is dropped (its tab is gone).
    let stale = EventTarget::Tab(datarig_tui::app::TabId(1));
    h.app.on_app_event(AppEvent::Db { target: stale, generation: 1, ev: DbEvent::TxOpen(true) });
    assert!(!h.app.any_tx_open());
    assert!(h.status(160, 45).contains(ko(datarig_core::i18n::Label::TabClosedQuery)));
}

#[test]
fn ctrl_w_in_insert_mode_does_not_close_the_tab() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('t');
    h.keys("i");
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), 2);
    assert!(h.overlay_kind().is_none());
    h.key(KeyCode::Esc);
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), 1, "Normal mode closes it (nothing running, no question)");
}

#[test]
fn closed_tabs_come_back_and_closing_the_last_leaves_none() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('t');
    type_sql(&mut h, "select 'keep me'");
    h.ctrl('e');
    h.tab_db(1, DbEvent::Failed { id: last_query_id(&h, 1), error: "x".into(), cancelled: false });
    h.command("tabclose");
    assert_eq!(h.app.tabs.len(), 1);
    h.keys(" tu");
    assert_eq!(h.app.tabs.len(), 2);
    assert_eq!(h.app.tabs.active_index(), 1, "back where it was");
    assert_eq!(h.app.tab().editor.text(), "select 'keep me'");
    assert!(h.app.tab().exec.session.is_none(), "no session until it runs");
    h.keys(" tu");
    assert!(h.status(160, 45).contains("No closed tab to reopen"));
    // Closing the only tab leaves none (the empty state), never a console without a
    // connection; `Space t u` brings it back.
    h.command("tabclose");
    h.command("tabclose");
    assert!(h.app.tabs.is_empty());
    assert_eq!(h.app.focus, Focus::Tree);
    assert!(h.screen(160, 45).contains("No open tabs"));
    // With no tab, `:tabclose` says so; it never runs an action its letters happen to match
    // (`tab.reopen_closed`).
    h.command("tabclose");
    assert!(h.app.tabs.is_empty(), "no closed tab came back");
    assert!(h.screen(160, 45).contains(":tabclose is not available here"), "{}", h.screen(160, 45));
    h.key(KeyCode::Esc);
    h.keys(" tu");
    assert_eq!(h.app.tabs.len(), 1);
    assert_eq!(h.app.tab().editor.text(), SAMPLE_SQL);
}

#[test]
fn tab_keys_move_between_tabs() {
    let mut h = Harness::connected(Lang::En);
    h.command("tabnew");
    h.ctrl('t');
    assert_eq!(h.app.tabs.active_index(), 2);
    h.keys(" 1");
    assert_eq!(h.app.tabs.active_index(), 0);
    h.keys(" 9");
    assert_eq!(h.app.tabs.active_index(), 0, "no ninth tab: nothing happens");
    h.keys("gt");
    assert_eq!(h.app.tabs.active_index(), 1);
    h.keys("[");
    assert_eq!(h.app.tabs.active_index(), 1, "in the editor `[` is vim's bracket motion");
    h.key(KeyCode::F(6)); // explorer (no results pane before a run)
    // `[` and `]` are no tab keys anywhere (they were vim's
    // motions in the editor and tab keys elsewhere); `g T` / `g t` switch.
    h.keys("[");
    h.keys("]");
    assert_eq!(h.app.tabs.active_index(), 1, "`[` / `]` do not switch tabs");
    h.keys("gT");
    assert_eq!(h.app.tabs.active_index(), 0);
    h.key(KeyCode::F(6)); // editor again
    h.key_mod(KeyCode::PageUp, KeyModifiers::CONTROL);
    assert_eq!(h.app.tabs.active_index(), 2, "wraps");
    // Ctrl+PageDown works in Insert mode too (protected key).
    h.keys("i");
    h.key_mod(KeyCode::PageDown, KeyModifiers::CONTROL);
    assert_eq!(h.app.tabs.active_index(), 0);
    // Korean (2-Set) input source: the jamo U+314E U+3145 are the keys `g t`.
    h.key(KeyCode::Esc);
    h.type_text("\u{314E}\u{3145}");
    assert_eq!(h.app.tabs.active_index(), 1);
    // F6 moves between panes (Insert mode types Tab); no results pane before a run.
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Editor);
    h.key(KeyCode::F(6));
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Tree);
}

#[test]
fn a_lost_tab_reconnects_on_its_next_run() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let id = last_query_id(&h, 0);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    let old_gen = h.app.tab().exec.generation;
    // The server ended the connection mid-statement: Failed first, then Lost.
    h.tab_db(0, DbEvent::Failed { id, error: "server closed the connection".into(), cancelled: false });
    h.tab_db(0, DbEvent::Lost { error: "server closed the connection".into() });
    assert_eq!(h.app.tab().exec.state, SessionState::Lost);
    assert!(!h.app.tab().exec.tx_open && h.app.tab().exec.session.is_none());
    assert!(tab_line(&mut h, 160, 45).contains("console 1 ! ×"), "the warning mark (icons off: never an emoji)");
    assert_eq!(tab_state(&h, 0), State::Trouble);
    assert!(h.status(160, 45).contains("This tab's connection was lost"));
    // The next run opens a new session and says the transaction is gone.
    h.ctrl('e');
    assert_eq!(h.roles(), [Meta, Query, Query]);
    assert!(h.status(160, 45).contains("Reconnected; the transaction that was open is gone"));
    assert!(!tab_line(&mut h, 160, 45).contains('⚠'));
    // Events of the lost session's generation are dropped.
    let target = EventTarget::Tab(h.app.tab().id);
    h.app.on_app_event(AppEvent::Db { target, generation: old_gen, ev: DbEvent::TxOpen(true) });
    assert!(!h.app.tab().exec.tx_open);
    h.app.on_app_event(AppEvent::Db { target, generation: h.app.tab().exec.generation, ev: DbEvent::TxOpen(true) });
    assert!(h.app.tab().exec.tx_open, "the new session's events count");
}

#[test]
fn a_lost_metadata_session_reopens_with_the_next_statement() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Lost { error: "terminated".into() });
    assert!(h.app.current_conn().is_none_or(|c| !c.connected && c.meta.is_none()));
    assert!(h.status(160, 45).contains("Connection to local-pg lost: terminated"));
    h.ctrl('e');
    assert_eq!(h.roles(), [Meta, Meta, Query], "the metadata session reopens once, then the tab's");
}

#[test]
fn quitting_asks_when_any_tab_has_work_and_closes_every_session() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.tab_db(0, done(last_query_id(&h, 0)));
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.ctrl('t');
    type_sql(&mut h, "select 1");
    h.ctrl('e');
    h.tab_db(1, DbEvent::Failed { id: last_query_id(&h, 1), error: "x".into(), cancelled: false });
    // The open transaction is in the other tab: still asked.
    h.ctrl('q');
    assert!(h.screen(160, 45).contains("A transaction is open"));
    h.keys("y");
    assert!(h.app.quit);
    assert!(h.session_closed(0) && h.session_closed(1) && h.session_closed(2), "every session closed");
}

#[test]
fn disconnecting_closes_every_tab_session_and_keeps_the_tabs() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.tab_db(0, DbEvent::Failed { id: last_query_id(&h, 0), error: "x".into(), cancelled: false });
    h.ctrl('t');
    h.keys(" cx"); // disconnect this tab's connection
    assert!(h.overlay_kind().is_none(), "nothing at risk: no question");
    assert!(h.session_closed(0) && h.session_closed(1));
    assert_eq!(h.app.tabs.len(), 2, "the tabs stay");
    assert!(h.app.current_conn().is_some_and(|c| !c.connected && c.meta.is_none()));
    assert!(tab_line(&mut h, 160, 45).contains("○"), "{}", tab_line(&mut h, 160, 45));
}

/// Disconnecting retires every tab session of the profile: a late event of a closed session
/// (here the `TxOpen(true)` of a `BEGIN` that finished after the disconnect) is dropped instead
/// of marking the tab as holding a transaction.
#[test]
fn events_of_sessions_closed_by_a_disconnect_are_dropped() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let target = EventTarget::Tab(h.app.tab().id);
    let old_gen = h.app.tab().exec.generation;
    let id = last_query_id(&h, 0);
    h.keys(" cx");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "a statement runs: asked first");
    assert!(h.screen(160, 45).contains("Disconnect?"));
    h.keys("y");
    assert!(h.session_closed(1));
    h.app.on_app_event(AppEvent::Db { target, generation: old_gen, ev: done(id) });
    h.app.on_app_event(AppEvent::Db { target, generation: old_gen, ev: DbEvent::TxOpen(true) });
    assert!(!h.app.tab().exec.tx_open && !h.app.any_tx_open(), "a closed session's transaction is gone");
    assert!(h.app.tab().exec.running.is_none());
}

/// The screen column where `needle` starts on row `y` of a drawn frame (cell by cell, so wide
/// letters count twice).
fn column_of(h: &mut Harness, w: u16, rows: u16, y: u16, needle: &str) -> Option<u16> {
    let t = h.draw(w, rows);
    let buf = t.backend().buffer();
    let cells: Vec<(u16, String)> = (0..w).map(|x| (x, buf[(x, y)].symbol().to_string())).collect();
    (0..cells.len()).find_map(|i| {
        let mut s = String::new();
        for (_, sym) in &cells[i..] {
            if s.len() >= needle.len() {
                break;
            }
            s.push_str(sym);
        }
        s.starts_with(needle).then_some(cells[i].0)
    })
}

/// The document tab bar as drawn (right of the explorer) and the column it starts at.
fn tab_bar(h: &mut Harness, w: u16, rows: u16) -> (String, u16) {
    let t = h.draw(w, rows);
    let x0 = h.app.layout.tree.x + h.app.layout.tree.width;
    let buf = t.backend().buffer();
    ((x0..w).map(|x| buf[(x, 0)].symbol().to_string()).collect(), x0)
}

/// The label numbers of the tabs the bar shows (each tab ends with its `×`; a clipped last one
/// may not).
fn shown_tabs(bar: &str) -> Vec<usize> {
    bar.trim_start_matches('‹')
        .split('×')
        .filter_map(|chunk| chunk.split_whitespace().next().and_then(|w| w.parse().ok()))
        .collect()
}

/// The state mark tab `index` shows.
fn tab_state(h: &Harness, index: usize) -> State {
    datarig_tui::widgets::tabbar::state(&h.app, h.app.tabs.iter().nth(index).unwrap())
}

/// A click on the document tab bar activates the tab under the
/// pointer, as drawn, also when the bar scrolls and clips; the scroll marks go to the next
/// hidden tab on their side; a middle click closes the tab. Failed on `98a6840` (clicks on the
/// bar did nothing).
#[test]
fn a_click_on_the_tab_bar_activates_that_tab() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = Harness::connected(Lang::En);
    h.command("tabnew");
    h.command("tabnew");
    assert_eq!(h.app.tabs.active_index(), 2);
    h.key(KeyCode::F(6));
    let focus = h.app.focus;
    let x = column_of(&mut h, 120, 40, 0, "1 console").expect("tab 1 drawn");
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 3, 0);
    assert_eq!(h.app.tabs.active_index(), 0, "{}", h.screen(120, 40));
    assert_eq!(h.app.focus, focus, "a tab click keeps the focus");
    let x = column_of(&mut h, 120, 40, 0, "2 console").expect("tab 2 drawn");
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, 0);
    assert_eq!(h.app.tabs.active_index(), 1);
    // Many tabs at 80 columns: the bar scrolls; `›` goes to the first hidden tab on the right.
    for _ in 0..9 {
        h.command("tabnew");
    }
    h.keys(" 1");
    assert_eq!(h.app.tabs.active_index(), 0);
    let (bar, _) = tab_bar(&mut h, 80, 24);
    assert!(bar.trim_end().ends_with('›'), "{bar}");
    let right = shown_tabs(&bar).last().copied().unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), 79, 0);
    assert_eq!(h.app.tabs.active_index(), right, "the tab after {right} (index {right}): {bar}");
    let (bar, x0) = tab_bar(&mut h, 80, 24);
    assert!(bar.starts_with('‹'), "{bar}");
    let left = shown_tabs(&bar)[0];
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0, 0);
    assert_eq!(h.app.tabs.active_index(), left - 2, "the tab before {left}: {bar}");
    // A clipped tab at the right edge is clicked where it is drawn.
    let (bar, x0) = tab_bar(&mut h, 80, 24);
    let clipped = bar.trim_end_matches(['›', ' ']).rsplit("  ").next().unwrap().trim().to_string();
    let n: usize = clipped.split_whitespace().next().unwrap().parse().unwrap();
    let x = column_of(&mut h, 80, 24, 0, &clipped).unwrap().max(x0) + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, 0);
    assert_eq!(h.app.tabs.active_index(), n - 1, "{bar}");
    // A middle click closes the tab under the pointer (nothing to lose: no question).
    let count = h.app.tabs.len();
    h.keys(" 1");
    // Wide enough for every name to be whole (at 120 columns twelve tabs shorten them).
    let x = column_of(&mut h, 300, 40, 0, "2 console").unwrap();
    let second = h.app.tabs.iter().nth(1).unwrap().id;
    h.mouse(MouseEventKind::Down(MouseButton::Middle), x, 0);
    assert_eq!(h.app.tabs.len(), count - 1);
    assert!(h.app.tabs.get(second).is_none(), "the tab under the pointer closed");
}

/// The state mark of a tab, by shape first and in theme colors (never red): `○`
/// dim with no session, `●` accent once connected, the spinner (accent) while a statement
/// runs, `◆` warning while the user's transaction is open, `!` warm for an aborted
/// transaction or a lost connection.
#[test]
fn the_state_mark_says_what_the_tabs_session_does() {
    use datarig_tui::theme;
    let mut h = Harness::connected(Lang::En);
    let mark = |h: &mut Harness| {
        let t = h.draw(160, 45);
        let buf = t.backend().buffer();
        let close = (0..160u16).find(|x| buf[(*x, 0)].symbol() == "×").expect("the close button");
        let cell = &buf[(close - 2, 0)];
        (cell.symbol().to_string(), cell.fg)
    };
    assert_eq!(mark(&mut h), ("○".to_string(), theme::DARK.fg_dim), "no session yet");
    h.ctrl('e');
    assert_eq!(mark(&mut h), (datarig_tui::widgets::SPINNER[0].to_string(), theme::DARK.accent), "running");
    h.tab_db(0, done(last_query_id(&h, 0)));
    assert_eq!(mark(&mut h), ("●".to_string(), theme::DARK.accent), "connected");
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    assert_eq!(mark(&mut h), ("◆".to_string(), theme::DARK.warning), "a transaction is open: a shape of its own");
    h.tab_db(0, DbEvent::TxAborted(true));
    assert_eq!(mark(&mut h), ("!".to_string(), theme::DARK.accent_warm), "aborted: ROLLBACK required");
    h.tab_db(0, DbEvent::Lost { error: "gone".into() });
    assert_eq!(mark(&mut h), ("!".to_string(), theme::DARK.accent_warm), "lost");
    // Icons off: the same marks (plain Unicode, not Nerd Font glyphs).
    h.app.icons = datarig_core::config::IconsSetting::Off;
    assert_eq!(mark(&mut h).0, "!");
}

/// A click on a tab's `×` closes it as `Ctrl+W` does: at once when nothing would
/// be lost, else after asking (Enter keeps the tab, `y` closes it). A click on the rest of
/// the tab only activates it.
#[test]
fn the_close_button_closes_as_ctrl_w_does() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = Harness::connected(Lang::En);
    h.command("tabnew");
    h.command("tabnew");
    let closes = |h: &mut Harness| -> Vec<u16> {
        let t = h.draw(160, 45);
        let buf = t.backend().buffer();
        (0..160u16).filter(|x| buf[(*x, 0)].symbol() == "×").collect()
    };
    // The body of tab 1: activated, nothing closes.
    let x = column_of(&mut h, 160, 45, 0, "1 console").unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 4, 0);
    assert_eq!((h.app.tabs.active_index(), h.app.tabs.len()), (0, 3));
    // The × of tab 3 (clean): closed at once.
    let third = h.app.tabs.iter().nth(2).unwrap().id;
    let x = closes(&mut h)[2];
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, 0);
    assert_eq!(h.app.tabs.len(), 2);
    assert!(h.app.tabs.get(third).is_none());
    assert_eq!(h.overlay_kind(), None, "nothing to ask");
    // Tab 1 runs a statement: its × asks; Enter keeps it, `y` closes it.
    h.keys(" 1");
    h.ctrl('e');
    let x = closes(&mut h)[0];
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, 0);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), 2, "Enter keeps the tab");
    let x = closes(&mut h)[0];
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, 0);
    h.keys("y");
    assert_eq!(h.app.tabs.len(), 1);
    assert!(h.session_cancelled(1), "its statement was cancelled");
}
