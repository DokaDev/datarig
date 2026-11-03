//! The preview of a result's pending changes (`space d p` / `space d a`, `:changes` / `:apply`):
//! the statements that apply them, each with the count it must give; rows found by their values
//! counted first on the tab's session; why nothing can be applied yet; the transaction the Apply
//! runs in. `space d x` / `:discard` drops the changes once the user says so.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ValueKind};
use datarig_core::edit::Value;
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::edit::{EditState, PreviewLine};
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
