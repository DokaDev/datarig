//! The checks of a table tab's run hold whichever way its query is changed: in the whole filter
//! dialog (Advanced), in a condition's popup, with a chip's `×` or with a `LIMIT` typed in the
//! bar. Each way is driven through the same flows: a function the app cannot check is asked
//! about, a MySQL read that may lock rows is asked about and asked again when the session's
//! state moved before the yes, a run waiting for its session's state is cancelled and never
//! sent, a read-only profile refuses, pending edits are asked about. The counts of a column's
//! values answer only the popup that asked for them, the rows popup counts rows (never values),
//! and an edit waiting for the table is not done once other columns are shown. In the terminal
//! theme a popup says a failure, an expression and a refusal in the theme's own colours.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, GroupCounts, GroupValue, Isolation, ServerMessage};
use datarig_core::policy::{Environment, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::{Label, Lang};
use datarig_tui::app::filter_dialog::view::PopupKindId;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::qbar::BarHit;
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::time::Duration;

const PG: &str = r#"SELECT * FROM "shop"."users""#;
const MY: &str = "SELECT * FROM `shop`.`users`";

/// The conditions every flow starts from: the change of each way is made on them.
const BASE: &str = "id > 0 AND id < 9";

/// A way to change a table tab's query.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Via {
    /// The whole filter dialog, turned to from the second condition's popup.
    Advanced,
    /// The second condition's popup.
    ConditionPopup,
    /// The first condition's `×` in the bar.
    ChipX,
    /// `Space r l`, a cap typed in the bar.
    LimitInline,
}

const VIAS: [Via; 4] = [Via::Advanced, Via::ConditionPopup, Via::ChipX, Via::LimitInline];

impl Via {
    /// The change is applied from a dialog view, which stays open until it is sent.
    fn dialog(self) -> bool {
        matches!(self, Via::Advanced | Via::ConditionPopup)
    }

    /// Make the change on [`BASE`] (and the conditions after it) and apply it.
    fn change(self, h: &mut Harness) {
        h.app.focus = Focus::Results;
        match self {
            Via::Advanced => {
                open_chip(h, 1);
                h.keys("A");
                assert_eq!(popup(h), None, "the whole dialog");
                type_value(h, "0");
                h.key(KeyCode::Enter);
            }
            Via::ConditionPopup => {
                open_chip(h, 1);
                type_value(h, "0");
                h.key(KeyCode::Enter);
            }
            Via::ChipX => bar_click(h, BarHit::Remove(0)),
            Via::LimitInline => {
                h.keys(" rl5");
                h.key(KeyCode::Enter);
            }
        }
    }

    /// Apply the same change again after a no: a dialog view still holds it; a bar change is
    /// made again.
    fn again(self, h: &mut Harness) {
        if self.dialog() {
            h.key(KeyCode::Enter);
        } else {
            self.change(h);
        }
    }

    /// The statement and the cap the change sends, over `all` with [`BASE`] and `rest` (the
    /// conditions after it).
    fn sent(self, all: &str, rest: &str) -> (String, Option<u64>) {
        match self {
            Via::Advanced | Via::ConditionPopup => (format!("{all} WHERE (id > 0 AND id < 90{rest})"), None),
            Via::ChipX => (format!("{all} WHERE (id < 9{rest})"), None),
            Via::LimitInline => (format!("{all} WHERE ({BASE}{rest})"), Some(5)),
        }
    }

    /// After a no the dialog view the change was made in is still open, as it was.
    fn kept(self, h: &Harness) {
        match self {
            Via::Advanced => assert!(open(h) && popup(h).is_none(), "{self:?}: the whole dialog stays"),
            Via::ConditionPopup => assert_eq!(popup(h), Some(PopupKindId::Condition), "{self:?}: the popup stays"),
            Via::ChipX | Via::LimitInline => assert!(!open(h), "{self:?}: no dialog"),
        }
        if self.dialog() {
            assert!(draft_where(h).contains("id < 90"), "{self:?}: the draft kept: {}", draft_where(h));
        }
    }
}

