//! A result's rows: `o` adds a new row and `O` a copy of the cursor's, both shown after the rows
//! read; `D` marks rows to be deleted (or no longer) and removes new rows. A new row's cells are
//! edited as a row's are, by what a new row takes; what reads rows by index never shows the row
//! read before a new row.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ValueKind};
use datarig_core::edit::{Counts, Value};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::edit::EditState;
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

/// `shop.users`' id, email and nickname.
fn users_columns() -> Vec<ColumnMeta> {
    vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("nickname", "text", false, USERS, 4),
    ]
}

fn users_rows() -> Vec<Vec<Option<String>>> {
    vec![vec![s("1"), s("a@x"), s("ann")], vec![s("2"), s("b@x"), None], vec![s("3"), s("c@x"), s("cy")]]
}

/// Run `sql` and let `columns` and `rows` come back as its result (`more`: the server has more),
/// the grid focused.
fn answer(h: &mut Harness, sql: &str, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>, more: bool) {
    h.app.run(vec![sql.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
}

/// Connected (the keys of `shop` read), the users' rows in the grid; nothing sent yet.
fn users_more(lang: Lang, more: bool) -> Harness {
    let mut h = Harness::connected(lang);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), more);
    h.sent();
    h
}

fn structure(h: &mut Harness, result: Result<TableStructure, DbError>) {
    let name = h.app.profiles[0].name.clone();
    let result = result.map(Box::new);
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result });
}

fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
        _ => panic!("no row result"),
    }
}

/// The new rows' values, in the order shown.
fn drafts(h: &Harness) -> Vec<Vec<Value>> {
    edits(h).batch.drafts().iter().map(|d| d.cells.clone()).collect()
}

fn counts(h: &Harness) -> Counts {
    edits(h).batch.counts()
}

fn cursor(h: &Harness) -> (usize, usize) {
    (h.app.tab().grid.row, h.app.tab().grid.col)
}

fn said(h: &mut Harness) -> Option<Msg> {
    h.app.transient.take().map(|(n, _)| n.msg)
}

fn refused(reason: &str) -> Option<Msg> {
    Some(Msg::EditRefused { reason: reason.into() })
}

