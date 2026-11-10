//! A table tab's filter and order are kept: in the workspace across a restart, and with the tab
//! when it is closed and brought back. They come back in the filter line as applied and are not
//! run; the run the user then asks for checks them again as any filter is checked (a file edited
//! by hand may hold a filter the line would refuse, or one whose functions must be confirmed).

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

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The filter line's two fields, as drawn.
fn line_texts(h: &mut Harness) -> (String, String) {
    h.screen(120, 30);
    let tq = h.app.tab().doc.table_query.as_ref().expect("a table query");
    (tq.line.where_.text().to_string(), tq.line.order.text().to_string())
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
    // Typed in the line, not applied: not kept.
    h.key(KeyCode::Tab);
    h.keys("f");
    h.type_text(" AND name <> ''");
    assert!(h.app.tab().doc.table_query.as_ref().unwrap().line.focus == Some(Field::Where));
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
    // The run key: the statement is the compiled query.
    h.key(KeyCode::Tab);
    h.ctrl('e');
    assert_eq!(runs(&mut h), vec![vec![want.clone()]]);
    answer(&mut h);
    let shown = h.app.tab().doc.table_query.as_ref().and_then(|q| q.shown.clone()).expect("shown");
    assert_eq!(filter_text(&shown), Some("id > 1"));
    // Saved again as it was.
    quit(h);
    let (text, tab) = state.saved();
    assert_eq!(tab.filter.as_deref(), Some("id > 1"), "{text}");
}

/// A filter edited by hand into the file that the line refuses: back in the line, and the run
/// key sends nothing and says why.
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
        // The refresh glyph and Enter in the field are runs too: refused the same way.
        h.keys("f");
        h.key(KeyCode::Enter);
        assert!(runs(&mut h).is_empty(), "{filter}: nothing sent");
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
    assert_eq!(runs(&mut h), vec![vec![want]]);
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
    h.screen(120, 30);
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.line.limit.text(), "2");
    assert!(h.screen(120, 30).contains("LIMIT 2"), "{}", h.screen(120, 30));
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
    // Another limit typed in the line and applied: kept.
    h.keys("f");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.ctrl('l');
    h.type_text("50");
    h.key(KeyCode::Enter);
    answer(&mut h);
    quit(h);
    let (text, tab) = state.saved();
    assert_eq!(tab.limit, Some(50), "{text}");
}
