//! Statements queued for a connection that is not up yet: a queued statement runs only in the tab, on the profile and under the tab
//! generation it was queued for, and counts as work in progress wherever the app asks before
//! ending work (changing the tab's connection, closing the tab, disconnecting or deleting the
//! profile, quitting). Both profiles get their password from a command, so the connect is
//! "slow" as long as the test holds back the command's result (`AppEvent::Resolved`), like the
//! 8-second password command of a repro; nothing connects until the test feeds
//! `Connected`.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{MemoryStore, PasswordSource};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{AppEvent, Focus, NodeState, Paging, Startup, TabId};
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

/// Next background event other than the launch-time keychain probe, or fail after a few
/// seconds.
async fn next_event(rx: &mut UnboundedReceiver<AppEvent>) -> AppEvent {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a background event")
            .expect("channel open");
        if !matches!(ev, AppEvent::KeychainProbed(_)) {
            return ev;
        }
    }
}

/// `local-pg` connected in tab 1; `slow` is another profile, not connected. Both passwords
/// come from a command.
async fn setup() -> (Harness, UnboundedReceiver<AppEvent>) {
    let profile = |name: &str, database: &str| {
        let mut p = ConnectionConfig {
            name: name.into(),
            database: database.into(),
            password: String::new(),
            ..ConnectionConfig::test_db()
        };
        p.set_source(PasswordSource::Command("printf datarig".into()));
        p
    };
    let cfg =
        Config { connections: vec![profile("local-pg", "datarig"), profile("slow", "postgres")], ..Config::default() };
    let (h, mut rx) = Harness::started(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    let mut h = h.with_fake_driver();
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    let resolved = next_event(&mut rx).await;
    h.app.on_app_event(resolved);
    h.meta_db("local-pg", DbEvent::Connected);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-pg")), "the first console runs on local-pg");
    h.app.focus = Focus::Editor;
    (h, rx)
}

fn id(h: &Harness, name: &str) -> ProfileId {
    h.app.profiles.iter().find(|p| p.name == name).unwrap().id
}

fn executes(cmds: &[DbCommand]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements.join(";")) } else { None })
        .collect()
}

/// `o` on `slow`: a console on it while its password command runs, then Ctrl+E on a
/// statement: it waits for the connection. Returns the tab and the command's result, held
/// back (the slow connect).
async fn queue_on_slow(h: &mut Harness, rx: &mut UnboundedReceiver<AppEvent>) -> (TabId, AppEvent) {
    h.explore("slow");
    h.keys("o");
    assert_eq!(h.app.conns.state(id(h, "slow")), NodeState::Connecting);
    let resolved = next_event(rx).await;
    assert!(matches!(resolved, AppEvent::Resolved { .. }), "{resolved:?}");
    h.keys("i");
    h.type_text("select 'PENDING-SLOW'");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    let tab = h.app.tab().id;
    assert!(h.app.is_queued(tab), "the statement waits for slow");
    assert!(h.app.tab_busy(tab) && h.app.any_running(), "and counts as work in progress");
    h.sent();
    (tab, resolved)
}

fn confirm_text(h: &Harness) -> Option<Msg> {
    h.app.overlays.confirm().map(|c| c.text.clone())
}

fn tab_status(h: &Harness, tab: TabId) -> Option<Msg> {
    h.app.tabs.get(tab).and_then(|t| t.status.as_ref()).map(|n| n.msg.clone())
}

/// A repro: the tab's profile is slow to connect, Ctrl+E queues the statement,
/// `Space c s` moves the tab to another (connected) profile. The switch asks first, drops the
/// statement, and when the slow profile connects nothing runs anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_statement_never_runs_on_the_connection_the_tab_switched_to() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.keys(" cs");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "the waiting statement asks first");
    assert_eq!(confirm_text(&h), Some(Msg::Label(Label::TabRebindQueued)));
    assert!(h.screen(160, 45).contains("waiting for its connection"));
    h.keys("y");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    h.type_text("local");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-pg")));
    assert!(!h.app.is_queued(tab), "the statement was dropped");
    assert_eq!(tab_status(&h, tab), Some(Msg::QueryQueuedDropped { name: "slow".into() }));
    // Eight seconds later the slow profile connects: nothing runs, on either profile.
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&h.sent()).is_empty(), "nothing ran anywhere");
    assert!(h.app.tab().exec.running.is_none() && h.app.tab().exec.session.is_none());
    // Run again: now it runs on local-pg, as the tab says.
    h.ctrl('e');
    assert_eq!(executes(&h.sent()), ["select 'PENDING-SLOW'"]);
}

/// The "not run" notice of a dropped statement stays in the status bar of its tab when the
/// profile it waited for connects later: "Connected to …" is less severe, so it only flashes.
#[tokio::test(flavor = "multi_thread")]
async fn the_dropped_statement_notice_is_not_replaced_by_the_connect() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.keys(" cs");
    h.keys("y");
    h.type_text("local");
    h.key(KeyCode::Enter);
    let dropped = Msg::QueryQueuedDropped { name: "slow".into() };
    assert_eq!(tab_status(&h, tab), Some(dropped.clone()));
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert_eq!(h.app.status.as_ref().map(|n| n.msg.clone()), Some(dropped.clone()), "the warning stays");
    let flashed = h.app.transient.as_ref().map(|(n, _)| n.msg.clone());
    assert_eq!(flashed, Some(Msg::ConnConnected { name: "slow".into() }), "the connect only flashes");
    h.app.transient = None;
    assert!(h.status(160, 45).contains("Not run: this statement waited for slow"), "{}", h.status(160, 45));
    // The tab's next outcome replaces it.
    h.ctrl('e');
    let qid = h.app.tab().exec.query_id;
    h.tab_db(
        1,
        DbEvent::Done {
            id: qid,
            outcome: datarig_core::driver::Outcome::Command("SELECT".into()),
            elapsed: Duration::from_millis(1),
        },
    );
    assert!(!h.status(160, 45).contains("Not run"), "{}", h.status(160, 45));
}

