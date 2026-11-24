//! The choice among rows the same in every column (change one of them, or all) on MySQL: it
//! is offered only when the session that read the table's structure and the tab's own session,
//! which applies the change, report one engine and one known version, and the edit format the
//! change is written in is the one the tab's own session takes. Otherwise such a change is
//! refused before anything is written, the reason in the Changes tab; the rows are still counted
//! as before. Rows found by a key, a row that matches one row, and new rows are not touched.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::keys::{KeyCatalog, NameRule};
use datarig_core::driver::structure::{Collation, ColumnFill, StructureColumn, TableStructure};
use datarig_core::driver::{
    AppliedStep, ColumnMeta, ColumnOrigin, DbCommand, DbError, DbEvent, EditFormat, MySqlEdit, SessionContext,
    SessionValues, ValueKind,
};
use datarig_core::edit::Expect;
use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, ServerFacts, Topology, Version, Why};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::{Lang, Msg};
use datarig_tui::app::edit::{PreviewLine, Target};
use datarig_tui::app::{AppEvent, EventTarget, Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::time::Duration;

const SQL: &str = "SELECT a, b FROM shop.items";
const TEXT_SQL: &str = "SELECT a, b, c FROM shop.items";

/// What the Changes tab says of a row that matches two rows the same in every column, by why
/// one or all cannot be offered.
const SESSIONS_DIFFER: &str = "matches 2 rows that look the same, but datarig cannot confirm that the session that read the table and the one that would change them report the same engine and version, so they may differ";
const FORMAT_DIFFERS: &str = "matches 2 rows that look the same, but this edit format differs from the one the session that would change them takes, so one or all cannot be offered";
const TOO_OLD: &str = "matches 2 rows that look the same, but this session's server is older than MySQL 8.0.17, which changing one of them or all needs";
const NOT_CHECKED: &str = "matches 2 rows that look the same, but the server version of the session that read the table or of the one that would change them is not known, so they may differ. Run the query again to edit these rows.";
const NOT_BUILT: &str = "matches 2 rows that look the same, but datarig does not change rows on this server yet";

fn v(major: u16, minor: u16, patch: u16) -> Version {
    Version::new(major, minor, patch)
}

/// What a session of a MySQL-family server found it to be: its `VERSION()` line names `line`
/// (its version `version`), its greeting names `greeting` at `handshake`; reached directly.
fn facts(line: EngineId, version: Fact<Version>, greeting: EngineId, handshake: Version) -> Arc<ServerFacts> {
    let my = MyFacts::new(handshake, greeting);
    let detail = match line {
        EngineId::MariaDb => EngineFacts::MariaDb(my),
        EngineId::MySql => EngineFacts::MySql(my),
        EngineId::Postgres => panic!("a MySQL-family server"),
    };
    Arc::new(ServerFacts {
        version,
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail,
    })
}

/// A MySQL server at `at`, its reports agreeing.
fn mysql(at: Version) -> DbEvent {
    DbEvent::Server(facts(EngineId::MySql, Fact::Known(at), EngineId::MySql, at))
}

/// The edits a MySQL session says its server takes (8.0.17 on: `nowait` true).
fn edits(nowait: bool) -> DbEvent {
    DbEvent::Edit(Some(EditFormat::MySql(MySqlEdit { nowait })))
}

/// A MySQL profile, connected, the keys of `shop.items` read (no key): `meta` is what its
/// metadata session says first.
fn profile(meta: Vec<DbEvent>) -> Harness {
    profile_in(meta, false)
}

/// [`profile`], its environment confirming every write when `confirm` (an Apply shows the
/// preview first).
fn profile_in(meta: Vec<DbEvent>, confirm: bool) -> Harness {
    use datarig_core::policy::{Confirm, Policy};
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut cfg = Config {
        connections: vec![ConnectionConfig {
            driver: "mysql".into(),
            policy: confirm.then(|| "stg".into()),
            ..ConnectionConfig::test_db()
        }],
        ..Config::default()
    };
    cfg.policies.insert("stg", Policy { confirm: Confirm::Writes, ..Policy::default() });
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    // A key that lands in the grid copies there: never to the system's clipboard.
    FakeClipboard::attach(&mut h, false, &[]);
    for ev in meta {
        meta_db(&mut h, ev);
    }
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(items_keys())));
    h
}

