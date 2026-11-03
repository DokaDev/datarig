//! The preview of a result's pending changes (`space d p` / `space d a`, `:changes` / `:apply`):
//! the statements that apply them, each with the count it must give; rows found by their values
//! counted first on the tab's session; why nothing can be applied yet; the transaction the Apply
//! runs in. `space d x` / `:discard` drops the changes once the user says so.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{AppliedStep, ApplyTx, ColumnMeta, DbCommand, DbError, DbEvent, ValueKind};
use datarig_core::edit::{Expect, Step, Value};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::edit::{Applied, EditState, PreviewLine};
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

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

fn structure(h: &mut Harness, st: TableStructure) {
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
}

/// `h` connected, `sql`'s result in the grid, resolved with `st` (`e` on the first row's second
/// column, then the editor closed); nothing sent yet.
fn resolved(
    mut h: Harness,
    sql: &str,
    columns: Vec<ColumnMeta>,
    rows: Vec<Vec<Option<String>>>,
    st: TableStructure,
) -> Harness {
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, sql, columns, rows);
    h.keys("le");
    structure(&mut h, st);
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// `shop.users` whose `name` (NOT NULL, not in the results here) has a default: a new row may
/// leave it out.
fn named() -> TableStructure {
    let mut st = users_structure();
    st.columns[2].default = Some("''::text".into());
    st
}

/// The users' rows by their key.
fn ready(lang: Lang) -> Harness {
    resolved(
        Harness::connected(lang),
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        named(),
    )
}

/// `shop.users` without any key: its rows are found by their values. `four`: only its id,
/// email, name and nickname (every one shown and exact: identical rows are a choice).
fn keyless(four: bool) -> TableStructure {
    let mut st = named();
    st.primary_key = None;
    st.unique_constraints.clear();
    st.indexes.clear();
    if four {
        st.columns.truncate(4);
    }
    st
}

/// The users' id, email and nickname, found by their values (identical rows are refused).
fn by_values(lang: Lang) -> Harness {
    resolved(
        Harness::connected(lang),
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        keyless(false),
    )
}

/// Every column of a keyless `shop.users`: identical rows are a choice.
fn identical(lang: Lang) -> Harness {
    let columns = vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("name", "text", false, USERS, 3),
        meta_from("nickname", "text", false, USERS, 4),
    ];
    let rows = vec![vec![s("1"), s("a@x"), s("Ann"), s("ann")], vec![s("1"), s("a@x"), s("Ann"), s("ann")]];
    resolved(Harness::connected(lang), "SELECT id, email, name, nickname FROM shop.users", columns, rows, keyless(true))
}

fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
        _ => panic!("no row result"),
    }
}

fn said(h: &mut Harness) -> Option<Msg> {
    h.app.transient.take().map(|(n, _)| n.msg)
}

/// Type `v` into the cursor's cell.
fn type_cell(h: &mut Harness, v: &str) {
    h.keys("e");
    let n = h.app.tab().cell_edit.as_ref().map_or(0, |e| e.input.text().chars().count());
    for _ in 0..n {
        h.key(KeyCode::Backspace);
    }
    h.type_text(v);
    h.key(KeyCode::Enter);
}

fn lines(h: &Harness) -> Vec<PreviewLine> {
    h.app.edit_preview_view().expect("the preview is open").lines
}

fn step(sql: &str, n: u64, risky: bool) -> PreviewLine {
    PreviewLine::Step { sql: sql.into(), expect: format!("expect {n}"), risky }
}

fn head(text: &str) -> PreviewLine {
    PreviewLine::Head(text.into())
}

/// The counts the preview sent, with their ids.
fn counts(h: &mut Harness) -> Vec<(u64, String)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { id, sql } => Some((id, sql)),
            _ => None,
        })
        .collect()
}

const CHECK_1: &str = r#"SELECT 1 FROM "shop"."users" WHERE "id" = '1' AND "email" = 'a@x' FOR UPDATE NOWAIT"#;
const UPDATE_1: &str = r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE "id" = '1' AND "email" = 'a@x' RETURNING *"#;

#[test]
fn a_keyed_change_shows_its_check_and_its_write_and_cancel_is_the_default() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys(" dp");
    assert_eq!(h.app.key_context(), Ctx::EditPreview);
    assert!(counts(&mut h).is_empty(), "a row found by its key is not counted");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.title.to_string(), "Apply changes — shop.users");
    assert_eq!(v.summary, "1 update");
    assert_eq!(v.footer, "Runs in one transaction; commits at the end.");
    assert!(v.apply);
    assert_eq!(
        v.lines,
        [head("Row checks (lock, no waiting)"), step(CHECK_1, 1, false), head("Writes"), step(UPDATE_1, 1, false)]
    );
    // Enter cancels (Cancel has the focus), as Esc does; the changes stay.
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.keys(" da");
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "space d a opens the same dialog");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(edits(&h).batch.counts().total(), 1);
    for cmd in ["changes", "apply"] {
        h.command(cmd);
        assert_eq!(h.app.key_context(), Ctx::EditPreview, ":{cmd}");
        h.key(KeyCode::Char('q'));
    }
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::ApplyEdits { .. })), "nothing applied");
}