/// Resolved with `st`: `e` on the first row's email, the structure answered, the editor closed.
fn ready_with(lang: Lang, st: TableStructure) -> Harness {
    let mut h = users_more(lang, false);
    h.keys("le");
    structure(&mut h, Ok(st));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

fn ready(lang: Lang) -> Harness {
    ready_with(lang, users_structure())
}

fn editor_text(h: &Harness) -> String {
    h.app.tab().cell_edit.as_ref().expect("the cell editor is open").input.text().to_string()
}

fn clear(h: &mut Harness) {
    for _ in 0..editor_text(h).chars().count() {
        h.key(KeyCode::Backspace);
    }
}

/// Type `v` into the cursor's cell.
fn type_cell(h: &mut Harness, v: &str) {
    h.keys("e");
    clear(h);
    h.type_text(v);
    h.key(KeyCode::Enter);
}

/// The text of the inspector's panel as drawn at 120x40.
fn inspector(h: &mut Harness) -> String {
    let t = h.draw(120, 40);
    let d = h.app.layout.detail;
    let buf = t.backend().buffer();
    (d.y..d.y + d.height)
        .map(|y| row_text(buf, y).chars().skip(d.x as usize).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn o_adds_a_new_row_after_the_rows_read_with_every_value_its_default() {
    let mut h = ready(Lang::En);
    h.keys("o");
    assert_eq!(drafts(&h), [vec![Value::Default; 3]]);
    assert_eq!(cursor(&h), (3, 0), "on the new row's first column that takes a value");
    assert_eq!(counts(&h), Counts { updates: 0, inserts: 1, deletes: 0 });
    // `!`, not `+`: its email (NOT NULL, no default) needs a value.
    let screen = h.screen(120, 40);
    assert!(screen.contains("  !│") && !screen.contains(" 4 │"), "{screen}");
    // From a row read: after the other new rows.
    h.keys("ggo");
    assert_eq!(cursor(&h), (4, 0));
    // From a new row: right after it.
    let first = edits(&h).batch.drafts()[0].id;
    h.keys("ko");
    let ids: Vec<u64> = edits(&h).batch.drafts().iter().map(|d| d.id).collect();
    assert_eq!((ids.len(), ids[0]), (3, first));
    assert_eq!(cursor(&h), (4, 0));
    assert_eq!(edits(&h).batch.drafts()[1].id, ids[1]);
    assert!(ids[1] > ids[2], "the newest is next to the cursor's new row");
    // Undone, the cursor stays on a row shown.
    h.keys("uuu");
    assert!(drafts(&h).is_empty());
    assert_eq!(cursor(&h).0, 2);
    h.ctrl('r');
    assert_eq!(drafts(&h).len(), 1);
    h.keys(" do");
    assert_eq!(drafts(&h).len(), 2, "Space d o too");
}

#[test]
fn a_result_whose_columns_take_no_value_in_a_new_row_adds_none() {
    let mut st = users_structure();
    st.columns[1].fill = ColumnFill::Stored("lower(name)".into());
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let columns = vec![meta_from("email", "text", false, USERS, 2)];
    answer(&mut h, "SELECT email FROM shop.users", columns, vec![vec![s("a@x")]], false);
    h.keys("o");
    structure(&mut h, Ok(st));
    assert_eq!(said(&mut h), refused("no column of the result takes a value in a new row"));
    assert!(edits(&h).batch.is_empty());
    h.keys("O");
    assert_eq!(said(&mut h), refused("no column of the result takes a value in a new row"));
}

#[test]
fn o_cap_copies_the_cursors_row_but_its_key_and_what_the_table_numbers() {
    let mut h = ready(Lang::En);
    // A pending value is copied as it is.
    h.keys("l");
    type_cell(&mut h, "zed");
    h.keys("O");
    assert_eq!(drafts(&h), [vec![Value::Default, text("a@x"), text("zed")]], "the key takes its default");
    assert_eq!(cursor(&h), (3, 2), "the cursor's column kept");
    // A new row's copy: right after it; NULL copies as NULL.
    h.keys("jjj");
    assert_eq!(cursor(&h).0, 3, "the last row shown");
    h.keys("kkO");
    assert_eq!(drafts(&h)[1], [Value::Default, text("b@x"), Value::Null]);
    h.keys("O");
    assert_eq!(drafts(&h).len(), 3);
    assert_eq!(drafts(&h)[2], drafts(&h)[1], "the copy of the copy, right after it");
    assert_eq!(cursor(&h).0, 5);
    h.keys("u");
    assert_eq!(drafts(&h).len(), 2);
    h.keys(" dO");
    assert_eq!(drafts(&h).len(), 3, "Space d O too");
}

#[test]
fn a_copy_takes_the_default_of_an_identity_and_of_the_key() {
    // No primary key: the rows are found by the email; the id is an identity.
    let mut st = users_structure();
    st.primary_key = None;
    st.indexes.retain(|i| !i.primary);
    st.columns[0].default = None;
    st.columns[0].fill = ColumnFill::IdentityByDefault;
    let mut h = ready_with(Lang::En, st);
    h.keys("O");
    assert_eq!(drafts(&h), [vec![Value::Default, Value::Default, text("ann")]]);
    // A generated column takes its default too.
    let mut st = users_structure();
    st.columns[3].fill = ColumnFill::Stored("upper(name)".into());
    let mut h = ready_with(Lang::En, st);
    h.keys("O");
    assert_eq!(drafts(&h), [vec![Value::Default, text("a@x"), Value::Default]]);
}

/// A MySQL profile, connected, the keys of `shop.users` read; its id, email and a `FLOAT`
/// nickname in the grid.
fn mysql_floats(rows: Vec<Vec<Option<String>>>) -> Harness {
    use datarig_core::driver::keys::{KeyCatalog, KeyKind, NameRule};
    use datarig_core::driver::{ColumnOrigin, SessionValues};
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let cfg = Config {
        connections: vec![ConnectionConfig { driver: "mysql".into(), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.set_server_zone("UTC");
    let names = ["id", "email", "name", "nickname", "profile"];
    k.add_table(1, "shop", "users", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k.mark(1, &[1], KeyKind::Primary);
    h.db(DbEvent::Keys(Ok(k)));
    h.db(DbEvent::SessionValues(SessionValues { time_zone: "+00:00".into(), charset: "utf8mb4".into() }));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "users".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = vec![
        col("id", "bigint", ValueKind::Integer),
        col("email", "varchar", ValueKind::Text),
        col("nickname", "float", ValueKind::Float),
    ];
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", columns, rows, false);
    h.sent();
    let mut st = users_structure();
    st.columns[0].type_name = "bigint".into();
    st.columns[0].default = None;
    st.columns[0].fill = ColumnFill::AutoIncrement;
    st.columns[1].type_name = "varchar(100)".into();
    st.columns[3].type_name = "float".into();
    h.keys("e");
    structure(&mut h, Ok(st));
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h
}

#[test]
fn a_copy_of_a_value_not_shown_whole_is_refused() {
    let mut h = mysql_floats(vec![vec![s("1"), s("a@x"), s("1.5")], vec![s("2"), s("b@x"), None]]);
    h.keys("O");
    assert_eq!(said(&mut h), refused("nickname is not shown whole; it can't be copied"));
    assert!(edits(&h).batch.is_empty());
    // NULL is whole; the AUTO_INCREMENT id takes its default.
    h.keys("jO");
    assert_eq!(drafts(&h), [vec![Value::Default, text("b@x"), Value::Null]]);
}

#[test]
fn d_marks_a_row_to_be_deleted_and_again_no_longer() {
    let mut h = ready(Lang::En);
    h.keys("jD");
    assert!(edits(&h).batch.deleted(1));
    assert_eq!(counts(&h), Counts { updates: 0, inserts: 0, deletes: 1 });
    assert!(h.screen(120, 40).contains(" 2-│"));
    h.keys("D");
    assert!(edits(&h).batch.is_empty());
    h.keys(" dd");
    assert!(edits(&h).batch.deleted(1), "Space d d too");
    // Its cells are not edited.
    h.keys("e");
    assert_eq!(said(&mut h), refused("row 2 is marked for deletion"));
    h.keys(" dn");
    assert_eq!(said(&mut h), refused("row 2 is marked for deletion"));
    h.keys("u");
    assert!(!edits(&h).batch.deleted(1));
    h.ctrl('r');
    assert!(edits(&h).batch.deleted(1));
    // A new row copied from it takes its values as read.
    h.keys("O");
    assert_eq!(drafts(&h), [vec![Value::Default, text("b@x"), Value::Null]]);
}

#[test]
fn revert_on_a_row_marked_for_deletion_unmarks_it() {
    let mut h = ready(Lang::En);
    h.keys("jD");
    h.keys(" dr");
    assert_eq!(said(&mut h), None);
    assert!(edits(&h).batch.is_empty());
    h.keys("u");
    assert!(edits(&h).batch.deleted(1));
}

#[test]
fn revert_over_rows_read_and_new_rows_is_one_change() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "x@x");
    h.keys("jjDol");
    type_cell(&mut h, "y@x");
    assert_eq!(drafts(&h), [vec![Value::Default, text("y@x"), Value::Default]]);
    h.keys("Vkkk dr");
    assert_ne!(said(&mut h), Some(Label::EditNothingToRevert.into()));
    assert_eq!(counts(&h), Counts { updates: 0, inserts: 1, deletes: 0 });
    assert_eq!(drafts(&h), [vec![Value::Default; 3]], "a new row's cells go back to their default");
    h.keys("u");
    assert_eq!(counts(&h), Counts { updates: 1, inserts: 1, deletes: 1 }, "one undo brings all back");
    assert_eq!(drafts(&h)[0][1], text("y@x"));
}

#[test]
fn d_on_a_new_row_removes_it() {
    let mut h = ready(Lang::En);
    h.keys("oD");
    assert!(edits(&h).batch.is_empty());
    assert_eq!(cursor(&h).0, 2, "the cursor stays on a row shown");
    h.keys("u");
    assert_eq!(drafts(&h).len(), 1);
}

#[test]
fn rows_and_new_rows_selected_are_deleted_as_one_change() {
    let mut h = ready(Lang::En);
    h.keys("okVjD");
    assert!(drafts(&h).is_empty() && edits(&h).batch.deleted(2));
    assert_eq!(counts(&h), Counts { updates: 0, inserts: 0, deletes: 1 });
    assert_eq!(h.app.tab().grid.anchor, None, "the selection goes");
    h.keys("u");
    assert!(drafts(&h).len() == 1 && !edits(&h).batch.deleted(2), "one undo brings both back");
    h.ctrl('r');
    assert!(drafts(&h).is_empty() && edits(&h).batch.deleted(2));
    // A `v` selection deletes its rows; whole columns are not rows.
    h.keys("u");
    h.keys("gglvjD");
    assert!(edits(&h).batch.deleted(0) && edits(&h).batch.deleted(1));
    h.draw(120, 40);
    let g = h.app.tab().grid.clone();
    let (x, y) = (g.hit_cols[1].0 + 1, g.header_y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.keys("D");
    assert_eq!(said(&mut h), Some(Label::EditDeleteColumns.into()));
}

#[test]
fn a_new_rows_cells_are_edited_by_what_a_new_row_takes() {
    let mut h = ready(Lang::En);
    h.keys("ol");
    h.keys("e");
    assert_eq!(editor_text(&h), "", "a default is no text");
    h.type_text("n@x");
    h.key(KeyCode::Enter);
    assert_eq!(drafts(&h)[0][1], text("n@x"));
    h.keys("e");
    assert_eq!(editor_text(&h), "n@x");
    // NULL in a NOT NULL column is refused; the editor stays.
    h.ctrl('n');
    assert_eq!(said(&mut h), refused("email is NOT NULL"));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    // DEFAULT is taken in any column of a new row.
    h.ctrl('d');
    assert_eq!(drafts(&h)[0][1], Value::Default);
    h.keys("l dn");
    assert_eq!(drafts(&h)[0][2], Value::Null);
    h.keys(" dD");
    assert_eq!(drafts(&h)[0][2], Value::Default);
    // The typed value is checked against the column's type.
    h.keys("hh");
    type_cell(&mut h, "x");
    assert_eq!(said(&mut h), refused("not a whole number"));
    h.key(KeyCode::Esc);
    type_cell(&mut h, "7");
    assert_eq!(drafts(&h)[0][0], text("7"));
    // Undo and redo, one step each.
    h.keys("u");
    assert_eq!(drafts(&h)[0][0], Value::Default);
    h.ctrl('r');
    assert_eq!(drafts(&h)[0][0], text("7"));
    // Revert sets the cell back to its default; the new row stays.
    h.keys(" dr");
    assert_eq!(drafts(&h), [vec![Value::Default, Value::Default, Value::Default]]);
    h.keys(" dr");
    assert_eq!(said(&mut h), Some(Label::EditNothingToRevert.into()));
    h.keys("u");
    assert_eq!(drafts(&h)[0][0], text("7"));
    // A selection of new rows and rows read is not edited together.
    h.keys("k");
    assert_eq!(cursor(&h).0, 2);
    h.keys("vje");
    assert_eq!(said(&mut h), Some(Label::EditMixedRows.into()));
}

#[test]
fn a_new_rows_identity_and_sequence_take_their_default_and_a_typed_value() {
    let mut st = users_structure();
    st.columns[0].default = None;
    st.columns[0].fill = ColumnFill::IdentityAlways;
    let mut h = ready_with(Lang::En, st);
    h.keys("o");
    type_cell(&mut h, "5");
    assert_eq!(drafts(&h)[0][0], text("5"), "GENERATED ALWAYS takes a value in a new row");
    h.keys(" dD");
    assert_eq!(drafts(&h)[0][0], Value::Default);
    // A sequence's default too (refused on a row read).
    let mut h = ready(Lang::En);
    h.keys("o");
    h.keys("e");
    h.ctrl('d');
    assert_eq!(drafts(&h)[0][0], Value::Default);
    assert_eq!(said(&mut h), None);
    // A generated column takes no value.
    let mut st = users_structure();
    st.columns[3].fill = ColumnFill::Stored("upper(name)".into());
    let mut h = ready_with(Lang::En, st);
    h.keys("olle");
    assert_eq!(said(&mut h), refused("nickname is generated by the database"));
}

#[test]
fn rows_read_later_come_before_the_new_rows() {
    let mut h = users_more(Lang::En, true);
    h.keys("le");
    structure(&mut h, Ok(users_structure()));
    h.key(KeyCode::Esc);
    h.keys("o");
    let id = edits(&h).batch.drafts()[0].id;
    assert_eq!(cursor(&h).0, 3);
    h.keys("kV");
    h.keys("j");
    let query = h.app.tab().exec.query_id;
    let rows = vec![vec![s("4"), s("d@x"), None], vec![s("5"), s("e@x"), None]];
    h.db(DbEvent::Page { id: query, columns: None, rows, more: false, elapsed: Duration::ZERO });
    assert_eq!(cursor(&h).0, 5, "still on the new row");
    assert_eq!(edits(&h).batch.drafts()[0].id, id);
    // A selection from a row read to a new row would take in the rows that came: it goes.
    assert_eq!(h.app.tab().grid.anchor, None, "the selection goes");
    let screen = h.screen(120, 40);
    let at = |t: &str| screen.find(t).unwrap_or_else(|| panic!("{t}:\n{screen}"));
    assert!(at(" 5 │") < at("  !│"), "{screen}");
    h.keys("D");
    assert_eq!(counts(&h), Counts::default(), "only the new row goes, no row that came is marked");
}

#[test]
fn rows_that_come_keep_the_cursor_on_its_new_row_and_its_page() {
    let mut h = Harness::connected(Lang::En);
    h.app.page_size = 3;
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), true);
    h.sent();
    h.keys("le");
    structure(&mut h, Ok(users_structure()));
    h.key(KeyCode::Esc);
    h.keys("o");
    assert_eq!(cursor(&h).0, 3);
    let query = h.app.tab().exec.query_id;
    let rows = vec![vec![s("4"), s("d@x"), None], vec![s("5"), s("e@x"), None]];
    h.db(DbEvent::Page { id: query, columns: None, rows, more: false, elapsed: Duration::ZERO });
    h.draw(120, 40);
    assert_eq!(cursor(&h).0, 5, "still on the new row, drawn");
    assert_eq!(h.app.tab().grid.page, 1, "the page the new row is on");
    h.keys("D");
    assert_eq!(counts(&h), Counts::default(), "the new row goes, not the row read before it");
}

#[test]
fn copy_search_and_the_inspector_never_take_the_row_before_a_new_row() {
    let mut h = ready(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("o");
    let text = inspector(&mut h);
    assert!(text.contains("A new row, not applied yet") && !text.contains("c@x"), "{text}");
    h.keys("I");
    let text = inspector(&mut h);
    assert!(text.contains("A new row, not applied yet") && !text.contains("cy"), "{text}");
    h.keys("y");
    assert_eq!(said(&mut h), Some(Label::CopyNewRows.into()));
    h.keys("Y");
    assert_eq!(said(&mut h), Some(Label::CopyNewRows.into()));
    h.keys("kvjy");
    assert_eq!(said(&mut h), Some(Label::CopyNewRows.into()));
    h.key(KeyCode::Esc);
    assert_eq!(clip.last(), None, "nothing copied");
    // Every row of the page: the rows read.
    h.ctrl('a');
    h.keys("y");
    assert_eq!(clip.last().map(|t| t.lines().count()), Some(4), "the header and the rows read");
    // The cell viewer shows nothing; the menu has no cell.
    h.keys("Gl");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    // A search from a new row goes on from the rows read.
    h.keys("/");
    h.type_text("b@x");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (1, 1));
    assert!(clip.all().len() == 1, "{:?}", clip.all());
}

#[test]
fn the_cursor_goes_across_new_rows_by_key_and_by_mouse() {
    let mut h = ready(Lang::En);
    h.keys("oo");
    h.keys("gg");
    h.keys("jjj");
    assert_eq!(cursor(&h).0, 3, "down from the last row read");
    h.keys("G");
    assert_eq!(cursor(&h).0, 4);
    h.keys("k");
    assert_eq!(cursor(&h).0, 3);
    h.keys("gg");
    h.draw(120, 40);
    let g = h.app.tab().grid.clone();
    let (x, y) = (g.hit_cols[1].0 + 2, g.data_y + 4 - g.top as u16);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(cursor(&h), (4, 1), "a click on a new row");
    h.keys("e");
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "its cell edited");
    h.type_text("q@x");
    h.key(KeyCode::Enter);
    assert_eq!(drafts(&h)[1][1], text("q@x"));
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::LoadStructure { .. })));
}

