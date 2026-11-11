//! A table tab's filter and order are kept: in the workspace across a restart, and with the tab
//! when it is closed and brought back. They come back in the tab as applied and are not
//! run; the run the user then asks for checks them again as any filter is checked (a file edited
//! by hand may hold a filter the dialog would refuse, or one whose functions must be confirmed).

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::paths::Paths;
use datarig_core::query::{Dir, OrderKey};
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_i18n::{Lang, Msg};
use datarig_tui::app::Startup;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::table_query::{Field, filter_text};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// A state directory of the test's own, removed when it ends.
struct State(PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-filter-keep-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("workspace.toml")
    }

    /// A workspace of one table tab on `shop.users` of the profile of `cfg`, with `keys`.
    fn write_table_tab(&self, cfg: &Config, keys: &str) {
        let p = cfg.connections[0].id;
        let text = format!(
            "version = 3\nactive = 0\n\n[[tabs]]\nid = \"cccc\"\nkind = \"table\"\nprofile = \"{p}\"\n\
             schema = \"shop\"\ntable = \"users\"\n{keys}\n"
        );
        std::fs::write(self.file(), text).unwrap();
    }

    /// The file's text, and its table tab as read back.
    fn saved(&self) -> (String, datarig_core::workspace::TabState) {
        let text = std::fs::read_to_string(self.file()).unwrap();
        let mut l = datarig_core::workspace::load(&self.0);
        assert!(l.broken.is_none(), "{text}");
        let i = l.state.tabs.iter().position(|t| t.table.is_some()).expect("a table tab");
        (text, l.state.tabs.remove(i))
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn launch(cfg: &Config, state: &State, lang: Lang) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, lang);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    let h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    let mut h = h.with_fake_driver();
    h.app.launch(Startup::Normal);
    h
}

/// Connect the profile from the explorer (its metadata session; no tab runs).
fn connect(h: &mut Harness) {
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
}

/// Open `shop.users` from the explorer (the profile connected): a table tab, which runs.
fn open_users(h: &mut Harness) {
    h.explore("local-pg");
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".to_string()], Vec::new()).into()) });
    h.keys("jj"); // Tables, users
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with a first page of `shop.users` rows.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// An ordered query put back in a tab waits for the table's keys before it runs: nothing is
/// sent but the keys' read, then answered with the primary key `id`, the run that waited.
fn keys_first(h: &mut Harness) -> Vec<Vec<String>> {
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Execute { .. })), "waits for the keys: {sent:?}");
    let request = sent.iter().find_map(|c| match c {
        DbCommand::LoadTableKeys { id, .. } => Some(*id),
        _ => None,
    });
    let keys = datarig_core::driver::keys::OrderKeys { primary_key: vec!["id".into()], ..Default::default() };
    h.meta_db("local-pg", DbEvent::TableKeys { id: request.expect("the keys are asked for"), result: Ok(keys) });
    runs(h)
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The condition and the order the filter dialog opens with (opened, then closed again).
fn line_texts(h: &mut Harness) -> (String, String) {
    use datarig_tui::app::action::Action;
    use datarig_tui::app::table_filter::TableAction;
    h.app.dispatch(Action::Table(TableAction::EditFilter));
    let texts = h.app.filter_dialog_draft().expect("the filter dialog");
    h.key(KeyCode::Esc);
    assert!(!h.app.overlays.is_open(OverlayKind::FilterDialog));
    texts
}

/// What the active table tab holds as applied: its filter text and its order.
fn applied(h: &Harness) -> (Option<String>, Vec<OrderKey>) {
    let q = h.app.tab().doc.table_query.as_ref().expect("a table query").applied();
    (filter_text(q).map(str::to_string), q.order.clone())
}

fn quit(mut h: Harness) {
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
}

