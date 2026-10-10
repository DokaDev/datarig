//! The explorer and the multi-connection workspace through the real `App` event path: several profiles connected at
//! once, the console a first connect opens, tables opened in their own profile's tab, quick
//! connect, the password prompt queue, and tabs without a connection. A fake driver records the
//! commands of every session.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, SessionContext, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{Focus, Keys, NodeState, Startup};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::time::Duration;

/// Two more profiles on top of the connected `local-pg`: `other` and `third`.
fn three_profiles() -> Harness {
    let mut h = Harness::connected(Lang::En);
    for name in ["other", "third"] {
        h.app.profiles.push(ConnectionConfig { name: name.into(), port: 6543, ..ConnectionConfig::test_db() });
    }
    h
}

fn id(h: &Harness, name: &str) -> ProfileId {
    h.app.profiles.iter().find(|p| p.name == name).unwrap().id
}

/// Index (in opening order) of the session with `role` opened last.
fn last_session(h: &Harness, role: SessionRole) -> usize {
    h.roles().iter().rposition(|r| *r == role).expect("a session with that role")
}

fn executes(cmds: &[DbCommand]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements.join(";")) } else { None })
        .collect()
}

/// Connect profile `name` from the explorer (Enter on its node) and answer `Connected`.
fn connect(h: &mut Harness, name: &str) {
    h.explore(name);
    h.key(KeyCode::Enter);
    h.meta_db(name, DbEvent::Connected);
}

#[test]
fn a_first_connect_opens_a_console_and_keeps_the_focus() {
    let mut h = three_profiles();
    connect(&mut h, "other");
    assert!(h.app.conns.is_connected(id(&h, "other")) && h.app.conns.is_connected(id(&h, "local-pg")));
    assert_eq!(h.app.tabs.len(), 2, "a console for other");
    assert_eq!(h.app.tab().profile, Some(id(&h, "other")), "and it is the active tab");
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
    assert!(h.rows().contains(&"other".to_string()));
    // Its node is open: its own database first, the schemas under it once they
    // arrive, then the server's other databases, asked for when it opened.
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)), "the databases are asked for");
    h.meta_db("other", DbEvent::Schemas(Ok(vec!["public".into()])));
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "other").unwrap();
    assert!(rows[at + 1].starts_with("  db:") && rows[at + 1].ends_with('*'), "{rows:?}");
    assert_eq!(rows[at + 2], "    public", "{rows:?}");
    // A profile that has a tab opens no other one when it connects again.
    h.keys("x");
    assert_eq!(h.app.conns.state(id(&h, "other")), NodeState::Disconnected);
    assert_eq!(h.app.tabs.len(), 2, "its tab stays");
    h.key(KeyCode::Enter);
    h.meta_db("other", DbEvent::Connected);
    assert_eq!(h.app.tabs.len(), 2);
    // `o` always opens a new console, focused.
    h.keys("o");
    assert_eq!((h.app.tabs.len(), h.app.focus), (3, Focus::Editor));
    assert_eq!(h.app.tab().profile, Some(id(&h, "other")));
}

/// Enter on a table of profile B runs in B's tab even while A's tab is active, and
/// nothing reaches A's session.
#[test]
fn opening_a_table_runs_in_its_own_profiles_tab() {
    let mut h = three_profiles();
    connect(&mut h, "other");
    h.meta_db("other", DbEvent::Schemas(Ok(vec!["shop".into()])));
    // local-pg's tab runs something, so it has a session of its own.
    h.app.dispatch(datarig_tui::app::action::Action::GotoTab(1));
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-pg")));
    h.ctrl('e');
    let local_q = last_session(&h, SessionRole::Query);
    h.sent();
    // other's shop.users, from the explorer.
    h.explore("other");
    h.keys("jj"); // its database, shop
    h.key(KeyCode::Enter); // shop: load its objects
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "shop")));
    h.meta_db(
        "other",
        DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) },
    );
    h.keys("jj"); // Tables, users
    assert!(h.rows().contains(&"        users".to_string()), "{:?}", h.rows());
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "other")), "other's tab is active now");
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
    let other_q = last_session(&h, SessionRole::Query);
    assert_ne!(other_q, local_q, "other's tab opened its own session");
    assert_eq!(executes(&h.sent_to(other_q)), [r#"SELECT * FROM "shop"."users""#]);
    assert!(executes(&h.sent_to(local_q)).is_empty(), "nothing ran in local-pg's tab");
    // Without a tab of its own, the profile gets a new console (not the active tab of another).
    h.app.dispatch(datarig_tui::app::action::Action::GotoTab(1));
    let tabs = h.app.tabs.len();
    connect(&mut h, "third");
    assert_eq!(h.app.tabs.len(), tabs + 1, "its first connect opened a console");
    h.app.dispatch(datarig_tui::app::action::Action::CloseTab);
    assert!(h.app.tabs.iter().all(|t| t.profile != Some(id(&h, "third"))));
    h.app.dispatch(datarig_tui::app::action::Action::GotoTab(1));
    h.meta_db("third", DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.meta_db(
        "third",
        DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) },
    );
    h.explore("third");
    h.keys("jj");
    h.key(KeyCode::Enter);
    h.keys("jj");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), tabs + 1, "a new console for third");
    assert_eq!(h.app.tab().profile, Some(id(&h, "third")));
    let third_q = last_session(&h, SessionRole::Query);
    assert_eq!(executes(&h.sent_to(third_q)), [r#"SELECT * FROM "shop"."users""#]);
    assert!(executes(&h.sent_to(local_q)).is_empty());
}

#[test]
fn quick_connect_goes_to_a_tab_or_opens_a_console() {
    let mut h = three_profiles();
    // No tab for `third`: connect it and open a focused console.
    h.ctrl('o');
    assert_eq!(h.app.key_context(), Ctx::QuickConnect);
    h.type_text("thi");
    assert_eq!(h.app.overlays.quick().unwrap().items, [datarig_tui::app::quick::QuickRow::Profile(id(&h, "third"))]);
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.quick().is_none());
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("third"));
    assert_eq!(h.app.tabs.len(), 1, "the console opens once it is connected");
    h.meta_db("third", DbEvent::Connected);
    assert_eq!((h.app.tabs.len(), h.app.focus), (2, Focus::Editor));
    assert_eq!(h.app.tab().profile, Some(id(&h, "third")));
    // A profile with a tab: that tab, no new one.
    h.ctrl('o');
    h.type_text("local");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), 2);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-pg")));
    // Esc closes it without doing anything; ↑↓ move.
    h.ctrl('o');
    h.key(KeyCode::Down);
    assert_eq!(h.app.overlays.quick().unwrap().selected, 1);
    h.key(KeyCode::Esc);
    assert!(h.overlay_kind().is_none());
}

