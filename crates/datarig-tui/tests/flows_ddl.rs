//! Show DDL: `D` (and the node's menu, `:ddl`) opens an object's DDL in a read-only tab, read
//! through the metadata session (`DbCommand::LoadDdl`); `F` a trigger's function. The answer is
//! bound to its request: an older one, or one for a tab that went, is dropped. A locked object
//! shows only that it is locked. The text can be moved in, searched and yanked, never changed;
//! `o` opens it in a new console, `r` and the run key read it again; a restored tab waits for
//! that. A fake driver records the requests and the test feeds the answers.

mod common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::ddl::{DdlObject, DdlSource, FunctionDdl, RelationDdl};
use datarig_core::driver::{DbCommand, DbError, DbEvent, SchemaObjects};
use datarig_core::i18n::Lang;
use datarig_core::sql::ddl::ddl_text;
use datarig_tui::app::tabs::DdlState;
use datarig_tui::app::{Focus, TabKind};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

/// Connected, the explorer focused, `shop` open with its tables and its view.
fn shop_open() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.icons = IconsSetting::Off;
    h.key(KeyCode::BackTab); // editor -> explorer
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    let objects = SchemaObjects {
        tables: vec!["order_items".into(), "orders".into(), "users".into()],
        views: vec!["order_summary".into()],
        ..Default::default()
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    h.sent();
    h
}

/// The DDL requests among `cmds`: their ids and objects.
fn asked(cmds: &[DbCommand]) -> Vec<(u64, DdlObject)> {
    cmds.iter()
        .filter_map(|c| match c {
            DbCommand::LoadDdl { id, object } => Some((*id, object.clone())),
            _ => None,
        })
        .collect()
}

fn relation(schema: &str, name: &str) -> DdlObject {
    DdlObject::Relation { schema: schema.into(), name: name.into() }
}

/// `shop.users` as the driver would read it.
fn users_ddl() -> DdlSource {
    let mut s = users_structure();
    s.primary_key.as_mut().unwrap().definition = "PRIMARY KEY (id)".into();
    s.unique_constraints[0].definition = "UNIQUE (email)".into();
    s.indexes[1].definition = "CREATE INDEX users_nickname_idx ON shop.users USING btree (nickname DESC)".into();
    s.triggers.clear();
    DdlSource::Relation(Box::new(RelationDdl::new("shop", "users", "datarig", s)))
}

/// The answer to request `id`.
fn answer(id: u64, result: Result<DdlSource, DbError>) -> DbEvent {
    DbEvent::Ddl { id, result }
}

/// The open DDL tab's state.
fn state(h: &Harness) -> DdlState {
    h.app.tab().doc.ddl.as_ref().expect("a DDL tab").state.clone()
}

/// `shop.users` open in its DDL tab (asked with `D`, answered), the editor focused.
fn users_open() -> (Harness, String) {
    let mut h = shop_open();
    h.goto("users");
    h.keys("D");
    let asked = asked(&h.sent());
    h.db(answer(asked[0].0, Ok(users_ddl())));
    h.app.focus = Focus::Editor;
    (h, ddl_text(&users_ddl()))
}

/// `D` on a table opens its DDL tab and asks once; until the answer comes the tab says it is
/// reading, then it shows the DDL (the header first, syntax colours) under "DDL · shop.users",
/// in a tab named so. The focus stays in the explorer. `D` again goes to that tab and asks
/// nothing; the run key reads it again.
#[test]
fn d_on_a_table_shows_its_ddl_in_a_tab_of_its_own() {
    let mut h = shop_open();
    h.goto("users");
    h.keys("D");
    let first = asked(&h.sent());
    assert_eq!(first.iter().map(|a| a.1.clone()).collect::<Vec<_>>(), [relation("shop", "users")]);
    assert!(h.app.tab().is_ddl());
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer, as when a table opens");
    assert_eq!(state(&h), DdlState::Loading);
    assert!(h.screen(120, 30).contains("Reading the DDL from the catalog"), "{}", h.screen(120, 30));
    h.db(answer(first[0].0, Ok(users_ddl())));
    assert_eq!(state(&h), DdlState::Loaded);
    let text = ddl_text(&users_ddl());
    assert_eq!(h.app.tab().editor.text(), text);
    let screen = h.screen(120, 30);
    assert!(screen.contains("DDL · shop.users"), "{screen}");
    assert!(screen.contains("DDL shop.users"), "the tab's name: {screen}");
    assert!(screen.contains("-- Reconstructed by datarig from the catalog (not pg_dump)"), "{screen}");
    assert!(screen.contains("CREATE TABLE shop.users ("), "{screen}");

    // From another tab, `D` again.
    h.app.dispatch(datarig_tui::app::action::Action::PrevTab);
    assert!(!h.app.tab().is_ddl());
    h.keys("D");
    assert!(h.app.tab().is_ddl(), "back to its tab");
    assert!(asked(&h.sent()).is_empty(), "already read: no new request");
    assert_eq!(h.app.tabs.len(), 2, "the same tab");
    h.app.focus = Focus::Editor;
    h.ctrl('e');
    let again = asked(&h.sent());
    assert_eq!(again.len(), 1, "the run key reads it again");
    assert!(again[0].0 > first[0].0);
}