/// Open `shop.users` from the explorer (a table tab, which runs its query).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// The statements of the runs sent since the last look, each with its cap.
fn sends(h: &mut Harness) -> Vec<(String, Option<u64>)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, max_rows, .. } => Some((statements.join("; "), max_rows)),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with three rows of `shop.users`.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// `shop.users` open on `cfg`'s PostgreSQL profile, filtered with `filter` (a question about it
/// answered yes) and answered, the grid focused, nothing sent since.
fn pg_where_in(cfg: &Config, filter: &str) -> Harness {
    let mut h = Harness::connected_with(cfg, Lang::En);
    open_users(&mut h);
    answer(&mut h);
    settle(h, filter)
}

fn pg_where(filter: &str) -> Harness {
    pg_where_in(&test_db_config(), filter)
}

/// `:where filter` (a question about it answered yes), answered; the grid focused, nothing
/// sent since.
fn settle(mut h: Harness, filter: &str) -> Harness {
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.app.detail.visible = false;
    h.command(&format!("where {filter}"));
    if h.app.overlays.is_open(OverlayKind::Confirm) {
        h.keys("y");
    }
    assert_eq!(sends(&mut h).len(), 1, "{filter} runs");
    answer(&mut h);
    h.app.focus = Focus::Results;
    h
}

/// `shop.users` open on a MySQL profile (its session with autocommit on, the level not said
/// yet), filtered with `filter` and answered, the grid focused, nothing sent since.
fn my_where(filter: &str) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    open_users(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
    h.tab_db(index, DbEvent::Connected);
    answer(&mut h);
    settle(h, filter)
}

/// The active tab's session says `ev`.
fn session(h: &mut Harness, ev: DbEvent) {
    let index = h.app.tabs.active_index();
    h.tab_db(index, ev);
}

/// A press and a release at (x, y).
fn click_at(h: &mut Harness, x: u16, y: u16) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// A click on part `hit` of the bar at 120x34, once the bar has been on screen long enough to
/// take it.
fn bar_click(h: &mut Harness, hit: BarHit) {
    h.draw(120, 34);
    let r = h.app.layout.qbar.iter().find(|(_, b)| *b == hit).map(|(r, _)| *r);
    let r = r.unwrap_or_else(|| panic!("{hit:?} not drawn: {:?}", h.app.layout.qbar));
    h.advance(Duration::from_millis(500));
    h.draw(120, 34);
    click_at(h, r.x, r.y);
}

/// The popup of condition `term`, on screen long enough to take clicks.
fn open_chip(h: &mut Harness, term: usize) {
    bar_click(h, BarHit::Chip(term));
    assert_eq!(popup(h), Some(PopupKindId::Condition));
    h.draw(120, 34);
    h.advance(Duration::from_millis(500));
}

/// From the operator of the condition into its value, `text` typed at its end.
fn type_value(h: &mut Harness, text: &str) {
    h.key(KeyCode::Right);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    h.key(KeyCode::End);
    h.type_text(text);
}

fn popup(h: &Harness) -> Option<PopupKindId> {
    h.app.filter_dialog_popup()
}

fn open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::FilterDialog)
}

fn draft_where(h: &Harness) -> String {
    h.app.filter_dialog_draft().map(|d| d.0).unwrap_or_default()
}

fn confirm(h: &Harness) -> Option<ConfirmAction> {
    h.app.overlays.confirm().map(|c| c.action)
}

/// The first place `text` is on the screen at 120x34 (all of the line before it one column a
/// character).
fn find(h: &mut Harness, text: &str) -> Option<(u16, u16)> {
    let t = h.draw(120, 34);
    let buf = t.backend().buffer();
    (0..34).find_map(|y| {
        let line = row_text(buf, y);
        line.find(text).map(|at| (line[..at].chars().count() as u16, y))
    })
}

/// A function the app cannot check is asked about before any way's run that changes the
/// condition: a no sends nothing and keeps the dialog view the change was made in; a yes sends
/// what the change made. A new cap keeps the condition the yes was given to.
#[test]
fn every_way_asks_about_a_function_it_cannot_check() {
    const REST: &str = " AND my_fn(id) > 0";
    for via in VIAS {
        let mut h = pg_where(&format!("{BASE}{REST}"));
        via.change(&mut h);
        if via == Via::LimitInline {
            // The cap leaves the condition as it was let through: not asked again.
            assert_eq!(confirm(&h), None, "{via:?}");
            assert_eq!(sends(&mut h), vec![via.sent(PG, REST)], "{via:?}");
            continue;
        }
        assert_eq!(confirm(&h), Some(ConfirmAction::FilterCalls(h.app.tab().id)), "{via:?}");
        assert!(sends(&mut h).is_empty(), "{via:?}: nothing sent while it asks");
        h.key(KeyCode::Esc);
        assert_eq!(confirm(&h), None, "{via:?}");
        assert!(sends(&mut h).is_empty(), "{via:?}: a no sends nothing");
        via.kept(&h);
        via.again(&mut h);
        assert!(confirm(&h).is_some(), "{via:?}: asked again");
        h.keys("y");
        assert_eq!(sends(&mut h), vec![via.sent(PG, REST)], "{via:?}");
        assert!(!open(&h), "{via:?}: closed once sent");
    }
}