#[test]
fn deletes_and_new_rows_come_after_the_checks_in_the_risk_tone_for_deletes() {
    let mut h = ready(Lang::En);
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("changes");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.summary, "1 insert · 1 delete");
    assert_eq!(
        v.lines,
        [
            head("Row checks (lock, no waiting)"),
            step(
                r#"SELECT 1 FROM "shop"."users" WHERE "id" = '2' AND "email" = 'b@x' AND "nickname" IS NULL FOR UPDATE NOWAIT"#,
                1,
                false
            ),
            head("Writes"),
            step(r#"DELETE FROM "shop"."users" WHERE "id" = '2' AND "email" = 'b@x' AND "nickname" IS NULL"#, 1, true),
            step(r#"INSERT INTO "shop"."users" ("email") VALUES ('n@x') RETURNING *"#, 1, false),
        ]
    );
}

#[test]
fn nothing_pending_and_a_read_only_environment_are_refused_before_the_dialog() {
    let mut h = ready(Lang::En);
    h.keys(" dp");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    h.command("apply");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    assert!(h.app.overlays.top().is_none());
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    h.command("changes");
    let reason = "the connection's environment is read-only".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditReadOnly { reason }));
    assert!(h.app.overlays.top().is_none());
}

#[test]
fn rows_found_by_their_values_are_counted_on_the_tab_first() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    let asked = counts(&mut h);
    let condition = r#""id" = '1' AND "email" = 'a@x' AND "nickname" = 'ann'"#;
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, format!(r#"SELECT count(*) FROM "shop"."users" WHERE {condition}"#));
    assert_eq!(lines(&h), [PreviewLine::Note("Counting matching rows…".into())]);
    assert!(!h.app.edit_preview_view().unwrap().apply);
    h.key(KeyCode::Char('y'));
    let reason = "Counting matching rows…".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditPreviewCantApply { reason }));
    // Another count's answer is not the preview's.
    h.db(DbEvent::Counted { id: asked[0].0 + 100, result: Ok(5), snapshot: false });
    assert_eq!(lines(&h), [PreviewLine::Note("Counting matching rows…".into())]);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    assert_eq!(
        lines(&h),
        [
            head("Row checks (lock, no waiting)"),
            step(&format!(r#"SELECT 1 FROM "shop"."users" WHERE {condition} FOR UPDATE NOWAIT"#), 1, false),
            head("Writes"),
            step(&format!(r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE {condition} RETURNING *"#), 1, false),
        ]
    );
    assert!(h.app.edit_preview_view().unwrap().apply);
}

#[test]
fn a_row_gone_ambiguous_or_not_counted_is_said_and_nothing_can_be_applied() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    for (answer, want) in [
        (Ok(0), "Row 1 matches no row of the table now: it was changed or deleted. Re-run the query."),
        (Ok(2), "Row 1 matches 2 rows; add the key to the SELECT to edit it"),
        (Err(DbError::Server("canceling statement".into())), "Couldn't count the matching rows: canceling statement"),
    ] {
        h.command("changes");
        let asked = counts(&mut h);
        h.db(DbEvent::Counted { id: asked[0].0, result: answer, snapshot: false });
        let v = h.app.edit_preview_view().unwrap();
        assert!(!v.apply, "{want}");
        match &v.lines[..] {
            [PreviewLine::Problem(text)] => assert!(text.starts_with(want), "{text}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(h.app.edit_preview_buttons().len(), 2, "no Apply button");
        h.key(KeyCode::Char('y'));
        assert!(matches!(said(&mut h), Some(Msg::EditPreviewCantApply { .. })));
        h.key(KeyCode::Esc);
    }
}

#[test]
fn identical_rows_wait_for_one_or_all() {
    let mut h = identical(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    let asked = counts(&mut h);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(3), snapshot: false });
    assert_eq!(
        lines(&h),
        [PreviewLine::Choose {
            text: "Row 1 matches 3 identical rows".into(),
            keys: "[1] just one   [a] all 3".into()
        }]
    );
    h.key(KeyCode::Char('y'));
    assert!(matches!(said(&mut h), Some(Msg::EditPreviewCantApply { .. })));
    h.key(KeyCode::Char('a'));
    let all = lines(&h);
    assert!(matches!(&all[1], PreviewLine::Step { expect, risky: true, .. } if expect == "expect 3"), "{all:?}");
    assert!(matches!(&all[3], PreviewLine::Step { expect, risky: true, .. } if expect == "expect 3"), "{all:?}");
    h.key(KeyCode::Esc);
    // Asked again when opened again: the count and the answer.
    h.command("changes");
    let asked = counts(&mut h);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(3), snapshot: false });
    h.key(KeyCode::Char('1'));
    let one = lines(&h);
    assert!(matches!(&one[1], PreviewLine::Step { expect, risky: true, .. } if expect == "expect 3"), "{one:?}");
    match &one[3] {
        PreviewLine::Step { sql, expect, risky: false } => {
            assert_eq!(expect, "expect 1");
            assert!(sql.contains("ctid"), "{sql}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_new_row_leaving_a_not_null_column_without_a_default_is_listed_and_not_counted() {
    let mut h = by_values(Lang::En);
    h.keys("o");
    h.keys("jo");
    h.command("changes");
    assert!(counts(&mut h).is_empty(), "nothing is counted for changes that cannot be made");
    let want =
        |row: u32| PreviewLine::Problem(format!("Row {row}: email is NOT NULL and has no default; give it a value"));
    assert_eq!(lines(&h), [want(4), want(5)]);
    h.key(KeyCode::Char('y'));
    assert!(matches!(said(&mut h), Some(Msg::EditPreviewCantApply { .. })));
    h.key(KeyCode::Esc);
    h.keys("Gk");
    h.keys("l");
    type_cell(&mut h, "n@x");
    h.keys("j");
    type_cell(&mut h, "m@x");
    h.command("changes");
    let v = h.app.edit_preview_view().unwrap();
    assert!(v.apply);
    assert_eq!(v.summary, "2 inserts");
}

#[test]
fn c_copies_every_statement_through_the_clipboard_setting() {
    let mut h = ready(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    type_cell(&mut h, "z@x");
    h.command("changes");
    h.key(KeyCode::Char('c'));
    assert_eq!(clip.last(), Some(format!("{CHECK_1};\n{UPDATE_1};\n")));
    assert!(matches!(said(&mut h), Some(Msg::EditPreviewCopied { count: 2, .. })));
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "the dialog stays");
    // Nothing to copy while counting.
    let mut h = by_values(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    type_cell(&mut h, "z@x");
    h.command("changes");
    h.key(KeyCode::Char('c'));
    assert_eq!(said(&mut h), Some(Label::EditPreviewNothingToCopy.into()));
    assert_eq!(clip.last(), None);
}

#[test]
fn the_footer_says_how_the_apply_stands_to_a_transaction() {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("stg", Policy { commit: CommitRule::Manual, ..Policy::default() });
    let mut h = resolved(
        {
            let mut h = Harness::with_config(&cfg, Lang::En);
            h.db(DbEvent::Connected);
            h
        },
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        users_structure(),
    );
    type_cell(&mut h, "z@x");
    h.command("changes");
    assert_eq!(h.app.edit_preview_view().unwrap().footer, "Opens a transaction; nothing is committed.");
    h.key(KeyCode::Esc);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.command("changes");
    assert_eq!(h.app.edit_preview_view().unwrap().footer, "Runs inside the open transaction; nothing is committed.");
}

#[test]
fn the_preview_closes_when_its_result_is_replaced_or_its_session_lost() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(h.app.overlays.edit_preview().is_none());
    assert_eq!(said(&mut h), Some(Label::EditPreviewClosed.into()));
    // The tab's session lost while it counts: no answer comes there any more.
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    assert_eq!(counts(&mut h).len(), 1);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(h.app.overlays.edit_preview().is_none());
}

#[test]
fn the_hint_line_offers_apply_and_undo_while_changes_are_pending() {
    let mut h = ready(Lang::En);
    let has = |h: &Harness, label: &str| h.app.hints().iter().any(|(_, l)| &**l == label);
    assert!(!has(&h, "apply"));
    type_cell(&mut h, "z@x");
    let hints = h.app.hints();
    assert_eq!((hints[0].0.as_str(), hints[0].1.to_string().as_str()), ("Space d a", "apply"), "{hints:?}");
    assert_eq!(hints[1].1.to_string(), "undo");
}

#[test]
fn discard_asks_and_enter_keeps_the_changes() {
    let mut h = ready(Lang::En);
    h.keys(" dx");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys(" dx");
    let c = h.app.overlays.confirm().expect("asked");
    assert!(matches!(c.action, ConfirmAction::DiscardEdits { .. }));
    assert_eq!(c.text, Msg::EditDiscardText { count: 2 });
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.top().is_none());
    assert_eq!(edits(&h).batch.counts().total(), 2, "Enter keeps them");
    h.command("discard");
    assert_eq!(h.app.overlays.top().map(|o| o.kind()), Some(OverlayKind::Confirm));
    h.key(KeyCode::Char('y'));
    assert!(edits(&h).batch.is_empty());
    assert_eq!(said(&mut h), Some(Msg::EditDiscarded { count: 2 }));
    // One change: undone, they come back.
    h.keys("u");
    assert_eq!(edits(&h).batch.counts().total(), 2);
}

/// A MySQL profile (connected, the keys of `shop.products` read) whose tab's first run read
/// `shop.products` as its session said its values when it connected; resolved, nothing sent.
fn mysql_products() -> Harness {
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
    let names = ["id", "name", "price"];
    k.add_table(1, "shop", "products", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k.mark(1, &[1], KeyKind::Primary);
    h.db(DbEvent::Keys(Ok(k)));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "products".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = vec![
        col("id", "bigint", ValueKind::Integer),
        col("name", "varchar", ValueKind::Text),
        col("price", "decimal", ValueKind::Decimal),
    ];
    h.app.run(vec!["SELECT id, name, price FROM shop.products".to_string()]);
    h.db(DbEvent::SessionValues(SessionValues { time_zone: "SYSTEM".into(), charset: "utf8mb4".into() }));
    let id = h.app.tab().exec.query_id;
    let rows = vec![vec![s("1"), s("pen"), s("1.50")]];
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    let mut st = users_structure();
    st.columns.truncate(3);
    st.columns[0].type_name = "bigint".into();
    st.columns[0].default = None;
    st.columns[0].fill = ColumnFill::AutoIncrement;
    st.columns[1] = datarig_core::driver::structure::StructureColumn {
        name: "name".into(),
        type_name: "varchar(100)".into(),
        ..st.columns[1].clone()
    };
    st.columns[2] = datarig_core::driver::structure::StructureColumn {
        name: "price".into(),
        type_name: "decimal(12,2)".into(),
        not_null: true,
        default: None,
        ..st.columns[2].clone()
    };
    st.unique_constraints.clear();
    st.indexes.retain(|x| x.primary);
    h.keys("le");
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "products".into(), result: Ok(Box::new(st)) });
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "{:?}", said(&mut h));
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// QA saw a MySQL new row's `price` (DECIMAL) refused as read across a change of the session's
/// time zone or character set: a DECIMAL is under no such rule, and it is not refused.
#[test]
fn a_mysql_new_rows_decimal_is_typed_and_its_not_null_columns_must_be_given() {
    let mut h = mysql_products();
    h.keys("o");
    h.command("changes");
    let want = |c: &str| PreviewLine::Problem(format!("Row 2: {c} is NOT NULL and has no default; give it a value"));
    assert_eq!(lines(&h), [want("name"), want("price")]);
    h.key(KeyCode::Esc);
    h.keys("$");
    type_cell(&mut h, "9.99");
    assert_eq!(said(&mut h), None, "not refused");
    h.keys("h");
    type_cell(&mut h, "ink");
    let new = &edits(&h).batch.drafts()[0].cells;
    assert_eq!(new[1..], [Value::Text("ink".into()), Value::Text("9.99".into())]);
    h.command("changes");
    let v = h.app.edit_preview_view().unwrap();
    assert!(v.apply);
    assert_eq!(v.lines[1], step("INSERT INTO `shop`.`products` (`name`, `price`) VALUES ('ink', 9.99)", 1, false));
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

/// An update, a delete and a new row, previewed in `h`.
fn three_changes(mut h: Harness) -> Harness {
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("changes");
    h.app.transient = None;
    h
}

/// Connected to the test profile in an environment that commits by hand.
fn manual(lang: Lang) -> Harness {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("stg", Policy { commit: CommitRule::Manual, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, lang);
    h.db(DbEvent::Connected);
    resolved(h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), named())
}

#[test]
fn the_preview_on_screen() {
    screens("edit_preview_auto", |lang| three_changes(ready(lang)));
    screens("edit_preview_manual_none", |lang| three_changes(manual(lang)));
    screens("edit_preview_manual_open", |lang| {
        let mut h = manual(lang);
        h.tab_db(0, DbEvent::Block(true));
        h.tab_db(0, DbEvent::TxOpen(true));
        three_changes(h)
    });
    screens("edit_preview_choose", |lang| {
        let mut h = identical(lang);
        type_cell(&mut h, "z@x");
        h.command("changes");
        let asked = counts(&mut h);
        h.db(DbEvent::Counted { id: asked[0].0, result: Ok(3), snapshot: false });
        h
    });
    screens("edit_preview_counting", |lang| {
        let mut h = by_values(lang);
        type_cell(&mut h, "z@x");
        h.command("changes");
        h
    });
    screens("edit_preview_count_failed", |lang| {
        let mut h = by_values(lang);
        type_cell(&mut h, "z@x");
        h.command("changes");
        let asked = counts(&mut h);
        let result = Err(DbError::Server("canceling statement due to lock timeout".into()));
        h.db(DbEvent::Counted { id: asked[0].0, result, snapshot: false });
        h
    });
    screens("edit_preview_no_default", |lang| {
        let mut h = ready(lang);
        h.keys("o");
        h.command("changes");
        h
    });
}

/// A new row leaves `name` (NOT NULL, no default) to a default it lacks when the result does
/// not show it: the server would refuse the row, or (MySQL, not strict) store an empty name.
/// Listed, nothing counted, `y` refused, nothing sent.
#[test]
fn a_new_row_is_refused_while_the_table_has_a_required_column_the_result_does_not_show() {
    let mut h = resolved(
        Harness::connected(Lang::En),
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        users_structure(),
    );
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("changes");
    let want = "Row 4: name is NOT NULL, has no default and is not in the result; add it to the SELECT";
    assert_eq!(lines(&h), [PreviewLine::Problem(want.into())]);
    assert!(!h.app.edit_preview_view().unwrap().apply);
    h.key(KeyCode::Char('y'));
    assert_eq!(said(&mut h), Some(Msg::EditPreviewCantApply { reason: want.into() }));
    assert!(h.sent().is_empty(), "nothing counted, nothing applied");
    // Changes of the rows read leave the table's other columns as they are.
    h.key(KeyCode::Esc);
    h.keys("G");
    h.keys("D");
    h.keys("gg");
    type_cell(&mut h, "z@x");
    h.command("changes");
    assert!(h.app.edit_preview_view().unwrap().apply, "{:?}", lines(&h));
}

// The Apply: sent on `y`, answered by `DbEvent::Applied`.

/// The Apply sent since the last call: its id, its steps and its transaction.
fn applies(h: &mut Harness) -> (u64, Vec<Step>, ApplyTx) {
    let mut sent: Vec<(u64, Vec<Step>, ApplyTx)> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::ApplyEdits { id, steps, tx } => Some((id, steps, tx)),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "one Apply sent");
    sent.remove(0)
}

fn no_apply_sent(h: &mut Harness) -> bool {
    h.sent().iter().all(|c| !matches!(c, DbCommand::ApplyEdits { .. }))
}

/// Every step of `steps` stood: each gave its count, nothing returned.
fn stood(steps: &[Step]) -> Vec<AppliedStep> {
    let n = |s: &Step| match s.expect {
        Expect::Rows(n) | Expect::Affected(n) => n,
    };
    steps.iter().map(|s| AppliedStep { count: n(s), ..AppliedStep::default() }).collect()
}

/// The columns of `shop.users` as `RETURNING *` gives them.
type Returned = Option<(Vec<ColumnMeta>, Vec<Vec<Option<String>>>)>;

fn users_returned(row: [Option<&str>; 5]) -> Returned {
    let cols = ["id", "email", "name", "nickname", "profile"].map(|n| meta_from(n, "text", false, USERS, 1)).to_vec();
    Some((cols, vec![row.iter().map(|v| v.map(str::to_string)).collect()]))
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

fn applied(h: &Harness) -> &Applied {
    edits(h).applied.as_deref().expect("an applied layer")
}

fn screen(h: &mut Harness, w: u16, hh: u16) -> String {
    buffer_text(h.draw(w, hh).backend().buffer())
}

/// An update, a delete and a new row, applied (`y` pressed, nothing answered yet).
fn three_applying(mut h: Harness) -> (Harness, u64, Vec<Step>) {
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.app.transient = None;
    (h, id, steps)
}

/// The answer to `three_applying`'s Apply: every step stood, the update's and the insert's rows
/// returned (a trigger changed the nickname; the table numbered the new row).
fn three_done(steps: &[Step]) -> Vec<AppliedStep> {
    let mut done = stood(steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            d.returned = users_returned([Some("1"), Some("z@x"), Some("Ann"), Some("ann!"), Some("{}")]);
        } else if s.sql.starts_with("INSERT ") {
            d.returned = users_returned([Some("4"), Some("n@x"), Some(""), None, Some("{}")]);
        }
    }
    done
}

#[test]
fn y_sends_the_apply_on_the_tabs_session_and_edits_wait_for_its_answer() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (_, steps, tx) = applies(&mut h);
    assert_eq!(steps.iter().map(|s| s.sql.as_str()).collect::<Vec<_>>(), [CHECK_1, UPDATE_1]);
    assert_eq!(tx, ApplyTx::Commit);
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "the dialog stays");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.lines[0], PreviewLine::Note("Applying the changes…".into()));
    assert!(!v.apply);
    assert_eq!(h.app.edit_preview_buttons().len(), 2, "no Apply button");
    h.key(KeyCode::Char('y'));
    let reason = "Applying the changes…".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditPreviewCantApply { reason }));
    assert!(no_apply_sent(&mut h), "sent once");
    assert!(screen(&mut h, 120, 40).contains("Applying the changes…"), "the status bar says it");
    // Nothing changes the changes while they are applied.
    h.key(KeyCode::Esc);
    let refused = Some(Msg::EditRefused { reason: "the pending changes are being applied".into() });
    for keys in ["u", "D", "e", " dx", " da"] {
        h.keys(keys);
        assert_eq!(said(&mut h), refused, "{keys}");
        assert!(h.app.overlays.top().is_none(), "{keys}");
    }
    assert_eq!(edits(&h).batch.counts().total(), 1);
    assert!(screen(&mut h, 120, 40).contains("applying…"), "the strip says it");
}