fn items_keys() -> KeyCatalog {
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.set_server_zone("UTC");
    k.add_table(1, "shop", "items", ["a", "b", "c"].iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k
}

fn meta_db(h: &mut Harness, ev: DbEvent) {
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, ev);
}

/// An event of the profile's metadata session in database `db`.
fn aux_db(h: &mut Harness, db: &str, ev: DbEvent) {
    let p = h.app.profiles[0].id;
    let id = h.app.conns.aux(p, db).expect("an aux session").id;
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Aux(id), generation: id, ev });
}

fn col(name: &str, ty: &str, kind: ValueKind) -> ColumnMeta {
    let origin = ColumnOrigin::Named { schema: "shop".into(), table: "items".into(), column: name.into() };
    ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
}

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// `SELECT a, b` (or, `text`, `a, b, c`) of `shop.items` run in the active tab, whose session
/// says `tab` first: two rows the same in every column, the grid focused.
fn read(h: &mut Harness, tab: Vec<DbEvent>, text: bool) {
    h.app.focus = Focus::Editor;
    h.app.run(vec![(if text { TEXT_SQL } else { SQL }).to_string()]);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::SessionValues(SessionValues { time_zone: "+00:00".into(), charset: "utf8mb4".into() }));
    for ev in tab {
        h.tab_db(index, ev);
    }
    let mut columns = vec![col("a", "int", ValueKind::Integer), col("b", "int", ValueKind::Integer)];
    let mut row = vec![s("1"), s("2")];
    if text {
        columns.push(col("c", "varchar", ValueKind::Text));
        row.push(s("x"));
    }
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id,
        columns: Some(columns),
        rows: vec![row.clone(), row],
        more: false,
        elapsed: Duration::ZERO,
    });
    h.app.focus = Focus::Results;
}

/// `shop.items` as MySQL has it: no key, `a` and `b` numbers (and, `text`, `c` text in
/// utf8mb4): rows the same in every column are a choice where the sessions agree.
fn items(text: bool) -> TableStructure {
    let col = |name: &str, ty: &str| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: true,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation: None,
    };
    let mut t = TableStructure::new(datarig_core::driver::structure::RelationKind::Table);
    t.columns = vec![col("a", "int"), col("b", "int")];
    if text {
        let mut c = col("c", "varchar(20)");
        c.collation =
            Some(Collation { name: "utf8mb4_0900_ai_ci".into(), charset: Some("utf8mb4".into()), deterministic: true });
        t.columns.push(c);
    }
    t
}

fn structure_answer(text: bool) -> DbEvent {
    DbEvent::Structure { schema: "shop".into(), table: "items".into(), result: Ok(Box::new(items(text))) }
}

/// `b` of the first row set to 9: the edit key asks for the table's structure, `answer` brings
/// it (the profile's metadata session's, unless it answers itself), the editor opens.
fn change_b(h: &mut Harness, answer: impl FnOnce(&mut Harness)) {
    h.keys("le");
    answer(h);
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "{:?}", said(h));
    let n = h.app.tab().cell_edit.as_ref().map_or(0, |e| e.input.text().chars().count());
    for _ in 0..n {
        h.key(KeyCode::Backspace);
    }
    h.type_text("9");
    h.key(KeyCode::Enter);
    h.app.transient = None;
}

/// [`change_b`] with the structure from the profile's own metadata session.
fn change(h: &mut Harness, text: bool) {
    change_b(h, |h| meta_db(h, structure_answer(text)));
}

/// `:apply`: the one row counted, `n` rows matching.
fn apply_counted(h: &mut Harness, n: u64) {
    h.command("apply");
    let asked = counts(h);
    assert_eq!(asked.len(), 1, "one count");
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(n), snapshot: false });
}

/// The counts sent, with their ids; no Apply is sent with them.
fn counts(h: &mut Harness) -> Vec<(u64, String)> {
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::ApplyEdits { .. })), "no Apply with the counts");
    sent.into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { id, sql, limit: None } => Some((id, sql)),
            _ => None,
        })
        .collect()
}

