//! The explorer's `/` filter beyond the profiles: on a connected profile it finds the
//! databases, schemas, tables and views and the items of a table's structure the explorer has
//! already read, and shows them with the way to them opened, without asking the server for
//! anything and without changing what is open (`Esc` gives the tree back as it was).

mod common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::structure::{ColumnFill, KeyConstraint, RelationKind, StructureColumn, TableStructure};
use datarig_core::driver::{DbCommand, DbEvent, SchemaObjects};
use datarig_core::i18n::Lang;
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::Focus;
use datarig_tui::app::explorer::RowKind;
use ratatui::crossterm::event::KeyCode;

/// `users` as the server describes it: two columns and its primary key.
fn users_structure() -> TableStructure {
    let col = |name: &str| StructureColumn {
        name: name.into(),
        type_name: "text".into(),
        not_null: true,
        default: None,
        fill: ColumnFill::Default,
    };
    let mut st = TableStructure::new(RelationKind::Table);
    st.columns = vec![col("id"), col("email")];
    st.primary_key = Some(KeyConstraint {
        name: "users_pkey".into(),
        columns: vec!["id".into()],
        definition: "PRIMARY KEY (id)".into(),
    });
    st
}

/// Connected `local-pg` (and a profile `cold` that is not), the explorer focused: `shop`'s
/// objects listed and the structure of `users` read, then both closed again; `analytics` and
/// `public` never opened; the server has a second database, `sales_dw`.
fn searchable() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.icons = IconsSetting::Off;
    h.app.profiles.push(ConnectionConfig { name: "cold".into(), port: 6543, ..ConnectionConfig::test_db() });
    h.db(DbEvent::Databases(Ok(vec!["datarig".into(), "sales_dw".into()])));
    h.key(KeyCode::BackTab);
    to(&mut h, "shop");
    h.key(KeyCode::Char('l'));
    let objects = SchemaObjects {
        tables: vec!["order_items".into(), "orders".into(), "users".into()],
        views: vec!["order_summary".into()],
        ..Default::default()
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    to(&mut h, "users");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) });
    h.key(KeyCode::Char('h'));
    to(&mut h, "shop");
    h.key(KeyCode::Char('h'));
    h.sent();
    h
}

/// Put the explorer's cursor on the row the harness shows as `name`.
fn to(h: &mut Harness, name: &str) {
    let i = h.rows().iter().position(|r| r.trim() == name).unwrap_or_else(|| panic!("no {name} in {:?}", h.rows()));
    let rows = h.app.explorer_rows();
    h.app.explorer.select(&rows, i);
}

/// Type `q` in the `/` filter.
fn filter(h: &mut Harness, q: &str) {
    h.app.focus = Focus::Tree;
    h.keys("/");
    h.type_text(q);
}

/// The explorer's lines, whole, trimmed at the end.
fn lines(h: &Harness) -> Vec<String> {
    h.app.explorer_rows().iter().map(|r| h.app.explorer_line_text(r).trim_end().to_string()).collect()
}

/// A table's name finds it under its closed schema: the profile, its database, the schema and
/// the group are shown open (their arrows too) with only the way to it; the cursor goes to it.
/// Nothing is asked for, and `Esc` gives the tree back exactly as it was.
#[test]
fn the_filter_finds_a_loaded_table_and_opens_the_way_to_it() {
    let mut h = searchable();
    let before = lines(&h);
    let shop = before.iter().find(|l| l.contains("shop")).unwrap().clone();
    assert!(shop.contains("▸ shop"), "closed: {shop}");
    filter(&mut h, "ITEMS");
    assert_eq!(h.rows(), ["+", "local-pg", "  db:datarig*", "    shop", "      Tables", "        order_items"]);
    let shown = lines(&h);
    assert!(shown[1].starts_with("▾ ●") && shown[1].ends_with("local-pg"), "{shown:?}");
    assert!(shown[3].trim() == "▾ shop" && shown[4].trim() == "▾ Tables", "{shown:?}");
    assert_eq!(shown[5].trim(), "▸ order_items", "the match itself as its own expansion says");
    assert_eq!(h.explorer_line().trim(), "▸ order_items", "the cursor on the first match");
    assert!(h.sent().is_empty(), "nothing asked of the server");
    h.key(KeyCode::Esc);
    assert_eq!(lines(&h), before, "the tree as it was");
}