#[test]
fn deleting_a_profile_closes_its_tabs_after_saying_so() {
    let mut h = three_profiles();
    connect(&mut h, "other");
    h.keys("o");
    assert_eq!(h.app.tabs.len(), 3, "two tabs on other");
    h.explore("other");
    h.keys("d");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Its 2 open tabs will close."), "{screen}");
    h.keys("y");
    assert!(h.session_closed(1), "its metadata session closed");
    assert!(h.app.profiles.iter().all(|p| p.name != "other"));
    assert_eq!(h.app.tabs.len(), 1, "its tabs closed");
    assert!(h.app.tabs.iter().all(|t| t.profile == Some(id(&h, "local-pg"))));
    assert!(h.status(160, 45).contains("Deleted other; 2 tabs closed"));
    // Deleting the profile of the only tab leaves no tab (never a console without a
    // connection): the workspace shows its empty state.
    h.explore("local-pg");
    h.keys("dy");
    assert!(h.app.tabs.is_empty());
    assert_eq!(h.app.focus, Focus::Tree);
    assert!(h.screen(160, 45).contains("No open tabs"));
}

/// A first connect takes over only a console nobody used. A repro: a tab
/// whose profile was deleted, brought back with `Space t u` (unbound, with its text), is not
/// rebound silently when another profile connects; the profile gets a new console instead.
#[test]
fn a_first_connect_never_takes_over_a_used_tab_without_a_connection() {
    let mut h = three_profiles();
    connect(&mut h, "other");
    assert_eq!(h.app.tabs.len(), 2, "other's console");
    h.app.focus = Focus::Editor;
    h.keys("i");
    h.type_text("select 'OLD-OTHER-TEXT'");
    h.key(KeyCode::Esc);
    h.explore("other");
    h.keys("dy");
    assert_eq!(h.app.tabs.len(), 1);
    h.keys(" tu");
    let reopened = h.app.tab().id;
    assert_eq!(h.app.tab().profile, None, "back without a connection");
    assert!(h.app.tab().editor.text().contains("OLD-OTHER-TEXT"));
    connect(&mut h, "third");
    assert_eq!(h.app.tabs.get(reopened).map(|t| t.profile), Some(None), "the reopened tab stays unbound");
    assert_eq!(h.app.tabs.len(), 3, "third got a new console");
    assert_eq!(h.app.tab().profile, Some(id(&h, "third")));
    assert!(h.app.tab().editor.text().is_empty());
}

/// A fresh workspace has no tab (no console without a connection): its empty state says how
/// to open one, the focus stays in the explorer, and a first connect opens the profile's
/// console.
#[test]
fn a_fresh_workspace_has_no_tab_until_a_first_connect_opens_a_console() {
    let mut h = launched(&sample_config(None)).with_fake_driver();
    assert!(h.app.tabs.is_empty());
    let screen = h.screen(80, 24);
    assert!(screen.contains("No open tabs") && screen.contains("Pick a connection in the explorer"), "{screen}");
    assert!(screen.contains("Ctrl+O     Quick connect"), "{screen}");
    // Nothing else to focus, and the keys of a tab do nothing.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.ctrl('e');
    h.ctrl('w');
    h.keys(" 1gt");
    assert!(h.app.tabs.is_empty() && h.overlay_kind().is_none());
    h.explore("v6");
    h.key(KeyCode::Enter);
    h.meta_db("v6", DbEvent::Connected);
    assert_eq!((h.app.tabs.len(), h.app.tab().profile), (1, Some(id(&h, "v6"))));
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
}

/// The level and text of the notice the status bar flashes.
fn flashed(h: &Harness) -> Option<(datarig_tui::app::Level, String)> {
    h.app.transient.as_ref().map(|(n, _)| (n.level, n.render(&h.app.i18n).to_string()))
}

/// Deleting a connected profile lists, tab by tab, the running query it
/// cancels; the notice afterwards says so at warning level.
#[test]
fn deleting_a_profile_with_a_running_query_lists_it_and_says_it_was_cancelled() {
    let mut h = three_profiles();
    connect(&mut h, "other");
    h.app.focus = Focus::Editor;
    h.keys("i");
    h.type_text("select pg_sleep(60)");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    let q = last_session(&h, SessionRole::Query);
    assert!(h.app.tab().exec.running.is_some());
    h.explore("other");
    h.keys("o"); // a second, idle tab of other
    h.explore("other");
    h.keys("d");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Its 2 open tabs will close."), "{screen}");
    assert!(screen.contains("A query is still running in tab 2. Deleting cancels it;"), "{screen}");
    assert!(!screen.contains("tab 3"), "the idle tab has nothing to lose: {screen}");
    h.keys("y");
    assert!(h.session_cancelled(q), "the query was cancelled");
    assert!(h.session_closed(q));
    let (level, text) = flashed(&h).expect("a notice");
    assert_eq!(level, datarig_tui::app::Level::Warning);
    assert_eq!(text, "Deleted other; its 2 tabs closed and their running queries were cancelled");
}

/// The same for an open transaction: listed per tab, rolled back (the session closes), said.
#[test]
fn deleting_a_profile_with_an_open_transaction_lists_it_and_says_it_was_rolled_back() {
    let mut h = three_profiles();
    connect(&mut h, "other");
    h.app.focus = Focus::Editor;
    h.keys("i");
    h.type_text("begin");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    let q = last_session(&h, SessionRole::Query);
    let qid = h.app.tab().exec.query_id;
    h.tab_db(1, DbEvent::TxOpen(true));
    h.tab_db(
        1,
        DbEvent::Done {
            id: qid,
            outcome: datarig_core::driver::Outcome::Command("BEGIN".into()),
            elapsed: std::time::Duration::from_millis(1),
        },
    );
    assert!(h.app.tab().exec.tx_open && h.app.tab().exec.running.is_none());
    h.explore("other");
    h.keys("d");
    let screen = h.screen(160, 45);
    assert!(screen.contains("A transaction is open in tab 2. Deleting rolls it back;"), "{screen}");
    assert!(screen.contains("Its open tab will close."), "{screen}");
    h.keys("y");
    assert!(h.session_closed(q), "the session closed, so the server rolled back");
    let (level, text) = flashed(&h).expect("a notice");
    assert_eq!(level, datarig_tui::app::Level::Warning);
    assert_eq!(text, "Deleted other; its tab closed and its open transaction was rolled back");
}

/// The delete confirmation names the password store only for a source that stores one.
#[test]
fn the_delete_confirmation_mentions_the_saved_password_per_source() {
    use datarig_core::secret::PasswordSource;
    let mut h = three_profiles();
    let cases = [
        (PasswordSource::Keychain, Some("Its password saved in the OS keychain is removed too.")),
        (PasswordSource::File, Some("Its password saved in the secrets file is removed too.")),
        (PasswordSource::Command("pass show db".into()), None),
        (PasswordSource::Env("DB_PW".into()), None),
        (PasswordSource::Prompt, None),
    ];
    for (source, line) in cases {
        let i = h.app.profiles.iter().position(|p| p.name == "third").unwrap();
        h.app.profiles[i].set_source(source.clone());
        h.explore("third");
        h.keys("d");
        let screen = h.screen(160, 45);
        assert!(screen.contains("Delete the connection profile “third”?"), "{screen}");
        match line {
            Some(l) => assert!(screen.contains(l), "{source:?}: {screen}"),
            None => assert!(!screen.contains("password"), "{source:?}: {screen}"),
        }
        h.keys("n");
    }
}