/// The environment confirms every write: the dialog is that confirmation, nothing asks again.
#[test]
fn the_dialog_is_the_confirmation_of_an_environment_that_confirms_writes() {
    use datarig_core::policy::Confirm;
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("stg", Policy { confirm: Confirm::Writes, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    let mut h = resolved(h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), named());
    h.keys("D");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    applies(&mut h);
    assert_eq!(h.app.overlays.top().map(|o| o.kind()), Some(OverlayKind::EditPreview), "no second dialog");
}

#[test]
fn applied_changes_show_over_the_rows_and_the_changes_are_cleared() {
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    assert!(h.app.overlays.edit_preview().is_none(), "the dialog closed");
    assert_eq!(said(&mut h), Some(Msg::EditApplied { count: 3 }));
    let e = edits(&h);
    assert!(e.batch.is_empty() && !e.batch.can_undo(), "the changes and their history are cleared");
    assert!(e.applying.is_none());
    let a = applied(&h);
    // The row as the server returned it: the nickname a trigger set too.
    assert_eq!(a.set[&0], [(1, text("z@x")), (2, text("ann!"))].into());
    assert_eq!(a.deleted, [1].into());
    assert_eq!(a.new, [vec![text("4"), text("n@x"), Value::Null]], "the new row as stored");
    assert_eq!(a.tx, None);
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("applied · re-run to refresh"), "{s}");
    assert!(s.contains("ann!") && s.contains("n@x"), "{s}");
    assert!(h.app.tab().exec.running.is_none(), "the tab is free");
    // The rows are read again before they are edited again.
    h.keys("e");
    let reason = "changes were applied to these rows; re-run the query to edit them".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditRefused { reason }));
    // Running the query again shows the table as it is.
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(edits(&h).applied.is_none());
}

