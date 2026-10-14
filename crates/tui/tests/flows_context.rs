//! A database and schema per tab through the real `App` event path with the fake
//! driver: the picker's two levels (`Space c d`, quick connect), `:use`, the explorer's console
//! here, the rebind doctrine when switching, the session's options, completion in the tab's
//! schema, the editor's first line and the workspace across a restart.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent, SessionContext, SessionRole};
use datarig_core::i18n::{I18n, Lang, Msg};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::quick::{QuickNote, QuickRow};
use datarig_tui::app::{AppEvent, EventTarget, Focus};
use ratatui::crossterm::event::KeyCode;

fn ctx(db: Option<&str>, schema: Option<&str>) -> SessionContext {
    SessionContext { database: db.map(str::to_string), schema: schema.map(str::to_string) }
}

fn rows(h: &Harness) -> Vec<QuickRow> {
    h.app.overlays.quick().map(|q| q.items.clone()).unwrap_or_default()
}

fn selected(h: &Harness) -> Option<QuickRow> {
    h.app.overlays.quick().and_then(|q| q.items.get(q.selected).cloned())
}

/// Move the picker's selection to `row`.
fn select(h: &mut Harness, row: &QuickRow) {
    for _ in 0..rows(h).len() {
        if selected(h).as_ref() == Some(row) {
            return;
        }
        h.key(KeyCode::Down);
    }
    panic!("{row:?} not in {:?}", rows(h));
}

/// An event of the metadata session of profile `p` in database `db`.
fn aux_db(h: &mut Harness, db: &str, ev: DbEvent) {
    let p = h.app.profiles[0].id;
    let id = h.app.conns.aux(p, db).expect("an aux session").id;
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Aux(id), generation: id, ev });
}

/// Forget the server's databases the harness answered with: what is not known yet goes to
/// the server (only names a list that was read does not have are refused).
fn unknown_databases(h: &mut Harness) {
    let p = h.app.profiles[0].id;
    h.app.conns.entry(p).databases = None;
}

/// The options of the last session the app opened.
fn last_opts(h: &Harness) -> (SessionRole, SessionContext) {
    let s = h.driver.sessions.lock().unwrap();
    let last = s.last().unwrap();
    (last.role, last.opts.context.clone())
}

#[test]
fn the_picker_lists_databases_and_schemas_and_binds_the_tab_there() {
    let mut h = Harness::connected(Lang::En);
    unknown_databases(&mut h);
    let p = h.app.profiles[0].id;
    h.sent();
    h.keys(" cd");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)), "asked for the databases");
    assert_eq!(rows(&h), [QuickRow::Profile(p), QuickRow::Note(p, None, QuickNote::Loading)]);
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
    // The tab's database is open (its schemas from the explorer's tree), the cursor on it.
    let own = ["analytics", "public", "shop"].map(|s| QuickRow::Schema(p, "datarig".into(), s.into()));
    let want: Vec<QuickRow> = [QuickRow::Profile(p), QuickRow::Database(p, "datarig".into())]
        .into_iter()
        .chain(own.iter().cloned())
        .chain([QuickRow::Database(p, "sales".into())])
        .collect();
    assert_eq!(rows(&h), want);
    assert_eq!(selected(&h), Some(QuickRow::Database(p, "datarig".into())));
    let screen = h.screen(120, 40);
    assert!(screen.contains("Database and schema of this tab") && screen.contains("▾ datarig"), "{screen}");
    // `→` on another database opens a metadata session there for its schemas.
    select(&mut h, &QuickRow::Database(p, "sales".into()));
    h.key(KeyCode::Right);
    assert_eq!(last_opts(&h), (SessionRole::Meta, ctx(Some("sales"), None)));
    assert_eq!(rows(&h).last(), Some(&QuickRow::Note(p, Some("sales".into()), QuickNote::Loading)));
    aux_db(&mut h, "sales", DbEvent::Connected);
    aux_db(&mut h, "sales", DbEvent::Schemas(Ok(vec!["public".into(), "q1".into()])));
    assert_eq!(rows(&h).last(), Some(&QuickRow::Schema(p, "sales".into(), "q1".into())));
    // Typing filters every level.
    h.type_text("q1");
    assert_eq!(
        rows(&h),
        [QuickRow::Profile(p), QuickRow::Database(p, "sales".into()), QuickRow::Schema(p, "sales".into(), "q1".into())]
    );
    select(&mut h, &QuickRow::Schema(p, "sales".into(), "q1".into()));
    let binding = h.app.tab().binding;
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().context, ctx(Some("sales"), Some("q1")));
    assert_ne!(h.app.tab().binding, binding, "a new binding");
    // The next run opens the query session there.
    h.ctrl('e');
    assert_eq!(last_opts(&h), (SessionRole::Query, ctx(Some("sales"), Some("q1"))));
    let t = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    let ev = DbEvent::Context { database: "sales".into(), schemas: vec!["q1".into()] };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    let line = h.screen(120, 40).lines().nth(2).unwrap().to_string();
    assert!(line.contains("local-pg / sales / q1  127.0.0.1:55432"), "{line}");
    let id = h.app.tab().exec.query_id;
    let done = DbEvent::Done { id, outcome: datarig_core::driver::Outcome::Affected(0), elapsed: Default::default() };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev: done });
    // The profile's own database with a schema: no database of its own (the defaults' session).
    h.keys(" cd");
    assert_eq!(selected(&h), Some(QuickRow::Schema(p, "sales".into(), "q1".into())), "where the tab works");
    select(&mut h, &QuickRow::Database(p, "datarig".into()));
    h.key(KeyCode::Right);
    select(&mut h, &QuickRow::Schema(p, "datarig".into(), "shop".into()));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().context, ctx(None, Some("shop")));
    // Enter on the profile: its defaults.
    h.keys(" cd");
    select(&mut h, &QuickRow::Profile(p));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().context, SessionContext::default());
}