fn said(h: &mut Harness) -> Option<Msg> {
    h.app.transient.take().map(|(n, _)| n.msg)
}

fn said_text(h: &mut Harness) -> Option<String> {
    let m = said(h)?;
    Some(h.app.i18n.msg(&m).to_string())
}

fn no_apply_sent(h: &mut Harness) -> bool {
    h.sent().iter().all(|c| !matches!(c, DbCommand::ApplyEdits { .. }))
}

/// The duplicate-row change is refused for `words`: the Changes tab shows it on the row (no
/// preview, no one or all to pick), and nothing is applied, `y` included.
fn withheld(h: &mut Harness, words: &str) {
    assert!(h.app.overlays.top().is_none(), "no preview: {:?}", h.app.edit_preview_view().map(|v| v.lines));
    assert!(h.app.ask.is_none(), "nothing asked");
    assert_eq!(h.app.key_context(), Ctx::Changes);
    let m = h.app.shown_changes().expect("a model");
    let texts: Vec<String> = m.problems.iter().map(|p| p.text.clone()).collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].ends_with(words), "{texts:?}");
    assert!(!m.problems[0].choice, "not a choice");
    assert!(no_apply_sent(h), "nothing applied");
    h.key(KeyCode::Char('y'));
    assert!(no_apply_sent(h), "y applies nothing");
    assert!(h.app.overlays.top().is_none());
}

/// The preview offers one or all of the two rows.
fn offered(h: &Harness) {
    let lines = h.app.edit_preview_view().expect("the preview is open").lines;
    match &lines[..] {
        [PreviewLine::Choose { keys, .. }] => assert_eq!(keys, "[1] just one   [a] all 2"),
        other => panic!("{other:?}"),
    }
}

/// `a` then `y` in the preview: one Apply, whose update expects both rows changed.
fn all_applied(h: &mut Harness) {
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('y'));
    let steps: Vec<_> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::ApplyEdits { steps, .. } => Some(steps),
            _ => None,
        })
        .collect();
    assert_eq!(steps.len(), 1, "one Apply sent");
    let update = steps[0].iter().find(|s| s.sql.starts_with("UPDATE ")).expect("an update");
    assert_eq!(update.expect, Expect::Affected(2));
}

// ── Refused: the sessions differ, or say nothing ────────────────────────────────────────────

/// The metadata session (whose structure the change rests on) greets 8.0.15, the tab's own
/// session 8.4.6: a structure read below 8.0.16 may miss columns the account is not shown.
#[test]
fn a_duplicate_row_edit_is_withheld_when_the_structure_came_from_another_greeting() {
    let mut h = profile(vec![mysql(v(8, 0, 15))]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, SESSIONS_DIFFER);
}

/// The tab's own session's reports name two engines (its greeting MySQL 8.4.6, its `VERSION()`
/// line MariaDB), and it says what such a session says: MySQL's language and edits (by its
/// greeting); the metadata session's reports agree, at the same greeting.
#[test]
fn a_duplicate_row_edit_is_withheld_when_the_tab_reports_two_engines() {
    let mut h = profile(vec![mysql(v(8, 4, 6))]);
    let two = facts(EngineId::MariaDb, Fact::Unknown(Why::EnginesDisagree), EngineId::MySql, v(8, 4, 6));
    let language = DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default())));
    read(&mut h, vec![DbEvent::Server(two), language, edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, SESSIONS_DIFFER);
}

/// The metadata session's own reports name two versions (greeting 8.4.6, `VERSION()` 8.4.7).
#[test]
fn a_duplicate_row_edit_is_withheld_when_the_metadata_session_reports_two_versions() {
    let two = facts(EngineId::MySql, Fact::Unknown(Why::VersionsDisagree), EngineId::MySql, v(8, 4, 6));
    let mut h = profile(vec![DbEvent::Server(two)]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, SESSIONS_DIFFER);
}