/// A MySQL insert returns no row: the number its `AUTO_INCREMENT` column took fills that column;
/// a column left to its default stays `default`.
#[test]
fn a_mysql_new_rows_number_comes_from_the_insert_id() {
    let mut h = mysql_products();
    h.keys("o");
    h.keys("l");
    type_cell(&mut h, "ink");
    h.keys("l");
    type_cell(&mut h, "9.99");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, tx) = applies(&mut h);
    assert_eq!(tx, ApplyTx::Commit);
    let mut done = stood(&steps);
    done.last_mut().unwrap().insert_id = Some(7);
    h.db(DbEvent::Applied { id, result: Ok(done) });
    assert_eq!(applied(&h).new, [vec![text("7"), text("ink"), text("9.99")]]);
}

#[test]
fn each_failure_says_why_and_the_changes_stay() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let update = r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE "id" = '1' AND "email" = 'a@x' RETURNING *"#;
    let cases: Vec<(Option<usize>, ApplyError, String)> = vec![
        (
            Some(0),
            ApplyError::Count { got: 0 },
            "Row 1 (id = 1) was changed or deleted since it was read. Nothing was applied. Re-run the query to see the current values.".into(),
        ),
        (Some(1), ApplyError::Count { got: 4 }, "Row 1 (id = 1) now matches 4 rows, not 1. Nothing was applied.".into()),
        (
            Some(0),
            ApplyError::Db(DbError::LockBusy("could not obtain lock".into())),
            "Row 1 (id = 1) is locked by another session. Nothing was applied.".into(),
        ),
        (
            Some(1),
            ApplyError::Db(DbError::Server("value too long".into())),
            format!("Row 1 (id = 1): value too long. Nothing was applied. Statement: {update}"),
        ),
        (None, ApplyError::Db(DbError::Server("could not serialize access".into())), "could not serialize access. Nothing was applied.".into()),
        (Some(0), ApplyError::Db(DbError::Cancelled), "Apply cancelled; nothing was applied.".into()),
        (
            None,
            ApplyError::Db(DbError::ReadWriteRefused),
            "not sent: the session is read-only and this statement asks for read-write. Nothing was applied.".into(),
        ),
    ];
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    for (step, why, want) in cases {
        h.command("apply");
        h.key(KeyCode::Char('y'));
        let (id, _, _) = applies(&mut h);
        h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step, why }) });
        assert_eq!(said(&mut h).map(|m| h.app.i18n.msg(&m).to_string()), Some(want.clone()));
        let v = h.app.edit_preview_view().expect("the dialog stays");
        assert_eq!(v.lines[0], PreviewLine::Problem(want.clone()));
        assert!(v.apply, "it may be applied again: {want}");
        assert_eq!(edits(&h).batch.counts().total(), 1, "the changes stay: {want}");
        assert!(edits(&h).applied.is_none() && edits(&h).applying.is_none());
        let notes = &h.app.tab().exec.run.notes;
        assert_eq!(notes.last().map(|n| h.app.i18n.msg(&n.msg).to_string()), Some(want), "in the Messages");
        h.key(KeyCode::Esc);
    }
}