#[test]
fn quick_connect_opens_a_console_in_a_schema() {
    let mut h = Harness::connected(Lang::En);
    let p = h.app.profiles[0].id;
    h.ctrl('o');
    assert_eq!(rows(&h), [QuickRow::Profile(p)]);
    h.key(KeyCode::Right);
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into()])));
    select(&mut h, &QuickRow::Database(p, "datarig".into()));
    h.key(KeyCode::Right);
    select(&mut h, &QuickRow::Schema(p, "datarig".into(), "shop".into()));
    let n = h.app.tabs.len();
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), n + 1, "a new console");
    assert_eq!(h.app.tab().context, ctx(None, Some("shop")));
    assert_eq!(h.app.focus, Focus::Editor);
    // `←` goes back up and closes.
    h.ctrl('o');
    h.key(KeyCode::Right);
    select(&mut h, &QuickRow::Database(p, "datarig".into()));
    h.key(KeyCode::Left);
    assert_eq!(selected(&h), Some(QuickRow::Profile(p)));
    h.key(KeyCode::Left);
    assert_eq!(rows(&h), [QuickRow::Profile(p)]);
}

#[test]
fn use_switches_the_tab_and_asks_first_with_a_transaction_open() {
    let mut h = Harness::connected(Lang::En);
    unknown_databases(&mut h);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.tab_db(
        0,
        DbEvent::Done {
            id,
            outcome: datarig_core::driver::Outcome::Command("BEGIN".into()),
            elapsed: Default::default(),
        },
    );
    h.tab_db(0, DbEvent::TxOpen(true));
    h.tab_db(0, DbEvent::Block(true));
    let binding = h.app.tab().binding;
    h.command("use .shop");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "an open transaction asks");
    let screen = h.screen(120, 40);
    assert!(screen.contains("Change this tab's database or schema?") && screen.contains("n / Enter keep"), "{screen}");
    // Enter keeps (Cancel is the default).
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!((h.app.tab().binding, h.app.tab().context.clone()), (binding, SessionContext::default()));
    assert!(h.app.tab().exec.tx_open);
    // `y` switches: a rebind, the session closed (the server rolls back).
    h.command("use .shop");
    h.keys("y");
    assert_eq!(h.app.tab().context, ctx(None, Some("shop")));
    assert_ne!(h.app.tab().binding, binding);
    assert!(h.app.tab().exec.session.is_none() && !h.app.tab().exec.tx_open);
    // Nothing open: at once. A quoted name keeps its dot; the profile's own database is its
    // default.
    h.command("use \"odd.db\".\"My Schema\"");
    assert_eq!(h.app.tab().context, ctx(Some("odd.db"), Some("My Schema")));
    h.command("use datarig");
    assert_eq!(h.app.tab().context, SessionContext::default());
    // A bad argument says how.
    h.key(KeyCode::Char(':'));
    h.type_text("use a.b.c");
    h.key(KeyCode::Enter);
    assert!(h.screen(120, 40).contains("Usage: :use <database>"), "{}", h.screen(120, 40));
}

#[test]
fn a_missing_schema_is_said() {
    let mut h = Harness::connected(Lang::En);
    unknown_databases(&mut h);
    // A database whose schemas are not read yet: the server says (one the app knows is not
    // there is refused before).
    h.command("use sales.nope");
    h.ctrl('e');
    let t = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    let ev = DbEvent::Context { database: "sales".into(), schemas: vec![] };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    let want = Msg::ContextSchemaMissing { schema: "nope".into(), database: "sales".into() };
    assert_eq!(h.app.tab().status.as_ref().map(|n| n.msg.clone()), Some(want.clone()));
    let text = I18n::new(Lang::En).msg(&want).to_string();
    assert!(text.contains("nope") && text.contains("sales"), "{text}");
}