/// An answer is bound to its request: one of an older request (read again since) is dropped,
/// and so is one for a tab that went; the newest one is shown.
#[test]
fn an_answer_counts_only_for_the_request_the_tab_waits_for() {
    let mut h = shop_open();
    h.goto("users");
    h.keys("D");
    let old = asked(&h.sent())[0].0;
    h.app.focus = Focus::Editor;
    h.keys("r");
    let new = asked(&h.sent())[0].0;
    assert_ne!(old, new);
    h.db(answer(old, Err(DbError::Locked)));
    assert_eq!(state(&h), DdlState::Loading, "the older request's answer is dropped");
    h.db(answer(new, Ok(users_ddl())));
    assert_eq!(state(&h), DdlState::Loaded);
    // Read again, then the tab closes before the answer: nothing else changes.
    h.keys("r");
    let gone = asked(&h.sent())[0].0;
    h.ctrl('w');
    let tabs = h.app.tabs.len();
    h.db(answer(gone, Ok(users_ddl())));
    assert_eq!(h.app.tabs.len(), tabs);
    assert!(h.app.tabs.iter().all(|t| !t.is_ddl()));
}

/// A locked object shows only that it is locked: no part of an earlier read stays, nothing can
/// be copied or opened from it; `r` asks again.
#[test]
fn a_locked_object_shows_only_that_it_is_locked() {
    let (mut h, _) = users_open();
    h.keys("r");
    let id = asked(&h.sent())[0].0;
    h.db(answer(id, Err(DbError::Locked)));
    assert_eq!(state(&h), DdlState::Locked);
    assert_eq!(h.app.tab().editor.text(), "", "nothing of the earlier read");
    let screen = h.screen(160, 30);
    assert!(screen.contains("nothing was read"), "{screen}");
    assert!(screen.contains("r tries again"), "{screen}");
    assert!(!screen.contains("CREATE"), "{screen}");
    let tabs = h.app.tabs.len();
    h.keys("o");
    assert_eq!(h.app.tabs.len(), tabs, "nothing to open in a console");
    h.keys("r");
    assert_eq!(asked(&h.sent()).len(), 1, "asked again");
    // A failure says why.
    let id = h.app.tab().doc.ddl.as_ref().unwrap().request.unwrap();
    h.db(answer(id, Err(DbError::Server("ERROR: permission denied for table users".into()))));
    let screen = h.screen(160, 30);
    assert!(screen.contains("The DDL could not be read: ERROR: permission denied for table users"), "{screen}");
}