/// The connection ended while committing: whether the changes are in the table is not known.
/// They are applied again only once the user says so again; a run reads the rows again.
#[test]
fn an_apply_whose_result_is_unknown_is_applied_again_only_when_asked() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, _, _) = applies(&mut h);
    let why = ApplyError::Db(DbError::CommitUnknown);
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: None, why }) });
    let unknown =
        "Apply result unknown — the connection was lost before the server confirmed. Re-run the query to check.";
    assert_eq!(said(&mut h).map(|m| h.app.i18n.msg(&m).to_string()).as_deref(), Some(unknown));
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.lines[0], PreviewLine::Problem(unknown.into()));
    assert!(!v.apply, "Apply is disabled");
    h.key(KeyCode::Esc);
    h.command("apply");
    assert_eq!(h.app.edit_preview_view().unwrap().lines[0], PreviewLine::Problem(unknown.into()), "still");
    h.key(KeyCode::Char('y'));
    assert!(no_apply_sent(&mut h), "warned first");
    let again = "The last Apply's result is unknown: its changes may be in the table already. y applies them again; Enter or Esc keeps them as they are.";
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.lines[1], PreviewLine::Problem(again.into()));
    assert!(v.apply);
    h.key(KeyCode::Enter);
    assert!(no_apply_sent(&mut h), "Enter keeps them as they are");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.command("apply");
    assert!(!h.app.edit_preview_view().unwrap().apply, "warned again");
    h.key(KeyCode::Char('y'));
    h.key(KeyCode::Char('y'));
    applies(&mut h);
    // A run reads the rows again: the new result has no such doubt.
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, _, _) = applies(&mut h);
    let why = ApplyError::Db(DbError::CommitUnknown);
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: None, why }) });
    h.key(KeyCode::Esc);
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(!edits(&h).apply_unknown);
}