/// The filter and order applied in a table tab are written to the workspace (a text typed in
/// the line but not applied is not), come back in the line after a restart, and nothing runs
/// until the run key: then the statement sent is the compiled query.
#[test]
fn a_restored_table_tab_keeps_its_query_and_runs_it_only_when_asked() {
    let state = State::new("restore");
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state, Lang::En);
    connect(&mut h);
    open_users(&mut h);
    answer(&mut h);
    h.command("where id > 1");
    answer(&mut h);
    h.command("orderby NAME desc");
    answer(&mut h);
    let want = format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC"#);
    assert_eq!(runs(&mut h).last(), Some(&vec![want.clone()]));
    // A condition built in the dialog, not applied: not kept.
    h.key(KeyCode::Tab);
    h.keys("f");
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text("5");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    quit(h);
    let (text, tab) = state.saved();
    assert_eq!(tab.filter.as_deref(), Some("id > 1"), "{text}");
    assert_eq!(tab.order, [OrderKey::user("name", Dir::Desc)], "{text}");
    assert!(text.contains(r#"order = [{ column = "name", dir = "desc" }]"#) && !text.contains("limit"), "{text}");

    let mut h = launch(&cfg, &state, Lang::En);
    assert!(h.app.tab().is_table());
    assert_eq!(applied(&h), (Some("id > 1".into()), vec![OrderKey::user("name", Dir::Desc)]));
    assert_eq!(line_texts(&mut h), ("id > 1".into(), "name DESC".into()));
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing opened");
    connect(&mut h);
    assert!(runs(&mut h).is_empty(), "nothing runs by itself");
    // The run key: the statement is the compiled query, made total with the keys read first,
    // as the order typed after they came was.
    h.key(KeyCode::Tab);
    h.ctrl('e');
    let total = format!(r#"{want}, "id" ASC"#);
    assert_eq!(keys_first(&mut h), vec![vec![total]]);
    answer(&mut h);
    let shown = h.app.tab().doc.table_query.as_ref().and_then(|q| q.shown.clone()).expect("shown");
    assert_eq!(filter_text(&shown), Some("id > 1"));
    // Saved again as it was.
    quit(h);
    let (text, tab) = state.saved();
    assert_eq!(tab.filter.as_deref(), Some("id > 1"), "{text}");
}

/// A filter edited by hand into the file that the app refuses: back as the tab's query, and the
/// run key sends nothing and says why; applied from the filter dialog, the dialog stays open
/// and says why.
#[test]
fn a_restored_filter_the_line_refuses_sends_nothing() {
    let cfg = test_db_config();
    for filter in ["id = 1; DROP TABLE shop.users", "id = 1 -- x"] {
        let state = State::new("refused");
        state.write_table_tab(&cfg, &format!("filter = {filter:?}"));
        let mut h = launch(&cfg, &state, Lang::En);
        assert_eq!(applied(&h).0.as_deref(), Some(filter));
        assert_eq!(line_texts(&mut h).0, filter);
        connect(&mut h);
        h.key(KeyCode::Tab);
        h.ctrl('e');
        assert!(runs(&mut h).is_empty(), "{filter}: nothing sent");
        assert!(status(&h).starts_with("Filter refused: "), "{filter}: {}", status(&h));
        let refusal = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("refusal");
        assert_eq!(refusal.field, Field::Where);
        // `Enter` in the filter dialog is a run too: refused the same way, the dialog open.
        h.keys("f");
        h.key(KeyCode::Enter);
        assert!(runs(&mut h).is_empty(), "{filter}: nothing sent");
        assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "{filter}");
        assert!(h.screen(120, 34).contains("Filter refused: "), "{filter}: {}", h.screen(120, 34));
        quit(h);
    }
}

/// A filter that calls a function whose effects are not known, confirmed in one run of the
/// app: after a restart it is asked about again before anything is sent.
#[test]
fn a_restored_filter_asks_again_about_its_calls() {
    let state = State::new("calls");
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state, Lang::En);
    connect(&mut h);
    open_users(&mut h);
    answer(&mut h);
    h.command("where my_fn(id) > 0");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    let want = format!("{ALL} WHERE (my_fn(id) > 0)");
    assert_eq!(runs(&mut h).last(), Some(&vec![want.clone()]));
    answer(&mut h);
    quit(h);

    let mut h = launch(&cfg, &state, Lang::En);
    connect(&mut h);
    h.key(KeyCode::Tab);
    let id = h.app.tab().id;
    h.ctrl('e');
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::FilterCalls(id)));
    assert!(runs(&mut h).is_empty(), "nothing sent while it asks");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "declined");
    h.ctrl('e');
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![want]]);
}