/// The tab's own session never said what its server is.
#[test]
fn a_duplicate_row_edit_is_withheld_while_the_tab_session_has_said_no_facts() {
    let mut h = profile(vec![mysql(v(8, 4, 6))]);
    read(&mut h, vec![edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, NOT_CHECKED);
}

/// The connection is kept, but its metadata session never said what its server is.
#[test]
fn a_duplicate_row_edit_is_withheld_while_the_metadata_session_has_said_no_facts() {
    let mut h = profile(Vec::new());
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, NOT_CHECKED);
}

/// The tab's own session greets 8.0.15 (it takes no edits), the metadata session 8.4.6 said
/// the profile's edits last: the greetings differ first.
#[test]
fn a_duplicate_row_edit_is_withheld_when_the_tab_cannot_apply() {
    let mut h = profile(vec![mysql(v(8, 4, 6))]);
    read(&mut h, vec![mysql(v(8, 0, 15)), DbEvent::Edit(None)], false);
    meta_db(&mut h, edits(true));
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, SESSIONS_DIFFER);
}

/// Open a second tab of the profile whose own session says `says`, then go back to the first.
fn third_tab(h: &mut Harness, says: Vec<DbEvent>) {
    h.ctrl('t');
    let index = h.app.tabs.active_index();
    assert_eq!(index, 1, "a second tab");
    h.app.focus = Focus::Editor;
    h.app.run(vec!["SELECT 1".to_string()]);
    for ev in says {
        h.tab_db(index, ev);
    }
    h.app.tabs.activate(0);
    h.app.focus = Focus::Results;
    h.sent();
}

/// Both sessions greet 8.0.15 (the Apply's would refuse it); another tab's session said the
/// profile's edits last, so the change is not refused up front.
#[test]
fn a_duplicate_row_edit_is_withheld_on_an_old_server_whose_format_came_from_another_tab() {
    let mut h = profile(vec![mysql(v(8, 0, 15)), DbEvent::Edit(None)]);
    read(&mut h, vec![mysql(v(8, 0, 15)), DbEvent::Edit(None)], false);
    third_tab(&mut h, vec![mysql(v(8, 4, 6)), edits(true)]);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, TOO_OLD);
    // As before: with the tab's own word last, every edit is refused up front.
    let mut h = profile(vec![mysql(v(8, 0, 15)), DbEvent::Edit(None)]);
    read(&mut h, vec![mysql(v(8, 0, 15)), DbEvent::Edit(None)], false);
    h.keys("le");
    assert!(matches!(said(&mut h), Some(Msg::EditReadOnly { .. })));
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })), "nothing asked");
}

/// The metadata session and the Apply's tab greet 8.4.6; a third tab's session (8.0.17) said
/// the profile's edits last, in a format the Apply's own session does not take.
#[test]
fn a_duplicate_row_edit_is_withheld_when_a_third_tab_wrote_the_format() {
    let mut h = profile(vec![mysql(v(8, 4, 6))]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    third_tab(&mut h, vec![mysql(v(8, 0, 17)), edits(false)]);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, FORMAT_DIFFERS);
}

/// One greeting, the table resolved with its choice; then another tab's session says another
/// format, and the Apply is planned in it.
#[test]
fn a_format_written_after_the_target_was_ready_withholds_the_choice() {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    assert!(matches!(&rows(&h).edit.target, Target::Ready(_)));
    third_tab(&mut h, vec![mysql(v(8, 4, 6)), edits(false)]);
    apply_counted(&mut h, 2);
    withheld(&mut h, FORMAT_DIFFERS);
}

/// While the Apply waits for its counts, the profile's format changes and changes back: the
/// Apply is cancelled, said so, and nothing is applied; the Changes tab plans in the format now
/// in force.
#[test]
fn a_format_change_while_the_rows_are_counted_cancels_the_apply() {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    h.command("apply");
    let asked = counts(&mut h);
    assert_eq!(asked.len(), 1);
    assert!(h.app.apply_wait.is_some(), "waiting for the count");
    meta_db(&mut h, edits(false));
    meta_db(&mut h, edits(true));
    assert!(h.app.apply_wait.is_none(), "the wait is cancelled");
    assert_eq!(said_text(&mut h).as_deref(), Some("The edit format changed while the rows were counted; apply again"));
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(2), snapshot: false });
    assert!(h.app.overlays.top().is_none(), "no preview");
    assert!(h.app.ask.is_none(), "nothing asked");
    assert!(no_apply_sent(&mut h));
    // The format in force again is the one the choice was checked for: a choice, not refused.
    let m = h.app.shown_changes().expect("a model");
    assert_eq!(m.problems.len(), 1);
    assert!(m.problems[0].choice, "{:?}", m.problems[0].text);
}