/// The `/` filter matches a part of the name, the host or the database (like the old profile
/// manager's filter).
#[test]
fn the_explorer_filter_matches_name_host_and_database() {
    let mut h = launched(&sample_config(None));
    let filtered = |h: &mut Harness, q: &str| {
        h.app.focus = Focus::Tree;
        h.keys("/");
        h.type_text(q);
        let rows = h.rows();
        h.key(KeyCode::Esc);
        rows
    };
    assert_eq!(filtered(&mut h, "replica"), ["+", "分析-replica"], "name");
    assert_eq!(filtered(&mut h, "INTERNAL"), ["+", "分析-replica"], "host, ignoring case");
    assert_eq!(filtered(&mut h, "warehouse"), ["+", "分析-replica"], "database");
    assert_eq!(filtered(&mut h, "::1"), ["+", "v6"], "an IPv6 host");
    assert_eq!(filtered(&mut h, "127.0.0.1:55432"), ["+", "local-pg"], "host and port");
    assert_eq!(filtered(&mut h, "zzz"), ["+"]);
}

/// One password prompt at a time: a second profile that needs one waits for the first.
#[test]
fn password_prompts_wait_for_each_other() {
    let mut h = launched(&sample_config(None));
    h.explore("v6");
    h.key(KeyCode::Enter);
    h.explore("分析-replica");
    h.key(KeyCode::Enter);
    let fail = || DbEvent::ConnectFailed { error: "password missing".into(), auth: true };
    h.meta_db("v6", fail());
    h.meta_db("分析-replica", fail());
    assert_eq!(h.prompt().map(|p| p.profile.as_str()), Some("v6"), "the first one asks");
    h.type_text("pw");
    h.key(KeyCode::Enter);
    assert_eq!(h.prompt().map(|p| p.profile.as_str()), Some("分析-replica"), "then the next one");
    h.key(KeyCode::Esc);
    assert!(h.prompt().is_none());
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("v6"), "v6 tries again with the typed password");
}

fn launched(cfg: &Config) -> Harness {
    Harness::launched(cfg, Lang::En, Arc::new(datarig_core::secret::MemoryStore::new()), Startup::Normal)
}

/// A tab without a connection says so in a banner and asks for one before it runs, and `Space c s` changes the connection of a tab.
#[test]
fn a_tab_without_a_connection_asks_for_one_and_space_c_s_changes_it() {
    let mut h = three_profiles();
    // A tab of a profile that is deleted comes back with `Space t u` without a connection.
    connect(&mut h, "other");
    h.app.focus = Focus::Editor;
    h.keys("i");
    h.type_text("select 1");
    h.key(KeyCode::Esc);
    h.explore("other");
    h.keys("dy");
    h.keys(" tu");
    assert_eq!(h.app.tab().profile, None);
    let screen = h.screen(80, 24);
    assert!(screen.contains("No connection · Space c s picks one for this tab"), "{screen}");
    assert_eq!(h.app.focus, Focus::Editor);
    h.ctrl('e');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    assert!(h.screen(80, 24).contains("Connection for this tab"));
    h.type_text("third");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "third")), "the tab runs on third now");
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("third"), "which connects first");
    assert!(h.app.tab().exec.running.is_none(), "the statement waits for the connection");
    // Once connected the statement runs; this console is third's, so no other one opens.
    h.meta_db("third", DbEvent::Connected);
    assert_eq!(h.app.tabs.len(), 2, "local-pg's and this one");
    assert!(!h.screen(80, 24).contains("No connection ·"), "no banner once bound");
    assert!(h.app.tab().exec.running.is_some(), "it runs now");
    // Space c s: another connection for this tab; a running statement asks first.
    h.keys(" cs");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(160, 45).contains("Change this tab's connection?"));
    h.keys("y");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    h.type_text("local");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-pg")));
    assert!(h.app.tab().exec.running.is_none() && h.app.tab().exec.session.is_none(), "the old session is gone");
    assert!(h.connecting().is_none(), "local-pg is connected already");
}

/// Explorer keys: `l`/`h` open and close, Esc cancels an attempt, `t` tests the selected
/// profile, Enter on "＋ New connection" opens the form in the selected folder.
#[test]
fn explorer_keys_on_nodes_and_folders() {
    let mut cfg = sample_config(None);
    cfg.connections[1].folder = Some("work/prod".into());
    let mut h = launched(&cfg);
    assert_eq!(h.rows(), ["+", "work/", "local-pg", "v6"], "folders start closed");
    h.keys("gg");
    h.keys("j");
    h.keys("l");
    assert_eq!(h.rows(), ["+", "work/", "  prod/", "local-pg", "v6"]);
    h.keys("jl");
    assert_eq!(h.rows(), ["+", "work/", "  prod/", "    分析-replica", "local-pg", "v6"]);
    // `n` in a folder: the new profile goes there.
    h.keys("n");
    h.type_text("inside");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("me");
    h.ctrl('s');
    let inside = h.app.profiles.iter().find(|p| p.name == "inside").unwrap();
    assert_eq!(inside.folder.as_deref(), Some("work/prod"));
    assert_eq!(h.app.selected_profile(), Some(inside.id), "the cursor is on it");
    // `h` on a profile goes to its folder, then closes it.
    h.keys("h");
    assert_eq!(h.rows()[h.selected()], "  prod/");
    h.keys("h");
    assert_eq!(h.rows(), ["+", "work/", "  prod/", "local-pg", "v6"]);
    // Enter on "＋ New connection".
    h.keys("gg");
    h.key(KeyCode::Enter);
    assert!(h.form_open());
    h.key(KeyCode::Esc);
    // `r` on a profile that is not connected does nothing; Esc with nothing to cancel neither.
    h.explore("v6");
    h.keys("r");
    h.key(KeyCode::Esc);
    assert!(h.sent().is_empty() && h.overlay_kind().is_none());
    // A folder with profiles cannot be deleted; an empty one can (after asking).
    h.keys("gg");
    h.keys("j");
    h.keys("d");
    assert!(h.status(160, 45).contains("Folder “work” is not empty"));
    let empty = datarig_core::profile::folder::FolderPath::parse("scratch").unwrap();
    h.app.folders.insert(&empty);
    h.keys("gg");
    h.keys("j");
    assert_eq!(h.rows()[h.selected()], "scratch/", "{:?}", h.rows());
    h.keys("d");
    assert!(h.screen(160, 45).contains("Delete the empty folder “scratch”?"));
    h.keys("y");
    assert!(!h.rows().contains(&"scratch/".to_string()));
}