fn screens(name: &str, setup: impl Fn(Lang) -> Harness) {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = setup(lang);
        let tag = format!("{lang:?}").to_lowercase();
        for (w, hh) in [(120, 40), (80, 24)] {
            assert_screen!(format!("{name}_{tag}_{w}x{hh}"), lang, h.draw(w, hh));
        }
    }
}

/// Two new rows (one typed in, its copy): `+` in the gutter, no number.
#[test]
fn new_rows_show_after_the_rows_read() {
    screens("edit_rows_new", |lang| {
        let mut h = ready(lang);
        h.keys("ol");
        type_cell(&mut h, "n@x");
        h.keys("O");
        h.app.transient = None;
        h
    });
}

#[test]
fn a_row_to_be_deleted_shows_crossed_out() {
    screens("edit_rows_deleted", |lang| {
        let mut h = ready(lang);
        h.keys("jD");
        h.app.transient = None;
        h
    });
}

/// A changed cell, a row to be deleted and a new row at once.
#[test]
fn changed_deleted_and_new_rows_show_together() {
    screens("edit_rows_mixed", |lang| {
        let mut h = ready(lang);
        type_cell(&mut h, "a@y");
        h.keys("jD");
        h.keys("o");
        h.keys("l");
        type_cell(&mut h, "n@x");
        h.keys("gg");
        h.app.transient = None;
        h
    });
}