/// A MariaDB tab of a profile whose MySQL tab said the profile's edits: the MariaDB tab's
/// rows may be edited (its driver refuses the Apply), but one or all of identical rows is never
/// offered there. A row that matches one row goes to its session as before.
#[test]
fn a_mariadb_tab_withholds_duplicates_when_a_mysql_tab_set_the_format() {
    let maria = || {
        vec![
            DbEvent::Server(facts(EngineId::MariaDb, Fact::Known(v(11, 4, 13)), EngineId::MariaDb, v(11, 4, 13))),
            DbEvent::Language(Language::Sql(Dialect::MariaDb(MySqlMode::default()))),
            DbEvent::Edit(None),
        ]
    };
    let mut h = profile(maria());
    read(&mut h, maria(), false);
    third_tab(&mut h, vec![mysql(v(8, 4, 6)), edits(true)]);
    assert!(matches!(h.app.tab().exec.prepared.language().dialect(), Dialect::MariaDb(_)), "a MariaDB tab");
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, NOT_BUILT);
    // The row matches one row now: the Apply is asked and sent as before.
    apply_counted(&mut h, 1);
    assert!(h.app.ask.is_some(), "asked in the status bar");
    h.key(KeyCode::Char('y'));
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::ApplyEdits { .. })), "the Apply sent");
}

// ── Judged when applied: what changes while the preview is open ────────────────────────────

/// One greeting everywhere, the change of a row that matches two identical rows asked with
/// `:apply`: the preview offers one or all, nothing sent yet.
fn previewed() -> Harness {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    offered(&h);
    h
}

/// `a` (all of them) then `y`, wherever the keys land: nothing is applied.
fn all_then_yes_sends_nothing(h: &mut Harness) {
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('y'));
    assert!(no_apply_sent(h), "nothing applied");
}

/// The preview is closed, saying why one or all is no longer offered.
fn closed_saying(h: &mut Harness, words: &str) {
    assert!(h.app.edit_preview_view().is_none(), "the preview is closed");
    let said = said_text(h).unwrap_or_default();
    assert!(said.contains(words), "{said}");
}

/// Another session says another edit format while the preview offers one or all.
#[test]
fn a_format_change_with_the_preview_open_applies_nothing() {
    let mut h = previewed();
    meta_db(&mut h, edits(false));
    closed_saying(&mut h, "edit format");
    all_then_yes_sends_nothing(&mut h);
}

/// The metadata session that read the structure is lost while the preview is open, and another
/// tab's run opens it again (the same server): the structure's session is not the one now.
#[test]
fn the_metadata_session_lost_and_opened_again_with_the_preview_open_applies_nothing() {
    let mut h = previewed();
    meta_db(&mut h, DbEvent::Lost { error: DbError::Closed });
    assert!(h.app.edit_preview_view().is_none(), "the preview is closed");
    assert_eq!(said_text(&mut h), Some(format!("Can't apply yet: {NOT_CHECKED}")));
    h.ctrl('t');
    h.app.focus = Focus::Editor;
    h.app.run(vec!["SELECT 1".to_string()]);
    h.app.tabs.activate(0);
    h.app.focus = Focus::Results;
    meta_db(&mut h, mysql(v(8, 4, 6)));
    meta_db(&mut h, edits(true));
    meta_db(&mut h, DbEvent::Connected);
    h.app.transient = None;
    all_then_yes_sends_nothing(&mut h);
    // The same server again, but not the session that read the structure: asked again, the
    // change of the two rows is still refused.
    h.advance(Duration::from_secs(2));
    h.app.transient = None;
    apply_counted(&mut h, 2);
    withheld(&mut h, NOT_CHECKED);
}