/// A MySQL read that locks rows (SERIALIZABLE without autocommit) is asked about in every way;
/// so is one at a level not known. The state moving between the question and the yes asks
/// again before anything is sent: the yes is for the state it was given in.
#[test]
fn every_way_asks_about_a_locking_read_and_again_when_the_state_moved() {
    for via in VIAS {
        let mut h = my_where(BASE);
        session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
        session(&mut h, DbEvent::Autocommit(false));
        via.change(&mut h);
        assert_eq!(confirm(&h), Some(ConfirmAction::TableReadLocks(h.app.tab().id)), "{via:?}");
        assert!(sends(&mut h).is_empty(), "{via:?}: nothing sent while it asks");
        h.key(KeyCode::Enter);
        assert_eq!(confirm(&h), None, "{via:?}: Enter is the no");
        assert!(sends(&mut h).is_empty(), "{via:?}");
        via.kept(&h);
        // The level not known: asked too.
        session(&mut h, DbEvent::Isolation(None));
        via.again(&mut h);
        assert_eq!(confirm(&h), Some(ConfirmAction::TableReadLocks(h.app.tab().id)), "{via:?}: unknown level");
        assert!(sends(&mut h).is_empty(), "{via:?}");
        // The state moves before the yes: asked again at the send, nothing sent; the next yes
        // sends.
        session(&mut h, DbEvent::Isolation(Some(Isolation::Serializable)));
        h.keys("y");
        assert!(confirm(&h).is_some(), "{via:?}: asked again in the new state");
        assert!(sends(&mut h).is_empty(), "{via:?}: the final check sent nothing");
        h.keys("y");
        assert_eq!(sends(&mut h), vec![via.sent(MY, "")], "{via:?}");
        assert!(!open(&h), "{via:?}: closed once sent");
    }
}

/// A run waiting for its new session to say its state (MySQL: whether its reads lock rows),
/// cancelled, is never sent: not when the session says its state.
#[test]
fn every_way_cancelled_while_it_waits_for_the_sessions_state_never_runs() {
    for via in VIAS {
        let mut h = my_where(BASE);
        session(&mut h, DbEvent::Lost { error: DbError::Closed });
        h.app.transient = None;
        via.change(&mut h);
        let tab = h.app.tab().id;
        assert!(sends(&mut h).is_empty(), "{via:?}: the run waits");
        assert!(h.app.tab_busy(tab), "{via:?}: the wait is the tab's work");
        assert_eq!(open(&h), via.dialog(), "{via:?}: a dialog view stays open while it waits");
        if via.dialog() {
            h.key(KeyCode::Esc);
            assert!(!open(&h), "{via:?}");
        } else {
            h.key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL);
        }
        assert!(!h.app.tab_busy(tab), "{via:?}: cancelled");
        session(&mut h, DbEvent::Connected);
        session(&mut h, DbEvent::Isolation(Some(Isolation::RepeatableRead)));
        session(&mut h, DbEvent::Autocommit(true));
        assert!(sends(&mut h).is_empty(), "{via:?}: the run cancelled never goes");
    }
}

/// A read-only profile refuses a function it cannot check in every way: nothing is sent, the
/// refusal is said, a dialog view stays open with its draft.
#[test]
fn every_way_is_refused_on_a_read_only_profile() {
    const REST: &str = " AND my_fn(id) > 0";
    for via in VIAS {
        let mut h = pg_where(&format!("{BASE}{REST}"));
        let ro = Environment::new("ro", Policy { read_only: true, ..Policy::default() });
        h.app.policies.environments_mut().push(ro);
        h.app.profiles[0].policy = Some("ro".into());
        via.change(&mut h);
        assert_eq!(confirm(&h), None, "{via:?}: refused, not asked");
        assert!(sends(&mut h).is_empty(), "{via:?}");
        let screen = h.screen(120, 34);
        assert!(screen.contains("Filter refused"), "{via:?}: {screen}");
        via.kept(&h);
    }
}

