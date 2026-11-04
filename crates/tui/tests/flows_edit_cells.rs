//! Editing a result's cells: the first edit key reads the table's structure (once), and the
//! press is done when it comes; the cell editor, NULL and DEFAULT, undo and redo, a column's
//! selected cells as one change, the values of an enum or a boolean in a list; and the reasons
//! a result stays read-only, each said once per press with nothing changed.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ValueKind};
use datarig_core::edit::Value;
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::edit::{EditState, Target};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{Event, KeyCode};
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// `shop.users`' id, email and nickname, three rows (the second without a nickname).
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

/// Run `sql` and let `columns` and `rows` come back as its result, the grid focused.
fn answer(h: &mut Harness, sql: &str, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) {
    h.app.run(vec![sql.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
}

/// Connected (the keys of `shop` read), the users' rows in the grid; nothing sent yet.
fn users(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    h.sent();
    h
}

/// The structures asked for since the last call.
fn asked(h: &mut Harness) -> Vec<(String, String)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::LoadStructure { schema, table } => Some((schema, table)),
            _ => None,
        })
        .collect()
}

/// The metadata session answers with `shop.users`' structure (or why it could not be read).
fn structure(h: &mut Harness, result: Result<TableStructure, DbError>) {
    let name = h.app.profiles[0].name.clone();
    let result = result.map(Box::new);
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result });
}

/// The shown result's edits.
fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
        _ => panic!("no row result"),
    }
}

fn value(h: &Harness, row: usize, col: usize) -> Option<Value> {
    edits(h).batch.value(row, col).cloned()
}

/// What the status line said last (and forget it).
fn said(h: &mut Harness) -> Option<Msg> {
    h.app.transient.take().map(|(n, _)| n.msg)
}

/// Resolved: `e` on the first row's email, the structure answered, the editor closed again.
fn ready(lang: Lang) -> Harness {
    let mut h = users(lang);
    h.keys("le");
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

fn editor_text(h: &Harness) -> String {
    h.app.tab().cell_edit.as_ref().expect("the cell editor is open").input.text().to_string()
}

/// Clear the editor's text.
fn clear(h: &mut Harness) {
    for _ in 0..editor_text(h).chars().count() {
        h.key(KeyCode::Backspace);
    }
}

fn read_only(reason: &str) -> Option<Msg> {
    Some(Msg::EditReadOnly { reason: reason.into() })
}

fn refused(reason: &str) -> Option<Msg> {
    Some(Msg::EditRefused { reason: reason.into() })
}

#[test]
fn the_first_edit_key_reads_the_structure_once_and_opens_the_editor_when_it_comes() {
    let mut h = users(Lang::En);
    h.keys("le");
    assert_eq!(asked(&mut h), [("shop".to_string(), "users".to_string())]);
    assert_eq!(said(&mut h), Some(Label::EditLoading.into()));
    // Pressed again while it loads: said again, nothing sent.
    h.keys("e");
    assert_eq!(asked(&mut h), Vec::<(String, String)>::new());
    assert_eq!(said(&mut h), Some(Label::EditLoading.into()));
    assert_eq!(h.app.key_context(), Ctx::Grid);
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "the press is done once the structure came");
    assert_eq!(editor_text(&h), "a@x", "the value as read");
    h.type_text("y");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(value(&h, 0, 1), Some(Value::Text("a@xy".into())));
    assert_eq!(edits(&h).batch.counts().total(), 1);
    let screen = h.screen(120, 40);
    assert!(screen.contains(" 1~│") && screen.contains("a@xy"), "{screen}");
    assert!(!screen.contains(" 2~│"), "{screen}");
    // Resolved: the next key opens the editor at once, on the pending value.
    h.keys("e");
    assert_eq!(asked(&mut h), Vec::<(String, String)>::new());
    assert_eq!(editor_text(&h), "a@xy");
}

#[test]
fn u_undoes_and_ctrl_r_redoes() {
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("1");
    h.key(KeyCode::Enter);
    h.keys("je");
    h.type_text("2");
    h.key(KeyCode::Enter);
    assert_eq!(edits(&h).batch.counts().total(), 2);
    h.keys("u");
    assert_eq!((value(&h, 0, 1), value(&h, 1, 1)), (Some(Value::Text("a@x1".into())), None));
    h.keys("u");
    assert_eq!(value(&h, 0, 1), None);
    h.keys("u");
    assert_eq!(said(&mut h), Some(Label::EditNothingToUndo.into()));
    h.ctrl('r');
    h.ctrl('r');
    assert_eq!(value(&h, 1, 1), Some(Value::Text("b@x2".into())));
    h.ctrl('r');
    assert_eq!(said(&mut h), Some(Label::EditNothingToRedo.into()));
    // The chords of `Space d` too, and revert drops the cursor's cell's change.
    h.keys(" du");
    assert_eq!(value(&h, 1, 1), None);
    h.keys(" dU");
    assert_eq!(value(&h, 1, 1), Some(Value::Text("b@x2".into())));
    h.keys(" dr");
    assert_eq!(value(&h, 1, 1), None);
    h.keys(" dr");
    assert_eq!(said(&mut h), Some(Label::EditNothingToRevert.into()));
}

#[test]
fn an_empty_text_is_the_empty_string_and_ctrl_n_ctrl_d_set_null_and_default() {
    let mut h = ready(Lang::En);
    // The nickname: nullable, no default.
    h.keys("le");
    clear(&mut h);
    h.key(KeyCode::Enter);
    assert_eq!(value(&h, 0, 2), Some(Value::Text(String::new())), "never NULL");
    h.keys("e");
    assert_eq!(editor_text(&h), "");
    h.ctrl('n');
    assert_eq!(value(&h, 0, 2), Some(Value::Null));
    h.keys("e");
    h.ctrl('d');
    assert_eq!(said(&mut h), refused("nickname has no default"));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "refused: the editor stays");
    h.key(KeyCode::Esc);
    assert_eq!(value(&h, 0, 2), Some(Value::Null), "Esc changes nothing");
    // The email is NOT NULL.
    h.keys("he");
    h.ctrl('n');
    assert_eq!(said(&mut h), refused("email is NOT NULL"));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    assert_eq!(value(&h, 0, 1), None);
    // The actions without the editor.
    h.keys("l");
    h.keys(" dn");
    assert_eq!(value(&h, 0, 2), Some(Value::Null));
    h.keys("h");
    h.keys(" dn");
    assert_eq!(said(&mut h), refused("email is NOT NULL"));
    h.keys(" dD");
    assert_eq!(said(&mut h), refused("email has no default"));
}