/// An Apply sent from the preview is reported when its answer comes, whatever the sessions
/// say meanwhile (the change may be committed).
#[test]
fn an_apply_in_flight_is_reported_whatever_the_sessions_say_meanwhile() {
    let mut h = previewed();
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('y'));
    let sent: Vec<(u64, Vec<datarig_core::edit::Step>)> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::ApplyEdits { id, steps, .. } => Some((id, steps)),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "one Apply sent");
    meta_db(&mut h, edits(false));
    h.tab_db(0, mysql(v(8, 4, 7)));
    assert!(h.app.edit_preview_view().is_some(), "the preview still says it applies");
    let (id, steps) = sent.into_iter().next().unwrap();
    let stood = steps
        .iter()
        .map(|s| {
            let count = match s.expect {
                Expect::Rows(n) | Expect::Affected(n) => n,
            };
            AppliedStep { count, ..AppliedStep::default() }
        })
        .collect();
    h.db(DbEvent::Applied { id, result: Ok(stood) });
    assert!(rows(&h).edit.applied.is_some(), "the changes are applied");
    assert!(rows(&h).edit.batch.is_empty());
}

/// The preview closes by itself after `a` was pressed: the `y` meant for it is not taken by the
/// grid (which would copy a cell over the clipboard); it is ignored, said so.
#[test]
fn the_answer_key_after_the_preview_closed_by_itself_is_not_taken_by_the_grid() {
    let mut h = previewed();
    let clipboard = FakeClipboard::attach(&mut h, false, &[]);
    h.key(KeyCode::Char('a'));
    meta_db(&mut h, edits(false));
    assert!(h.app.edit_preview_view().is_none(), "the preview is closed");
    h.key(KeyCode::Char('y'));
    assert!(clipboard.all().is_empty(), "nothing copied: {:?}", clipboard.all());
    assert!(matches!(said(&mut h), Some(Msg::EditAskIgnored { .. })));
    assert!(no_apply_sent(&mut h));
}

/// The structure came from another database's session, which is lost while the preview is open.
#[test]
fn the_other_databases_session_lost_with_the_preview_open_applies_nothing() {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    aux_db(&mut h, "sales", mysql(v(8, 4, 6)));
    aux_db(&mut h, "sales", DbEvent::Connected);
    aux_db(&mut h, "sales", DbEvent::Keys(Ok(items_keys())));
    change_b(&mut h, |h| aux_db(h, "sales", structure_answer(false)));
    apply_counted(&mut h, 2);
    offered(&h);
    aux_db(&mut h, "sales", DbEvent::Lost { error: DbError::Closed });
    h.app.transient = None;
    all_then_yes_sends_nothing(&mut h);
}

/// The tab's own session says late that its server is another version than the structure's.
#[test]
fn a_late_word_of_another_version_with_the_preview_open_applies_nothing() {
    let mut h = previewed();
    h.tab_db(0, mysql(v(8, 4, 7)));
    closed_saying(&mut h, "same engine and version");
    all_then_yes_sends_nothing(&mut h);
}

/// The structure comes before the metadata session says what its server is: refused while not
/// known, offered once it is (a normal setup is not left without the choice).
#[test]
fn a_choice_refused_before_the_facts_came_is_offered_once_they_do() {
    let mut h = profile(Vec::new());
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    withheld(&mut h, NOT_CHECKED);
    meta_db(&mut h, mysql(v(8, 4, 6)));
    h.app.focus = Focus::Results;
    apply_counted(&mut h, 2);
    offered(&h);
    all_applied(&mut h);
}

/// `a` then `y` in the preview: the one Apply sent, its id.
fn all_sent(h: &mut Harness) -> u64 {
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('y'));
    let ids: Vec<u64> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::ApplyEdits { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 1, "one Apply sent");
    ids[0]
}

/// The edit format changes after `y` sent the Apply, then the Apply fails: what the answer says
/// stands (its commit not known, or the failure), never covered by the preview's closing words.
#[test]
fn a_sent_edits_answer_is_never_covered_by_the_preview_closing() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let unknown = ApplyError::Db(DbError::CommitUnknown);
    let failed = ApplyError::Db(DbError::Server("ERROR 1205: Lock wait timeout exceeded".into()));
    for why in [unknown, failed] {
        let mut h = previewed();
        let id = all_sent(&mut h);
        meta_db(&mut h, edits(false));
        h.app.transient = None;
        h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: None, why: why.clone() }) });
        let said = said(&mut h);
        assert!(!matches!(said, Some(Msg::EditPreviewCantApply { .. }) | None), "{why:?}: {said:?}");
        if matches!(why, ApplyError::Db(DbError::CommitUnknown)) {
            assert_eq!(said, Some(datarig_i18n::Label::EditFailedUnknown.into()));
        }
        assert!(no_apply_sent(&mut h));
    }
}