/// In a dark and a light theme: a new row's typed cell in `edit_inserted` (a default dim), a
/// row to be deleted in `edit_deleted`, crossed out.
#[test]
fn new_and_deleted_rows_take_their_tokens() {
    use datarig_tui::theme;
    use ratatui::style::Modifier;
    for th in [theme::DARK, theme::LIGHT] {
        let mut h = ready(Lang::En);
        h.app.theme = std::sync::Arc::new(th.clone());
        h.keys("jD");
        h.keys("ol");
        type_cell(&mut h, "n@x");
        h.keys("gg");
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let g = &h.app.tab().grid;
        let new = g.cell_rect(3, 1, 3).expect("on screen");
        let at = buf[(new.x + 1, new.y)].clone();
        assert_eq!((at.fg, at.bg), (th.edit_inserted.fg.unwrap(), th.surface_alt), "a new row's value");
        assert!(at.modifier.contains(Modifier::BOLD));
        let default = g.cell_rect(3, 2, 3).expect("on screen");
        let at = buf[(default.x + 1, default.y)].clone();
        assert!(at.fg == th.fg_muted && at.modifier.contains(Modifier::DIM), "a default: {at:?}");
        let gone = g.cell_rect(1, 1, 3).expect("on screen");
        let at = buf[(gone.x + 1, gone.y)].clone();
        assert_eq!(at.fg, th.edit_deleted.fg.unwrap(), "a row to be deleted");
        assert!(at.modifier.contains(Modifier::CROSSED_OUT));
        let kept = g.cell_rect(2, 1, 3).expect("on screen");
        assert_eq!(buf[(kept.x + 1, kept.y)].fg, th.fg, "a row kept");
    }
}