#[test]
fn esc_cancels_and_a_paste_of_several_lines_is_refused() {
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("zz");
    h.event(Event::Paste("one\ntwo".into()));
    assert_eq!(said(&mut h), Some(Label::EditPasteLines.into()));
    assert_eq!(editor_text(&h), "a@xzz");
    h.event(Event::Paste("!".into()));
    assert_eq!(editor_text(&h), "a@xzz!");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn cells_selected_in_one_column_take_the_value_as_one_change() {
    let mut h = ready(Lang::En);
    h.keys("vjj");
    h.keys("e");
    assert_eq!(editor_text(&h), "c@x", "the cursor's value");
    clear(&mut h);
    h.type_text("same");
    h.key(KeyCode::Enter);
    for r in 0..3 {
        assert_eq!(value(&h, r, 1), Some(Value::Text("same".into())), "row {r}");
    }
    h.keys("u");
    assert!(edits(&h).batch.is_empty(), "one undo step");
    // Cells of two columns, or whole rows: refused (`v` first drops the selection there is).
    h.keys("v");
    h.keys("vl");
    h.keys("e");
    assert_eq!(said(&mut h), Some(Label::EditSeveralColumns.into()));
    h.keys("v");
    h.keys("V");
    h.keys(" dn");
    assert_eq!(said(&mut h), Some(Label::EditSeveralColumns.into()));
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn a_read_only_environment_refuses_before_anything_is_asked() {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    h.sent();
    h.keys("e");
    assert_eq!(said(&mut h), read_only("the connection's environment is read-only"));
    assert!(asked(&mut h).is_empty());
}

#[test]
fn an_environment_that_is_not_defined_refuses() {
    let mut h = users(Lang::En);
    h.app.profiles[0].policy = Some("nowhere".into());
    h.keys("e");
    assert!(matches!(said(&mut h), Some(Msg::EnvironmentNotFound { .. })));
    assert!(asked(&mut h).is_empty());
}

#[test]
fn a_server_that_takes_no_edits_refuses() {
    let mut h = users(Lang::En);
    h.db(DbEvent::Edit(None));
    h.keys("e");
    assert_eq!(said(&mut h), read_only("this connection type can't edit data"));
    assert!(asked(&mut h).is_empty());
}

#[test]
fn a_join_is_read_only() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let columns = vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("id", "int8", true, USERS, 1)];
    let sql = "SELECT o.id, u.id FROM shop.orders o JOIN shop.users u ON u.id = o.user_id";
    answer(&mut h, sql, columns, vec![vec![s("1"), s("1")]]);
    h.sent();
    h.keys("e");
    let Some(Msg::EditReadOnly { reason }) = said(&mut h) else { panic!("read-only") };
    assert!(reason.starts_with("it reads more than one table"), "{reason}");
    assert!(asked(&mut h).is_empty());
}

#[test]
fn computed_and_generated_columns_are_refused() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let columns = vec![
        meta_from("id", "int8", true, USERS, 1),
        meta("shout", "text", false, false),
        meta_from("name", "text", false, USERS, 3),
    ];
    answer(
        &mut h,
        "SELECT id, upper(email) AS shout, name FROM shop.users",
        columns,
        vec![vec![s("1"), s("A@X"), s("Ann")]],
    );
    let mut s = users_structure();
    s.columns[2].fill = ColumnFill::Stored("upper(email)".into());
    h.keys("le");
    structure(&mut h, Ok(s));
    assert_eq!(said(&mut h), refused("shout is computed in the query, not a column of the table"));
    h.keys("le");
    assert_eq!(said(&mut h), refused("name is generated by the database"));
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn a_structure_that_could_not_be_read_is_asked_again() {
    let mut h = users(Lang::En);
    h.keys("e");
    assert_eq!(asked(&mut h).len(), 1);
    structure(&mut h, Err(DbError::Server("permission denied".into())));
    let Some(Msg::EditReadOnly { reason }) = said(&mut h) else { panic!("read-only") };
    assert!(reason.starts_with("the table's details could not be read"), "{reason}");
    assert!(matches!(edits(&h).target, Target::ReadOnly { retry: true, .. }));
    h.keys("e");
    assert_eq!(asked(&mut h).len(), 1, "unknown is not absent: asked again");
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
}

#[test]
fn a_structure_for_another_attempt_or_result_makes_nothing() {
    let mut h = users(Lang::En);
    h.keys("e");
    assert_eq!(asked(&mut h).len(), 1);
    // The profile connected again meanwhile: the answer is not the waiting press's.
    let id = h.app.profiles[0].id;
    h.app.conns.next_generation(id);
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(matches!(edits(&h).target, Target::Unresolved), "it waits no more");
    h.keys("e");
    assert_eq!(asked(&mut h).len(), 1);
    // The query ran again: a new result, which waits for nothing.
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(h.app.tab().cell_edit.is_none());
    assert!(matches!(edits(&h).target, Target::Unresolved));
}