/// After the preview closed by itself, its choice keys (`a`, `1`) are not taken by the grid
/// either: ignored a while, said so.
#[test]
fn the_choice_keys_after_the_preview_closed_by_itself_are_ignored() {
    for key in ['a', '1'] {
        let mut h = previewed();
        meta_db(&mut h, edits(false));
        assert!(h.app.edit_preview_view().is_none(), "the preview is closed");
        h.app.transient = None;
        h.key(KeyCode::Char(key));
        assert!(matches!(said(&mut h), Some(Msg::EditAskIgnored { .. })), "{key}");
    }
}

/// A preview opened to confirm a write of a row that matches one row closes on a format change
/// saying only that the format changed (no count of identical rows).
#[test]
fn a_format_change_closes_a_confirming_preview_with_the_format_words_only() {
    let mut h = profile_in(vec![mysql(v(8, 4, 6)), edits(true)], true);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 1);
    assert!(h.app.edit_preview_view().is_some(), "the preview confirms the write");
    meta_db(&mut h, edits(false));
    assert!(h.app.edit_preview_view().is_none(), "the preview is closed");
    assert_eq!(
        said_text(&mut h).as_deref(),
        Some("Can't apply yet: this edit format differs from the one the applying session takes")
    );
}

// ── Offered: the sessions agree ─────────────────────────────────────────────────────────────

/// Every session of the profile greets 8.4.6 and says the same edits: one or all is offered,
/// and all changes both rows.
#[test]
fn a_duplicate_row_edit_on_one_greeting_still_offers_one_or_all() {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    change(&mut h, false);
    apply_counted(&mut h, 2);
    offered(&h);
    all_applied(&mut h);
}

/// A tab that works in another database (as a tab restored with its database does): the
/// structure comes from that database's metadata session, which greets as the tab's own.
#[test]
fn a_restored_mysql_tab_in_another_database_keeps_its_duplicate_choice() {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    aux_db(&mut h, "sales", mysql(v(8, 4, 6)));
    aux_db(&mut h, "sales", DbEvent::Connected);
    aux_db(&mut h, "sales", DbEvent::Keys(Ok(items_keys())));
    h.sent();
    change_b(&mut h, |h| aux_db(h, "sales", structure_answer(false)));
    apply_counted(&mut h, 2);
    offered(&h);
    all_applied(&mut h);
}

// ── What stays as it was ───────────────────────────────────────────────────────────────────

/// A structure asked of the metadata session, which is lost and opened again before it
/// answers: the old answer is dropped, nothing is asked then; the next edit key asks the new
/// session once.
#[test]
fn a_structure_asked_before_the_metadata_session_opened_again_is_dropped() {
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    h.sent();
    h.keys("le");
    let asked = |h: &mut Harness| h.sent().iter().filter(|c| matches!(c, DbCommand::LoadStructure { .. })).count();
    assert_eq!(asked(&mut h), 1);
    let p = h.app.profiles[0].id;
    let old = h.app.conns.get(p).expect("the connection").generation;
    meta_db(&mut h, DbEvent::Lost { error: DbError::Closed });
    // Another tab's run opens the metadata session again.
    h.ctrl('t');
    h.app.focus = Focus::Editor;
    h.app.run(vec!["SELECT 1".to_string()]);
    h.app.tabs.activate(0);
    h.app.focus = Focus::Results;
    meta_db(&mut h, mysql(v(8, 4, 6)));
    meta_db(&mut h, edits(true));
    meta_db(&mut h, DbEvent::Connected);
    assert_ne!(h.app.conns.get(p).expect("the connection").generation, old, "opened again");
    h.app.transient = None;
    h.sent();
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Meta(p), generation: old, ev: structure_answer(false) });
    assert!(matches!(&rows(&h).edit.target, Target::Loading(_)), "the old answer is dropped");
    assert_eq!(asked(&mut h), 0, "nothing asked then");
    h.keys("e");
    assert_eq!(asked(&mut h), 1, "asked again on the next edit key");
    meta_db(&mut h, structure_answer(false));
    assert!(matches!(&rows(&h).edit.target, Target::Ready(_)));
}