/// The connect checks the tab, the profile and the tab generation: a tab rebound meanwhile
/// (even back to the same profile) gets a notice instead of the run.
#[tokio::test(flavor = "multi_thread")]
async fn on_connect_a_queued_statement_needs_the_same_tab_profile_and_generation() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    let (slow, local) = (id(&h, "slow"), id(&h, "local-pg"));
    // Rebound behind the app's back (no confirm), then back to slow: a new tab generation.
    h.app.tabs.bind(tab, Some(local));
    h.app.tabs.bind(tab, Some(slow));
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&h.sent()).is_empty(), "nothing ran");
    assert_eq!(tab_status(&h, tab), Some(Msg::QueryQueuedDropped { name: "slow".into() }));
    assert!(h.status(160, 45).contains("Not run: this statement waited for slow"));
    // Unchanged, it runs once connected.
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    h.ctrl('e');
    assert!(h.app.is_queued(tab));
    let resolved = next_event(&mut rx).await;
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert_eq!(executes(&h.sent()), ["select 'PENDING-SLOW'"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_tab_with_a_queued_statement_asks_and_drops_it() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.app.dispatch(datarig_tui::app::action::Action::CloseTab);
    assert_eq!(confirm_text(&h), Some(Msg::Label(Label::TabCloseQueued)));
    h.keys("y");
    assert!(h.app.tabs.get(tab).is_none() && !h.app.is_queued(tab));
    assert!(h.status(160, 45).contains("the statement waiting for its connection was dropped"));
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&h.sent()).is_empty(), "nothing ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnecting_a_profile_with_a_queued_statement_asks_and_drops_it() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    assert_eq!(confirm_text(&h), Some(Msg::Label(Label::DisconnectQueued)));
    h.keys("y");
    assert!(!h.app.is_queued(tab));
    assert_eq!(h.app.conns.state(id(&h, "slow")), NodeState::Disconnected);
    assert!(h.status(160, 45).contains("the statement waiting for it was dropped"));
    // The late password of the old attempt is dropped; nothing connects, nothing ran.
    h.app.on_app_event(resolved);
    assert_eq!(h.app.conns.state(id(&h, "slow")), NodeState::Disconnected);
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&h.sent()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_profile_with_a_queued_statement_says_so_and_drops_it() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.explore("slow");
    h.keys("d");
    let screen = h.screen(160, 45);
    assert!(screen.contains("tab 2 is waiting for this connection"), "{screen}");
    h.keys("y");
    assert!(h.app.tabs.get(tab).is_none() && !h.app.is_queued(tab));
    assert!(h.status(160, 45).contains("the statement waiting for it was dropped"), "{}", h.status(160, 45));
    // The profile is gone: its late password is dropped and nothing ran.
    h.app.on_app_event(resolved);
    assert!(executes(&h.sent()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn quitting_with_a_queued_statement_asks_and_drops_it() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.command("qa");
    assert_eq!(confirm_text(&h), Some(Msg::Label(Label::QuitQueued)));
    h.keys("n");
    assert!(!h.app.quit && h.app.is_queued(tab), "n keeps it");
    h.command("qa");
    h.keys("y");
    assert!(h.app.quit, "nothing runs, so it quits at once");
    assert!(!h.app.is_queued(tab));
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&h.sent()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_queued_statement_drops_it_and_a_failed_connect_says_so() {
    let (mut h, mut rx) = setup().await;
    let (tab, resolved) = queue_on_slow(&mut h, &mut rx).await;
    h.ctrl('e');
    assert!(h.status(160, 45).contains("already running"), "one statement at a time, queued or not");
    h.app.dispatch(datarig_tui::app::action::Action::CancelQuery);
    assert!(!h.app.is_queued(tab));
    assert_eq!(tab_status(&h, tab), Some(Msg::Label(Label::QueryCancelled)));
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&h.sent()).is_empty());
    // Queued again on a connection that fails: the tab says it did not run.
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    h.ctrl('e');
    assert!(h.app.is_queued(tab));
    let resolved = next_event(&mut rx).await;
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::ConnectFailed { error: "refused".into(), auth: false });
    assert!(!h.app.is_queued(tab));
    assert_eq!(tab_status(&h, tab), Some(Msg::QueryQueuedFailed { name: "slow".into() }));
}

/// Paging (audit of the other deferred paths): after the tab moved to another connection, the
/// rows stay but reaching their end fetches nothing (the old portal is gone with its session).
#[tokio::test(flavor = "multi_thread")]
async fn a_rebound_tab_does_not_fetch_more_rows_of_its_old_result() {
    let (mut h, _rx) = setup().await;
    h.ctrl('e');
    let qid = h.app.tab().exec.query_id;
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows: Vec<Vec<Option<String>>> = (0..5).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(0, DbEvent::Page { id: qid, columns, rows, more: true, elapsed: Duration::from_millis(1) });
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { .. }));
    h.keys(" cs");
    h.type_text("slow");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "slow")));
    h.sent();
    h.key(KeyCode::Tab); // the results
    h.key(KeyCode::Char('G'));
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::FetchMore { .. })), "no fetch");
    assert!(h.app.tab().exec.running.is_none(), "and the tab is not stuck running");
}