#[test]
fn a_parked_result_keeps_its_own_changes() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT id, email, nickname FROM shop.users".into(), "SELECT 2 AS b".into()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::StepRows { id, index: 0, columns: Some(users_columns()), rows: users_rows(), more: false });
    let columns = Some(vec![meta("b", "int4", true, false)]);
    h.db(DbEvent::Page { id, columns, rows: vec![vec![s("2")]], more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    // The first statement's result.
    h.keys("H");
    assert_eq!(h.app.tab().exec.shown, Some(0));
    h.keys("le");
    structure(&mut h, Ok(users_structure()));
    h.type_text("!");
    h.key(KeyCode::Enter);
    assert_eq!(value(&h, 0, 1), Some(Value::Text("a@x!".into())));
    // The second one has none; back on the first, its change is there.
    h.keys("L");
    assert_eq!(h.app.tab().exec.shown, Some(1));
    assert!(edits(&h).batch.is_empty());
    h.keys("H");
    assert_eq!(value(&h, 0, 1), Some(Value::Text("a@x!".into())));
}

/// `shop.users` with the nickname an enum and the profile a boolean that defaults to `true`.
fn enum_and_bool(lang: Lang) -> (Harness, TableStructure) {
    let mut h = Harness::connected(lang);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let mut active = meta_from("profile", "bool", false, USERS, 5);
    active.kind = ValueKind::Bool;
    let columns = vec![meta_from("id", "int8", true, USERS, 1), meta_from("nickname", "mood", false, USERS, 4), active];
    answer(&mut h, "SELECT id, nickname, profile FROM shop.users", columns, vec![vec![s("1"), s("ok"), s("true")]]);
    h.sent();
    let mut st = users_structure();
    st.columns[3].type_name = "mood".into();
    st.columns[3].enum_labels = vec!["sad".into(), "ok".into(), "happy".into()];
    st.columns[4].type_name = "boolean".into();
    st.columns[4].default = Some("true".into());
    (h, st)
}

fn picker_labels(h: &Harness) -> Vec<String> {
    use datarig_tui::app::picker::Picker;
    match h.app.overlays.picker() {
        Some(Picker::CellValue { list, .. }) => list.entries().iter().map(|e| e.label.clone()).collect(),
        _ => panic!("the list of the cell's values"),
    }
}

#[test]
fn an_enum_and_a_boolean_are_picked_from_their_values() {
    let (mut h, st) = enum_and_bool(Lang::En);
    h.keys("le");
    structure(&mut h, Ok(st));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
    assert_eq!(picker_labels(&h), ["sad", "ok", "happy", "NULL"], "in the server's order; nullable, no default");
    // The value as read is selected: the next one down is `happy`.
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(value(&h, 0, 1), Some(Value::Text("happy".into())));
    h.keys("le");
    assert_eq!(picker_labels(&h), ["true", "false", "NULL", "DEFAULT"]);
    for _ in 0..3 {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Enter);
    assert_eq!(value(&h, 0, 2), Some(Value::Default));
    h.keys("e");
    h.key(KeyCode::Esc);
    assert_eq!(value(&h, 0, 2), Some(Value::Default), "Esc picks nothing");
    let screen = h.screen(120, 40);
    assert!(screen.contains("default"), "{screen}");
}

#[test]
fn the_list_of_values_closes_when_its_result_goes() {
    let (mut h, st) = enum_and_bool(Lang::En);
    h.keys("le");
    structure(&mut h, Ok(st));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
    answer(&mut h, "SELECT 1 AS a", vec![meta("a", "int4", true, false)], vec![vec![s("1")]]);
    assert_eq!(h.overlay_kind(), None);
}

#[test]
fn the_editor_closes_when_its_result_goes() {
    let mut h = ready(Lang::En);
    h.keys("e");
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(h.app.tab().cell_edit.is_none());
    assert_eq!(h.app.key_context(), Ctx::Grid);
}

#[test]
fn a_value_not_of_the_columns_type_keeps_the_editor_open() {
    let mut h = ready(Lang::En);
    // The id is a bigint.
    h.keys("he");
    clear(&mut h);
    h.type_text("12a");
    h.key(KeyCode::Enter);
    assert_eq!(said(&mut h), refused("not a whole number"));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "the text stays to be fixed");
    assert_eq!(editor_text(&h), "12a");
    assert_eq!(h.app.tab().cell_edit.as_ref().and_then(|e| e.error.clone()).as_deref(), Some("not a whole number"));
    assert!(edits(&h).batch.is_empty());
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(value(&h, 0, 0), Some(Value::Text("12".into())));
}

/// The screens of `setup` (built in each language) at 120x40 and 80x24, as `<name>_<lang>_<size>`:
/// English ones pinned, Korean ones checked to show their catalog's texts.
fn screens(name: &str, setup: impl Fn(Lang) -> Harness) {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = setup(lang);
        let tag = format!("{lang:?}").to_lowercase();
        for (w, hh) in [(120, 40), (80, 24)] {
            assert_screen!(format!("{name}_{tag}_{w}x{hh}"), lang, h.draw(w, hh));
        }
    }
}

/// The badge of a join (after an edit key said why), the status line's notice gone.
#[test]
fn the_badge_says_why_a_result_is_read_only() {
    screens("edit_badge_read_only", |lang| {
        let mut h = Harness::connected(lang);
        h.db(DbEvent::Keys(Ok(shop_keys())));
        let columns = vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("id", "int8", true, USERS, 1)];
        let sql = "SELECT o.id, u.id FROM shop.orders o JOIN shop.users u ON u.id = o.user_id";
        answer(&mut h, sql, columns, vec![vec![s("1"), s("1")]]);
        h.keys("e");
        h.app.transient = None;
        h
    });
}

#[test]
fn the_badge_names_the_table_and_how_its_rows_are_found() {
    screens("edit_badge_keyed", ready);
    // Wide enough for the key.
    insta::assert_snapshot!("edit_badge_keyed_en_160x45", ready(Lang::En).draw(160, 45).backend());
    screens("edit_badge_by_values", |lang| {
        let mut h = Harness::connected(lang);
        h.db(DbEvent::Keys(Ok(shop_keys())));
        let columns = vec![meta_from("nickname", "text", false, USERS, 4)];
        answer(&mut h, "SELECT nickname FROM shop.users", columns, vec![vec![s("ann")], vec![s("cy")]]);
        h.keys("e");
        structure(&mut h, Ok(users_structure()));
        h.key(KeyCode::Esc);
        h.app.transient = None;
        h
    });
}

/// A change pending: the badge counts it, the row has `~`, the cell shows the new value, the
/// hint line says how many and the undo key.
#[test]
fn a_pending_change_shows_in_the_badge_the_gutter_the_cell_and_the_hints() {
    screens("edit_pending", |lang| {
        let mut h = ready(lang);
        h.keys("e");
        h.type_text("y");
        h.key(KeyCode::Enter);
        h.keys("j");
        h.app.transient = None;
        h
    });
}

#[test]
fn the_editor_and_its_error_show_over_the_cell() {
    screens("edit_editor", |lang| {
        let mut h = ready(lang);
        h.keys("e");
        h.type_text("z");
        h
    });
    // The reason stays on the status line while the editor is open.
    screens("edit_editor_error", |lang| {
        let mut h = ready(lang);
        h.keys("he");
        clear(&mut h);
        h.type_text("12a");
        h.key(KeyCode::Enter);
        h.app.transient = None;
        h
    });
}

#[test]
fn the_list_of_an_enums_values_shows_over_the_grid() {
    screens("edit_enum_picker", |lang| {
        let (mut h, st) = enum_and_bool(lang);
        h.keys("le");
        structure(&mut h, Ok(st));
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
        h.app.transient = None;
        h
    });
}

