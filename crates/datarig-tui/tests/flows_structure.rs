//! A table's structure in the explorer: an open table shows its size estimate and its groups
//! (Columns, Primary Key, Foreign Keys, Indexes, Unique and Check Constraints, Triggers), read
//! once through the metadata session when it first opens and again with `r`; Enter on a foreign
//! key goes to the table it references. A fake driver records the requests and the test feeds
//! the answers.

mod common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::structure::{RelationKind, TableStructure};
use datarig_core::driver::{DbCommand, DbError, DbEvent, SchemaObjects, SessionRole};
use datarig_core::i18n::Lang;
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::Ordering;

/// Connected, the explorer focused, `shop` open with its tables and its view.
fn shop_open(icons: bool) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.icons = if icons { IconsSetting::On } else { IconsSetting::Off };
    h.key(KeyCode::BackTab); // editor -> explorer
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    let objects = SchemaObjects {
        tables: vec!["order_items".into(), "orders".into(), "users".into()],
        views: vec!["order_summary".into()],
        materialized: Default::default(),
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    h.sent();
    h
}

/// The tables whose structure `cmds` ask for.
fn asked(cmds: &[DbCommand]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| match c {
            DbCommand::LoadStructure { schema, table } => Some(format!("{schema}.{table}")),
            _ => None,
        })
        .collect()
}

fn structure(schema: &str, table: &str, s: TableStructure) -> DbEvent {
    DbEvent::Structure { schema: schema.into(), table: table.into(), result: Ok(Box::new(s)) }
}

/// The explorer's lines, whole (not cut at the explorer's width).
fn lines(h: &mut Harness) -> Vec<String> {
    let rows = h.app.explorer_rows();
    rows.iter().map(|r| h.app.explorer_line_text(r)).collect()
}

/// The line that shows `text` (after the arrow), whole.
fn line_of(h: &mut Harness, text: &str) -> String {
    lines(h).into_iter().find(|l| l.contains(text)).unwrap_or_else(|| panic!("no line with {text:?}"))
}

/// Opening a table asks for its structure once, one request (cached: closing and opening it
/// again asks nothing); until it comes the table says it is loading. Then it shows its size
/// estimate and the groups of a table, Columns open: each column with its marks and `type, not
/// null, default …`. An empty group is dim, without a count, and does not open; the others
/// open to their items, an index or trigger with what it is. The status bar shows the line under
/// the cursor whole.
#[test]
fn an_open_table_shows_its_structure_read_once() {
    let mut h = shop_open(false);
    h.goto("users");
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent()), ["shop.users"], "one request when it opens");
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "Loading…");
    h.db(structure("shop", "users", users_structure()));
    for want in [
        "~11k rows · ~4.2 MB",
        "▾ Columns (5)",
        "PK id  bigint, not null, default nextval('shop.users_id_seq'::regclass)",
        "UQ email  text, not null",
        "name  text, not null",
        "nickname  text",
        "profile  jsonb, default '{}'::jsonb",
        "▸ Primary Key",
        "▸ Indexes (3)",
        "▸ Unique Constraints (1)",
        "▸ Triggers (2)",
    ] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    // A table has one primary key: no count. Empty groups: no arrow, no count.
    assert!(!line_of(&mut h, "Primary Key").contains('('));
    for empty in ["Foreign Keys", "Check Constraints"] {
        let l = line_of(&mut h, empty);
        assert!(l.ends_with(&format!("  {empty}")) && !l.contains('▸'), "{l:?}");
    }
    h.goto("Foreign Keys");
    h.key(KeyCode::Char('l'));
    assert!(!line_of(&mut h, "Foreign Keys").contains('▾'), "an empty group does not open");
    // Indexes and triggers open to what each is.
    h.goto("Indexes");
    h.key(KeyCode::Char('l'));
    h.goto("Triggers");
    h.key(KeyCode::Enter);
    for want in [
        "users_email_key  (email) UNIQUE btree · constraint",
        "users_nickname_idx  (nickname DESC) btree WHERE nickname IS NOT NULL",
        "users_pkey  (id) UNIQUE btree · primary key",
        "users_audit  AFTER INSERT OR DELETE · FOR EACH STATEMENT · shop.audit() · disabled",
        "users_touch  BEFORE UPDATE OF name · FOR EACH ROW · WHEN (old.name IS DISTINCT FROM new.name) · shop.touch()",
    ] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    // The explorer cuts deep lines: the status bar shows the one under the cursor whole.
    h.explore("local-pg");
    h.goto("users_nickname_idx");
    let status = h.status(200, 45);
    assert!(status.contains("users_nickname_idx  (nickname DESC) btree WHERE nickname IS NOT NULL"), "{status}");
    h.explore("local-pg");
    h.goto("users");
    assert!(!h.status(200, 45).contains("users_"), "only for a line of a structure");
    // The primary key opens to its columns.
    h.explore("local-pg");
    h.goto("Primary Key");
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert!(h.explorer_line().ends_with("▸ users_pkey"), "{}", h.explorer_line());
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "id");
    // Closed and opened again: nothing is asked, and what was open is open.
    h.explore("local-pg");
    h.goto("users");
    h.key(KeyCode::Char('h'));
    assert!(!lines(&mut h).iter().any(|l| l.contains("Columns")));
    h.key(KeyCode::Char('l'));
    assert!(asked(&h.sent()).is_empty(), "cached");
    assert!(lines(&mut h).iter().any(|l| l.contains("users_touch")));
}