#[test]
fn the_explorer_opens_a_console_in_a_schema() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::F(6));
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.keys("O");
    assert_eq!(h.app.tab().context, ctx(None, Some("shop")));
    assert_eq!(h.app.focus, Focus::Editor);
    // It is in the node's context menu too.
    h.key(KeyCode::F(6));
    h.keys("k");
    let items = h.app.menu_items();
    assert!(items.contains(&datarig_tui::app::action::by_id("explorer.new_console_here").unwrap().action));
}

#[test]
fn completion_resolves_names_in_the_tabs_schema() {
    let mut h = Harness::connected(Lang::En);
    h.command("use .shop");
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new("");
    h.keys("i");
    h.type_text("select * from us");
    h.settle();
    let labels: Vec<String> = h.app.tab().popup.as_ref().unwrap().items.iter().map(|c| c.label.clone()).collect();
    assert_eq!(labels.first().map(String::as_str), Some("users"), "{labels:?}");
    // Another database completes from its own catalog, never this one's. (`sales` is not in the
    // list the harness read: the server is asked again, and has it.)
    h.key(KeyCode::Esc);
    h.command("use sales.shop");
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
    assert_eq!(h.app.tab().context, ctx(Some("sales"), Some("shop")));
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new("");
    h.keys("i");
    h.type_text("select * from us");
    h.settle();
    assert!(h.app.tab().popup.as_ref().is_none_or(|p| p.items.iter().all(|c| !c.label.contains("users"))));
}

/// A server that dropped the schema's startup option (a pooler): the tab says, in
/// the run's Messages and the status bar, that each transaction sets the path instead.
#[test]
fn a_path_set_per_transaction_is_said_in_the_runs_messages() {
    let mut h = Harness::connected(Lang::En);
    h.command("use .shop");
    h.ctrl('e');
    let t = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev: DbEvent::ContextPerTransaction });
    let want = Msg::ContextPerTransaction { schema: "shop".into() };
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == want), "{:?}", h.app.tab().exec.run.notes);
    assert_eq!(h.app.tab().status.as_ref().map(|n| n.msg.clone()), Some(want.clone()));
    let text = I18n::new(Lang::En).msg(&want).to_string();
    assert!(text.contains("each transaction sets search_path to shop"), "{text}");
}

/// The session opens with the first run, whose outcome replaced the
/// missing-schema status at once. The warning now stays: the run's Messages keep it and the
/// editor's first line marks the schema (`nope?`) for as long as the server says so.
#[test]
fn a_missing_schema_stays_visible_after_the_run() {
    let mut h = Harness::connected(Lang::En);
    unknown_databases(&mut h);
    h.command("use sales.nope");
    h.ctrl('e');
    let t = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    let ev = DbEvent::Context { database: "sales".into(), schemas: vec!["public".into()] };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    let id = h.app.tab().exec.query_id;
    let done = DbEvent::Done { id, outcome: datarig_core::driver::Outcome::Affected(0), elapsed: Default::default() };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev: done });
    let want = Msg::ContextSchemaMissing { schema: "nope".into(), database: "sales".into() };
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == want), "in the run's Messages");
    let screen = h.screen(120, 40);
    let line = screen.lines().nth(2).unwrap();
    assert!(line.contains("local-pg / sales / nope?"), "{line}");
    assert!(screen.contains("Schema nope is not in database sales"), "Messages shows it:\n{screen}");
    // A schema the server has: no mark.
    h.command("use .shop");
    h.ctrl('e');
    let generation = h.app.tab().exec.generation;
    let ev = DbEvent::Context { database: "datarig".into(), schemas: vec!["shop".into(), "public".into()] };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    let line = h.screen(120, 40).lines().nth(2).unwrap().to_string();
    assert!(line.contains("local-pg / datarig / shop ") && !line.contains("shop?"), "{line}");
}