/// The text is read-only: every command that would change it does nothing and says so (in
/// Normal and Visual mode, a put, a paste, an undo, `:s`), Insert mode never starts; moving,
/// Visual mode, search, marks and yanks work (a yank goes to the clipboard).
#[test]
fn the_ddl_text_can_be_read_and_yanked_never_changed() {
    let (mut h, text) = users_open();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    // (`o` and `r` are the tab's own keys: open in a console, read again.)
    for keys in ["x", "dd", "dw", "i", "a", "O", "p", "J", "~", ".", "u", "cw", "S", "D", "guu", ">>", "&", "\"ap"] {
        h.keys(keys);
        h.key(KeyCode::Esc);
        assert_eq!(h.app.tab().editor.text(), text, "{keys}");
        assert_eq!(h.app.tab().editor.mode, datarig_tui::widgets::editor::Mode::Normal, "{keys}");
    }
    h.keys("x");
    assert!(h.status(120, 30).contains("The DDL tab is read-only: o opens it in a new console"));
    h.ctrl('r');
    assert_eq!(h.app.tab().editor.text(), text);
    for keys in ["vjd", "Vx", "vc", "vp", "vU", "v>", "vrX", "vJ"] {
        h.keys(keys);
        h.key(KeyCode::Esc);
        assert_eq!(h.app.tab().editor.text(), text, "{keys}");
    }
    // By block too (`Ctrl+V`): `I`, `d`.
    for keys in ["jIx", "jd"] {
        h.ctrl('v');
        h.keys(keys);
        h.key(KeyCode::Esc);
        assert_eq!(h.app.tab().editor.text(), text, "block {keys}");
    }
    h.app.handle_event(ratatui::crossterm::event::Event::Paste("DROP TABLE x;".into()));
    assert_eq!(h.app.tab().editor.text(), text, "a paste");
    h.command("s/users/people/g");
    assert_eq!(h.app.tab().editor.text(), text, ":s");
    assert!(h.status(120, 30).contains("read-only"));
    // Moving, searching and yanking work.
    h.keys("gg");
    h.keys("j");
    assert_eq!(h.app.tab().editor.row, 1);
    h.keys("/CREATE TABLE");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().editor.lines[h.app.tab().editor.row].starts_with("CREATE TABLE"));
    h.keys("yy");
    assert_eq!(clip.last().as_deref(), Some("CREATE TABLE shop.users (\n"));
    h.keys("Vjy");
    assert!(clip.last().is_some_and(|c| c.starts_with("CREATE TABLE shop.users (\n    id bigint")));
    // The menu's "copy the whole DDL".
    h.command("ddl.copy");
    assert_eq!(clip.last(), Some(text.clone()));
    // The formatter and the comment toggle are refused too.
    h.command("format");
    h.key_mod(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(h.app.tab().editor.text(), text);
}

/// `o` opens the text in a new console on the same profile and database, with the focus in its
/// editor; the DDL tab stays as it was.
#[test]
fn o_opens_the_ddl_in_a_new_console() {
    let (mut h, text) = users_open();
    let ddl = h.app.tab().id;
    let profile = h.app.tab().profile;
    h.keys("o");
    let t = h.app.tab();
    assert_eq!(t.kind, TabKind::Console);
    assert_ne!(t.id, ddl);
    assert_eq!(t.editor.text(), text);
    assert_eq!((t.profile, t.context.clone()), (profile, h.app.tabs.get(ddl).unwrap().context.clone()));
    assert_eq!(h.app.focus, Focus::Editor);
    assert_eq!(h.app.tabs.get(ddl).unwrap().editor.text(), text);
    // It is the user's text now: it changes.
    h.keys("ggdd");
    assert_ne!(h.app.tab().editor.text(), text);
}