/// In a dark and a light theme: a changed cell's text in `edit_changed` on its row's background
/// (on the cursor's background where the cursor is), the pending count in `edit_pending`.
#[test]
fn a_changed_cell_and_the_pending_count_take_their_tokens() {
    use datarig_tui::theme;
    use ratatui::style::Modifier;
    for th in [theme::DARK, theme::LIGHT] {
        let mut h = ready(Lang::En);
        h.app.theme = std::sync::Arc::new(th.clone());
        h.keys("e");
        h.type_text("y");
        h.key(KeyCode::Enter);
        h.app.transient = None;
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let g = &h.app.tab().grid;
        let cell = g.cell_rect(0, 1, 3).expect("on screen");
        let at = buf[(cell.x + 1, cell.y)].clone();
        assert_eq!((at.fg, at.bg), (th.edit_changed.fg.unwrap(), th.selection.bg.unwrap()), "under the cursor");
        assert!(at.modifier.contains(Modifier::BOLD));
        let other = g.cell_rect(0, 2, 3).expect("on screen");
        assert_eq!(buf[(other.x + 1, other.y)].fg, th.fg, "a cell not changed");
        // Off the cursor: the row's own background.
        h.keys("j");
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let at = buf[(cell.x + 1, cell.y)].clone();
        assert_eq!((at.fg, at.bg), (th.edit_changed.fg.unwrap(), th.bg));
        let screen = buffer_text(buf);
        let (y, line) = screen.lines().enumerate().find(|(_, l)| l.contains("1 pending")).expect("the badge");
        let x = line[..line.find("1 pending").unwrap()].chars().count() as u16;
        assert_eq!(buf[(x, y as u16)].fg, th.edit_pending.fg.unwrap(), "{line}");
    }
}

/// A MySQL profile, connected, the keys of `shop.users` read, its session reading in time zone
/// `zone` and character set `charset`; `shop.users`' id, email and a `TIMESTAMP` nickname in the
/// grid, one row.
fn mysql_users(zone: &str, charset: &str) -> Harness {
    mysql_users_with(zone, charset, ("timestamp", ValueKind::TimestampTz, "2024-10-27 02:30:00"))
}

/// As [`mysql_users`], the nickname of type `nickname` (its name, kind and the row's value).
fn mysql_users_with(zone: &str, charset: &str, nickname: (&str, ValueKind, &str)) -> Harness {
    let rows = vec![vec![s("1"), s("a@x"), s(nickname.2)]];
    mysql_users_rows(zone, charset, (nickname.0, nickname.1), rows)
}

/// As [`mysql_users_with`], the grid's rows `rows`.
fn mysql_users_rows(zone: &str, charset: &str, nickname: (&str, ValueKind), rows: Vec<Vec<Option<String>>>) -> Harness {
    use datarig_core::driver::{ColumnOrigin, SessionValues};
    let mut h = mysql_profile();
    h.db(DbEvent::SessionValues(SessionValues { time_zone: zone.into(), charset: charset.into() }));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "users".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = vec![
        col("id", "bigint", ValueKind::Integer),
        col("email", "varchar", ValueKind::Text),
        col("nickname", nickname.0, nickname.1),
    ];
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", columns, rows);
    h.sent();
    h
}

/// A MySQL profile, connected, the keys of `shop.users` read; its session said nothing yet.
fn mysql_profile() -> Harness {
    use datarig_core::driver::keys::{KeyCatalog, KeyKind, NameRule};
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
    h
}

/// `shop.users` as MySQL has it: the nickname a `TIMESTAMP`.
fn mysql_users_structure() -> TableStructure {
    let mut st = users_structure();
    st.columns[0].type_name = "bigint".into();
    st.columns[0].default = None;
    st.columns[0].fill = ColumnFill::AutoIncrement;
    st.columns[1].type_name = "varchar(100)".into();
    st.columns[3].type_name = "timestamp".into();
    st
}

#[test]
fn a_value_not_shown_whole_is_not_edited() {
    // A TIMESTAMP shown in a zone with daylight saving: none of the column's values.
    let mut h = mysql_users("Europe/Berlin", "utf8mb4");
    h.keys("lle");
    structure(&mut h, Ok(mysql_users_structure()));
    let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused") };
    assert!(reason.contains("nickname") && reason.contains("Europe/Berlin"), "{reason}");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty());
    // The id is shown whole.
    h.keys("hhe");
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    // Text read in latin1: not the text.
    let mut h = mysql_users("+00:00", "latin1");
    h.keys("le");
    structure(&mut h, Ok(mysql_users_structure()));
    let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused") };
    assert!(reason.contains("row 1") && reason.contains("email") && reason.contains("latin1"), "{reason}");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn a_result_read_in_a_session_that_said_its_values_as_it_connected_is_edited() {
    use datarig_core::driver::{ColumnOrigin, SessionValues};
    use datarig_tui::app::action::Action;
    use datarig_tui::app::copy::{CopyFormat, CopyScope};
    let col = |name: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "users".into(), column: name.into() };
        ColumnMeta::new(name.into(), "varchar".into(), kind, Some(origin))
    };
    let columns =
        || vec![col("id", ValueKind::Integer), col("email", ValueKind::Text), col("nickname", ValueKind::Text)];
    let values = |zone: &str| SessionValues { time_zone: zone.into(), charset: "utf8mb4".into() };
    let mut h = mysql_profile();
    // The tab's first run opens its session, which says its values as it connects, before the
    // statement runs.
    h.app.run(vec!["SELECT id, email, nickname FROM shop.users".to_string()]);
    h.db(DbEvent::SessionValues(values("SYSTEM")));
    let id = h.app.tab().exec.query_id;
    let rows = vec![vec![s("1"), s("a@x"), s("one")]];
    h.db(DbEvent::Page { id, columns: Some(columns()), rows: rows.clone(), more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.sent();
    // Copied as SQL too: its text is not refused.
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.app.transient = None;
    h.app.dispatch(Action::Copy(CopyScope::Fetched, CopyFormat::SqlInsert));
    let sql = clip.texts.lock().unwrap().last().cloned();
    assert!(sql.as_deref().is_some_and(|s| s.contains("'a@x'")), "{sql:?} {:?}", said(&mut h));
    h.keys("le");
    structure(&mut h, Ok(mysql_users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "{:?}", said(&mut h));
    h.key(KeyCode::Esc);
    // Values that change while a statement runs: its result is still refused.
    h.app.run(vec!["SELECT id, email, nickname FROM shop.users".to_string()]);
    h.db(DbEvent::SessionValues(values("+02:00")));
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns()), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.app.transient = None;
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 0));
    h.keys("le");
    structure(&mut h, Ok(mysql_users_structure()));
    let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused") };
    assert!(reason.contains("across a change"), "{reason}");
}