/// Views, items of a read structure and other databases' names are found; a schema matches by
/// its name and shows as its expansion says (closed); a database whose name matches is listed
/// under the profile as without the filter.
#[test]
fn the_filter_finds_views_structure_items_schemas_and_databases() {
    let mut h = searchable();
    filter(&mut h, "summary");
    assert_eq!(h.rows(), ["+", "local-pg", "  db:datarig*", "    shop", "      Views", "        order_summary"]);
    h.key(KeyCode::Esc);
    filter(&mut h, "email");
    let shown: Vec<String> = lines(&h).iter().map(|l| l.trim().to_string()).collect();
    assert_eq!(
        shown[2..],
        ["▾ datarig default", "▾ shop", "▾ Tables", "▾ users", "▾ Columns (2)", "email  text, not null"],
        "only the matching item of the structure"
    );
    h.key(KeyCode::Esc);
    filter(&mut h, "pkey");
    let shown: Vec<String> = lines(&h).iter().map(|l| l.trim().to_string()).collect();
    assert_eq!(shown[6..], ["▾ Primary Key", "▸ users_pkey"]);
    h.key(KeyCode::Esc);
    filter(&mut h, "analyt");
    assert_eq!(h.rows(), ["+", "local-pg", "  db:datarig*", "    analytics"]);
    assert_eq!(h.explorer_line().trim(), "▸ analytics");
    h.key(KeyCode::Esc);
    filter(&mut h, "sales");
    assert_eq!(h.rows(), ["+", "local-pg", "  db:sales_dw"]);
    assert_eq!(h.explorer_line().trim(), "▸ sales_dw");
    assert!(h.sent().is_empty());
}

/// Only what was read is searched: a table of a schema whose objects were never listed (the
/// completion catalog knows `analytics.events`) is not found and its schema is not loaded, and
/// a profile that is not connected keeps whatever tree it had out of the search.
#[test]
fn unread_schemas_and_profiles_that_are_not_connected_are_not_searched() {
    let mut h = searchable();
    filter(&mut h, "events");
    assert_eq!(h.rows(), ["+"]);
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::LoadObjects { .. })), "no schema is listed for the filter");
    h.key(KeyCode::Esc);
    // `cold` has a tree left (a previous session) but is not connected.
    let cold = h.app.profiles.iter().find(|p| p.name == "cold").unwrap().id;
    let c = h.app.conns.entry(cold);
    c.tree.set_schemas(vec!["warehouse_stage".into()]);
    assert!(!c.connected);
    filter(&mut h, "warehouse");
    assert_eq!(h.rows(), ["+"]);
}

/// While filtering, Up and Down move among the rows shown and Enter keeps the filter; the
/// cursor goes to the first row that matches whatever its kind. Opening a node the filter
/// opened for its matches changes only that node's own expansion.
#[test]
fn keys_work_on_the_filtered_rows() {
    let mut h = searchable();
    filter(&mut h, "order");
    assert_eq!(
        h.rows(),
        [
            "+",
            "local-pg",
            "  db:datarig*",
            "    shop",
            "      Tables",
            "        order_items",
            "        orders",
            "      Views",
            "        order_summary"
        ]
    );
    assert_eq!(h.explorer_line().trim(), "▸ order_items");
    h.key(KeyCode::Down);
    assert_eq!(h.explorer_line().trim(), "▸ orders");
    h.key(KeyCode::Enter);
    assert!(!h.app.explorer.filtering && h.app.explorer.filter.text() == "order", "Enter keeps the filter");
    let rows = h.app.explorer_rows();
    let shop = rows.iter().find(|r| matches!(r.kind, RowKind::Node(..)) && r.opened).unwrap();
    let RowKind::Node(id, n) = shop.kind else { unreachable!() };
    assert_eq!(h.app.conns.get(id).unwrap().tree.is_expanded(n), Some(false), "shown open, still closed");
}

/// A filtered tree on a small screen.
#[test]
fn a_filtered_tree_snapshot() {
    let mut h = searchable();
    filter(&mut h, "email");
    let t = h.draw(80, 24);
    insta::assert_snapshot!("explorer_search_en_80x24", t.backend());
}