/// A table tab closed and brought back keeps its filter and order in the line, not run; the
/// next run checks it again (its calls asked about again) and sends the compiled query.
#[test]
fn a_reopened_table_tab_keeps_its_query() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    answer(&mut h);
    h.command("where my_fn(id) > 0");
    h.keys("y");
    answer(&mut h);
    h.command("orderby id desc");
    answer(&mut h);
    let want = format!(r#"{ALL} WHERE (my_fn(id) > 0) ORDER BY "id" DESC"#);
    assert_eq!(runs(&mut h).last(), Some(&vec![want.clone()]));
    h.key(KeyCode::Tab);
    let tabs = h.app.tabs.len();
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), tabs - 1);
    h.keys(" tu");
    assert!(h.app.tab().is_table());
    assert_eq!(applied(&h), (Some("my_fn(id) > 0".into()), vec![OrderKey::user("id", Dir::Desc)]));
    assert_eq!(line_texts(&mut h), ("my_fn(id) > 0".into(), "id DESC".into()));
    assert!(runs(&mut h).is_empty(), "not run by coming back");
    h.key(KeyCode::Tab);
    h.ctrl('e');
    assert!(h.app.overlays.is_open(OverlayKind::Confirm), "asked again");
    assert!(runs(&mut h).is_empty());
    h.keys("y");
    assert_eq!(keys_first(&mut h), vec![vec![want]]);
    answer(&mut h);
    assert_eq!(line_texts(&mut h).1, "id DESC");
}

/// A query the file holds that cannot be read is said, in words, and left out of the tab.
#[test]
fn a_query_the_file_holds_that_cannot_be_read_is_said() {
    let cfg = test_db_config();
    for lang in [Lang::En, Lang::Ko] {
        let state = State::new("unread");
        state.write_table_tab(&cfg, "filter = 7\norder = [{ column = \"id\", dir = \"desc\" }]");
        let h = launch(&cfg, &state, lang);
        assert!(h.app.notices.iter().any(|n| matches!(n.msg, Msg::WorkspaceQueriesUnread { count: 1 })));
        let texts: Vec<String> = h.app.notices.iter().map(|n| h.app.i18n.msg(&n.msg).to_string()).collect();
        let en = "1 table tab's filter, order or row cap could not be read: it is left out of the tab and kept \
                  in the file as it is";
        assert_eq!(texts.iter().any(|t| t == en), lang == Lang::En, "{texts:?}");
        assert_eq!(applied(&h), (None, vec![OrderKey::user("id", Dir::Desc)]));
        quit(h);
        let (text, tab) = state.saved();
        assert!(text.contains("filter = 7") && tab.filter.is_none(), "kept in the file: {text}");
    }
}

/// A restored tab's line before it runs: drawn as a query applied (no mark of its own: the
/// results say the tab is not loaded yet).
#[test]
fn a_restored_tab_shows_its_query_in_the_line() {
    let cfg = test_db_config();
    let state = State::new("snap");
    state.write_table_tab(
        &cfg,
        "filter = \"status = 'done' AND amount > 1500\"\norder = [{ column = \"id\", dir = \"desc\" }]",
    );
    let mut h = launch(&cfg, &state, Lang::En);
    h.key(KeyCode::Tab);
    assert_screen!("filter_keep_restored_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_keep_restored_en_120x30", Lang::En, h.draw(120, 30));
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing opened");
}

/// A limit kept in the file comes back in the line's `LIMIT` field as applied, is not run, and
/// caps the rows of the run asked for (the statement sent has no `LIMIT`); one set in the line
/// is written back.
#[test]
fn a_restored_limit_shows_in_its_field_and_caps_the_run() {
    let state = State::new("limit");
    let cfg = test_db_config();
    state.write_table_tab(&cfg, "limit = 2");
    let mut h = launch(&cfg, &state, Lang::En);
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().applied().limit, Some(2));
    connect(&mut h);
    assert!(runs(&mut h).is_empty(), "nothing runs by itself");
    h.key(KeyCode::Tab);
    h.ctrl('e');
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
    answer(&mut h);
    match &h.app.tab().results {
        datarig_tui::app::Results::Rows(rs) => assert_eq!((rs.rows.len(), rs.more), (2, false)),
        _ => panic!("no rows"),
    }
    // Another limit typed in the dialog and applied: kept.
    h.keys("f");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.ctrl('u');
    h.type_text("50");
    h.key(KeyCode::Enter);
    answer(&mut h);
    quit(h);
    let (text, tab) = state.saved();
    assert_eq!(tab.limit, Some(50), "{text}");
}