#[test]
fn only_the_answer_to_the_presss_own_request_resolves_it() {
    let mut h = users(Lang::En);
    // Asked for before the press (by the explorer, say): its answer may be of the table as it
    // was before an ALTER.
    let id = h.app.profiles[0].id;
    let cmd = DbCommand::LoadStructure { schema: "shop".into(), table: "users".into() };
    h.app.conns.get(id).and_then(|c| c.meta.as_ref()).expect("the metadata session").send(cmd);
    h.sent();
    h.keys("le");
    assert_eq!(asked(&mut h).len(), 1);
    let mut before = users_structure();
    before.columns[1].fill = ColumnFill::Stored("lower(name)".into());
    structure(&mut h, Ok(before));
    assert_eq!(h.app.key_context(), Ctx::Grid, "the older answer is not the press's");
    assert!(matches!(edits(&h).target, Target::Loading(_)), "it still waits");
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
}

#[test]
fn enter_writes_nothing_once_the_environment_no_longer_lets_it() {
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("y");
    h.app.profiles[0].policy = Some("nowhere".into());
    h.key(KeyCode::Enter);
    assert!(matches!(said(&mut h), Some(Msg::EnvironmentNotFound { .. })));
    assert!(edits(&h).batch.is_empty());
    let mut h = ready(Lang::En);
    h.keys("le");
    h.app.profiles[0].policy = Some("nowhere".into());
    h.ctrl('n');
    assert!(matches!(said(&mut h), Some(Msg::EnvironmentNotFound { .. })));
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn a_pick_writes_nothing_once_the_connection_changed() {
    let (mut h, st) = enum_and_bool(Lang::En);
    h.keys("le");
    structure(&mut h, Ok(st));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
    // The tab's session opened again meanwhile.
    h.app.tab_mut().exec.generation += 1;
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert_eq!(said(&mut h), Some(Label::EditConnectionChanged.into()));
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn rows_read_under_another_connection_are_not_edited() {
    let mut h = users(Lang::En);
    // The tab's session opened again after the rows were read.
    h.app.tab_mut().exec.generation += 1;
    h.keys("e");
    assert_eq!(said(&mut h), Some(Label::EditConnectionChanged.into()));
    assert!(asked(&mut h).is_empty());
}

#[test]
fn a_row_shorter_than_its_columns_is_not_edited() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), vec![vec![s("1")]]);
    h.keys("le");
    structure(&mut h, Ok(users_structure()));
    assert_eq!(said(&mut h), refused("row 1 could not be read"));
    assert_eq!(h.app.key_context(), Ctx::Grid);
}

#[test]
fn changes_pending_on_a_result_no_longer_resolved_say_why() {
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("y");
    h.key(KeyCode::Enter);
    h.app.transient = None;
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.edit.target = Target::Unresolved;
    }
    h.keys("e");
    assert!(matches!(said(&mut h), Some(Msg::EditReadOnly { .. })));
    assert!(asked(&mut h).is_empty());
}

#[test]
fn a_structure_asked_of_another_databases_lost_session_is_asked_again() {
    use datarig_core::driver::{SessionContext, SessionRole};
    use datarig_tui::app::{AppEvent, EventTarget};
    let aux = |h: &mut Harness, ev: DbEvent| {
        let p = h.app.profiles[0].id;
        let a = h.app.conns.aux(p, "sales").expect("an aux session").id;
        h.app.on_app_event(AppEvent::Db { target: EventTarget::Aux(a), generation: a, ev });
    };
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.tab_mut().context = SessionContext { database: Some("sales".into()), schema: None };
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    aux(&mut h, DbEvent::Connected);
    aux(&mut h, DbEvent::Keys(Ok(shop_keys())));
    h.sent();
    h.keys("le");
    assert_eq!(asked(&mut h).len(), 1);
    // Its session is lost before it answers: the answer never comes.
    aux(&mut h, DbEvent::Lost { error: DbError::Closed });
    h.app.transient = None;
    // Not waiting for an answer that never comes: asked again, on its session opened again.
    h.keys("e");
    let metas = h.roles().iter().filter(|r| **r == SessionRole::Meta).count();
    assert_eq!(metas, 3, "the database's session opened again");
    assert_eq!(asked(&mut h).len(), 1, "asked again");
    aux(
        &mut h,
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
}

#[test]
fn only_what_changes_how_typed_text_is_stored_refuses_an_edited_column() {
    // A MySQL FLOAT is shown rounded, but a value it holds as typed is stored as typed.
    let mut h = mysql_users_with("+00:00", "utf8mb4", ("float", ValueKind::Float, "3.14159"));
    let mut st = mysql_users_structure();
    st.columns[3].type_name = "float".into();
    h.keys("lle");
    structure(&mut h, Ok(st));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "{:?}", said(&mut h));
    clear(&mut h);
    h.type_text("0.123456789012");
    h.key(KeyCode::Enter);
    assert!(matches!(said(&mut h), Some(Msg::EditRefused { .. })));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    clear(&mut h);
    h.type_text("1.5");
    h.key(KeyCode::Enter);
    assert_eq!(value(&h, 0, 2), Some(Value::Text("1.5".into())));
    h.keys(" dn");
    assert_eq!(value(&h, 0, 2), Some(Value::Null));
    // A TIMESTAMP shown in a zone with daylight saving: NULL reads no value, a typed one is
    // refused.
    let mut h = mysql_users("Europe/Berlin", "utf8mb4");
    h.keys("ll dn");
    structure(&mut h, Ok(mysql_users_structure()));
    assert_eq!(value(&h, 0, 2), Some(Value::Null), "{:?}", said(&mut h));
    h.keys("e");
    let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused") };
    assert!(reason.contains("Europe/Berlin"), "{reason}");
    assert_eq!(h.app.key_context(), Ctx::Grid);
}