// ── folders, moving, pasting a URL, the sectioned form ─────

#[test]
fn folders_are_created_renamed_and_profiles_moved() {
    let mut h = launched(&sample_config(None));
    // `N` at the top: a new folder.
    h.keys("N");
    assert_eq!(h.app.key_context(), Ctx::NameInput);
    h.key(KeyCode::Enter);
    assert!(h.screen(80, 24).contains("type a name"), "an empty name is refused");
    h.type_text("work");
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert_eq!(h.rows(), ["+", "work/", "local-pg", "v6", "分析-replica"]);
    assert_eq!(h.rows()[h.selected()], "work/", "the cursor is on it");
    // Inside it (`N` on a folder), a name that exists is refused.
    h.keys("N");
    h.type_text("prod");
    h.key(KeyCode::Enter);
    assert_eq!(h.rows(), ["+", "work/", "  prod/", "local-pg", "v6", "分析-replica"], "the parent opens");
    assert_eq!(h.rows()[h.selected()], "  prod/");
    h.keys("k");
    h.keys("N");
    h.type_text("prod");
    h.key(KeyCode::Enter);
    assert!(h.screen(80, 24).contains("that folder exists already"));
    h.key(KeyCode::Esc);
    // `m` moves a profile to a folder picked from a list (with a `/` filter).
    h.explore("v6");
    h.keys("m");
    assert_eq!(h.app.key_context(), Ctx::Chooser);
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::ChooserFilter);
    h.type_text("prod");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.profiles.iter().find(|p| p.name == "v6").unwrap().folder.as_deref(), Some("work/prod"));
    assert_eq!(h.rows(), ["+", "work/", "  prod/", "    v6", "local-pg", "分析-replica"]);
    assert!(h.status(160, 45).contains("Moved v6 to work/prod/"));
    // `R` renames a folder; its profiles follow.
    h.keys("gg");
    h.keys("j");
    h.keys("R");
    h.ctrl('u');
    h.type_text("team");
    h.key(KeyCode::Enter);
    assert_eq!(h.rows(), ["+", "team/", "  prod/", "    v6", "local-pg", "分析-replica"]);
    assert_eq!(h.app.profiles.iter().find(|p| p.name == "v6").unwrap().folder.as_deref(), Some("team/prod"));
    // And back to the top level.
    h.explore("v6");
    h.keys("m");
    assert_eq!(h.app.overlays.chooser().unwrap().selected, 2, "the list starts on its folder");
    h.keys("kkk"); // "(top level)" is first
    h.key(KeyCode::Enter);
    assert_eq!(h.app.profiles.iter().find(|p| p.name == "v6").unwrap().folder, None);
}

#[test]
fn pasting_a_connection_url_opens_a_filled_form() {
    let mut h = launched(&sample_config(None));
    h.app.handle_event(ratatui::crossterm::event::Event::Paste(
        "postgres://report:s3cret@warehouse.internal:6432/sales?sslmode=require".into(),
    ));
    let f = h.form();
    assert_eq!(
        (f.host.text(), f.port.text(), f.user.text(), f.database.text(), f.name.text()),
        ("warehouse.internal", "6432", "report", "sales", "sales")
    );
    assert_eq!(f.password.text(), "s3cret", "the password goes to its field");
    assert!(!f.dsn.text().contains("s3cret"), "and out of the DSN");
    h.ctrl('s');
    assert!(h.app.profiles.iter().any(|p| p.name == "sales" && p.sslmode == "require"));
    // Something else is not a URL: say what a paste does here.
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("hello".into()));
    assert!(!h.form_open());
    assert!(h.status(160, 45).contains("Paste a postgres:// or mysql:// URL here to add a connection"));
    // The welcome panel takes a URL too; a taken name gets a suffix.
    let mut h = launched(&Config::default());
    h.key(KeyCode::Esc);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Welcome);
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("postgresql://me@db.local/app".into()));
    assert_eq!(h.form().name.text(), "app");
}

#[test]
fn the_form_has_basic_and_advanced_sections() {
    use datarig_tui::app::profiles::{Field, Section};
    let mut h = launched(&sample_config(None));
    h.keys("e"); // local-pg
    assert_eq!((h.form().section, h.form().focus), (Section::Basic, Field::Name));
    // The driver comes first; PostgreSQL and MySQL can be picked.
    h.key(KeyCode::BackTab);
    assert_eq!(h.form().focus, Field::Driver);
    h.key(KeyCode::Right);
    assert_eq!(h.form().driver, 1, "MySQL");
    assert!(h.form().dsn.text().starts_with("mysql://"), "{}", h.form().dsn.text());
    h.key(KeyCode::Right);
    assert_eq!(h.form().driver, 0, "the others are not supported yet: back to PostgreSQL");
    assert!(h.form().dsn.text().starts_with("postgres://"), "{}", h.form().dsn.text());
    assert!(h.screen(160, 45).contains("not yet supported"));
    // At 80 columns the full label does not fit: the short one does, uncut.
    let screen = h.screen(80, 24);
    let row = screen.lines().find(|l| l.contains("Driver")).unwrap();
    assert!(row.contains("Elasticsearch   unsupported") && !row.contains('…'), "{row}");
    // Ctrl+N: the SSH section, then the advanced one (SSL mode, policy, color, icon, folder).
    h.ctrl('n');
    assert_eq!((h.form().section, h.form().focus), (Section::Ssh, Field::SshEnabled));
    h.ctrl('n');
    assert_eq!((h.form().section, h.form().focus), (Section::Advanced, Field::SslMode));
    assert!(h.form().fields().iter().all(|f| !matches!(f, Field::Name | Field::Dsn)));
    h.key(KeyCode::Right); // disable -> prefer
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right); // statement cache: on -> off
    h.key(KeyCode::Tab);
    h.type_text("prod-rules");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right); // color: auto -> red
    h.key(KeyCode::Tab);
    h.key(KeyCode::Enter); // icon: the list
    assert_eq!(h.app.key_context(), Ctx::Chooser);
    h.keys("/");
    h.type_text("rock");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Enter);
    assert_eq!(h.form().icon.as_deref(), Some("rocket"));
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right); // folder: top -> (no folders exist yet: stays)
    h.ctrl('p');
    h.ctrl('p');
    assert_eq!(h.form().section, Section::Basic);
    assert!(h.form().dsn.text().ends_with("sslmode=prefer"), "the DSN follows the SSL mode");
    h.ctrl('s');
    let p = h.app.profiles.iter().find(|p| p.name == "local-pg").unwrap();
    assert_eq!(
        (p.sslmode.as_str(), p.policy.as_deref(), p.color.as_deref(), p.icon.as_deref()),
        ("prefer", Some("prod-rules"), Some("red"), Some("rocket"))
    );
    assert!(!p.statement_cache, "the statement cache was turned off");
    // An empty policy is the default one.
    h.keys("e");
    h.ctrl('p');
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.ctrl('u');
    h.ctrl('s');
    assert_eq!(h.app.profiles.iter().find(|p| p.name == "local-pg").unwrap().policy, None);
}