/// `r` on an open table (or anything of its structure) reads its structure again: it is loading
/// meanwhile, and what was open stays open. `r` on a table that is closed lists its schema
/// again, as before.
#[test]
fn r_reads_a_structure_again() {
    let mut h = shop_open(false);
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users_structure()));
    h.goto("Triggers");
    h.key(KeyCode::Char('l'));
    h.sent();
    h.goto("users_touch");
    h.keys("r");
    let sent = h.sent();
    assert_eq!(asked(&sent), ["shop.users"]);
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::LoadObjects { .. })), "{sent:?}");
    assert!(lines(&mut h).iter().any(|l| l.trim() == "Loading…"));
    let mut again = users_structure();
    again.triggers.pop();
    h.db(structure("shop", "users", again));
    assert!(line_of(&mut h, "Triggers").contains("▾ Triggers (1)"), "still open");
    // On the table's own node while it is open: its structure.
    h.explore("local-pg");
    h.goto("users");
    h.keys("r");
    assert_eq!(asked(&h.sent()), ["shop.users"]);
    // On a closed table: the schema's objects.
    h.explore("local-pg");
    h.goto("orders");
    h.keys("r");
    let sent = h.sent();
    assert!(asked(&sent).is_empty() && sent.iter().any(|c| matches!(c, DbCommand::LoadObjects { .. })), "{sent:?}");
}

/// A table never analyzed has no row estimate: "rows unknown", never 0 rows. Its foreign keys
/// name their table and open to the columns and the actions (`NO ACTION` left out); Enter on
/// one, or on its line, puts the cursor on the table it references: in the same schema at
/// once, in a schema not read yet once its objects come. A table the explorer does not have is
/// said.
#[test]
fn foreign_keys_go_to_the_table_they_reference() {
    let mut h = shop_open(false);
    h.goto("orders");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "orders", orders_structure()));
    assert!(lines(&mut h).iter().any(|l| l.trim_start().starts_with("rows unknown · ~8 KB")));
    assert!(!lines(&mut h).iter().any(|l| l.contains("~0 rows")));
    assert!(line_of(&mut h, "PK FK id").ends_with("PK FK id  bigint, not null, identity always"));
    h.goto("Foreign Keys (2)");
    h.key(KeyCode::Char('l'));
    for want in ["orders_event_fkey  → analytics.events", "orders_user_id_fkey  → shop.users"] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    h.goto("orders_event_fkey");
    h.key(KeyCode::Char('l'));
    h.goto("orders_user_id_fkey");
    h.key(KeyCode::Char('l'));
    for want in ["id → analytics.events(id) · ON DELETE CASCADE · ON UPDATE SET NULL", "user_id → shop.users(id)"]
    {
        let l = line_of(&mut h, want);
        assert!(l.ends_with(want), "{l:?}");
    }
    // Enter on a foreign key of the same schema: its table.
    h.sent();
    h.key(KeyCode::Enter);
    assert_eq!(h.rows()[h.selected()].trim(), "users");
    assert!(h.sent().is_empty(), "nothing to read");
    // Enter on the line of one to a schema not read yet: its objects are asked for, the cursor
    // goes there once they come.
    h.explore("local-pg");
    h.goto("id → analytics.events(id)");
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "analytics")), "{sent:?}");
    h.db(DbEvent::Objects { schema: "analytics".into(), result: Ok((vec!["events".into()], vec![]).into()) });
    assert_eq!(h.rows()[h.selected()].trim(), "events");
    // A table that is not there.
    let mut gone = orders_structure();
    gone.foreign_keys[1].ref_table = "gone".into();
    h.db(structure("shop", "orders", gone));
    h.explore("local-pg");
    h.goto("orders_user_id_fkey");
    let at = h.selected();
    h.key(KeyCode::Enter);
    assert_eq!(h.selected(), at, "the cursor stays");
    assert!(h.status(160, 45).contains("shop.gone is not in the explorer"), "{}", h.status(160, 45));
}