/// `Enter` runs the `:use` argument as typed, never the top fuzzy
/// completion (`:use zz_db50.` switched to another database); a completion is taken only once
/// `Tab`/`↑`/`↓` picked it. A database or schema the app knows is not there is an error, and
/// nothing switches (once the server, asked again, does not have it either);
/// unquoted names fold to lower case.
#[test]
fn use_runs_what_was_typed() {
    let mut h = Harness::connected(Lang::En);
    let p = h.app.profiles[0].id;
    let dbs = ["datarig", "sales", "zz_db50", "zz_db 50.b"].map(String::from).to_vec();
    h.app.conns.entry(p).databases = Some(Ok(dbs));
    let binding = h.app.tab().binding;
    // A typo: the usage, nothing switched (the top completion was "zz_db 50.b").
    h.command("use zz_db50.");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands), "the command line stays");
    assert!(h.screen(120, 40).contains("Usage: :use <database>"), "{}", h.screen(120, 40));
    assert_eq!((h.app.tab().binding, h.app.tab().context.clone()), (binding, SessionContext::default()));
    h.key(KeyCode::Esc);
    // No entry is shown selected while the typed text is what Enter runs.
    h.ctrl('k');
    h.type_text("use zz_db5");
    assert!(h.app.typed_context().is_some());
    h.key(KeyCode::Esc);
    // An unknown database: the server is asked again; not there either: an error, nothing
    // switched.
    h.sent();
    h.command("use nosuch");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)), "asked again");
    let dbs = ["datarig", "sales", "zz_db50", "zz_db 50.b"].map(String::from).to_vec();
    h.meta_db("local-pg", DbEvent::Databases(Ok(dbs)));
    let screen = h.screen(120, 40);
    assert!(screen.contains("There is no database nosuch on this server"), "{screen}");
    assert_eq!(h.app.tab().binding, binding);
    // An unknown schema of the tab's database (its schemas are read): asked again, then an
    // error.
    h.command("use .nope");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadSchemas)), "asked again");
    h.meta_db("local-pg", DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    assert!(h.screen(120, 40).contains("There is no schema nope in database datarig"));
    assert_eq!(h.app.tab().binding, binding);
    // `Tab` picks the top completion, then Enter takes it.
    h.ctrl('k');
    h.type_text("use .sho");
    h.key(KeyCode::Tab);
    assert!(h.app.typed_context().is_none(), "picked");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().context, ctx(None, Some("shop")));
    // Unquoted names fold: `Sales` is `sales`.
    h.command("use Sales");
    assert_eq!(h.app.tab().context, ctx(Some("sales"), None));
    // A schema of another database not read yet goes to the server.
    h.command("use sales.q1");
    assert_eq!(h.app.tab().context, ctx(Some("sales"), Some("q1")));
}

/// A run the connection change ended (`:use`, `Space c d`, `Space c s`
/// with `y`) gets its terminal note, "cancelled by the connection change", in Messages and the
/// status bar; its statement is cancelled, never left `waiting`.
#[test]
fn a_run_ended_by_a_connection_change_says_so() {
    use datarig_tui::app::runlog::StatementOutcome;
    let mut h = Harness::connected(Lang::En);
    for switch in ["use", "cd", "cs"] {
        h.ctrl('e');
        assert!(h.app.tab().exec.running.is_some(), "{switch}: running");
        match switch {
            "use" => h.command("use .shop"),
            "cd" => h.keys(" cd"),
            _ => h.keys(" cs"),
        }
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "{switch}: a running query asks");
        h.keys("y");
        if switch != "use" {
            // The picker: the tab's profile with its defaults (or back to them).
            let p = h.app.profiles[0].id;
            if switch == "cs" {
                select(&mut h, &QuickRow::Profile(p));
            } else {
                h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
                select(&mut h, &QuickRow::Database(p, "sales".into()));
            }
            h.key(KeyCode::Enter);
        }
        let t = h.app.tab();
        assert!(t.exec.running.is_none(), "{switch}");
        let outcomes: Vec<StatementOutcome> = t.exec.run.statements.iter().map(|s| s.outcome.clone()).collect();
        assert_eq!(outcomes, [StatementOutcome::Cancelled], "{switch}");
        let want = datarig_core::i18n::Label::QueryCancelledRebound;
        assert!(t.exec.run.notes.iter().any(|n| n.msg == Msg::Label(want)), "{switch}: {:?}", t.exec.run.notes);
        assert_eq!(t.status.as_ref().map(|n| n.msg.clone()), Some(Msg::Label(want)), "{switch}");
        let screen = h.screen(120, 40);
        assert!(screen.contains("Query cancelled by the connection change"), "{switch}\n{screen}");
        assert!(!screen.contains("waiting"), "{switch}\n{screen}");
    }
}