#[test]
fn a_mysql_profile_takes_its_port_and_url_and_has_no_postgresql_settings() {
    use datarig_tui::app::profiles::{Field, Section};
    let mut h = launched(&sample_config(None));
    h.keys("n");
    h.key(KeyCode::BackTab);
    assert_eq!(h.form().focus, Field::Driver);
    // A port still at PostgreSQL's default moves to MySQL's, and the URL follows the driver.
    assert_eq!(h.form().port.text(), "5432");
    h.key(KeyCode::Right);
    assert_eq!((h.form().driver, h.form().port.text()), (1, "3306"));
    assert_eq!(h.form().dsn.text(), "mysql://localhost:3306");
    // Its own port stays when the driver changes.
    h.key(KeyCode::Tab);
    h.type_text("shop-db");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.ctrl('u');
    h.type_text("53306");
    h.key(KeyCode::Tab);
    h.type_text("datarig");
    assert_eq!(h.form().dsn.text(), "mysql://datarig@localhost:53306");
    // No SSL mode nor statement cache: those are PostgreSQL's. MySQL's own come first.
    h.ctrl('n');
    h.ctrl('n');
    assert_eq!(h.form().section, Section::Advanced);
    assert_eq!(h.form().focus, Field::ServerKey);
    assert!(h.form().fields().iter().all(|f| !matches!(f, Field::SslMode | Field::StatementCache)));
    let screen = h.screen(160, 45);
    assert!(!screen.contains("SSL mode"), "{screen}");
    h.ctrl('s');
    let p = h.app.profiles.iter().find(|p| p.name == "shop-db").unwrap();
    assert_eq!((p.driver.as_str(), p.port, p.user.as_str()), ("mysql", 53306, "datarig"));
    assert_eq!(p.display_dsn(), "mysql://datarig@localhost:53306");
    // A pasted mysql:// URL picks MySQL; a parameter is refused (a mysql:// URL takes none).
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("mysql://me:pw@db.example/sales".into()));
    let f = h.form();
    assert_eq!((f.driver, f.port.text(), f.database.text(), f.password.text()), (1, "3306", "sales", "pw"));
    h.key(KeyCode::Esc);
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("mariadb://h/db?ssl-mode=REQUIRED".into()));
    let problem = h.form().dsn_problem.clone().map(|p| h.app.i18n.msg(&p.message()).to_string());
    assert_eq!(problem.as_deref(), Some("unsupported parameter “ssl-mode” (a mysql:// URL takes none)"));
    // Refused as a whole: the driver did not switch to MySQL either (the form started on
    // PostgreSQL).
    assert_eq!(h.form().driver, 0);
}

// ── the context menu (right click) ───────────────────────────────────────

fn right_click(h: &mut Harness, row: u16) {
    use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let area = h.app.explorer.area;
    h.app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: area.x + 4,
        row: area.y + row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn menu_ids(h: &Harness) -> Vec<&'static str> {
    h.app.overlays.menu().unwrap().actions().iter().map(|a| datarig_tui::app::action::spec(*a).id).collect()
}

#[test]
fn right_click_opens_the_nodes_actions_and_runs_them() {
    let mut h = three_profiles();
    h.meta_db("local-pg", DbEvent::Schemas(Ok(vec!["public".into()])));
    h.draw(120, 30);
    // A connected profile (row 1): its actions, in the explorer's key order.
    right_click(&mut h, 1);
    assert_eq!(h.app.key_context(), Ctx::ContextMenu);
    assert_eq!(h.app.focus, Focus::Tree);
    assert_eq!(h.app.selected_profile(), Some(id(&h, "local-pg")), "the click selected the row");
    assert_eq!(
        menu_ids(&h),
        [
            "explorer.activate",
            "explorer.refresh",
            "conn.edit",
            "conn.duplicate",
            "explorer.delete",
            "conn.test",
            "conn.disconnect",
            "conn.open_console",
            "explorer.move",
            // The pane's, after the node's.
            "explorer.filter",
            "conn.new",
            "folder.new",
        ]
    );
    // Every item is an explorer action of the keyboard help (one list for both).
    let help: Vec<_> = h.app.keymap.section(Ctx::Explorer, Some(Ctx::Explorer)).iter().map(|e| e.action).collect();
    assert!(h.app.overlays.menu().unwrap().actions().iter().all(|a| help.contains(a)));
    let screen = h.screen(120, 30);
    assert!(screen.contains("Edit connection profile") && screen.contains(" e │"), "{screen}");
    // Typing filters; Enter runs the item found.
    h.menu_pick("Edit connection profile");
    assert!(h.form_open());
    h.key(KeyCode::Esc);
    // The arrows and Enter: the second item reloads the schemas.
    right_click(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadSchemas)));
    // A profile that is not connected has no disconnect or reload; a folder and blank space
    // have their own actions; Esc closes.
    h.draw(120, 30);
    let other = h.rows().iter().position(|r| r == "other").unwrap() as u16;
    right_click(&mut h, other);
    let ids = menu_ids(&h);
    assert!(!ids.contains(&"conn.disconnect") && !ids.contains(&"explorer.refresh"), "{ids:?}");
    h.key(KeyCode::Esc);
    assert!(h.overlay_kind().is_none());
    right_click(&mut h, 25);
    assert_eq!(h.selected(), 0, "blank space: the New connection row");
    assert_eq!(menu_ids(&h), ["explorer.filter", "conn.new", "folder.new"]);
    // A click on a heading does nothing; a click on an item runs it.
    use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    h.draw(120, 30);
    let list = h.app.overlays.menu().unwrap().list;
    let line = h.app.menu_lines().iter().position(|(_, l, _)| *l == "New connection profile").unwrap() as u16;
    for row in [list.y, list.y + line] {
        h.app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x + 2,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }
    assert!(h.form_open(), "the heading did nothing; the item: new connection");
}