/// A view shows its columns and triggers only, and no size (it has no storage); a materialized
/// view its columns and indexes, with its size.
#[test]
fn views_show_the_groups_of_their_kind() {
    let mut h = shop_open(false);
    h.goto("order_summary");
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent()), ["shop.order_summary"]);
    let mut v = TableStructure::new(RelationKind::View);
    v.columns = users_structure().columns;
    h.db(structure("shop", "order_summary", v));
    let tree = lines(&mut h);
    let at = tree.iter().position(|l| l.contains("order_summary")).unwrap();
    let below: Vec<&str> = tree[at + 1..].iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    assert_eq!(below.first(), Some(&"▾ Columns (5)"), "{below:?}");
    assert!(below.contains(&"Triggers"), "{below:?}");
    for absent in ["Indexes", "Primary Key", "rows"] {
        assert!(!below.iter().any(|l| l.contains(absent)), "{absent}: {below:?}");
    }
    let mut m = TableStructure::new(RelationKind::MaterializedView);
    m.estimated_rows = Some(950);
    m.total_bytes = Some(16_384);
    h.db(structure("shop", "order_summary", m));
    let tree = lines(&mut h);
    assert!(tree.iter().any(|l| l.trim() == "~950 rows · ~16 KB"), "{tree:?}");
    assert!(tree.iter().any(|l| l.trim() == "Indexes") && !tree.iter().any(|l| l.trim() == "Triggers"));
    // No statistics yet (never vacuumed or analyzed): neither estimate, never "0 B".
    h.db(structure("shop", "order_summary", TableStructure::new(RelationKind::MaterializedView)));
    let tree = lines(&mut h);
    assert!(tree.iter().any(|l| l.trim() == "rows and size unknown (no statistics yet)"), "{tree:?}");
    assert!(!tree.iter().any(|l| l.contains("0 B")), "{tree:?}");
}

/// A structure that cannot be read says why where it would be; opening the table again asks
/// again. A server too old says the version it needs; a table another session locks says so
/// (the lookup did not wait for it), and asked again once the lock is gone it opens.
#[test]
fn a_structure_that_cannot_be_read_says_why() {
    let mut h = shop_open(false);
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.sent();
    let error = DbError::from("ERROR: permission denied for table users");
    h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Err(error) });
    assert!(
        lines(&mut h).iter().any(|l| l.trim() == "(structure unavailable: ERROR: permission denied for table users)")
    );
    h.key(KeyCode::Char('h'));
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent()), ["shop.users"], "asked again after a failure");
    h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Err(DbError::ServerTooOld) });
    let want = "(structure unavailable: PostgreSQL 12 or newer is required for the table structure)";
    assert!(lines(&mut h).iter().any(|l| l.trim() == want), "{:?}", lines(&mut h));
    h.key(KeyCode::Char('h'));
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent()), ["shop.users"]);
    h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Err(DbError::Locked) });
    let want = "(structure unavailable: the table is locked by another session (try again))";
    assert!(lines(&mut h).iter().any(|l| l.trim() == want), "{:?}", lines(&mut h));
    h.key(KeyCode::Char('h'));
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent()), ["shop.users"], "asked again");
    h.db(structure("shop", "users", users_structure()));
    assert!(lines(&mut h).iter().any(|l| l.contains("Columns (5)")), "{:?}", lines(&mut h));
}

/// A line too long for the status bar keeps what matters: the connection's policy gives up its
/// room when that makes it fit, else the line is cut in its middle, so its end stays (what a trigger calls, what
/// to do). A structure that could not be read shows why alone there, in English and Korean at
/// 80 columns: the reason and "try again".
#[test]
fn a_long_line_keeps_its_end_in_the_status_bar() {
    use datarig_core::i18n::{I18n, Label, Msg};
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::connected(lang);
        h.key(KeyCode::BackTab);
        h.keys("jjjj");
        h.key(KeyCode::Char('l'));
        let objects = SchemaObjects { tables: vec!["users".into()], views: vec![], materialized: Default::default() };
        h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
        h.goto("users");
        h.key(KeyCode::Char('l'));
        h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Err(DbError::Locked) });
        h.keys("j");
        let why = Label::TreeStructureLocked.text(lang);
        let policy = I18n::new(lang).msg(&Msg::StatusPolicy { name: "default".into() }).to_string();
        let status = h.status(80, 24);
        assert!(status.contains(why) && !status.contains(&policy), "{lang:?}: {status}");
        // With room, the policy stays.
        let status = h.status(160, 24);
        assert!(status.contains(why) && status.contains(&policy), "{lang:?}: {status}");
    }
    let mut h = shop_open(false);
    let mut users = users_structure();
    users.triggers[1].function = "shop.touch_row_before_it_is_written".into();
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users));
    h.goto("Triggers");
    h.key(KeyCode::Char('l'));
    h.explore("local-pg");
    h.goto("users_touch");
    let status = h.status(120, 24);
    assert!(status.contains("users_touch  BEFORE UPDATE OF name"), "{status}");
    assert!(status.contains("… · shop.touch_row_before_it_is_written()"), "{status}");
}