#[test]
fn a_click_outside_the_editor_closes_it_and_does_what_it_does() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("z");
    h.draw(120, 40);
    let cell = h.app.tab().grid.cell_rect(2, 2, 3).expect("on screen");
    h.mouse(MouseEventKind::Down(MouseButton::Left), cell.x + 1, cell.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), cell.x + 1, cell.y);
    assert!(h.app.tab().cell_edit.is_none(), "the editor closed");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (2, 2), "the click moved the cursor");
    assert!(edits(&h).batch.is_empty(), "nothing changed");
}

#[test]
fn the_pending_count_shows_in_korean_at_80x24() {
    let mut h = ready(Lang::Ko);
    h.keys("e");
    h.type_text("y");
    h.key(KeyCode::Enter);
    h.app.transient = None;
    let badge = h.app.i18n.msg(&Msg::EditBadgePending { count: 1 }).to_string();
    let screen = h.screen(80, 24);
    let lines: Vec<&str> = screen.lines().collect();
    let strip = lines[..lines.len() - 1].iter().any(|l| l.contains(&badge));
    assert!(strip, "{screen}");
}

#[test]
fn the_editor_has_the_cells_pad_and_the_prompts_look() {
    use datarig_tui::theme;
    let mut h = ready(Lang::En);
    let th = theme::DARK;
    h.app.theme = std::sync::Arc::new(th.clone());
    h.keys("e");
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let cell = h.app.tab().grid.cell_rect(0, 1, 3).expect("on screen");
    assert_eq!(buf[(cell.x, cell.y)].symbol(), " ", "a column of pad");
    assert_eq!(buf[(cell.x + 1, cell.y)].symbol(), "a");
    assert_eq!(buf[(cell.x + 1, cell.y)].bg, th.surface);
    assert_ne!(Some(buf[(cell.x + 1, cell.y)].bg), th.selection.bg, "not a selected cell's look");
}

#[test]
fn the_editors_keys_show_on_the_hint_line_and_the_loading_notice_goes() {
    let mut h = users(Lang::En);
    h.keys("le");
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    assert_eq!(said(&mut h), None, "the details came: no longer loading");
    let line = h.status(120, 40);
    for hint in ["Enter keep", "Esc cancel", "Ctrl+N NULL", "Ctrl+D DEFAULT"] {
        assert!(line.contains(hint), "{hint}: {line}");
    }
}

#[test]
fn a_refusal_names_the_columns_own_type() {
    let mut h = mysql_users_with("+00:00", "utf8mb4", ("decimal", ValueKind::Decimal, "1.50"));
    let mut st = mysql_users_structure();
    st.columns[3].type_name = "decimal(14,2)".into();
    h.keys("lle");
    structure(&mut h, Ok(st));
    clear(&mut h);
    h.type_text("1.234");
    h.key(KeyCode::Enter);
    let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused") };
    assert!(reason.contains("decimal(14,2)") && !reason.contains("numeric"), "{reason}");
}

#[test]
fn a_default_that_takes_a_new_number_is_refused_on_a_row() {
    // The id's default calls its sequence.
    let mut h = ready(Lang::En);
    h.keys("h dD");
    assert_eq!(said(&mut h), refused("id would take a new number; type a value instead"));
    h.keys("e");
    h.ctrl('d');
    assert_eq!(said(&mut h), refused("id would take a new number; type a value instead"));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "refused: the editor stays");
    h.key(KeyCode::Esc);
    assert!(edits(&h).batch.is_empty());
    // MySQL's AUTO_INCREMENT.
    let mut h = mysql_users("+00:00", "utf8mb4");
    h.keys(" dD");
    structure(&mut h, Ok(mysql_users_structure()));
    assert_eq!(said(&mut h), refused("id would take a new number; type a value instead"));
    assert!(edits(&h).batch.is_empty());
    // An identity, in the list of a boolean's values: no DEFAULT.
    let (mut h, mut st) = enum_and_bool(Lang::En);
    st.columns[4].fill = ColumnFill::IdentityByDefault;
    h.keys("lle");
    structure(&mut h, Ok(st));
    assert_eq!(picker_labels(&h), ["true", "false", "NULL"]);
}

#[test]
fn a_value_typed_for_several_cells_shown_rounded_is_refused() {
    let rows = vec![vec![s("1"), s("a@x"), s("3.14159")], vec![s("2"), s("b@x"), s("2.5")]];
    let mut h = mysql_users_rows("+00:00", "utf8mb4", ("float", ValueKind::Float), rows);
    let mut st = mysql_users_structure();
    st.columns[3].type_name = "float".into();
    h.keys("lle");
    structure(&mut h, Ok(st));
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "one cell: the editor opens");
    h.key(KeyCode::Esc);
    h.app.transient = None;
    // Two cells: one shown 3.14159 may hold 3.1415927, so the same text would not reach it.
    h.keys("vje");
    assert_eq!(said(&mut h), refused("nickname is shown rounded; edit its cells one at a time"));
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty());
    // NULL reads no value: taken for both.
    h.keys(" dn");
    assert_eq!((value(&h, 0, 2), value(&h, 1, 2)), (Some(Value::Null), Some(Value::Null)));
}

#[test]
fn a_null_text_read_in_another_charset_takes_no_typed_text() {
    let rows = vec![vec![s("1"), None, s("2024-01-01 00:00:00")]];
    let mut h = mysql_users_rows("+00:00", "latin1", ("timestamp", ValueKind::TimestampTz), rows);
    h.keys("le");
    structure(&mut h, Ok(mysql_users_structure()));
    let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused") };
    assert!(reason.contains("email") && reason.contains("latin1"), "{reason}");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn a_press_inside_the_editor_stays_in_the_editor() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("z");
    h.draw(120, 40);
    let cell = h.app.tab().grid.cell_rect(0, 1, 3).expect("on screen");
    let below = h.app.tab().grid.cell_rect(2, 1, 3).expect("on screen");
    h.mouse(MouseEventKind::Down(MouseButton::Left), cell.x + 1, cell.y);
    assert_eq!(h.app.tab().cell_edit.as_ref().map(|e| e.input.cursor()), Some(0), "the text cursor moved");
    h.mouse(MouseEventKind::Drag(MouseButton::Left), below.x + 1, below.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), below.x + 1, below.y);
    let g = &h.app.tab().grid;
    assert_eq!((g.row, g.col, g.anchor), (0, 1, None), "the grid did not move");
    h.mouse(MouseEventKind::Down(MouseButton::Left), cell.x + 2, cell.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), cell.x + 2, cell.y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), cell.x + 2, cell.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), cell.x + 2, cell.y);
    assert_eq!(h.overlay_kind(), None, "no cell viewer over the editor");
    assert_eq!(editor_text(&h), "a@xz");
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
}