/// Key metadata: read once when the profile connects, marked without a
/// request per query, read again (and forgotten meanwhile) on a refresh; a failure is said and
/// never taken for "no keys". Open tables show their columns with the marks (a driver without the
/// table structure).
#[test]
fn key_columns_come_from_a_cache_that_a_refresh_renews() {
    let mut h = Harness::connected(Lang::En);
    // A driver without the table structure: an open table's columns come from the catalog.
    h.driver.no_structure.store(true, std::sync::atomic::Ordering::SeqCst);
    let pg = id(&h, "local-pg");
    assert!(matches!(h.app.conns.get(pg).unwrap().keys, Keys::Unknown), "not read yet");
    h.db(DbEvent::Keys(Ok(shop_keys())));
    assert!(h.app.conns.get(pg).unwrap().keys.catalog().is_some());
    h.sent();
    // A query: its columns are marked from the cache, the metadata session is not asked.
    h.ctrl('e');
    let cols = vec![meta_from("id", "int8", true, USERS, 1), meta_from("email", "text", false, USERS, 2)];
    h.db(DbEvent::Page {
        id: 1,
        columns: Some(cols),
        rows: vec![vec![Some("1".into()), None]],
        more: false,
        elapsed: Duration::ZERO,
    });
    let sent = h.sent();
    assert!(sent.iter().all(|c| !c.is_meta()), "no metadata request per query: {sent:?}");
    assert!(h.screen(160, 45).contains("PK id"));
    // The explorer's columns: `l` on a table opens them, with their marks.
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys("jjjj"); // its database, analytics, public, shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".into(), "users".into()], Vec::new()).into()),
    });
    h.keys("jjj"); // Tables, orders, users
    h.keys("l");
    let rows = h.rows();
    assert!(rows.iter().any(|r| r.trim() == "users"), "{rows:?}");
    let screen = h.screen(160, 45);
    for want in ["PK id  bigint", "UQ email  text", "name  text"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // A refresh reads the keys again; until they come the marks are unknown, not "none".
    h.explore("local-pg");
    h.keys("r");
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::LoadKeys)), "{sent:?}");
    assert!(matches!(h.app.conns.get(pg).unwrap().keys, Keys::Unknown));
    h.db(DbEvent::Keys(Err("ERROR: permission denied for table pg_index".into())));
    assert!(matches!(&h.app.conns.get(pg).unwrap().keys, Keys::Failed(e) if e.contains("permission denied")));
    assert!(h.status(160, 45).contains("Could not read the key columns: ERROR: permission denied"));
}

/// PostgreSQL 11 or older has no `attgenerated`: the driver says the version it needs instead of
/// the query's error, and a copy as SQL INSERT says so too (not "refresh the keys", which would
/// fail again).
#[test]
fn keys_on_a_server_older_than_12_say_the_version_needed() {
    use datarig_core::driver::DbError;
    let mut h = Harness::connected(Lang::En);
    let pg = id(&h, "local-pg");
    h.db(DbEvent::Keys(Err(DbError::ServerTooOld)));
    assert!(matches!(h.app.conns.get(pg).unwrap().keys, Keys::Failed(_)));
    let want = "PostgreSQL 12 or newer is required for key markers and INSERT copy";
    let status = h.status(200, 45);
    assert!(status.contains(&format!("Could not read the key columns: {want}")), "{status}");
    h.app.run(vec!["SELECT id FROM shop.users".into()]);
    let columns = Some(vec![meta_from("id", "int8", true, USERS, 1)]);
    let rows = vec![vec![Some("1".to_string())]];
    h.db(DbEvent::Page { id: 1, columns, rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.command("copy sql");
    let status = h.status(200, 45);
    assert!(
        status.contains(&format!("Not copied as INSERT: the tables' columns could not be read ({want})")),
        "{status}"
    );
    assert!(!status.contains("refresh"), "{status}");
}

/// DDL run in the app (also `DO` and `CALL`, which may run DDL) reads the profile's keys again:
/// at once in autocommit, and only when the transaction ends inside one (before, the metadata
/// session cannot see the change). Other statements never do.
#[test]
fn ddl_run_in_the_app_reads_the_keys_again() {
    use datarig_core::driver::Outcome;
    let loads = |h: &mut Harness| h.sent().iter().filter(|c| matches!(c, DbCommand::LoadKeys)).count();
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let pg = id(&h, "local-pg");
    h.sent();
    let done = |h: &mut Harness, qid: u64, tag: &str| {
        h.db(DbEvent::Done { id: qid, outcome: Outcome::Command(tag.into()), elapsed: Duration::ZERO });
    };
    h.app.run(vec!["SELECT 1".into()]);
    done(&mut h, 1, "SELECT");
    assert_eq!(loads(&mut h), 0, "not after other statements");
    h.app.run(vec!["-- a new table\nCREATE TABLE shop.tags (id int PRIMARY KEY)".into()]);
    assert_eq!(loads(&mut h), 0, "not before it ran");
    done(&mut h, 2, "CREATE TABLE");
    assert_eq!(loads(&mut h), 1, "autocommit: at once");
    assert!(matches!(h.app.conns.get(pg).unwrap().keys, Keys::Unknown));
    h.db(DbEvent::Keys(Ok(shop_keys())));
    // Inside a transaction: when it ends.
    h.app.run(vec!["BEGIN".into()]);
    h.db(DbEvent::TxOpen(true));
    done(&mut h, 3, "BEGIN");
    h.app.run(vec!["ALTER TABLE shop.tags ADD UNIQUE (id)".into()]);
    done(&mut h, 4, "ALTER TABLE");
    assert_eq!(loads(&mut h), 0, "not while the transaction is open");
    h.app.run(vec!["COMMIT".into()]);
    done(&mut h, 5, "COMMIT");
    h.db(DbEvent::TxOpen(false));
    assert_eq!(loads(&mut h), 1, "once it ends");
    h.db(DbEvent::TxOpen(true));
    h.db(DbEvent::TxOpen(false));
    assert_eq!(loads(&mut h), 0, "only once");
    // DO and CALL bodies can run any DDL the lexer cannot see: read the keys again after them
    // (they ask first: y runs them).
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["DO $$ BEGIN EXECUTE 'ALTER TABLE shop.tags ADD UNIQUE (id)'; END $$".into()]);
    h.keys("y");
    done(&mut h, 6, "DO");
    assert_eq!(loads(&mut h), 1, "after DO");
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["CALL shop.make_tags()".into()]);
    h.keys("y");
    done(&mut h, 7, "CALL");
    assert_eq!(loads(&mut h), 1, "after CALL");
}

// ── the server's databases ────────────────────────────

/// Put the explorer's cursor on the row shown as `text` (as `Harness::rows` writes it).
fn cursor_on(h: &mut Harness, text: &str) {
    h.app.focus = Focus::Tree;
    let rows = h.app.explorer_rows();
    let i = h.rows().iter().position(|r| r == text).unwrap_or_else(|| panic!("no {text:?} in {:?}", h.rows()));
    h.app.explorer.select(&rows, i);
}

/// An event of the aux session of `local-pg` in database `db`.
fn aux_event(h: &mut Harness, db: &str, ev: DbEvent) {
    let p = id(h, "local-pg");
    let a = h.app.conns.aux(p, db).expect("an aux session").id;
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target: datarig_tui::app::EventTarget::Aux(a),
        generation: a,
        ev,
    });
}