/// An aux metadata session (another database's) that no tab works in and
/// nothing used for `AUX_IDLE` closes; what it read stays. The next use opens it again; one a
/// tab works in stays open; a failed one says why and opens again on the next use.
#[test]
fn an_idle_aux_session_closes_and_opens_again_on_use() {
    let mut h = Harness::connected(Lang::En);
    let p = h.app.profiles[0].id;
    let open_sales = |h: &mut Harness| {
        h.ctrl('o');
        h.key(KeyCode::Right);
        h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
        select(h, &QuickRow::Database(p, "sales".into()));
        h.key(KeyCode::Right);
    };
    open_sales(&mut h);
    let first = h.roles().len() - 1;
    assert_eq!(last_opts(&h), (SessionRole::Meta, ctx(Some("sales"), None)));
    aux_db(&mut h, "sales", DbEvent::Connected);
    aux_db(&mut h, "sales", DbEvent::Schemas(Ok(vec!["public".into(), "q1".into()])));
    h.key(KeyCode::Esc);
    assert!(h.app.needs_tick(), "the idle close waits for time");
    h.advance(datarig_tui::app::AUX_IDLE - std::time::Duration::from_secs(1));
    assert!(!h.session_closed(first), "not yet");
    h.advance(std::time::Duration::from_secs(1));
    assert!(h.session_closed(first), "closed after AUX_IDLE unused");
    let a = h.app.conns.aux(p, "sales").expect("kept");
    assert!(a.session.is_none() && matches!(&a.schemas, Some(Ok(s)) if s.len() == 2), "what it read stays");
    // The next use opens it again, and the list shows what was read meanwhile.
    open_sales(&mut h);
    assert_eq!(h.roles().len(), first + 2, "a new session");
    assert_eq!(last_opts(&h), (SessionRole::Meta, ctx(Some("sales"), None)));
    assert_eq!(rows(&h).last(), Some(&QuickRow::Schema(p, "sales".into(), "q1".into())));
    h.key(KeyCode::Esc);
    // A tab works in `sales`: its session (the one opened again) stays open.
    let second = first + 1;
    h.command("use sales");
    h.advance(datarig_tui::app::AUX_IDLE * 3);
    assert!(!h.session_closed(second), "kept while a tab works there");
    assert!(h.app.conns.aux(p, "sales").is_some_and(|a| a.session.is_some()));
    // A failure says why; the next use opens it again.
    aux_db(&mut h, "sales", DbEvent::ConnectFailed { error: datarig_core::driver::DbError::Closed, auth: false });
    let status = h.status(160, 45);
    assert!(status.contains("Database sales could not be read"), "{status}");
    let before = h.roles().len();
    h.command("use datarig");
    open_sales(&mut h);
    assert_eq!(h.roles().len(), before + 1, "opened again");
}

/// Behind a pooler a statement that cannot run in a transaction
/// (the driver's `NeedsNoTransaction`) is said with why and the workaround, in the run's Messages
/// and the status bar: `:use` the tab's database without a schema. Any other failure is said as
/// before.
#[test]
fn a_statement_that_needs_no_transaction_says_how_to_run_it() {
    use datarig_core::driver::DbError;
    let mut h = Harness::connected(Lang::En);
    unknown_databases(&mut h);
    for (db, used) in [("datarig", "use .shop"), ("sales", "use sales.q1")] {
        h.command(used);
        h.ctrl('e');
        let t = h.app.tab().id;
        let generation = h.app.tab().exec.generation;
        let db_ev = |ev| AppEvent::Db { target: EventTarget::Tab(t), generation, ev };
        h.app.on_app_event(db_ev(DbEvent::ContextPerTransaction));
        let id = h.app.tab().exec.query_id;
        let error = DbError::NeedsNoTransaction("ERROR: VACUUM cannot run inside a transaction block".into());
        h.app.on_app_event(db_ev(DbEvent::Failed { id, error, cancelled: false }));
        let want = Msg::ContextNoTransaction { database: db.into() };
        assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == want), "{:?}", h.app.tab().exec.run.notes);
        let status = h.status(250, 40);
        assert!(status.contains("VACUUM cannot run inside a transaction block"), "{status}");
        assert!(status.contains(&format!(":use {db} without a schema")), "{status}");
        h.app.tab_mut().exec.view = datarig_tui::app::tabs::ResultView::Messages;
        let screen = h.screen(160, 45);
        // Whole, wrapped: its last words too.
        assert!(
            screen.contains("cannot run inside one") && screen.contains("then qualify names with their schema"),
            "{screen}"
        );
    }
    // A plain server error: no hint.
    h.ctrl('e');
    let t = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    let id = h.app.tab().exec.query_id;
    let ev = DbEvent::Failed { id, error: DbError::Server("ERROR: boom".into()), cancelled: false };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    assert!(!h.app.tab().exec.run.notes.iter().any(|n| matches!(n.msg, Msg::ContextNoTransaction { .. })));
    assert!(!h.status(250, 40).contains("without a schema"));
}