/// A driver without the table structure is never asked for it: an open table shows its
/// columns from the completion catalog, as before.
#[test]
fn without_the_capability_columns_come_from_the_catalog() {
    let mut h = shop_open(false);
    h.driver.no_structure.store(true, Ordering::SeqCst);
    h.goto("users");
    h.key(KeyCode::Char('l'));
    assert!(asked(&h.sent()).is_empty());
    assert!(lines(&mut h).iter().any(|l| l.trim_end().ends_with("nickname  text")));
    assert!(!lines(&mut h).iter().any(|l| l.contains("Columns")));
}

/// Another database's tables read their structure through that database's aux metadata
/// session (never the profile's own), and a foreign key goes to a table of that database.
#[test]
fn another_databases_tables_read_through_its_session() {
    let mut h = Harness::connected(Lang::En);
    h.app.icons = IconsSetting::Off;
    let p = h.app.profiles[0].id;
    h.app.conns.entry(p).databases = Some(Ok(vec!["datarig".into(), "sales".into()]));
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.explore("local-pg");
    h.goto("sales");
    h.key(KeyCode::Char('l'));
    let aux = h.roles().iter().rposition(|r| *r == SessionRole::Meta).unwrap();
    let a = h.app.conns.aux(p, "sales").unwrap().id;
    let ev = |h: &mut Harness, ev| {
        h.app.on_app_event(datarig_tui::app::AppEvent::Db {
            target: datarig_tui::app::EventTarget::Aux(a),
            generation: a,
            ev,
        })
    };
    ev(&mut h, DbEvent::Connected);
    ev(&mut h, DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    ev(
        &mut h,
        DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["orders".into(), "users".into()], vec![]).into()) },
    );
    h.sent();
    h.goto("orders");
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent_to(aux)), ["shop.orders"], "through the aux session");
    assert!(asked(&h.sent_to(0)).is_empty(), "not the profile's own");
    ev(&mut h, structure("shop", "orders", orders_structure()));
    h.goto("Foreign Keys (2)");
    h.key(KeyCode::Char('l'));
    h.goto("orders_user_id_fkey");
    h.key(KeyCode::Enter);
    let row = h.app.explorer_rows()[h.selected()].kind.clone();
    assert!(matches!(&row, datarig_tui::app::explorer::RowKind::AuxNode(_, db, _) if db == "sales"), "{row:?}");
    assert_eq!(h.explorer_line().trim(), "▸ users");
}

/// The structure tree drawn: users and orders open with their groups, English, icons on and
/// off, at 80x24 and 120x40. With icons on each group has its icon and each item its group's
/// (a column keeps its key marks or its type's icon).
#[test]
fn explorer_structure_icons_on_and_off() {
    for icons in [false, true] {
        for (w, hh) in [(80, 24), (120, 40)] {
            let mut h = shop_open(icons);
            h.goto("orders");
            h.key(KeyCode::Char('l'));
            h.db(structure("shop", "orders", orders_structure()));
            h.goto("Foreign Keys");
            h.key(KeyCode::Char('l'));
            h.goto("orders_event_fkey");
            h.key(KeyCode::Char('l'));
            // Past the foreign keys that name `shop.users`.
            h.goto("Triggers");
            h.goto("users");
            h.key(KeyCode::Char('l'));
            h.db(structure("shop", "users", users_structure()));
            for g in ["Indexes", "Triggers"] {
                h.goto(g);
                h.key(KeyCode::Char('l'));
            }
            // The cursor back on the first table, so both show from its top.
            h.explore("local-pg");
            h.goto("orders");
            let name = format!("explorer_structure_{}_en_{w}x{hh}", if icons { "icons" } else { "text" });
            insta::assert_snapshot!(name, h.draw(w, hh).backend());
        }
    }
    let mut h = shop_open(true);
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users_structure()));
    for want in [
        "\u{f0835} Columns (5)",
        "\u{f084} id  bigint",
        "\u{f0284} nickname  text",
        "\u{f0306} Primary Key",
        "\u{f119f} Foreign Keys",
        "\u{f027b} Indexes (3)",
        "\u{f0237} Unique Constraints (1)",
        "\u{f0135} Check Constraints",
        "\u{f140b} Triggers (2)",
    ] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    h.goto("Triggers");
    h.key(KeyCode::Char('l'));
    assert!(line_of(&mut h, "users_touch").contains("\u{f140b} users_touch"), "an item has its group's icon");
}