/// DataGrip's databases level: under a connected profile its own database first (marked, the
/// schema tree the explorer had), then the server's other databases, asked for when the node
/// opens (loading until they come). Another database opens through its own aux metadata
/// session: its schemas, a schema's tables, and a table opens in a table tab bound to that
/// database, found again by its
/// database. `O` opens a console on a database (its default schema) or a schema. `R` reads
/// another database's tree again.
#[test]
fn the_explorer_lists_the_servers_databases() {
    let mut h = Harness::connected(Lang::En);
    let p = id(&h, "local-pg");
    // Closed and opened again: the databases are asked for (again, after a refresh).
    h.explore("local-pg");
    h.key(KeyCode::Char('h'));
    h.app.conns.entry(p).databases = None;
    h.sent();
    h.key(KeyCode::Char('l'));
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)), "asked when the node opens");
    let rows = h.rows();
    assert_eq!(rows[1..6], ["local-pg", "  db:datarig*", "    analytics", "    public", "    shop"], "{rows:?}");
    assert_eq!(rows[6], "  (note)", "the others are loading: {rows:?}");
    assert!(h.screen(120, 40).contains("datarig default"));
    h.db(DbEvent::Databases(Ok(vec!["datarig".into(), "locked".into(), "sales".into()])));
    let rows = h.rows();
    assert_eq!(rows[6..8], ["  db:locked", "  db:sales"], "{rows:?}");
    // Another database opens through its aux session.
    cursor_on(&mut h, "  db:sales");
    h.key(KeyCode::Char('l'));
    let aux = last_session(&h, SessionRole::Meta);
    let s = h.driver.sessions.lock().unwrap()[aux].opts.context.clone();
    assert_eq!(s.database.as_deref(), Some("sales"));
    assert_eq!(h.rows()[8], "    Loading…");
    aux_event(&mut h, "sales", DbEvent::Connected);
    aux_event(&mut h, "sales", DbEvent::Schemas(Ok(vec!["public".into(), "q1".into()])));
    assert_eq!(h.rows()[8..10], ["    public", "    q1"]);
    cursor_on(&mut h, "    q1");
    h.key(KeyCode::Char('l'));
    assert!(h.sent_to(aux).iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "q1")));
    aux_event(
        &mut h,
        "sales",
        DbEvent::Objects { schema: "q1".into(), result: Ok((vec!["zz_t".into()], vec![]).into()) },
    );
    // `O` on the database and on the schema.
    cursor_on(&mut h, "  db:sales");
    h.keys("O");
    assert_eq!(h.app.tab().context.database.as_deref(), Some("sales"));
    assert_eq!(h.app.tab().context.schema, None);
    cursor_on(&mut h, "    q1");
    h.keys("O");
    assert_eq!(h.app.tab().context.database.as_deref(), Some("sales"));
    assert_eq!(h.app.tab().context.schema.as_deref(), Some("q1"));
    // `O` on the profile's own database: its defaults.
    cursor_on(&mut h, "  db:datarig*");
    h.keys("O");
    assert_eq!(h.app.tab().context, Default::default());
    // Enter on a table of another database: a table tab bound to that database (its default
    // schema: the query names the table's), running the table's SELECT there.
    cursor_on(&mut h, "        zz_t");
    let n = h.app.tabs.len();
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), n + 1);
    let t = h.app.tab();
    assert!(t.is_table(), "a table tab");
    assert_eq!((t.context.database.as_deref(), t.context.schema.as_deref()), (Some("sales"), None));
    let table_tab = t.id;
    let q = last_session(&h, SessionRole::Query);
    assert_eq!(h.driver.sessions.lock().unwrap()[q].opts.context.database.as_deref(), Some("sales"));
    assert_eq!(executes(&h.sent_to(q)), [r#"SELECT * FROM "q1"."zz_t""#]);
    assert!(h.screen(160, 40).contains("sales.q1.zz_t"), "named with its database");
    // Enter again: the same tab, not run again.
    h.app.tabs.activate(0);
    h.key(KeyCode::Enter);
    assert_eq!((h.app.tabs.len(), h.app.tab().id), (n + 1, table_tab));
    assert!(executes(&h.sent_to(q)).is_empty());
    // `R` on the database reads its tree again through its session.
    h.sent_to(aux);
    cursor_on(&mut h, "  db:sales");
    h.keys("r");
    let sent = h.sent_to(aux);
    for want in ["LoadSchemas", "LoadCatalog", "LoadKeys"] {
        assert!(sent.iter().any(|c| format!("{c:?}") == want), "{want}: {sent:?}");
    }
}

/// A database the user cannot connect to shows the reason as its only child (never empty),
/// and so does a server whose databases cannot be read.
#[test]
fn an_unreadable_database_shows_why() {
    let mut h = Harness::connected(Lang::En);
    let p = id(&h, "local-pg");
    h.app.conns.entry(p).databases = Some(Ok(vec!["datarig".into(), "locked".into()]));
    cursor_on(&mut h, "  db:locked");
    h.key(KeyCode::Char('l'));
    let error = datarig_core::driver::DbError::from("FATAL: permission denied for database \"locked\"");
    aux_event(&mut h, "locked", DbEvent::ConnectFailed { error, auth: false });
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "  db:locked").unwrap();
    assert_eq!(rows[at + 1], "    (note)", "{rows:?}");
    let screen = h.screen(160, 45);
    assert!(screen.contains("This database can't be read: FA"), "{screen}");
    assert!(!screen.contains("(empty)"), "{screen}");
    // The whole reason in the status bar.
    let status = h.status(200, 45);
    assert!(status.contains("Database locked could not be read: FATAL: permission denied"), "{status}");
    // Enter on the note tries again.
    let before = h.roles().len();
    cursor_on(&mut h, "    (note)");
    h.key(KeyCode::Enter);
    assert_eq!(h.roles().len(), before + 1, "a new attempt");
    // The server's list itself: the reason instead of the rows.
    h.meta_db("local-pg", DbEvent::Databases(Err("ERROR: permission denied for table pg_database".into())));
    let screen = h.screen(160, 45);
    assert!(screen.contains("The databases can't be read"), "{screen}");
    assert!(!screen.contains("db:locked"));
}

/// `Esc` while a profile connects ends the attempt, and once the "cancelled" flash
/// is gone the status bar does not go back to "Connecting to …" (the wait is over).
#[test]
fn a_cancelled_connect_leaves_no_connecting_status() {
    let mut h = three_profiles();
    h.explore("other");
    h.key(KeyCode::Enter);
    assert!(h.status(160, 45).contains("Connecting to other"), "{}", h.status(160, 45));
    h.key(KeyCode::Esc);
    assert_ne!(h.app.conns.state(id(&h, "other")), NodeState::Connecting);
    // The flash expires.
    h.app.transient = None;
    let status = h.status(160, 45);
    assert!(!status.contains("Connecting to other"), "{status}");
    assert!(status.contains("Connection to other cancelled"), "{status}");
}