/// In a tab whose path is set per transaction (a pooler), a statement that sets
/// `search_path` for the session runs (never blocked), and its run's Messages warn once that
/// the tab ignores it and that it stays on the pooled connection; known once the session said
/// so (the run that opened it too), from the parse tree (`SET LOCAL` does not warn).
#[test]
fn a_session_level_search_path_behind_a_pooler_is_warned_about() {
    let mut h = Harness::connected(Lang::En);
    let warning = Msg::ContextSessionPath { schema: "shop".into() };
    let warned = |h: &Harness| h.app.tab().exec.run.notes.iter().filter(|n| n.msg == warning).count();
    let run = |h: &mut Harness, sql: &str| {
        h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(sql);
        h.ctrl('e');
    };
    // Directly connected (the option applied): nothing to say.
    h.command("use .shop");
    run(&mut h, "SET search_path TO analytics");
    assert!(h.app.tab().exec.running.is_some(), "sent");
    assert_eq!(warned(&h), 0);
    let t = h.app.tab().id;
    let generation = h.app.tab().exec.generation;
    let finish = |h: &mut Harness| {
        let id = h.app.tab().exec.query_id;
        let ev = DbEvent::Done {
            id,
            outcome: datarig_core::driver::Outcome::Command("SET".into()),
            elapsed: Default::default(),
        };
        h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    };
    // The session says the path is set per transaction while that run is on: it is warned.
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev: DbEvent::ContextPerTransaction });
    assert_eq!(warned(&h), 1, "{:?}", h.app.tab().exec.run.notes);
    finish(&mut h);
    h.app.tab_mut().exec.view = datarig_tui::app::tabs::ResultView::Messages;
    let screen = h.screen(160, 45);
    assert!(screen.contains("is ignored by this tab") && screen.contains("for other clients"), "{screen}");
    // Known now: warned when sent; once per run.
    run(&mut h, "SELECT set_config('search_path', 'analytics', false)");
    assert_eq!(warned(&h), 1);
    finish(&mut h);
    for fine in ["SET LOCAL search_path TO analytics", "SELECT 1", "SHOW search_path"] {
        run(&mut h, fine);
        assert_eq!(warned(&h), 0, "{fine}");
        finish(&mut h);
    }
}

/// `:use` to where the tab works already, with a query running: nothing to switch,
/// so nothing is asked and the query goes on (it asked "Switching cancels it", then did nothing).
#[test]
fn use_to_the_current_context_is_a_no_op() {
    let mut h = Harness::connected(Lang::En);
    h.command("use .shop");
    let binding = h.app.tab().binding;
    h.ctrl('e');
    assert!(h.app.tab().exec.running.is_some());
    for same in ["use .shop", "use datarig.shop", "use .SHOP"] {
        h.command(same);
        assert_ne!(h.overlay_kind(), Some(OverlayKind::Confirm), "{same}: nothing to ask");
        assert!(h.app.tab().exec.running.is_some(), "{same}: the query goes on");
        assert!(!h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "{same}: not cancelled");
        assert_eq!(h.app.tab().binding, binding, "{same}");
    }
    // Another schema still asks.
    h.command("use .analytics");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
}

/// Why another database cannot be opened is seen whole: the explorer's note and
/// the status bar cut it, so it goes to `errors.log` and to the Messages (the app's own entries,
/// one per database, replaced when it fails again). Once it opens, the entry and the status
/// bar's old error go.
#[test]
fn why_a_database_cannot_be_opened_is_seen_whole() {
    use datarig_core::driver::DbError;
    let state = std::env::temp_dir().join(format!("datarig-aux-reason-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let mut h = Harness::connected(Lang::En);
    h.app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.clone()) });
    let p = h.app.profiles[0].id;
    let open_sales = |h: &mut Harness| {
        h.ctrl('o');
        h.key(KeyCode::Right);
        h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
        select(h, &QuickRow::Database(p, "sales".into()));
        h.key(KeyCode::Right);
        h.key(KeyCode::Esc);
    };
    open_sales(&mut h);
    let reason = "FATAL: database \"sales\" is not currently accepting connections (the end of a long reason)";
    for _ in 0..2 {
        aux_db(&mut h, "sales", DbEvent::ConnectFailed { error: DbError::Server(reason.into()), auth: false });
        open_sales(&mut h);
    }
    aux_db(&mut h, "sales", DbEvent::ConnectFailed { error: DbError::Server(reason.into()), auth: false });
    let entries = |h: &Harness| h.app.notices.iter().filter(|n| matches!(n.msg, Msg::AuxFailed { .. })).count();
    assert_eq!(entries(&h), 1, "one entry per database, replaced");
    h.app.tab_mut().exec.view = datarig_tui::app::tabs::ResultView::Messages;
    let screen = h.screen(120, 40);
    assert!(screen.contains("(the end of a long reason)"), "whole in Messages:\n{screen}");
    let log = std::fs::read_to_string(state.join("errors.log")).expect("logged");
    assert!(log.contains("aux.failed") && log.contains("the end of a long reason"), "{log}");
    // Opened on a retry: the entry and the status bar's old error go.
    open_sales(&mut h);
    aux_db(&mut h, "sales", DbEvent::Connected);
    aux_db(&mut h, "sales", DbEvent::Schemas(Ok(vec!["public".into()])));
    assert_eq!(entries(&h), 0);
    assert!(!h.status(160, 40).contains("could not be read"), "{}", h.status(160, 40));
    let _ = std::fs::remove_dir_all(&state);
}