/// A new row leaves `email` (NOT NULL, no default) to a default it lacks: Apply is refused for
/// it, so the grid says so where it is, in a dark and a light theme and at both sizes. The cell
/// reads that a value is needed in `edit_invalid` (bold, underlined, not the dim `default`) and
/// the gutter has `!` for `+`. A typed value makes it a plain new row again.
#[test]
fn a_new_row_without_a_needed_value_is_marked_in_the_grid() {
    use datarig_tui::theme;
    use ratatui::style::Modifier;
    for th in [theme::DARK, theme::LIGHT] {
        let mut h = ready(Lang::En);
        h.app.theme = std::sync::Arc::new(th.clone());
        h.keys("o");
        h.app.transient = None;
        // The grid's mark and the Changes tab's problem are one finding.
        let needed = "email is NOT NULL and has no default";
        assert_eq!(h.app.shown_invalid().expect("resolved").no_default.len(), 1);
        assert!(h.app.shown_changes().expect("a model").problems.iter().any(|p| p.text.contains(needed)));
        for (w, hh) in [(120, 40), (80, 24)] {
            let t = h.draw(w, hh);
            let buf = t.backend().buffer();
            let g = &h.app.tab().grid;
            let email = g.cell_rect(3, 1, 3).expect("on screen");
            let line = row_text(buf, email.y);
            assert!(line.contains(" !\u{2502} defa\u{2026} \u{2502} ! value\u{2026} \u{2502}"), "{w}x{hh}: {line:?}");
            let at = buf[(email.x + 1, email.y)].clone();
            assert_eq!(at.symbol(), "!");
            assert_eq!(at.fg, th.edit_invalid.fg.unwrap(), "{w}x{hh}: the marker's color");
            assert!(at.modifier.contains(Modifier::BOLD | Modifier::UNDERLINED), "{at:?}");
            assert!(!at.modifier.contains(Modifier::DIM), "not the dim default: {at:?}");
            // The gutter's `!`, in the error color, just before the first column's bar.
            let bar = (0..email.x).rev().find(|&x| buf[(x, email.y)].symbol() == "\u{2502}").expect("a bar");
            let x = (0..bar).rev().find(|&x| buf[(x, email.y)].symbol() == "!").expect("the gutter's mark");
            assert_eq!(buf[(x, email.y)].fg, th.error, "{w}x{hh}: the gutter's `!`");
            // The id, with a default of its own, keeps its dim `default`.
            let id = g.cell_rect(3, 0, 3).expect("on screen");
            assert!(buf[(id.x + 2, id.y)].modifier.contains(Modifier::DIM));
        }
        h.keys("l");
        type_cell(&mut h, "n@x");
        let t = h.draw(120, 40);
        let y = h.app.tab().grid.cell_rect(3, 1, 3).expect("on screen").y;
        let line = row_text(t.backend().buffer(), y);
        assert!(line.contains(" +\u{2502} defa\u{2026} \u{2502} n@x"), "{line:?}");
        assert!(!line.contains("value"), "{line:?}");
        assert!(h.app.shown_invalid().expect("resolved").no_default.is_empty());
        assert!(h.app.shown_changes().expect("a model").problems.iter().all(|p| !p.text.contains(needed)));
    }
}

/// The marker and the gutter's `!` of a new row that needs a value.
#[test]
fn a_new_row_without_a_needed_value_shows_its_marker() {
    screens("edit_rows_value_needed", |lang| {
        let mut h = ready(lang);
        h.keys("o");
        h.app.transient = None;
        h
    });
}

/// The Korean strings of the row keys fit at 80x24.
#[test]
fn the_row_refusals_read_in_korean() {
    let mut h = ready(Lang::Ko);
    h.keys("oy");
    let status = h.status(80, 24);
    let head: String = Label::CopyNewRows.text(Lang::Ko).chars().take(4).collect();
    assert!(status.contains(&head), "{status}");
}

#[test]
fn a_result_with_no_rows_takes_new_rows() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), Vec::new(), false);
    h.keys("o");
    structure(&mut h, Ok(users_structure()));
    assert_eq!(drafts(&h).len(), 1);
    assert_eq!(cursor(&h), (0, 0));
    let screen = h.screen(120, 40);
    assert!(screen.contains("  !│ defa…"), "{screen}");
    h.keys("D");
    assert!(edits(&h).batch.is_empty());
    assert!(h.screen(120, 40).contains("No rows"));
}