#[test]
fn the_wheel_closes_the_editor_and_scrolls() {
    use ratatui::crossterm::event::MouseEventKind;
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("z");
    h.draw(120, 40);
    let cell = h.app.tab().grid.cell_rect(0, 1, 3).expect("on screen");
    h.mouse(MouseEventKind::ScrollDown, cell.x + 1, cell.y);
    assert!(h.app.tab().cell_edit.is_none(), "the editor closed");
    assert!(edits(&h).batch.is_empty(), "nothing changed");
}

/// The edits the action menu of a cell can offer, as it labels them.
const EDIT_CELL: &str = "Edit the cell (the selected cells of one column)";
const SET_NULL: &str = "Set the cell (the selected cells) to NULL";
const UNDO: &str = "Undo the last change of the result's cells";
const PREVIEW: &str = "Show the pending changes and the statements that apply them";
const DISCARD: &str = "Drop every pending change of the result";
const SET_DEFAULT: &str = "Set the cell (the selected cells) to the column's default";
const ADD_ROW: &str = "Add a new row, shown after the rows read";
const CLONE_ROW: &str = "Add a new row with the values of the cursor's row";
const DELETE_ROW: &str = "Mark the row (the selected rows) to be deleted, or unmark it; a new row is removed";
const EDITS: [&str; 9] = [EDIT_CELL, SET_NULL, SET_DEFAULT, ADD_ROW, CLONE_ROW, DELETE_ROW, UNDO, PREVIEW, DISCARD];

/// The open menu's lines as shown: headings as `# text`, items as `label | keys`.
fn menu_lines(h: &Harness) -> Vec<String> {
    h.app
        .menu_lines()
        .into_iter()
        .map(|(heading, label, keys)| if heading { format!("# {label}") } else { format!("{label} | {keys}") })
        .collect()
}

/// The edits the open menu offers.
fn menu_edits(h: &Harness) -> Vec<String> {
    h.menu_labels().into_iter().filter(|l| EDITS.contains(&l.as_str())).collect()
}

/// Right click on the grid's cell at `row`, `col` (of the 3 rows), drawn at 120x40.
fn right_click_cell(h: &mut Harness, row: usize, col: usize) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    h.draw(120, 40);
    let cell = h.app.tab().grid.cell_rect(row, col, 3).expect("on screen");
    h.mouse(MouseEventKind::Down(MouseButton::Right), cell.x + 1, cell.y);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
}

#[test]
fn the_menu_of_a_cell_offers_its_edits_and_edit_cell_edits_the_cell_clicked() {
    use datarig_tui::app::edit::RowAt;
    let mut h = users(Lang::En);
    right_click_cell(&mut h, 1, 1);
    let g = &h.app.tab().grid;
    assert_eq!((g.row, g.col), (1, 1), "the clicked cell is selected first");
    let shown = menu_lines(&h);
    assert_eq!(shown[0], "# Cell email, row 2", "{shown:?}");
    for item in [
        "Edit the cell (the selected cells of one column) | e",
        "Set the cell (the selected cells) to NULL | Space d n",
        "Add a new row, shown after the rows read | o",
        "Add a new row with the values of the cursor's row | O",
        "Mark the row (the selected rows) to be deleted, or unmark it; a new row is removed | D",
    ] {
        assert!(shown.iter().any(|l| l == item), "{item}: {shown:?}");
    }
    // Nothing pending: nothing to undo, preview or drop.
    let offered = menu_edits(&h);
    assert!(![UNDO, PREVIEW, DISCARD].iter().any(|l| offered.iter().any(|o| o == l)), "{offered:?}");
    insta::assert_snapshot!("edit_menu_editable_en_120x40", h.draw(120, 40).backend());
    insta::assert_snapshot!("edit_menu_editable_en_80x24", h.draw(80, 24).backend());
    // Space Space next to the selection: the same menu.
    h.key(KeyCode::Esc);
    h.keys("  ");
    assert_eq!(menu_lines(&h), shown, "one menu for the mouse and the keyboard");
    h.key(KeyCode::Esc);
    // Edit cell runs as `e` does: the structure is read first, then that cell is edited.
    right_click_cell(&mut h, 1, 1);
    h.menu_pick(EDIT_CELL);
    assert_eq!(asked(&mut h), vec![("shop".to_string(), "users".to_string())]);
    structure(&mut h, Ok(users_structure()));
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    assert_eq!(h.app.tab().cell_edit.as_ref().map(|e| e.at), Some((RowAt::Read(1), 1)));
    assert_eq!(editor_text(&h), "b@x");
    insta::assert_snapshot!("edit_menu_cell_edit_en_120x40", h.draw(120, 40).backend());
}

#[test]
fn set_null_and_the_preview_from_the_menu() {
    let mut h = ready(Lang::En);
    right_click_cell(&mut h, 2, 2);
    h.menu_pick(SET_NULL);
    assert_eq!(value(&h, 2, 2), Some(Value::Null), "pending on the cell clicked");
    assert_eq!(h.overlay_kind(), None);
    // The table known: email takes no NULL; the changes pending can be undone, previewed, dropped.
    right_click_cell(&mut h, 0, 1);
    let offered = menu_edits(&h);
    assert!(!offered.iter().any(|l| l == SET_NULL), "a NOT NULL column: {offered:?}");
    for item in [EDIT_CELL, UNDO, PREVIEW, DISCARD] {
        assert!(offered.iter().any(|l| l == item), "{item}: {offered:?}");
    }
    insta::assert_snapshot!("edit_menu_pending_en_120x40", h.draw(120, 40).backend());
    h.menu_pick(PREVIEW);
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "the preview, as Space d p opens it");
    h.key(KeyCode::Esc);
    // Undo from the menu: as `u`.
    right_click_cell(&mut h, 0, 1);
    h.menu_pick(UNDO);
    assert!(edits(&h).batch.is_empty());
    right_click_cell(&mut h, 0, 1);
    assert!(!h.menu_labels().iter().any(|l| l == UNDO || l == PREVIEW), "nothing pending again");
}