/// Pending edits of the rows shown are asked about before any way's run: `Enter` keeps them
/// (nothing sent, the dialog view as it was), `y` leaves them and sends.
#[test]
fn every_way_asks_before_leaving_pending_edits() {
    for via in VIAS {
        let mut h = pg_where(BASE);
        if let Results::Rows(rs) = &mut h.app.tab_mut().results {
            assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
        }
        via.change(&mut h);
        assert!(matches!(confirm(&h), Some(ConfirmAction::LeaveEdits(_))), "{via:?}: {:?}", confirm(&h));
        assert!(sends(&mut h).is_empty(), "{via:?}");
        h.key(KeyCode::Enter);
        assert!(sends(&mut h).is_empty(), "{via:?}: Enter keeps the edits");
        assert!(matches!(&h.app.tab().results, Results::Rows(rs) if !rs.edit.batch.is_empty()), "{via:?}");
        via.kept(&h);
        via.again(&mut h);
        h.keys("y");
        assert_eq!(sends(&mut h), vec![via.sent(PG, "")], "{via:?}");
        assert!(!open(&h), "{via:?}");
    }
}

/// The ways that count a column's values: the condition popup and the whole dialog.
const COUNTING: [Via; 2] = [Via::ConditionPopup, Via::Advanced];

/// Condition `term`'s popup, or the whole dialog turned to from it.
fn open_on(h: &mut Harness, via: Via, term: usize) {
    open_chip(h, term);
    if via == Via::Advanced {
        h.keys("A");
        assert_eq!(popup(h), None);
    }
}

/// The counts of a column's values sent since the last look: request, statement, time limit.
fn group_counts(h: &mut Harness) -> Vec<(u64, String, Option<Duration>)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::GroupCount { id, sql, limit } => Some((id, sql, limit)),
            _ => None,
        })
        .collect()
}

/// The server's answer to count `id`.
fn counted(h: &mut Harness, id: u64, result: Result<GroupCounts, DbError>) {
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::GroupCounted { id, result, snapshot: false });
}

/// The server's answer to count `id`: one value, `zz-late`, in 7 rows.
fn counted_late(h: &mut Harness, id: u64) {
    let value = GroupValue {
        value: Some("zz-late".into()),
        whole: true,
        bytes: Some(7),
        rows: 7,
        prefix: false,
        inexact: false,
    };
    counted(h, id, Ok(GroupCounts { values: vec![value], more: false }));
}