/// A schema or database created after the list was read is not refused as
/// if the list were final: the server is asked again (one catalog query on the metadata session,
/// or the aux session of that database), and the switch happens when its answer has the name.
/// When the server cannot be asked, or its answer fails, the status bar says the list may be out
/// of date; an answer for a tab that is no longer active switches nothing.
#[test]
fn use_asks_the_server_before_refusing_a_name_a_stale_list_lacks() {
    use datarig_core::driver::DbError;
    let mut h = Harness::connected(Lang::En);
    let p = h.app.profiles[0].id;
    h.sent();
    // A schema of the profile's own database created after the tree was read.
    h.command("use .zz_new");
    assert!(h.status(160, 40).contains("asking the server again"), "{}", h.status(160, 40));
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadSchemas)));
    assert_eq!(h.app.tab().context, SessionContext::default(), "not yet");
    let schemas = vec!["analytics".into(), "public".into(), "shop".into(), "zz_new".into()];
    h.meta_db("local-pg", DbEvent::Schemas(Ok(schemas)));
    assert_eq!(h.app.tab().context, ctx(None, Some("zz_new")), "the server has it");
    assert!(!h.status(160, 40).contains("asking the server"), "no longer asking");
    // A database created after the list was read (the harness read `datarig` only).
    h.command("use zz_newdb");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)));
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "zz_newdb".into()])));
    assert_eq!(h.app.tab().context, ctx(Some("zz_newdb"), None));
    // A schema of another database whose list was read: its aux session reads it again (a run
    // there opened it).
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let outcome = datarig_core::driver::Outcome::Affected(0);
    h.tab_db(0, DbEvent::Done { id, outcome, elapsed: Default::default() });
    aux_db(&mut h, "zz_newdb", DbEvent::Connected);
    aux_db(&mut h, "zz_newdb", DbEvent::Schemas(Ok(vec!["public".into()])));
    let aux = h.app.conns.aux(p, "zz_newdb").unwrap().id;
    let aux_index = h
        .driver
        .sessions
        .lock()
        .unwrap()
        .iter()
        .rposition(|s| s.opts.context.database.as_deref() == Some("zz_newdb") && s.role == SessionRole::Meta)
        .unwrap();
    h.sent_to(aux_index);
    h.command("use .q2");
    assert!(h.sent_to(aux_index).iter().any(|c| matches!(c, DbCommand::LoadSchemas)), "on its aux session");
    assert_eq!(h.app.conns.aux(p, "zz_newdb").unwrap().id, aux, "the same session");
    aux_db(&mut h, "zz_newdb", DbEvent::Schemas(Ok(vec!["public".into(), "q2".into()])));
    assert_eq!(h.app.tab().context, ctx(Some("zz_newdb"), Some("q2")));
    // An answer that fails: the list may be out of date, nothing switched.
    h.command("use zz_other");
    h.meta_db("local-pg", DbEvent::Databases(Err(DbError::Server("ERROR: boom".into()))));
    let status = h.status(200, 40);
    assert!(status.contains("zz_other is not in the list read earlier") && status.contains("press r"), "{status}");
    assert_eq!(h.app.tab().context, ctx(Some("zz_newdb"), Some("q2")));
    // An answer for a tab that is no longer the active one: nothing switches.
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "zz_newdb".into()])));
    h.command("use zz_later");
    let first = h.app.tab().id;
    h.ctrl('t');
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "zz_later".into()])));
    assert!(h.app.tabs.iter().all(|t| t.context.database.as_deref() != Some("zz_later")), "{first:?}");
    // The server cannot be asked (the profile's connection is gone): said, nothing switched.
    h.meta_db("local-pg", DbEvent::Lost { error: DbError::Closed });
    h.command("use zz_gone");
    let status = h.status(200, 40);
    assert!(status.contains("the list may be out of date (press r"), "{status}");
    assert_eq!(h.app.tab().context.database, None);
}

/// A tab of `local-pg` in `shop` whose session sets its path in each transaction (a pooler),
/// with a run on: the session said so while it ran.
fn behind_a_pooler() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.command("use .shop");
    run_sql(&mut h, "SELECT 1");
    let (t, generation) = (h.app.tab().id, h.app.tab().exec.generation);
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev: DbEvent::ContextPerTransaction });
    finish_command(&mut h, "SELECT");
    h
}