/// A tab put back that has not run yet, its filter refused (edited by hand), its order kept:
/// the refused chip taken out in the filter dialog applies, the order as it was (its columns are not known before the tab runs, and need not be: its keys stand), the
/// profile connected and the keys read first. Never a dead end.
#[test]
fn a_restored_tab_that_never_ran_applies_without_its_refused_chip() {
    let cfg = test_db_config();
    let state = State::new("fixed");
    state.write_table_tab(
        &cfg,
        "filter = \"id = 1; DROP TABLE shop.users\"\norder = [{ column = \"id\", dir = \"desc\" }]",
    );
    let mut h = launch(&cfg, &state, Lang::En);
    assert!(h.app.tab().is_table());
    h.key(KeyCode::Tab);
    h.keys("f");
    // The refused chip out (no columns known yet: `Tab` goes from the list to the chips).
    h.key(KeyCode::Tab);
    h.key(KeyCode::Char('x'));
    assert_eq!(h.app.filter_dialog_draft(), Some((String::new(), "id DESC".into())));
    h.key(KeyCode::Enter);
    assert!(!status(&h).contains("columns are not known"), "{}", status(&h));
    assert_eq!(applied(&h), (None, vec![OrderKey::user("id", Dir::Desc)]));
    // It connects for the keys, then runs.
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    let ran = keys_first(&mut h);
    assert_eq!(ran, vec![vec![format!(r#"{ALL} ORDER BY "id" DESC"#)]]);
}

/// A workspace of one table tab on `shop.users` (in database `database`, when one) ordered by
/// `id` descending, launched: its run waits for the table's keys.
fn ordered(cfg: &Config, state: &State, database: Option<&str>) -> Harness {
    let db = database.map(|d| format!("database = \"{d}\"\n")).unwrap_or_default();
    state.write_table_tab(cfg, &format!("{db}order = [{{ column = \"id\", dir = \"desc\" }}]"));
    let h = launch(cfg, state, Lang::En);
    assert!(h.app.tab().is_table());
    h
}

fn keys_request(sent: &[DbCommand]) -> Option<u64> {
    sent.iter().find_map(|c| match c {
        DbCommand::LoadTableKeys { id, .. } => Some(*id),
        _ => None,
    })
}

fn pk() -> datarig_core::driver::keys::OrderKeys {
    datarig_core::driver::keys::OrderKeys { primary_key: vec!["id".into()], ..Default::default() }
}

/// A run waiting for its table's keys is work in progress of the tab: a cancel ends it, and
/// the keys' answer that comes after runs nothing.
#[test]
fn a_run_waiting_for_its_keys_is_cancelled_and_its_late_answer_runs_nothing() {
    let cfg = test_db_config();
    let state = State::new("keyscancel");
    let mut h = ordered(&cfg, &state, None);
    connect(&mut h);
    h.key(KeyCode::Tab);
    h.ctrl('e');
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Execute { .. })), "{sent:?}");
    let request = keys_request(&sent).expect("the keys are asked for");
    let tab = h.app.tab().id;
    assert!(h.app.tab_busy(tab), "the wait is work in progress");
    h.app.cancel();
    assert!(!h.app.tab_busy(tab));
    assert_eq!(status(&h), "Query cancelled");
    h.meta_db("local-pg", DbEvent::TableKeys { id: request, result: Ok(pk()) });
    assert!(runs(&mut h).is_empty(), "nothing runs after the cancel");
}

/// A connect for the keys that fails at once (an environment that is not defined) ends the
/// wait it was started for, and the tab says so.
#[test]
fn a_connect_for_the_keys_that_fails_at_once_ends_the_wait() {
    let mut cfg = test_db_config();
    cfg.connections[0].policy = Some("gone".into());
    let state = State::new("keyssync");
    let mut h = ordered(&cfg, &state, None);
    h.key(KeyCode::Tab);
    h.ctrl('e');
    let tab = h.app.tab().id;
    assert!(!h.app.tab_busy(tab), "nothing waits");
    assert!(status(&h).contains("which did not connect"), "{}", status(&h));
    assert!(runs(&mut h).is_empty());
}

/// A password refused for the profile's own database asks again, and the run waiting for the
/// keys goes on waiting: once the profile connects, the keys are read and it runs.
#[test]
fn a_run_waiting_for_its_keys_waits_through_a_password_asked_again() {
    let cfg = test_db_config();
    let state = State::new("keysretry");
    let mut h = ordered(&cfg, &state, None);
    h.key(KeyCode::Tab);
    h.ctrl('e');
    let tab = h.app.tab().id;
    assert!(h.app.tab_busy(tab));
    h.db(DbEvent::ConnectFailed { error: "password authentication failed for user \"datarig\"".into(), auth: true });
    assert!(h.prompt().is_some(), "asked again");
    assert!(h.app.tab_busy(tab), "still waiting");
    h.type_text("datarig");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    let ran = keys_first(&mut h);
    assert_eq!(ran, vec![vec![format!(r#"{ALL} ORDER BY "id" DESC"#)]]);
}

/// A run in another database waiting for the keys ends when the profile's attempt fails for
/// good, and says so; connecting later runs nothing by itself.
#[test]
fn a_run_in_another_database_waiting_for_its_keys_ends_with_a_failed_connect() {
    let cfg = test_db_config();
    let state = State::new("keysotherdb");
    let mut h = ordered(&cfg, &state, Some("postgres"));
    h.key(KeyCode::Tab);
    h.ctrl('e');
    let tab = h.app.tab().id;
    assert!(h.app.tab_busy(tab));
    h.db(DbEvent::ConnectFailed { error: "connection refused".into(), auth: false });
    assert!(!h.app.tab_busy(tab), "nothing waits");
    assert!(
        h.app
            .tabs
            .get(tab)
            .and_then(|t| t.status.as_ref())
            .is_some_and(|n| matches!(n.msg, Msg::QueryQueuedFailed { .. }))
    );
    connect(&mut h);
    assert!(runs(&mut h).is_empty(), "nothing runs by itself");
}

/// The metadata session the keys were asked on is lost while the run waits: the run goes
/// without them (the order not taken for total), and the lost session's late answer runs
/// nothing more.
#[test]
fn a_run_waiting_for_its_keys_on_a_lost_metadata_session_runs_once() {
    let cfg = test_db_config();
    let state = State::new("keyslost");
    let mut h = ordered(&cfg, &state, None);
    connect(&mut h);
    h.key(KeyCode::Tab);
    h.ctrl('e');
    let request = keys_request(&h.sent()).expect("the keys are asked for");
    h.db(DbEvent::Lost { error: datarig_core::driver::DbError::Closed });
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "id" DESC"#)]]);
    h.meta_db("local-pg", DbEvent::TableKeys { id: request, result: Ok(pk()) });
    assert!(runs(&mut h).is_empty(), "the late answer runs nothing");
}

/// A restored condition of more than one line (edited into the file by hand: the line cannot
/// hold it as it is) runs as it is, but the filter history never keeps it.
#[test]
fn a_restored_filter_of_more_lines_is_not_kept_in_the_history() {
    let state = State::new("lines");
    let cfg = test_db_config();
    state.write_table_tab(&cfg, "filter = \"id = 1 OR\\nid = 2\"");
    let mut h = launch(&cfg, &state, Lang::En);
    assert_eq!(applied(&h).0.as_deref(), Some("id = 1 OR\nid = 2"));
    connect(&mut h);
    h.key(KeyCode::Tab);
    h.ctrl('e');
    assert_eq!(runs(&mut h).len(), 1, "{}", status(&h));
    answer(&mut h);
    h.app.flush_filter_history();
    let file = state.0.join(datarig_core::filter_history::FILE);
    let kept = datarig_core::filter_history::load(&file).expect("the file reads").table_count();
    assert_eq!(kept, 0, "not kept");
    quit(h);
}

/// A tab put back with a filter, changed before it ever ran (in the filter dialog, or cleared
/// from the grid): the filter dialog's undo still goes back to the filter put back.
#[test]
fn the_first_change_of_a_restored_filter_is_undone_to_it() {
    for clear in [false, true] {
        let cfg = test_db_config();
        let state = State::new(if clear { "undo-clear" } else { "undo-dialog" });
        state.write_table_tab(&cfg, "filter = \"id = 1\"");
        let mut h = launch(&cfg, &state, Lang::En);
        connect(&mut h);
        h.key(KeyCode::Tab);
        if clear {
            h.keys(" rx");
        } else {
            h.keys("f");
            // Its chip out (no columns known yet: `Tab` goes from the list to the chips), then
            // a condition of the SQL section's `WHERE`.
            h.key(KeyCode::Tab);
            h.key(KeyCode::Char('x'));
            h.key(KeyCode::Char('Q'));
            h.type_text("id > 2");
            h.key(KeyCode::Enter);
        }
        let changed = if clear { ALL.to_string() } else { format!("{ALL} WHERE (id > 2)") };
        assert_eq!(runs(&mut h), vec![vec![changed]], "clear: {clear}");
        answer(&mut h);
        h.keys("f");
        h.keys("u");
        assert_eq!(h.app.filter_dialog_draft().map(|d| d.0), Some("id = 1".to_string()), "clear: {clear}");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id = 1)")]], "clear: {clear}");
    }
}

fn dialog_open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::FilterDialog)
}

/// The filter dialog of the tab put back, applied (`Enter`) while its run cannot be sent yet:
/// it stays open, the run waits; `Esc` then closes it and the run waits no more.
fn apply_then_cancel(h: &mut Harness) {
    h.key(KeyCode::Tab);
    h.keys("f");
    assert!(dialog_open(h));
    h.key(KeyCode::Enter);
    let tab = h.app.tab().id;
    assert!(dialog_open(h), "nothing sent yet: the dialog stays open");
    assert!(h.app.tab_busy(tab), "the run waits");
    h.key(KeyCode::Esc);
    assert!(!dialog_open(h));
    assert!(!h.app.tab_busy(tab), "cancelled with the dialog");
    assert_eq!(status(h), "Query cancelled");
}

/// A run applied in the filter dialog that waits for its profile to connect is cancelled
/// with the dialog: the connection that comes runs nothing.
#[test]
fn a_dialog_cancelled_while_its_run_waits_for_the_connection_runs_nothing() {
    let cfg = test_db_config();
    let state = State::new("dialogconnect");
    state.write_table_tab(&cfg, "filter = \"id > 1\"");
    let mut h = launch(&cfg, &state, Lang::En);
    apply_then_cancel(&mut h);
    connect(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "the run cancelled never goes");
}

/// A run applied in the filter dialog that waits for the table's keys is cancelled with the
/// dialog: the keys' answer runs nothing.
#[test]
fn a_dialog_cancelled_while_its_run_waits_for_the_keys_runs_nothing() {
    let cfg = test_db_config();
    let state = State::new("dialogkeys");
    let mut h = ordered(&cfg, &state, None);
    connect(&mut h);
    apply_then_cancel(&mut h);
    let request = keys_request(&h.sent()).expect("the keys are asked for");
    h.meta_db("local-pg", DbEvent::TableKeys { id: request, result: Ok(pk()) });
    assert!(runs(&mut h).is_empty(), "the run cancelled never goes");
}

/// A run applied in the filter dialog whose functions were confirmed (`y`), then waiting for
/// the connection, is cancelled with the dialog.
#[test]
fn a_dialog_cancelled_after_a_confirm_while_its_run_waits_runs_nothing() {
    let cfg = test_db_config();
    let state = State::new("dialogconfirm");
    state.write_table_tab(&cfg, "filter = \"my_fn(id) > 0\"");
    let mut h = launch(&cfg, &state, Lang::En);
    h.key(KeyCode::Tab);
    h.keys("f");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.is_open(OverlayKind::Confirm), "its functions are asked about");
    h.keys("y");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    let tab = h.app.tab().id;
    assert!(dialog_open(&h) && h.app.tab_busy(tab), "confirmed, it waits for the connection");
    h.key(KeyCode::Esc);
    assert!(!dialog_open(&h));
    assert!(!h.app.tab_busy(tab));
    connect(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "the run cancelled never goes");
}