/// An index's DDL from its item, a trigger's and its function's (`F`); `F` anywhere else says
/// it needs a trigger; `D` on a schema says what it works on.
#[test]
fn indexes_triggers_and_trigger_functions() {
    let mut h = shop_open();
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.sent();
    h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) });
    h.goto("Indexes");
    h.key(KeyCode::Char('l'));
    h.goto("users_nickname_idx");
    h.keys("D");
    assert_eq!(
        asked(&h.sent()).into_iter().map(|a| a.1).collect::<Vec<_>>(),
        [DdlObject::Index { schema: "shop".into(), name: "users_nickname_idx".into() }]
    );
    h.app.focus = Focus::Tree;
    h.keys("F");
    assert!(asked(&h.sent()).is_empty());
    assert!(h.status(120, 30).contains("The function's DDL: put the cursor on a trigger"));
    h.goto("Triggers");
    h.key(KeyCode::Char('l'));
    h.goto("users_touch");
    h.keys("D");
    h.app.focus = Focus::Tree;
    h.keys("F");
    let objects: Vec<DdlObject> = asked(&h.sent()).into_iter().map(|a| a.1).collect();
    assert_eq!(
        objects,
        [
            DdlObject::Trigger { schema: "shop".into(), table: "users".into(), name: "users_touch".into() },
            DdlObject::TriggerFunction { schema: "shop".into(), table: "users".into(), trigger: "users_touch".into() },
        ]
    );
    assert_eq!(h.app.tabs.len(), 4, "a tab each");
    let f = FunctionDdl {
        schema: "shop".into(),
        name: "touch".into(),
        arguments: String::new(),
        procedure: false,
        owner: "datarig".into(),
        definition: "CREATE OR REPLACE FUNCTION shop.touch()\n RETURNS trigger\n LANGUAGE plpgsql\nAS $f$BEGIN RETURN NEW; END$f$\n"
            .into(),
        comment: None,
        grants: None,
    };
    let id = h.app.tab().doc.ddl.as_ref().unwrap().request.unwrap();
    h.db(answer(id, Ok(DdlSource::Function(f))));
    assert!(h.screen(120, 30).contains("DDL · shop.touch()"), "the tab is named as the catalog names it");
    // A schema has no DDL here.
    h.app.focus = Focus::Tree;
    while !h.explorer_line().trim_end().ends_with("\u{25be} shop") {
        let before = h.selected();
        h.keys("k");
        assert_ne!(h.selected(), before, "the schema's line is above");
    }
    h.keys("D");
    assert!(asked(&h.sent()).is_empty());
    assert!(h.status(120, 30).contains("Show DDL works on a table, view, materialized view, index or trigger"));
}