/// An answer to another Apply is not this one's; an answer that comes after the session ended
/// is never read, and the Apply's result is not known.
#[test]
fn a_stale_answer_is_dropped() {
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id: id + 100, result: Ok(three_done(&steps)) });
    assert!(edits(&h).applying.is_some() && edits(&h).applied.is_none(), "still applying");
    assert_eq!(edits(&h).batch.counts().total(), 3);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(edits(&h).applying.is_none());
    assert!(edits(&h).apply_unknown);
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == Label::EditFailedUnknown.into()));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    assert!(edits(&h).applied.is_none(), "the lost session's answer is not read");
    assert_eq!(edits(&h).batch.counts().total(), 3, "the changes stay");
}

/// The query's cancel key cancels the Apply; nothing was applied.
#[test]
fn the_cancel_key_cancels_the_apply() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let (mut h, id, _) = three_applying(ready(Lang::En));
    h.ctrl('c');
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "the session was asked to cancel");
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: Some(2), why: ApplyError::Db(DbError::Cancelled) }) });
    assert_eq!(said(&mut h), Some(Label::EditFailedCancelled.into()));
    assert_eq!(edits(&h).batch.counts().total(), 3);
    assert!(h.app.tab().exec.running.is_none());
}

/// Under manual commit the Apply keeps its transaction open: the changes are not committed
/// until it ends. A commit keeps them; a rollback (the user's, the server's, a lost
/// connection) drops them, and says so.
#[test]
fn under_manual_commit_the_transactions_end_decides_the_applied_changes() {
    let open = |h: &mut Harness| {
        let (id, steps, tx) = applies(h);
        assert!(matches!(tx, ApplyTx::Keep { .. }), "{tx:?}");
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        assert_eq!(said(h), Some(Msg::EditAppliedKept { count: 3 }));
        // The transaction it opened follows its answer.
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        assert!(applied(h).tx.is_some());
        let s = screen(h, 120, 40);
        assert!(s.contains("applied · not committed"), "{s}");
    };
    let gone = "The transaction was rolled back; the applied changes are gone";
    // COMMIT.
    let (mut h, _, _) = {
        let mut h = manual(Lang::En);
        type_cell(&mut h, "z@x");
        h.keys("jD");
        h.keys("ol");
        type_cell(&mut h, "n@x");
        h.command("apply");
        h.key(KeyCode::Char('y'));
        (h, 0, ())
    };
    open(&mut h);
    h.command("commit");
    let end = h.sent().into_iter().find_map(|c| match c {
        DbCommand::EndTx { id, commit: true } => Some(id),
        _ => None,
    });
    let end = end.expect("a commit sent");
    h.tab_db(0, DbEvent::TxEnded { id: end, outcome: datarig_core::driver::TxOutcome::Committed });
    h.db(DbEvent::Block(false));
    h.db(DbEvent::TxOpen(false));
    assert_eq!(applied(&h).tx, None, "committed");
    assert!(screen(&mut h, 120, 40).contains("applied · re-run to refresh"));
    // ROLLBACK, the server's rollback, a lost connection.
    for how in ["rollback", "server", "lost"] {
        let mut h = manual(Lang::En);
        type_cell(&mut h, "z@x");
        h.command("apply");
        h.key(KeyCode::Char('y'));
        let (id, steps, _) = applies(&mut h);
        h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.app.transient = None;
        match how {
            "rollback" => {
                h.command("rollback");
                let end = h.sent().into_iter().find_map(|c| match c {
                    DbCommand::EndTx { id, commit: false } => Some(id),
                    _ => None,
                });
                let outcome = datarig_core::driver::TxOutcome::RolledBack;
                h.tab_db(0, DbEvent::TxEnded { id: end.expect("a rollback sent"), outcome });
                h.db(DbEvent::Block(false));
                h.db(DbEvent::TxOpen(false));
            }
            "server" => {
                h.tab_db(0, DbEvent::TxClosedBy { id, how: datarig_core::driver::ServerEnd::RolledBack });
                h.db(DbEvent::Block(false));
                h.db(DbEvent::TxOpen(false));
            }
            _ => h.tab_db(0, DbEvent::Lost { error: DbError::Closed }),
        }
        assert!(edits(&h).applied.is_none(), "{how}: the original values show again");
        let status = h.status(120, 40);
        assert!(status.contains(gone), "{how}: {status}");
        assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == Label::EditAppliedGone.into()), "{how}");
    }
}