/// What another database's metadata session found its server to be lasts as long as that
/// session: a late word of one closed for being idle is not kept, one opened again starts with
/// none (a duplicate-row change of that database is refused until it says), and a lost or failed
/// one forgets it.
#[test]
fn an_aux_session_closed_while_idle_forgets_its_facts() {
    let facts = |h: &Harness| {
        let p = h.app.profiles[0].id;
        h.app.conns.aux(p, "sales").expect("an aux session").facts.clone()
    };
    let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
    h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
    read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
    aux_db(&mut h, "sales", mysql(v(8, 4, 6)));
    aux_db(&mut h, "sales", DbEvent::Connected);
    aux_db(&mut h, "sales", DbEvent::Keys(Ok(items_keys())));
    assert!(facts(&h).is_some(), "said while open");
    // No tab works there for a while: it closes, and what its server was goes with it.
    h.app.tab_mut().context = SessionContext::default();
    h.advance(Duration::from_secs(61));
    let p = h.app.profiles[0].id;
    assert!(h.app.conns.aux(p, "sales").expect("kept").session.is_none(), "closed for being idle");
    assert!(facts(&h).is_none(), "forgotten when it closed");
    aux_db(&mut h, "sales", mysql(v(8, 4, 6)));
    assert!(facts(&h).is_none(), "a late word of the closed session is not kept");
    // The tab works there again: its edit key opens the session again, which answers the
    // structure before it says what its server is.
    h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
    h.sent();
    change_b(&mut h, |h| {
        assert!(facts(h).is_none(), "opened again with none");
        aux_db(h, "sales", structure_answer(false));
    });
    apply_counted(&mut h, 2);
    withheld(&mut h, NOT_CHECKED);
    // Lost, or failed: forgotten.
    for end in
        [DbEvent::Lost { error: DbError::Closed }, DbEvent::ConnectFailed { error: DbError::Closed, auth: false }]
    {
        let mut h = profile(vec![mysql(v(8, 4, 6)), edits(true)]);
        h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
        read(&mut h, vec![mysql(v(8, 4, 6)), edits(true)], false);
        aux_db(&mut h, "sales", mysql(v(8, 4, 6)));
        assert!(facts(&h).is_some());
        aux_db(&mut h, "sales", end);
        assert!(facts(&h).is_none());
    }
}

/// The tab's own session greets 8.0.15, another session said the profile's edits; a text
/// column's count compares with a collation that server lacks: the count fails with the
/// server's words, and nothing is written.
#[test]
fn a_text_duplicate_on_an_old_tab_fails_at_its_count() {
    let mut h = profile(vec![mysql(v(8, 4, 6))]);
    read(&mut h, vec![mysql(v(8, 0, 15)), DbEvent::Edit(None)], true);
    meta_db(&mut h, edits(true));
    change(&mut h, true);
    h.command("apply");
    let asked = counts(&mut h);
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.contains("utf8mb4_0900_bin"), "{}", asked[0].1);
    let error = DbError::Server("Unknown collation: 'utf8mb4_0900_bin'".into());
    h.db(DbEvent::Counted { id: asked[0].0, result: Err(error), snapshot: false });
    let m = h.app.shown_changes().expect("a model");
    let texts: Vec<String> = m.problems.iter().map(|p| p.text.clone()).collect();
    assert_eq!(texts, ["Couldn't count the matching rows: Unknown collation: 'utf8mb4_0900_bin'"]);
    assert!(h.app.overlays.top().is_none(), "no preview");
    assert!(no_apply_sent(&mut h));
}

fn rows(h: &Harness) -> &datarig_tui::widgets::grid::ResultSet {
    match &h.app.tab().results {
        Results::Rows(rs) => rs,
        _ => panic!("no row result"),
    }
}