/// The node's menu offers "Show DDL" on a table (and the function's on a trigger) and runs it.
#[test]
fn the_menu_shows_the_ddl() {
    let mut h = shop_open();
    h.right_click_row("users");
    let labels = h.menu_labels();
    assert!(labels.contains(&"Show DDL".to_string()), "{labels:?}");
    assert!(!labels.contains(&"Show the function's DDL".to_string()), "{labels:?}");
    h.menu_pick("Show DDL");
    assert_eq!(asked(&h.sent()).into_iter().map(|a| a.1).collect::<Vec<_>>(), [relation("shop", "users")]);
    // The DDL tab's own menu.
    h.app.focus = Focus::Editor;
    h.keys("  ");
    let labels = h.menu_labels();
    for l in ["DDL tab: read the DDL again", "DDL tab: open the DDL in a new console", "DDL tab: copy the whole DDL"] {
        assert!(labels.contains(&l.to_string()), "{l}: {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("Format")), "{labels:?}");
}

/// `:ddl name` reads what the name names on the active tab's connection, in the tab's schema;
/// `:ddl` alone the explorer's object, or the table tab's table; without either it says how.
#[test]
fn the_ddl_command() {
    let mut h = Harness::connected(Lang::En);
    h.app.focus = Focus::Editor;
    h.command("ddl");
    assert!(h.cmdline().is_some_and(|c| c.error.is_some()), "nothing to show: the line says how");
    h.key(KeyCode::Esc);
    h.command("use .shop");
    h.sent();
    h.command("ddl \"Order Items\"");
    assert_eq!(
        asked(&h.sent()).into_iter().map(|a| a.1).collect::<Vec<_>>(),
        [DdlObject::Named { name: "\"Order Items\"".into(), schema: Some("shop".into()) }]
    );
    let id = h.app.tab().doc.ddl.as_ref().unwrap().request.unwrap();
    h.db(answer(id, Err(DbError::NotFound)));
    assert!(h.screen(160, 30).contains("it does not exist (any more): renamed or dropped?"));
    // In the explorer: its object.
    let mut h = shop_open();
    h.goto("orders");
    h.command("ddl");
    assert_eq!(asked(&h.sent()).into_iter().map(|a| a.1).collect::<Vec<_>>(), [relation("shop", "orders")]);
}

/// A DDL tab of a profile that is not connected connects it and reads once it is; a lost
/// metadata session ends the wait with why.
#[test]
fn it_waits_for_the_connection_and_says_when_it_is_lost() {
    let (mut h, _) = users_open();
    let profile = h.app.tab().profile.unwrap();
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert!(!h.app.conns.is_connected(profile));
    h.app.focus = Focus::Editor;
    h.keys("r");
    assert_eq!(state(&h), DdlState::Connecting);
    assert!(h.screen(120, 30).contains("Connecting…"));
    h.sent();
    h.db(DbEvent::Connected);
    let after = asked(&h.sent());
    assert_eq!(after.len(), 1, "read once connected");
    assert_eq!(state(&h), DdlState::Loading);
    h.db(DbEvent::Lost { error: DbError::Closed });
    assert!(matches!(state(&h), DdlState::Failed(_)), "{:?}", state(&h));
    assert!(h.screen(160, 30).contains("The DDL could not be read"));
}

/// A DDL tab comes back after a restart as it was saved (its object, profile and database), not
/// read: it says how to read it, and reads on `r`.
#[test]
fn a_restored_ddl_tab_is_not_read_until_asked() {
    let (mut h, _) = users_open();
    let state_now = h.app.workspace_state();
    let saved = state_now.tabs.iter().find(|t| t.kind == datarig_core::workspace::TabKind::Ddl).expect("saved");
    assert_eq!(saved.ddl, Some(relation("shop", "users")));
    // Closed and brought back: the same, not read.
    h.ctrl('w');
    h.sent();
    h.key_mod(KeyCode::Char(' '), KeyModifiers::NONE);
    h.keys("tu");
    assert!(h.app.tab().is_ddl());
    assert_eq!(state(&h), DdlState::NotLoaded);
    assert!(asked(&h.sent()).is_empty(), "nothing asked by itself");
    assert!(h.screen(120, 30).contains("Not read yet — r reads the DDL from the catalog"));
    h.app.focus = Focus::Editor;
    h.keys("r");
    assert_eq!(asked(&h.sent()).len(), 1);
}

/// Another database's object is read through that database's metadata session, and only its
/// answer counts: the same request id from the profile's own session is not this tab's.
#[test]
fn another_databases_ddl_goes_through_its_own_session() {
    use datarig_core::driver::{SessionContext, SessionRole};
    use datarig_tui::app::{AppEvent, EventTarget};
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
    h.sent();
    h.command("ddl shop.users");
    let aux = h.roles().iter().rposition(|r| *r == SessionRole::Meta).unwrap();
    assert_ne!(aux, 0, "a metadata session of its own");
    let mine = asked(&h.sent_to(aux));
    assert_eq!(
        mine.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
        [DdlObject::Named { name: "shop.users".into(), schema: None }]
    );
    assert!(asked(&h.sent_to(0)).is_empty(), "not the profile's own session");
    assert_eq!(h.app.tab().context.database.as_deref(), Some("sales"));
    let id = mine[0].0;
    h.db(answer(id, Ok(users_ddl())));
    assert_eq!(state(&h), DdlState::Loading, "an answer of another database is not this tab's");
    let p = h.app.profiles[0].id;
    let a = h.app.conns.aux(p, "sales").expect("an aux session").id;
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Aux(a), generation: a, ev: answer(id, Ok(users_ddl())) });
    assert_eq!(state(&h), DdlState::Loaded);
    assert!(h.screen(120, 30).contains("DDL · sales.shop.users"), "{}", h.screen(120, 30));
}