/// Inside the user's open block the driver applies under a savepoint and commits nothing: the
/// changes are not committed until the block ends.
#[test]
fn inside_an_open_block_the_applied_changes_wait_for_its_end() {
    for rolled_back in [false, true] {
        let mut h = ready(Lang::En);
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        type_cell(&mut h, "z@x");
        h.command("apply");
        assert_eq!(
            h.app.edit_preview_view().unwrap().footer,
            "Runs inside the open transaction; nothing is committed."
        );
        h.key(KeyCode::Char('y'));
        let (id, steps, tx) = applies(&mut h);
        assert_eq!(tx, ApplyTx::Commit, "the driver uses a savepoint by itself");
        h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
        assert_eq!(said(&mut h), Some(Msg::EditAppliedKept { count: 1 }));
        assert!(applied(&h).tx.is_some());
        if rolled_back {
            h.tab_db(0, DbEvent::TxClosedBy { id, how: datarig_core::driver::ServerEnd::RolledBack });
        }
        h.db(DbEvent::Block(false));
        h.db(DbEvent::TxOpen(false));
        match rolled_back {
            true => assert!(edits(&h).applied.is_none()),
            false => assert_eq!(applied(&h).tx, None),
        }
    }
}

/// In a dark and a light theme: a value an Apply wrote in `edit_applied`, a row it deleted still
/// crossed out, a value the table filled in for a new row dim.
#[test]
fn applied_values_take_their_token() {
    use datarig_tui::theme;
    use ratatui::style::Modifier;
    for th in [theme::DARK, theme::LIGHT] {
        let (mut h, id, steps) = three_applying(ready(Lang::En));
        h.app.theme = std::sync::Arc::new(th.clone());
        let mut done = three_done(&steps);
        // The insert returned nothing: what the table filled in is not known.
        if let Some(d) = done.iter_mut().zip(&steps).find(|(_, s)| s.sql.starts_with("INSERT ")).map(|(d, _)| d) {
            d.returned = None;
        }
        h.db(DbEvent::Applied { id, result: Ok(done) });
        h.keys("G");
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let g = &h.app.tab().grid;
        let at = |r: usize, c: usize| {
            let x = g.cell_rect(r, c, 3).expect("on screen");
            buf[(x.x + 1, x.y)].clone()
        };
        let written = at(0, 1);
        assert_eq!((written.fg, written.bg), (th.edit_applied.fg.unwrap(), th.bg), "a value written");
        assert!(written.modifier.contains(Modifier::ITALIC) && !written.modifier.contains(Modifier::BOLD));
        let gone = at(1, 1);
        assert_eq!(gone.fg, th.edit_deleted.fg.unwrap(), "a row deleted");
        assert!(gone.modifier.contains(Modifier::CROSSED_OUT));
        let added = at(3, 1);
        assert_eq!(added.fg, th.edit_applied.fg.unwrap(), "a new row's value");
        let filled = at(3, 0);
        assert!(filled.fg == th.fg_muted && filled.modifier.contains(Modifier::DIM), "filled by the table: {filled:?}");
        assert_eq!(at(2, 1).fg, th.fg, "a row as read");
    }
}

#[test]
fn the_apply_on_screen() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    screens("edit_apply_applying", |lang| {
        let mut h = three_applying(ready(lang)).0;
        // The status bar's spinner by the wall clock: its first frame.
        let started = std::time::Instant::now() + Duration::from_secs(3600);
        h.app.tab_mut().exec.running.as_mut().expect("applying").started = started;
        h
    });
    screens("edit_apply_applied", |lang| {
        let (mut h, id, steps) = three_applying(ready(lang));
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        h.app.transient = None;
        h
    });
    screens("edit_apply_not_committed", |lang| {
        let (mut h, id, steps) = three_applying(manual(lang));
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.app.transient = None;
        h
    });
    screens("edit_apply_failed", |lang| {
        let (mut h, id, _) = three_applying(ready(lang));
        let why = ApplyError::Count { got: 0 };
        h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: Some(0), why }) });
        h.app.transient = None;
        h
    });
}