#[test]
fn a_result_that_cannot_be_edited_offers_no_edits() {
    // A join.
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let columns = vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("id", "int8", true, USERS, 1)];
    let sql = "SELECT o.id, u.id FROM shop.orders o JOIN shop.users u ON u.id = o.user_id";
    answer(&mut h, sql, columns, vec![vec![s("1"), s("1")]]);
    h.draw(120, 40);
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    assert_eq!(menu_edits(&h), Vec::<String>::new());
    assert!(h.menu_labels().iter().any(|l| l == "Copy the cell (a selected range as TSV)"), "the rest stays");
    insta::assert_snapshot!("edit_menu_read_only_en_120x40", h.draw(120, 40).backend());
    // A read-only environment.
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    right_click_cell(&mut h, 0, 1);
    assert_eq!(menu_edits(&h), Vec::<String>::new(), "read-only environment");
    // A server that takes no edits.
    let mut h = users(Lang::En);
    h.db(DbEvent::Edit(None));
    right_click_cell(&mut h, 0, 1);
    assert_eq!(menu_edits(&h), Vec::<String>::new(), "no edits on this server");
    // The connection lost.
    let mut h = users(Lang::En);
    h.db(DbEvent::Lost { error: DbError::Closed });
    right_click_cell(&mut h, 0, 1);
    assert_eq!(menu_edits(&h), Vec::<String>::new(), "not connected");
}

#[test]
fn a_selection_offers_only_the_edits_it_takes() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = ready(Lang::En);
    // Space Space on cells of two columns: the selection's menu, no edit of cells.
    h.keys("ggvjl");
    h.draw(120, 40);
    h.keys("  ");
    assert_eq!(menu_lines(&h)[0], "# Selection");
    let offered = menu_edits(&h);
    assert_eq!(offered, vec![ADD_ROW, CLONE_ROW, DELETE_ROW], "several columns: {offered:?}");
    h.key(KeyCode::Esc);
    h.keys("e");
    assert_eq!(said(&mut h), Some(Label::EditSeveralColumns.into()), "as the key says");
    // A right click inside the range keeps it: the same menu.
    let cell = h.app.tab().grid.cell_rect(1, 1, 3).expect("on screen");
    h.mouse(MouseEventKind::Down(MouseButton::Right), cell.x + 1, cell.y);
    assert_eq!(menu_lines(&h)[0], "# Selection");
    assert_eq!(menu_edits(&h), offered);
    h.key(KeyCode::Esc);
    // Cells of one column, nickname: what it takes (NULL, no default).
    h.key(KeyCode::Esc);
    h.keys("ggvj");
    assert_eq!(h.app.tab().grid.col, 2);
    h.keys("  ");
    assert_eq!(menu_edits(&h), vec![EDIT_CELL, SET_NULL, ADD_ROW, CLONE_ROW, DELETE_ROW]);
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    // A whole column: no row to delete.
    h.draw(120, 40);
    let g = h.app.tab().grid.clone();
    let (x, y) = (g.hit_cols[1].0 + 1, g.header_y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.keys("  ");
    assert!(!menu_edits(&h).iter().any(|l| l == DELETE_ROW), "{:?}", menu_edits(&h));
    h.key(KeyCode::Esc);
    h.keys("D");
    assert_eq!(said(&mut h), Some(Label::EditDeleteColumns.into()));
}

#[test]
fn a_row_marked_to_be_deleted_offers_no_change_of_its_cells() {
    let mut h = ready(Lang::En);
    h.keys("D");
    assert!(edits(&h).batch.deleted(0));
    right_click_cell(&mut h, 0, 2);
    let offered = menu_edits(&h);
    for item in [EDIT_CELL, SET_NULL, SET_DEFAULT] {
        assert!(!offered.iter().any(|l| l == item), "{item}: {offered:?}");
    }
    for item in [DELETE_ROW, CLONE_ROW, UNDO, PREVIEW, DISCARD] {
        assert!(offered.iter().any(|l| l == item), "{item}: {offered:?}");
    }
    // Unmarked from the menu, as `D` does.
    h.menu_pick(DELETE_ROW);
    assert!(!edits(&h).batch.deleted(0));
}

#[test]
fn a_new_row_offers_its_edits_and_nothing_that_reads_a_row() {
    use datarig_tui::app::edit::RowAt;
    let mut h = ready(Lang::En);
    h.keys("Go");
    assert_eq!(edits(&h).batch.drafts().len(), 1);
    let id = edits(&h).batch.drafts()[0].id;
    // The new row is the grid's 4th: its nickname.
    right_click_cell(&mut h, 3, 2);
    let g = &h.app.tab().grid;
    assert_eq!((g.row, g.col), (3, 2), "the new row's cell is selected");
    let shown = menu_lines(&h);
    assert_eq!(shown[0], "# Cell nickname, new row", "{shown:?}");
    assert_eq!(
        menu_edits(&h),
        vec![EDIT_CELL, SET_NULL, SET_DEFAULT, ADD_ROW, CLONE_ROW, DELETE_ROW, UNDO, PREVIEW, DISCARD]
    );
    let labels = h.menu_labels();
    for item in [
        "Results: view cell value",
        "Result detail → show / hide",
        "Copy the cell (a selected range as TSV)",
        "Copy the row (the selected rows) as TSV",
        "Copy selection ▸",
        "Copy all fetched rows ▸",
    ] {
        assert!(!labels.iter().any(|l| l == item), "{item}: {labels:?}");
    }
    insta::assert_snapshot!("edit_menu_new_row_en_120x40", h.draw(120, 40).backend());
    insta::assert_snapshot!("edit_menu_new_row_en_80x24", h.draw(80, 24).backend());
    h.key(KeyCode::Esc);
    // email takes no NULL, in a new row too.
    right_click_cell(&mut h, 3, 1);
    assert!(!menu_edits(&h).iter().any(|l| l == SET_NULL));
    h.key(KeyCode::Esc);
    // Set NULL, then Edit cell on that new row's cell, as the keys.
    right_click_cell(&mut h, 3, 2);
    h.menu_pick(SET_NULL);
    assert_eq!(edits(&h).batch.drafts()[0].cells[2], Value::Null);
    right_click_cell(&mut h, 3, 2);
    h.menu_pick(EDIT_CELL);
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    assert_eq!(h.app.tab().cell_edit.as_ref().map(|e| e.at), Some((RowAt::New(id), 2)));
    h.key(KeyCode::Esc);
    // Mark delete removes the new row.
    right_click_cell(&mut h, 3, 2);
    h.menu_pick(DELETE_ROW);
    assert!(edits(&h).batch.drafts().is_empty());
}