fn run_sql(h: &mut Harness, sql: &str) {
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(sql);
    h.ctrl('e');
}

/// The running run ends with a command tag.
fn finish_command(h: &mut Harness, tag: &str) {
    let (t, generation, id) = (h.app.tab().id, h.app.tab().exec.generation, h.app.tab().exec.query_id);
    let outcome = datarig_core::driver::Outcome::Command(tag.into());
    let ev = DbEvent::Done { id, outcome, elapsed: Default::default() };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
}

/// A session-level `set_config('search_path', …)` that returns a row
/// shows its grid, so the warning in Messages was out of sight: the status bar says it too
/// when the run ends.
#[test]
fn the_session_path_warning_of_a_run_with_rows_is_in_the_status_bar() {
    let mut h = behind_a_pooler();
    run_sql(&mut h, "SELECT set_config('search_path', 'analytics', false)");
    let (t, generation, id) = (h.app.tab().id, h.app.tab().exec.generation, h.app.tab().exec.query_id);
    let (cols, rows) = edge_rows();
    let ev = DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Default::default() };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    assert_eq!(h.app.tab().exec.view, datarig_tui::app::tabs::ResultView::Rows, "the grid is shown");
    let status = h.status(250, 40);
    assert!(status.contains("is ignored by this tab"), "{status}");
    // A run without the warning still says its rows.
    run_sql(&mut h, "SELECT 1");
    let (cols, rows) = edge_rows();
    let id = h.app.tab().exec.query_id;
    let ev = DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Default::default() };
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Tab(t), generation, ev });
    let status = h.status(250, 40);
    assert!(!status.contains("is ignored by this tab") && status.contains("rows"), "{status}");
}

/// `EXECUTE` of a statement prepared with a session-level
/// `set_config('search_path', …)` sets the path for the session as well: warned, through the
/// session's prepared statements (an earlier run's, or this run's own `PREPARE`).
#[test]
fn execute_of_a_prepared_session_level_set_config_is_warned_about() {
    let mut h = behind_a_pooler();
    let warning = Msg::ContextSessionPath { schema: "shop".into() };
    let warned = |h: &Harness| h.app.tab().exec.run.notes.iter().filter(|n| n.msg == warning).count();
    run_sql(&mut h, "PREPARE p AS SELECT set_config('search_path', 'analytics', false)");
    assert_eq!(warned(&h), 0, "preparing runs nothing");
    finish_command(&mut h, "PREPARE");
    run_sql(&mut h, "EXECUTE p");
    assert_eq!(warned(&h), 1, "{:?}", h.app.tab().exec.run.notes);
    finish_command(&mut h, "SELECT");
    // Prepared and run in one run.
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(
        "PREPARE q AS SELECT set_config('search_path', 'analytics', false);\nEXECUTE q;",
    );
    h.keys("ggvG$");
    h.ctrl('e');
    assert_eq!(h.app.tab().exec.run.statements.len(), 2, "one run of both");
    assert_eq!(warned(&h), 1);
    finish_command(&mut h, "SELECT");
    // A local one is fine.
    run_sql(&mut h, "PREPARE r AS SELECT set_config('search_path', 'analytics', true)");
    finish_command(&mut h, "PREPARE");
    run_sql(&mut h, "EXECUTE r");
    assert_eq!(warned(&h), 0);
}

/// The error log line of a database that could not be opened holds
/// the underlying error, not the UI's words for it ("… (details in errors.log)", pointing at
/// itself).
#[test]
fn the_error_log_holds_the_underlying_reason_a_database_cannot_be_opened() {
    use datarig_core::driver::DbError;
    use datarig_core::fault::{Fault, FaultKind};
    let state = std::env::temp_dir().join(format!("datarig-aux-raw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let mut h = Harness::connected(Lang::En);
    h.app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.clone()) });
    let p = h.app.profiles[0].id;
    h.ctrl('o');
    h.key(KeyCode::Right);
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
    select(&mut h, &QuickRow::Database(p, "sales".into()));
    h.key(KeyCode::Right);
    h.key(KeyCode::Esc);
    let fault = Fault::new(FaultKind::Other, "tls handshake eof while reading the server's reply");
    aux_db(&mut h, "sales", DbEvent::ConnectFailed { error: DbError::Connection(fault), auth: false });
    let log = std::fs::read_to_string(state.join("errors.log")).expect("logged");
    let line = log.lines().find(|l| l.contains("aux.failed")).expect("an aux.failed line");
    assert!(line.contains("sales: tls handshake eof while reading the server's reply"), "{log}");
    assert!(!line.contains("details in errors.log"), "{log}");
    let _ = std::fs::remove_dir_all(&state);
}