/// After a restart a DDL tab comes back with its object, profile and database, not read (no
/// session opens for it, nothing is asked); `r` connects the profile and reads it.
#[test]
fn a_ddl_tab_survives_a_restart_without_reading() {
    use datarig_core::paths::Paths;
    use datarig_core::secret::{MemoryStore, SecretStore};
    use datarig_tui::app::Startup;
    use std::sync::Arc;
    let dir = std::env::temp_dir().join(format!("datarig-ddl-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = test_db_config();
    let launch = || {
        let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
        let store = Arc::new(MemoryStore::new());
        app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
        app.set_paths(Paths { data: None, state: Some(dir.clone()) });
        let mut h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock }
            .with_fake_driver();
        h.app.launch(Startup::Normal);
        h
    };
    let mut h = launch();
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.command("ddl shop.users");
    let id = asked(&h.sent())[0].0;
    h.db(answer(id, Ok(users_ddl())));
    assert_eq!(state(&h), DdlState::Loaded);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);

    let mut h = launch();
    let i = h.app.tabs.iter().position(|t| t.is_ddl()).expect("the DDL tab is back");
    h.app.tabs.activate(i);
    assert_eq!(
        h.app.tab().doc.ddl.as_ref().map(|d| d.object.clone()),
        Some(DdlObject::Named { name: "shop.users".into(), schema: None })
    );
    assert_eq!(state(&h), DdlState::NotLoaded);
    assert_eq!(h.app.tab().editor.text(), "");
    h.app.focus = Focus::Editor;
    assert!(h.screen(120, 30).contains("Not read yet — r reads the DDL from the catalog"));
    assert!(h.roles().is_empty(), "no session opened by itself");
    h.keys("r");
    assert_eq!(state(&h), DdlState::Connecting);
    h.db(DbEvent::Connected);
    assert_eq!(asked(&h.sent()).len(), 1, "read once connected");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A driver that cannot show DDL: `D` says so and opens nothing, and the menu does not offer it.
#[test]
fn a_driver_without_ddl_says_so() {
    let mut h = shop_open();
    h.driver.no_ddl.store(true, std::sync::atomic::Ordering::SeqCst);
    h.goto("users");
    h.keys("D");
    assert!(asked(&h.sent()).is_empty());
    assert!(h.app.tabs.iter().all(|t| !t.is_ddl()));
    assert!(h.status(120, 30).contains("This connection's driver cannot show DDL"));
    h.right_click_row("users");
    assert!(!h.menu_labels().contains(&"Show DDL".to_string()), "{:?}", h.menu_labels());
}

/// `:w` (and `:w name`, `:wq`) in a DDL tab saves nothing and the tab stays a DDL tab (it
/// never becomes a saved query the run key would run); a table tab's query neither.
#[test]
fn w_in_a_ddl_or_table_tab_saves_nothing() {
    let (mut h, text) = users_open();
    for cmd in ["w", "w zz_saved", "wq"] {
        h.command(cmd);
        assert!(h.app.tab().is_ddl(), "{cmd}");
        assert_eq!(h.app.tab().doc.script, None, "{cmd}");
        assert!(h.status(120, 30).contains("read-only"), "{cmd}");
    }
    assert_eq!(h.app.tab().editor.text(), text);
    // The run key still reads it again, never runs it.
    h.sent();
    h.ctrl('e');
    let sent = h.sent();
    assert!(sent.iter().all(|c| !matches!(c, DbCommand::Execute { .. })), "{sent:?}");
    assert_eq!(asked(&sent).len(), 1);
}

/// A tab waiting for its profile to connect stops waiting when the user disconnects it.
#[test]
fn disconnecting_ends_the_wait_for_the_connection() {
    let (mut h, _) = users_open();
    let p = h.app.tab().profile.unwrap();
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    h.app.focus = Focus::Editor;
    h.keys("r");
    assert_eq!(state(&h), DdlState::Connecting);
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert!(!h.app.conns.is_connected(p));
    assert_eq!(state(&h), DdlState::NotLoaded, "not waiting any more");
}

/// A count typed in a DDL tab is the editor's: `3r` neither reads again nor leaves the count
/// for the next key.
#[test]
fn a_count_is_never_left_behind_by_the_tabs_own_keys() {
    let (mut h, text) = users_open();
    h.sent();
    h.keys("3r");
    assert!(asked(&h.sent()).is_empty(), "a count then r is vim's (refused), not a read");
    h.key(KeyCode::Esc);
    h.keys("ggj");
    assert_eq!(h.app.tab().editor.row, 1, "no count left behind");
    assert_eq!(h.app.tab().editor.text(), text);
}

/// A relation read with `:ddl name` and then with `D` is one tab.
#[test]
fn a_typed_name_and_the_explorer_find_the_same_tab() {
    let mut h = shop_open();
    h.command("ddl shop.users");
    let id = asked(&h.sent())[0].0;
    h.db(answer(id, Ok(users_ddl())));
    let tab = h.app.tab().id;
    h.app.focus = Focus::Tree;
    h.goto("users");
    h.keys("D");
    assert_eq!(h.app.tab().id, tab, "the same tab");
    assert!(asked(&h.sent()).is_empty());
}