/// `#` in the view open, the count it sends.
fn count_values(h: &mut Harness) -> u64 {
    h.keys("#");
    let sent = group_counts(h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    sent[0].0
}

/// A count of values answers only the view that asked for it while it is open: answered in
/// time it is listed; its answer coming again late, once the view was closed and opened again
/// or opened on another column, or after the session was opened again, is dropped.
#[test]
fn a_late_count_of_values_is_dropped_after_close_column_switch_and_reconnect() {
    for via in COUNTING {
        // In time: listed.
        let mut h = pg_where("name = 'n1' AND id > 0");
        open_on(&mut h, via, 0);
        let id = count_values(&mut h);
        counted_late(&mut h, id);
        assert!(h.screen(120, 34).contains("zz-late"), "{via:?}: {}", h.screen(120, 34));
        // Closed (its count cancelled first) and opened again on the same column.
        let mut h = pg_where("name = 'n1' AND id > 0");
        open_on(&mut h, via, 0);
        let id = count_values(&mut h);
        h.key(KeyCode::Esc);
        assert!(open(&h), "{via:?}: Esc cancels the count first");
        counted(&mut h, id, Err(DbError::Cancelled));
        h.key(KeyCode::Esc);
        assert!(!open(&h), "{via:?}");
        open_on(&mut h, via, 0);
        counted_late(&mut h, id);
        let screen = h.screen(120, 34);
        assert!(!screen.contains("zz-late"), "{via:?}: closed and opened again: {screen}");
        // Another column's view.
        let mut h = pg_where("name = 'n1' AND id > 0");
        open_on(&mut h, via, 0);
        let id = count_values(&mut h);
        h.key(KeyCode::Esc);
        counted(&mut h, id, Err(DbError::Cancelled));
        h.key(KeyCode::Esc);
        assert!(!open(&h), "{via:?}");
        open_on(&mut h, via, 1);
        counted_late(&mut h, id);
        let screen = h.screen(120, 34);
        assert!(!screen.contains("zz-late"), "{via:?}: another column: {screen}");
        // The session lost and opened again while the view is open.
        let mut h = pg_where("name = 'n1' AND id > 0");
        open_on(&mut h, via, 0);
        let id = count_values(&mut h);
        session(&mut h, DbEvent::Lost { error: DbError::Closed });
        session(&mut h, DbEvent::Connected);
        assert!(open(&h), "{via:?}: the view stays open");
        counted_late(&mut h, id);
        let screen = h.screen(120, 34);
        assert!(!screen.contains("zz-late"), "{via:?}: another session: {screen}");
    }
}

/// `#` in a condition's view counts the column's values (`GroupCount`, at most 201 values, with
/// a time limit) under the other conditions; the rows popup's exact count counts rows
/// (`Count`, within the cap), never values.
#[test]
fn counts_of_values_and_of_rows_are_each_their_own_payload() {
    for via in COUNTING {
        let mut h = pg_where("name = 'n1' AND id > 0");
        open_on(&mut h, via, 0);
        h.keys("#");
        let sent = h.sent();
        assert!(!sent.iter().any(|c| matches!(c, DbCommand::Count { .. })), "{via:?}: {sent:?}");
        let grouped: Vec<_> = sent
            .iter()
            .filter_map(|c| match c {
                DbCommand::GroupCount { sql, limit, .. } => Some((sql.clone(), *limit)),
                _ => None,
            })
            .collect();
        assert_eq!(grouped.len(), 1, "{via:?}: {sent:?}");
        let (sql, limit) = &grouped[0];
        assert_eq!(
            sql,
            r#"SELECT CASE WHEN octet_length("name") <= 4096 THEN "name" END, octet_length("name"), count(*) FROM "shop"."users" WHERE (id > 0) GROUP BY "name" ORDER BY count(*) DESC LIMIT 201"#,
            "{via:?}"
        );
        assert!(limit.is_some(), "{via:?}: an idle session's count has a time limit");
    }
    // The rows popup, a page and more past it, capped at 1000: the exact count of the rows
    // within the cap.
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    page_more(&mut h);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    h.command("where id > 0");
    page_more(&mut h);
    h.app.focus = Focus::Results;
    h.keys(" rl");
    h.type_text("1000");
    h.key(KeyCode::Enter);
    sends(&mut h);
    page_more(&mut h);
    h.app.focus = Focus::Results;
    h.sent();
    h.keys(" rr");
    assert_eq!(popup(&h), Some(PopupKindId::Rows));
    h.keys("jj");
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::GroupCount { .. })), "never values: {sent:?}");
    let rows: Vec<_> = sent
        .iter()
        .filter_map(|c| match c {
            DbCommand::Count { sql, limit, .. } => Some((sql.clone(), *limit)),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 1, "{sent:?}");
    assert!(rows[0].0.contains("count(*)") && rows[0].0.contains("id > 0"), "{rows:?}");
    assert!(rows[0].0.contains("LIMIT 1000"), "within the cap: {rows:?}");
    assert!(rows[0].1.is_some(), "with its time limit: {rows:?}");
    // `#` in the rows popup counts no values either.
    h.keys("#");
    assert!(group_counts(&mut h).is_empty());
}

/// Answer the active tab's run with a full first page and more past it, nothing held open.
fn page_more(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string()), Some(format!("n{}", i % 5))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
}