/// `Enter` in a question that would lose something keeps it; only `y` does it.
/// Deleting a profile and disconnecting one with an open transaction.
#[test]
fn enter_keeps_the_profile_and_the_connection() {
    let mut h = three_profiles();
    h.explore("other");
    h.command("explorer.delete");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(160, 45).contains("y delete · n/Enter cancel"), "{}", h.screen(160, 45));
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert!(h.app.profiles.iter().any(|p| p.name == "other"), "Enter keeps the profile");
    h.command("explorer.delete");
    h.keys("y");
    assert!(h.app.profiles.iter().all(|p| p.name != "other"), "y deletes it");
    // A transaction is open in the connected profile's tab.
    h.db(DbEvent::TxOpen(true));
    h.explore("local-pg");
    h.command("conn.disconnect");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(160, 45).contains("y disconnect · n/Enter stay"), "{}", h.screen(160, 45));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.conns.state(id(&h, "local-pg")), NodeState::Connected, "Enter stays connected");
    assert!(h.app.tab().exec.tx_open, "the transaction stays open");
    h.command("conn.disconnect");
    h.keys("y");
    assert_eq!(h.app.conns.state(id(&h, "local-pg")), NodeState::Disconnected, "y disconnects");
}

/// Right click on a database, a schema or a table while the keyboard's cursor is on another
/// row: the menu has one console item, which says where, and it opens the console there as
/// `O` does (it opened one with the profile's defaults).
#[test]
fn the_menus_console_opens_where_the_click_was() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.db(DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
    cursor_on(&mut h, "    shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".into(), "users".into()], Vec::new()).into()),
    });
    let at = |db: Option<&str>, schema: Option<&str>| SessionContext {
        database: db.map(str::to_string),
        schema: schema.map(str::to_string),
    };
    for (row, label, want) in [
        ("db:datarig*", "New console in datarig", at(None, None)),
        ("public", "New console in public", at(None, Some("public"))),
        ("users", "New console in shop", at(None, Some("shop"))),
        ("db:sales", "New console in sales", at(Some("sales"), None)),
    ] {
        h.explore("local-pg");
        h.right_click_row(row);
        let ids = menu_ids(&h);
        assert!(ids.contains(&"explorer.new_console_here"), "{row}: {ids:?}");
        assert!(!ids.contains(&"conn.open_console"), "{row}: one console item, {ids:?}");
        assert!(h.menu_labels().iter().any(|l| l == label), "{row}: {:?}", h.menu_labels());
        let tabs = h.app.tabs.len();
        h.menu_pick(label);
        assert_eq!(h.app.tabs.len(), tabs + 1, "{row}: a new console");
        assert_eq!(h.app.tab().context, want, "{row}");
    }
    // On the profile itself: its defaults, as before.
    cursor_on(&mut h, "+");
    h.right_click_row("local-pg");
    let ids = menu_ids(&h);
    assert!(ids.contains(&"conn.open_console") && !ids.contains(&"explorer.new_console_here"), "{ids:?}");
    h.menu_pick("New console on this connection");
    assert_eq!(h.app.tab().context, SessionContext::default());
}

/// A way to press a key (or run a command) in a test.
type Press = fn(&mut Harness);

/// A new console from the keyboard in the explorer (`Ctrl+T`, `Space t n`, `:tab.new_console`)
/// opens where the selected row is, as the row's menu does: the database and schema of a
/// database, a schema or a table, the profile's defaults on the profile; never the active tab's
/// binding with the schema dropped.
#[test]
fn a_new_console_from_the_explorer_opens_where_the_row_is() {
    let mut h = Harness::connected(Lang::En);
    h.explore("local-pg");
    h.db(DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
    cursor_on(&mut h, "    shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".into(), "users".into()], Vec::new()).into()),
    });
    let at = |db: Option<&str>, schema: Option<&str>| SessionContext {
        database: db.map(str::to_string),
        schema: schema.map(str::to_string),
    };
    let keys: [(&str, Press); 3] = [
        ("Ctrl+T", |h| h.ctrl('t')),
        ("Space t n", |h| h.keys(" tn")),
        (":tab.new_console", |h| h.command("tab.new_console")),
    ];
    for (row, want) in [
        ("db:datarig*", at(None, None)),
        ("    public", at(None, Some("public"))),
        ("users", at(None, Some("shop"))),
        ("db:sales", at(Some("sales"), None)),
        ("local-pg", SessionContext::default()),
    ] {
        for (name, press) in keys {
            h.explore("local-pg");
            h.app.focus = Focus::Tree;
            let rows = h.app.explorer_rows();
            let i =
                h.rows().iter().position(|r| r.trim() == row.trim()).unwrap_or_else(|| panic!("{row}: {:?}", h.rows()));
            h.app.explorer.select(&rows, i);
            let tabs = h.app.tabs.len();
            press(&mut h);
            assert_eq!(h.app.tabs.len(), tabs + 1, "{row} {name}: a new console");
            assert_eq!(h.app.tab().profile, Some(id(&h, "local-pg")), "{row} {name}");
            assert_eq!(h.app.tab().context, want, "{row} {name}");
            assert_eq!(h.app.focus, Focus::Editor, "{row} {name}");
        }
    }
    // The menu's console item on the same row binds the same.
    h.explore("local-pg");
    h.right_click_row("public");
    h.menu_pick("New console in public");
    assert_eq!(h.app.tab().context, at(None, Some("public")));
}

/// The menu of a folder and of a profile, each right-clicked while the keyboard's cursor is on
/// the other: the items say what they act on and act on the clicked row.
#[test]
fn the_menu_of_a_folder_and_of_a_profile() {
    use datarig_tui::app::chooser::NamePurpose;
    let mut cfg = sample_config(None);
    cfg.connections[1].folder = Some("work".into());
    let mut h = launched(&cfg);
    assert_eq!(h.rows(), ["+", "work/", "local-pg", "v6"]);
    // The folder (the cursor on a profile).
    h.explore("v6");
    h.right_click_row("work/");
    let labels = h.menu_labels();
    for want in ["Rename folder", "Delete folder"] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    assert!(labels.iter().all(|l| !l.contains("profile or")), "{labels:?}");
    h.menu_pick("Rename folder");
    let n = h.app.overlays.name_input().expect("the name dialog");
    assert!(matches!(&n.purpose, NamePurpose::RenameFolder(f) if f.to_string() == "work"));
    h.key(KeyCode::Esc);
    // A profile (the cursor on the folder).
    cursor_on(&mut h, "work/");
    h.right_click_row("v6");
    let labels = h.menu_labels();
    for want in ["Delete connection profile", "Move connection profile to a folder"] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    h.menu_pick("Delete connection profile");
    assert!(h.screen(160, 45).contains("Delete the connection profile “v6”?"));
    h.keys("n");
    h.right_click_row("v6");
    h.menu_pick("Move connection profile to a folder");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser));
    assert_eq!(h.app.selected_profile(), Some(id(&h, "v6")));
}