/// An edit key waiting for the table's structure is not done once the columns popup changed
/// the columns the grid shows meanwhile: it would land where the press did not aim. The popup
/// closed with nothing changed leaves the press to be done.
#[test]
fn an_edit_waiting_for_the_table_is_dropped_when_the_columns_popup_hides_one() {
    for hide in [true, false] {
        let mut h = pg_where("id > 0");
        h.db(DbEvent::Keys(Ok(shop_keys())));
        // The rows again, their columns those of `shop.users`.
        h.command("where id > 1");
        sends(&mut h);
        let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
        let columns = Some(vec![meta_from("id", "int8", true, USERS, 1), meta_from("name", "text", false, USERS, 3)]);
        let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
        h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
        h.app.focus = Focus::Results;
        h.sent();
        // On `name`, `e` asks for the structure.
        h.app.tab_mut().grid.col = 1;
        h.keys("e");
        let sent = h.sent();
        assert!(sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })), "{sent:?}");
        assert_eq!(h.app.transient.as_ref().map(|(n, _)| n.msg.clone()), Some(Label::EditLoading.into()));
        // `id` hidden from the grid in the columns popup and applied (no run), or the popup
        // closed as it opened.
        h.keys(" rc");
        assert_eq!(popup(&h), Some(PopupKindId::Columns));
        if hide {
            h.keys(" ");
            h.key(KeyCode::Enter);
        } else {
            h.key(KeyCode::Esc);
        }
        assert!(!open(&h));
        assert!(sends(&mut h).is_empty());
        assert_eq!(h.app.tab().grid.col, 1, "the cursor stays on `name`");
        h.app.focus = Focus::Results;
        let name = h.app.profiles[0].name.clone();
        let structure = Ok(Box::new(users_structure()));
        h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: structure });
        if hide {
            assert_ne!(h.app.key_context(), Ctx::CellEdit, "the press is not done on other columns");
            // A press now, on the columns shown, is.
            h.keys("e");
        }
        assert_eq!(h.app.key_context(), Ctx::CellEdit, "hide: {hide}");
    }
}

/// In the terminal theme a popup says a server error in the error colour, underlined where the
/// server points; a refusal on its hint line in the warning colour (a refusal, never a
/// failure); an expression in the WHERE list in the warning colour.
#[test]
fn the_terminal_theme_says_failed_expr_and_refused_in_its_colours() {
    let th = datarig_tui::theme::TERMINAL.clone();
    let terminal = |h: &mut Harness| h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
    // Failed: the popup opened again underlines the value the server points into.
    let mut h = pg_where("name = 'n1' AND id > 1");
    terminal(&mut h);
    open_chip(&mut h, 1);
    type_value(&mut h, "0");
    h.key(KeyCode::Enter);
    let sent = sends(&mut h);
    let sql = &sent[0].0;
    let position = sql[..sql.find("10").unwrap()].chars().count() as u32 + 1;
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    let error =
        DbError::Server(ServerMessage { text: "ERROR: out of range".into(), position: Some(position), hint: None });
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
    open_chip(&mut h, 1);
    let (x, y) = find(&mut h, "\u{25be}    10").expect("the value field");
    let t = h.draw(120, 34);
    let cell = &t.backend().buffer()[(x + 5, y)];
    assert_eq!((cell.symbol(), cell.fg), ("1", th.error));
    assert!(cell.modifier.contains(Modifier::UNDERLINED));
    let (x, y) = find(&mut h, "out of range").expect("the server's words");
    assert_eq!(h.draw(120, 34).backend().buffer()[(x, y)].fg, th.error);
    // Refused: said on the hint line in the warning colour, the popup open.
    let mut h = pg_where("name = 'n1' AND my_fn(id) > 0");
    terminal(&mut h);
    let ro = Environment::new("ro", Policy { read_only: true, ..Policy::default() });
    h.app.policies.environments_mut().push(ro);
    h.app.profiles[0].policy = Some("ro".into());
    open_chip(&mut h, 0);
    type_value(&mut h, "x");
    h.key(KeyCode::Enter);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    let (x, y) = find(&mut h, "Filter refused").expect("said");
    assert_eq!(h.draw(120, 34).backend().buffer()[(x, y)].fg, th.warning, "a refusal, not a failure");
    // An expression in the WHERE list: in the warning colour.
    let many = "(name = 'n1' OR id > 1) AND name = 'n1' AND id > 1 AND id < 9 AND name <> 'a rather long text'";
    let mut h = pg_where(many);
    terminal(&mut h);
    bar_click(&mut h, BarHit::More);
    assert_eq!(popup(&h), Some(PopupKindId::WhereList));
    let (x, y) = find(&mut h, "(name = 'n1' OR id > 1)").expect("listed");
    assert_eq!(h.draw(120, 34).backend().buffer()[(x, y)].fg, th.warning);
}
