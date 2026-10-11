//! A table's structure in the explorer: an open table shows its groups
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
        ..Default::default()
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
/// again asks nothing); until it comes the table says it is loading. Then its line has the
/// estimates read with it (no line of its own under it), and it shows the groups of a table,
/// every one closed; Columns opens to each column with its marks and `type, not
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
    assert!(line_of(&mut h, "users").ends_with("▾ users  ~11k rows · 4.2 MB"), "{}", line_of(&mut h, "users"));
    let tree = lines(&mut h);
    let at = tree.iter().position(|l| l.contains("▾ users")).unwrap();
    assert!(tree[at + 1].ends_with("▸ Columns (5)"), "the groups right under it: {:?}", tree[at + 1]);
    assert!(!tree.iter().any(|l| l.contains("~4.2 MB")), "{tree:?}");
    let groups: Vec<&str> = tree[at + 1..at + 8].iter().map(|l| l.trim()).collect();
    assert_eq!(
        groups,
        [
            "▸ Columns (5)",
            "▸ Primary Key",
            "Foreign Keys",
            "▸ Indexes (3)",
            "▸ Unique Constraints (1)",
            "Check Constraints",
            "▸ Triggers (2)"
        ],
        "every group closed"
    );
    h.goto("Columns");
    h.key(KeyCode::Char('l'));
    for want in [
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
        "▸ users_touch  BEFORE UPDATE OF name · FOR EACH ROW · shop.touch()",
    ] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    // A trigger's `WHEN` condition is on a line of its own, under it; one without has none.
    assert!(!line_of(&mut h, "users_audit").contains('▸'), "{}", line_of(&mut h, "users_audit"));
    h.goto("users_touch");
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "WHEN (old.name IS DISTINCT FROM new.name)");
    // The explorer cuts deep lines: the status bar shows the one under the cursor whole.
    h.explore("local-pg");
    h.goto("users_nickname_idx");
    let status = h.status(200, 45);
    assert!(status.contains("users_nickname_idx  (nickname DESC) btree WHERE nickname IS NOT NULL"), "{status}");
    h.explore("local-pg");
    h.goto("users");
    let status = h.status(200, 45);
    assert!(status.contains("users  ~11k rows · ~4.2 MB") && !status.contains("users_"), "{status}");
    // The primary key opens to its Columns, closed, which open to each column as the table's
    // Columns group has it.
    h.explore("local-pg");
    h.goto("Primary Key");
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert!(h.explorer_line().ends_with("▸ users_pkey"), "{}", h.explorer_line());
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "▸ Columns (1)");
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "PK id  bigint, not null, default nextval('shop.users_id_seq'::regclass)");
    // Closed and opened again: nothing is asked, and what was open is open.
    h.explore("local-pg");
    h.goto("users");
    h.key(KeyCode::Char('h'));
    assert!(!lines(&mut h).iter().any(|l| l.contains("Columns")));
    h.key(KeyCode::Char('l'));
    assert!(asked(&h.sent()).is_empty(), "cached");
    for want in ["▾ Columns (5)", "nickname  text", "users_touch", "▾ users_pkey"] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    // Closed again, as it was.
    h.goto("Columns");
    h.key(KeyCode::Char('h'));
    h.explore("local-pg");
    h.goto("users");
    h.key(KeyCode::Char('h'));
    h.key(KeyCode::Char('l'));
    assert!(line_of(&mut h, "Columns").ends_with("▸ Columns (5)") && line_of(&mut h, "Triggers").contains('▾'));
    assert!(!lines(&mut h).iter().any(|l| l.contains("nickname  text")));
}

/// What is open is each table's own: another table opens with its groups closed, whatever the
/// first has open, and each keeps its own while the other changes.
#[test]
fn each_table_remembers_its_own_open_groups() {
    let mut h = shop_open(false);
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users_structure()));
    h.goto("Columns");
    h.key(KeyCode::Char('l'));
    h.explore("local-pg");
    h.goto("orders");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "orders", orders_structure()));
    let tree = lines(&mut h);
    let at = tree.iter().position(|l| l.ends_with("▾ orders  ~8 KB")).unwrap();
    assert!(tree[at + 1].ends_with("▸ Columns (3)"), "{tree:?}");
    h.goto("Foreign Keys (2)");
    h.key(KeyCode::Char('l'));
    // users kept its columns open, orders its foreign keys.
    let tree = lines(&mut h);
    let users = tree.iter().position(|l| l.contains("▾ users")).unwrap();
    assert!(tree[users + 1].ends_with("▾ Columns (5)"), "{tree:?}");
    assert!(tree[at + 1].ends_with("▸ Columns (3)") && tree.iter().any(|l| l.contains("orders_user_id_fkey")));
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

/// A table never analyzed has no row estimate: its line has its size alone, and the status
/// bar says "rows unknown", never 0 rows. Its foreign keys
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
    assert!(line_of(&mut h, "orders").ends_with("▾ orders  ~8 KB"), "{}", line_of(&mut h, "orders"));
    assert!(!lines(&mut h).iter().any(|l| l.contains("~0 rows")));
    assert!(h.status(200, 45).contains("orders  rows unknown · ~8 KB"), "{}", h.status(200, 45));
    h.goto("Columns");
    h.key(KeyCode::Char('l'));
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
    // Opened there, it shows its groups, closed.
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users_structure()));
    assert!(h.explorer_line().ends_with("▾ users  ~11k rows · 4.2 MB"), "{}", h.explorer_line());
    h.keys("j");
    assert!(h.explorer_line().ends_with("▸ Columns (5)"), "{}", h.explorer_line());
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

/// A view shows its columns and triggers only, and no estimates (it has no storage); a
/// materialized view its columns and indexes, and its estimates on its line.
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
    assert_eq!(below.first(), Some(&"▸ Columns (5)"), "{below:?}");
    assert!(below.contains(&"Triggers"), "{below:?}");
    for absent in ["Indexes", "Primary Key", "rows"] {
        assert!(!below.iter().any(|l| l.contains(absent)), "{absent}: {below:?}");
    }
    assert!(line_of(&mut h, "order_summary").ends_with("▾ order_summary"), "a view: no estimates");
    assert!(!h.status(200, 45).contains("order_summary"), "nor in the status bar");
    let mut m = TableStructure::new(RelationKind::MaterializedView);
    m.estimated_rows = Some(950);
    m.total_bytes = Some(16_384);
    h.db(structure("shop", "order_summary", m));
    let tree = lines(&mut h);
    assert!(line_of(&mut h, "order_summary").ends_with("▾ order_summary  ~950 rows · 16 KB"), "{tree:?}");
    assert!(tree.iter().any(|l| l.trim() == "Indexes") && !tree.iter().any(|l| l.trim() == "Triggers"));
    // No statistics yet (never vacuumed or analyzed): nothing on its line, never "0 B"; the
    // status bar says so.
    h.db(structure("shop", "order_summary", TableStructure::new(RelationKind::MaterializedView)));
    let tree = lines(&mut h);
    assert!(line_of(&mut h, "order_summary").ends_with("▾ order_summary"), "{tree:?}");
    assert!(!tree.iter().any(|l| l.contains("0 B") || l.contains("unknown")), "{tree:?}");
    let status = h.status(200, 45);
    assert!(status.contains("order_summary  rows and size unknown (no statistics yet)"), "{status}");
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
/// room, and a line that still does not fit is cut in its middle, so its end stays (what a
/// trigger calls, what to do). A structure that could not be read shows why alone there, in
/// English and Korean at 80 columns: the reason and "try again". A trigger's `WHEN` condition,
/// on its own line, is whole there.
#[test]
fn a_long_line_keeps_its_end_in_the_status_bar() {
    use datarig_core::i18n::{I18n, Label, Msg};
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::connected(lang);
        h.key(KeyCode::BackTab);
        h.keys("jjjj");
        h.key(KeyCode::Char('l'));
        let objects = SchemaObjects { tables: vec!["users".into()], views: vec![], ..Default::default() };
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
    let status = h.status(100, 24);
    assert!(status.contains("users_touch  BEFORE UPDATE OF name"), "{status}");
    assert!(status.contains("… · shop.touch_row_before_it_is_written()"), "{status}");
    // Its `WHEN` condition, on its own line, is whole there at 120 columns.
    h.key(KeyCode::Char('l'));
    h.keys("j");
    let status = h.status(120, 24);
    assert!(status.contains("│ WHEN (old.name IS DISTINCT FROM new.name)"), "{status}");
}

/// A long connection name gives up its room to the message at 80 columns: the reason a
/// structure could not be read and what to do both stay whole, in English and Korean, and the
/// name keeps its start.
#[test]
fn a_long_connection_name_leaves_the_message_whole() {
    use datarig_core::i18n::Label;
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::connected(lang);
        h.app.profiles[0].name = "analytics-replica-eu-01".into();
        h.key(KeyCode::BackTab);
        h.keys("jjjj");
        h.key(KeyCode::Char('l'));
        let objects = SchemaObjects { tables: vec!["users".into()], views: vec![], ..Default::default() };
        h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
        h.goto("users");
        h.key(KeyCode::Char('l'));
        h.db(DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Err(DbError::Locked) });
        h.keys("j");
        let why = Label::TreeStructureLocked.text(lang);
        let t = h.draw(80, 24);
        let status = row_text(t.backend().buffer(), 23);
        assert!(status.contains(why), "{lang:?}: {status}");
        assert!(status.contains(" analyti") && status.contains('…'), "{lang:?}: {status}");
        if lang == Lang::Ko {
            check_localized("status_lock_long_name_ko", lang, t.backend().buffer());
        }
        // With room, the name is whole.
        assert!(h.status(160, 24).contains("analytics-replica-eu-01"), "{lang:?}");
    }
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

/// Another database's tables have their estimates from that database's aux metadata session
/// and read their structure through it (never the profile's own), and a foreign key goes to a
/// table of that database.
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
    let mut objects: SchemaObjects = (vec!["orders".into(), "users".into()], vec![]).into();
    objects
        .stats
        .insert("users".into(), datarig_core::driver::structure::RelationStats { rows: Some(40), bytes: None });
    ev(&mut h, DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    assert!(line_of(&mut h, "users").ends_with("▸ users  ~40 rows"), "its estimates with its objects");
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
    assert_eq!(h.explorer_line().trim(), "▸ users  ~40 rows");
}

/// The structure tree drawn: users and orders open with their groups (orders its columns and
/// foreign keys, users its indexes and triggers), English, icons on and off, at 80x24 and
/// 120x40. With icons on each group has its icon and each item its group's
/// (a column keeps its key marks or its type's icon).
#[test]
fn explorer_structure_icons_on_and_off() {
    for icons in [false, true] {
        for (w, hh) in [(80, 24), (120, 40)] {
            let mut h = shop_open(icons);
            h.goto("orders");
            h.key(KeyCode::Char('l'));
            h.db(structure("shop", "orders", orders_structure()));
            h.goto("Columns");
            h.key(KeyCode::Char('l'));
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
    h.goto("Columns");
    h.key(KeyCode::Char('l'));
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

/// `shop` listed with the estimates of its relations with storage: two tables with statistics,
/// one without (`orders`), a foreign table (none: no entry), a view and a materialized view.
fn shop_with_estimates(lang: Lang) -> Harness {
    use datarig_core::driver::structure::RelationStats;
    let mut h = Harness::connected(lang);
    h.app.icons = IconsSetting::Off;
    h.key(KeyCode::BackTab);
    h.keys("jjjj");
    h.key(KeyCode::Char('l'));
    let known = |rows, bytes| RelationStats { rows: Some(rows), bytes: Some(bytes) };
    let objects = SchemaObjects {
        tables: ["order_items", "orders", "remote_rates", "users"].map(String::from).to_vec(),
        views: vec!["order_summary".into(), "zz_mv".into()],
        materialized: ["zz_mv".to_string()].into(),
        stats: [
            ("order_items".to_string(), known(150_000, 19_922_944)),
            ("orders".to_string(), RelationStats::default()),
            ("users".to_string(), known(5_000, 1_677_722)),
            ("zz_mv".to_string(), known(950, 16_384)),
        ]
        .into(),
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    h.sent();
    h
}

/// The explorer's inner lines on a `w`x`hh` screen (between its borders).
fn explorer_screen(h: &mut Harness, w: u16, hh: u16) -> Vec<String> {
    h.screen(w, hh).lines().filter_map(|l| l.split('│').nth(1).map(str::to_string)).collect()
}

/// Every table and materialized view of a listed schema has its estimates on its own line,
/// without being opened (nothing is asked for): the rows and the size, dim, on the right. One
/// without statistics yet shows nothing there (no "unknown" on the line), nor do a view and a
/// foreign table. The status bar shows the line under the cursor whole, with the estimates in
/// words, "unknown" included; a view has no such line.
#[test]
fn every_table_line_has_its_estimates() {
    let mut h = shop_with_estimates(Lang::En);
    for (name, want) in [
        ("order_items", "▸ order_items  ~150k rows · 19 MB"),
        ("orders", "▸ orders"),
        ("remote_rates", "▸ remote_rates"),
        ("users", "▸ users  ~5k rows · 1.6 MB"),
        ("order_summary", "▸ order_summary"),
        ("zz_mv", "▸ zz_mv  ~950 rows · 16 KB"),
    ] {
        let l = line_of(&mut h, name);
        assert!(l.ends_with(want), "{name}: {l:?}");
    }
    assert!(h.sent().is_empty(), "nothing is read for them");
    // On the right of a 40-column explorer, dim.
    let screen = explorer_screen(&mut h, 160, 45);
    let users = screen.iter().find(|l| l.contains("users")).unwrap();
    assert!(users.ends_with("  ~5k rows · 1.6 MB") && users.starts_with("        ▸ users  "), "{users:?}");
    let t = h.draw(160, 45);
    let buf = t.backend().buffer();
    let y = (0..45).find(|&y| row_text(buf, y).contains("▸ users")).unwrap();
    let x = row_text(buf, y).find("~5k").unwrap() as u16;
    assert_eq!(buf[(x, y)].fg, datarig_tui::theme::DARK.fg_dim);
    assert_eq!(buf[(x - 8, y)].fg, datarig_tui::theme::DARK.fg, "the name as before");
    // The status bar: the whole line, the estimates in words.
    for (name, want) in [
        ("order_items", Some("order_items  ~150k rows · ~19 MB")),
        ("orders", Some("orders  rows and size unknown (no statistics yet)")),
        ("remote_rates", None),
        ("order_summary", None),
        ("zz_mv", Some("zz_mv  ~950 rows · ~16 KB")),
    ] {
        h.explore("local-pg");
        h.goto(name);
        let status = h.status(200, 45);
        match want {
            Some(w) => assert!(status.contains(w), "{name}: {status}"),
            None => assert!(!status.contains(name) && !status.contains("rows"), "{name}: {status}"),
        }
    }
}

/// The name always wins: the estimates take the room it leaves (two blanks at least), the
/// size going first, then the rows; a name too long for the explorer is cut as before, and
/// never for them. 80x24 (a 22-column explorer), 120x40 (28) and 160x45 (38).
#[test]
fn the_name_wins_over_the_estimates() {
    use datarig_core::driver::structure::RelationStats;
    let mut h = shop_with_estimates(Lang::En);
    let long = "customer_loyalty_points";
    let objects = SchemaObjects {
        tables: vec![long.into(), "order_items".into(), "users".into()],
        stats: [
            (long.to_string(), RelationStats { rows: Some(7), bytes: Some(8192) }),
            ("order_items".to_string(), RelationStats { rows: Some(150_000), bytes: Some(19_922_944) }),
            ("users".to_string(), RelationStats { rows: Some(5_000), bytes: Some(1_677_722) }),
        ]
        .into(),
        ..Default::default()
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    let line = |h: &mut Harness, w, hh, name: &str| {
        explorer_screen(h, w, hh).into_iter().find(|l| l.contains(&name[..5])).unwrap().trim_end().to_string()
    };
    // Room for both, for the rows alone, for none.
    assert_eq!(line(&mut h, 160, 45, "users"), "        ▸ users      ~5k rows · 1.6 MB");
    assert_eq!(line(&mut h, 160, 45, "order_items"), "        ▸ order_items       ~150k rows");
    assert_eq!(line(&mut h, 160, 45, long), "        ▸ customer_loyalty_points");
    assert_eq!(line(&mut h, 120, 40, "users"), "        ▸ users     ~5k rows");
    assert_eq!(line(&mut h, 120, 40, "order_items"), "        ▸ order_items");
    assert_eq!(line(&mut h, 120, 40, long), "        ▸ customer_loyalty_…");
    assert_eq!(line(&mut h, 80, 24, "users"), "        ▸ users");
    assert_eq!(line(&mut h, 80, 24, "order_items"), "        ▸ order_items");
}

/// `r` on the schema, or on a closed table, lists the schema's objects again, and their
/// estimates with them; a structure read again (`r` on an open table) brings its own.
#[test]
fn r_reads_the_estimates_again() {
    use datarig_core::driver::structure::RelationStats;
    let mut h = shop_with_estimates(Lang::En);
    h.goto("users");
    h.keys("r");
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "shop")), "{sent:?}");
    let objects = SchemaObjects {
        tables: vec!["orders".into(), "users".into()],
        stats: [
            ("orders".to_string(), RelationStats { rows: Some(50_000), bytes: Some(19_922_944) }),
            ("users".to_string(), RelationStats { rows: Some(6_000), bytes: Some(2_097_152) }),
        ]
        .into(),
        ..Default::default()
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    assert!(line_of(&mut h, "orders").ends_with("▸ orders  ~50k rows · 19 MB"), "analyzed since");
    assert!(line_of(&mut h, "users").ends_with("▸ users  ~6k rows · 2 MB"));
    h.explore("local-pg");
    h.goto("shop");
    h.keys("r");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "shop")));
    // An open table's structure, read again, has the estimates of now.
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], vec![]).into()) });
    h.explore("local-pg");
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users_structure()));
    h.sent();
    h.keys("r");
    assert_eq!(asked(&h.sent()), ["shop.users"]);
    let mut again = users_structure();
    again.estimated_rows = Some(12_000);
    h.db(structure("shop", "users", again));
    assert!(line_of(&mut h, "users").ends_with("▾ users  ~12k rows · 4.2 MB"), "{}", line_of(&mut h, "users"));
}

/// Korean words the rows (from its own catalog), not the English ones.
#[test]
fn estimates_are_worded_in_the_ui_language() {
    use datarig_core::i18n::{I18n, Msg};
    let mut h = shop_with_estimates(Lang::Ko);
    let [ko, en] =
        [Lang::Ko, Lang::En].map(|l| I18n::new(l).msg(&Msg::TreeStatsRows { rows: "5k".into() }).to_string());
    assert_ne!(ko, en);
    let l = line_of(&mut h, "users");
    assert!(l.ends_with(&format!("users  {ko} · 1.6 MB")), "{l:?}");
}

/// `shop.order_items` with keys of several columns: a composite primary key, two foreign keys
/// (one to another schema), a unique constraint, an index on a column with its collation and
/// operator class, an expression and a column in `DESC NULLS LAST` order that carries another
/// (`INCLUDE`), a check on two columns and one on none.
fn order_items_structure() -> TableStructure {
    use datarig_core::driver::structure::*;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let col = |name: &str, ty: &str, not_null: bool, default: Option<&str>| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null,
        default: default.map(str::to_string),
        fill: ColumnFill::Default,
    };
    let mut t = TableStructure::new(RelationKind::Table);
    t.columns = vec![
        col("order_id", "bigint", true, None),
        col("line_no", "integer", true, None),
        col("product_id", "bigint", true, None),
        col("quantity", "integer", true, Some("1")),
        col("sku", "text", false, None),
    ];
    t.primary_key = Some(KeyConstraint {
        name: "order_items_pkey".into(),
        columns: s(&["order_id", "line_no"]),
        definition: String::new(),
    });
    let fk = |name: &str, cols: &[&str], schema: &str, table: &str, refs: &[&str]| ForeignKey {
        name: name.into(),
        columns: s(cols),
        ref_schema: schema.into(),
        ref_table: table.into(),
        ref_columns: s(refs),
        on_delete: FkAction::NoAction,
        on_update: FkAction::NoAction,
        definition: String::new(),
    };
    t.foreign_keys = vec![
        fk("order_items_line_fkey", &["order_id", "line_no"], "sales", "lines", &["order_id", "no"]),
        fk("order_items_product_fkey", &["product_id"], "shop", "products", &["id"]),
    ];
    t.unique_constraints = vec![KeyConstraint {
        name: "order_items_sku_key".into(),
        columns: s(&["sku", "product_id"]),
        definition: String::new(),
    }];
    t.indexes = vec![Index {
        name: "order_items_lookup".into(),
        columns: s(&["sku", "lower(sku)", "quantity"]),
        options: s(&["COLLATE \"C\" text_pattern_ops", "DESC", "DESC NULLS LAST"]),
        include: s(&["line_no"]),
        key_columns: vec![Some("sku".into()), None, Some("quantity".into())],
        include_columns: s(&["line_no"]),
        unique: false,
        method: "btree".into(),
        predicate: None,
        primary: false,
        constraint: false,
        definition: String::new(),
    }];
    let check = |name: &str, expression: &str, cols: &[&str]| CheckConstraint {
        name: name.into(),
        expression: expression.into(),
        columns: s(cols),
        definition: String::new(),
    };
    t.checks = vec![
        check("order_items_always", "true", &[]),
        check("order_items_positive", "quantity > 0 AND line_no > 0", &["line_no", "quantity"]),
    ];
    t
}

/// `order_items` open with every group of keys open (their items closed).
fn order_items_open(icons: bool) -> Harness {
    let mut h = shop_open(icons);
    h.goto("order_items");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "order_items", order_items_structure()));
    for g in ["Primary Key", "Foreign Keys", "Indexes", "Unique Constraints", "Check Constraints"] {
        h.goto(g);
        h.key(KeyCode::Char('l'));
    }
    h.explore("local-pg");
    h
}

/// Open the item on the line with `item`, then its Columns (the line after its own lines).
fn open_columns(h: &mut Harness, item: &str) {
    h.explore("local-pg");
    h.goto(item);
    h.key(KeyCode::Char('l'));
    h.goto("Columns (");
    h.key(KeyCode::Char('l'));
}

/// Each key, index and check opens to `Columns (n)`, closed, which opens to the columns it
/// covers in its order, each as the table's Columns group shows it (marks, `type, not null,
/// default …`): an index key with its collation, operator class and order, an expression's key
/// as its text, the columns it carries last, a foreign key's with the column each references
/// (its schema named when it is another). A foreign key keeps its line of what it references;
/// a check that reads no column does not open. The status bar shows each line whole.
#[test]
fn keys_indexes_and_checks_list_the_columns_they_cover() {
    let mut h = order_items_open(false);
    h.goto("order_items_lookup");
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "▸ Columns (4)", "closed at first");
    h.key(KeyCode::Char('l'));
    let tree = lines(&mut h);
    let at = tree.iter().position(|l| l.trim() == "▾ Columns (4)").unwrap();
    let got: Vec<&str> = tree[at + 1..at + 5].iter().map(|l| l.trim()).collect();
    assert_eq!(
        got,
        [
            "UQ sku  text · COLLATE \"C\" text_pattern_ops",
            "lower(sku)  expression · DESC",
            "quantity  integer, not null, default 1 · DESC NULLS LAST",
            "PK FK line_no  integer, not null · include",
        ]
    );
    let indent = |l: &str| l.len() - l.trim_start().len();
    assert_eq!(indent(&tree[at + 1]), indent(&tree[at]) + 4, "a level below its Columns (no arrow)");
    for (item, want) in [
        ("order_items_pkey", vec!["PK FK order_id  bigint, not null", "PK FK line_no  integer, not null"]),
        ("order_items_sku_key", vec!["UQ sku  text", "FK UQ product_id  bigint, not null"]),
        ("order_items_positive", vec!["PK FK line_no  integer, not null", "quantity  integer, not null, default 1"]),
        (
            "order_items_line_fkey",
            vec![
                "PK FK order_id → sales.lines.order_id  bigint, not null",
                "PK FK line_no → sales.lines.no  integer, not null",
            ],
        ),
        ("order_items_product_fkey", vec!["FK UQ product_id → products.id  bigint, not null"]),
    ] {
        open_columns(&mut h, item);
        let tree = lines(&mut h);
        let at = tree.iter().position(|l| l.contains(item)).unwrap();
        let cols = tree[at + 1..].iter().position(|l| l.contains("▾ Columns (")).unwrap() + at + 1;
        let got: Vec<&str> = tree[cols + 1..cols + 1 + want.len()].iter().map(|l| l.trim()).collect();
        assert_eq!(got, want, "{item}");
        assert_eq!(tree[cols].trim(), format!("▾ Columns ({})", want.len()), "{item}");
    }
    // A foreign key keeps its line of what it references, before its Columns.
    let tree = lines(&mut h);
    let at = tree.iter().position(|l| l.contains("order_items_line_fkey")).unwrap();
    assert_eq!(tree[at + 1].trim(), "order_id, line_no → sales.lines(order_id, no)");
    assert_eq!(tree[at + 2].trim(), "▾ Columns (2)");
    // A check on no column has nothing to open.
    h.explore("local-pg");
    h.goto("order_items_always");
    assert!(h.explorer_line().ends_with("  order_items_always  true"), "{}", h.explorer_line());
    h.key(KeyCode::Char('l'));
    assert!(!h.explorer_line().contains('▾'));
    // The status bar has the whole line under the cursor.
    h.explore("local-pg");
    h.goto("quantity  integer, not null, default 1 · DESC");
    let status = h.status(200, 45);
    assert!(status.contains("quantity  integer, not null, default 1 · DESC NULLS LAST"), "{status}");
    h.explore("local-pg");
    h.goto("order_id → sales.lines.order_id");
    let status = h.status(200, 45);
    assert!(status.contains("PK FK order_id → sales.lines.order_id  bigint, not null"), "{status}");
    // Enter on a foreign key's column goes to the table it references, as on the key.
    h.explore("local-pg");
    h.goto("product_id → products.id");
    h.sent();
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(h.status(160, 45).contains("shop.products is not in the explorer"), "{sent:?}");
}

/// What is open under an item is remembered with the table's structure: closing the item, the
/// group or the table and opening it again shows its Columns as they were; another table's
/// start closed; `r` keeps them. Closing a Columns hides its lines.
#[test]
fn open_columns_are_remembered_per_table() {
    let mut h = order_items_open(false);
    open_columns(&mut h, "order_items_lookup");
    open_columns(&mut h, "order_items_pkey");
    h.sent();
    let open = |h: &mut Harness| lines(h).iter().filter(|l| l.contains("▾ Columns (")).count();
    assert_eq!(open(&mut h), 2);
    // The item closed and opened: its Columns still open.
    h.explore("local-pg");
    h.goto("order_items_lookup");
    h.key(KeyCode::Char('h'));
    assert_eq!(open(&mut h), 1);
    h.key(KeyCode::Char('l'));
    assert_eq!(open(&mut h), 2);
    // The table closed and opened: the same (nothing asked again).
    h.explore("local-pg");
    h.goto("order_items");
    h.key(KeyCode::Char('h'));
    h.key(KeyCode::Char('l'));
    assert!(asked(&h.sent()).is_empty());
    assert_eq!(open(&mut h), 2);
    assert!(lines(&mut h).iter().any(|l| l.trim() == "lower(sku)  expression · DESC"));
    // Read again: still open.
    h.keys("r");
    assert_eq!(asked(&h.sent()), ["shop.order_items"]);
    h.db(structure("shop", "order_items", order_items_structure()));
    assert_eq!(open(&mut h), 2);
    // Another table's are its own: closed.
    h.explore("local-pg");
    h.goto("users");
    h.key(KeyCode::Char('l'));
    h.db(structure("shop", "users", users_structure()));
    h.goto("Indexes");
    h.key(KeyCode::Char('l'));
    h.goto("users_email_key");
    h.key(KeyCode::Char('l'));
    h.keys("j");
    assert_eq!(h.explorer_line().trim(), "▸ Columns (1)");
    assert_eq!(open(&mut h), 2, "order_items' only");
    // A Columns closed: its lines go.
    h.explore("local-pg");
    h.goto("▾ Columns (4)");
    h.key(KeyCode::Char('h'));
    assert!(h.explorer_line().ends_with("▸ Columns (4)"));
    assert!(!lines(&mut h).iter().any(|l| l.contains("lower(sku)  expression")));
}

/// Icons on: a Columns line has the Columns group's icon, each column its key marks or its
/// type's icon, an expression the icon of an unknown type. Drawn, English, icons on and off.
#[test]
fn key_columns_icons_on_and_off() {
    let mut h = order_items_open(true);
    open_columns(&mut h, "order_items_lookup");
    open_columns(&mut h, "order_items_line_fkey");
    for want in [
        "\u{f0835} Columns (4)",
        "\u{f0832} lower(sku)  expression · DESC",
        "\u{f03a0} quantity  integer",
        "\u{f0835} Columns (2)",
        "order_id → sales.lines.order_id  bigint",
    ] {
        assert!(lines(&mut h).iter().any(|l| l.contains(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    for icons in [false, true] {
        let mut h = order_items_open(icons);
        open_columns(&mut h, "order_items_line_fkey");
        open_columns(&mut h, "order_items_lookup");
        h.explore("local-pg");
        h.goto("order_items_line_fkey");
        let name = format!("key_columns_{}_en_120x40", if icons { "icons" } else { "text" });
        insta::assert_snapshot!(name, h.draw(120, 40).backend());
    }
}

/// `shop.items` as the MySQL driver reports it: an `AUTO_INCREMENT` key, a foreign key to a
/// table of another database (`datarig.orders`), a prefix, a descending and a functional index,
/// a unique key, a check the server does not enforce, and two triggers, which call no function.
fn mysql_items_structure() -> TableStructure {
    use datarig_core::driver::structure::*;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let col = |name: &str, ty: &str, not_null: bool, default: Option<&str>| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null,
        default: default.map(str::to_string),
        fill: ColumnFill::Default,
    };
    let index = |name: &str, cols: &[&str], keys: &[Option<&str>], unique: bool| Index {
        name: name.into(),
        columns: s(cols),
        options: vec![String::new(); cols.len()],
        include: Vec::new(),
        key_columns: keys.iter().map(|k| k.map(str::to_string)).collect(),
        include_columns: Vec::new(),
        unique,
        method: "BTREE".into(),
        predicate: None,
        primary: name == "PRIMARY",
        constraint: unique && keys.iter().all(Option::is_some),
        definition: String::new(),
    };
    let trigger = |name: &str, timing, event| Trigger {
        name: name.into(),
        timing,
        events: vec![event],
        for_each_row: true,
        function: String::new(),
        enabled: true,
        update_columns: Vec::new(),
        condition: None,
        definition: String::new(),
    };
    let mut t = TableStructure::new(RelationKind::Table);
    t.estimated_rows = Some(1_200);
    t.total_bytes = Some(81_920);
    t.columns = vec![
        StructureColumn { fill: ColumnFill::AutoIncrement, ..col("id", "bigint unsigned", true, None) },
        col("order_id", "int", false, None),
        col("sku", "varchar(40)", true, Some("'none'")),
        col("qty", "int", true, Some("0")),
        col("created", "timestamp", true, Some("CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP")),
        StructureColumn { fill: ColumnFill::Stored("`qty` * 2".into()), ..col("twice", "int", false, None) },
    ];
    t.primary_key = Some(KeyConstraint { name: "PRIMARY".into(), columns: s(&["id"]), definition: String::new() });
    t.foreign_keys = vec![ForeignKey {
        name: "items_order_fk".into(),
        columns: s(&["order_id"]),
        ref_schema: "datarig".into(),
        ref_table: "orders".into(),
        ref_columns: s(&["id"]),
        on_delete: FkAction::Cascade,
        on_update: FkAction::NoAction,
        definition: String::new(),
    }];
    t.indexes = vec![
        index("PRIMARY", &["id"], &[Some("id")], true),
        index("items_order_fk", &["order_id"], &[Some("order_id")], false),
        index("ix_qty", &["qty"], &[Some("qty")], false),
        index("ix_sku_prefix", &["sku(8)"], &[Some("sku")], false),
        index("ix_upper", &["(upper(`sku`))"], &[None], false),
        index("uq_sku", &["sku"], &[Some("sku")], true),
    ];
    t.indexes[2].options = s(&["DESC"]);
    t.unique_constraints =
        vec![KeyConstraint { name: "uq_sku".into(), columns: s(&["sku"]), definition: String::new() }];
    t.checks = vec![
        CheckConstraint {
            name: "items_chk_1".into(),
            expression: "`qty` >= 0".into(),
            columns: s(&["qty"]),
            definition: "CONSTRAINT items_chk_1 CHECK (`qty` >= 0)".into(),
        },
        CheckConstraint {
            name: "items_chk_2".into(),
            expression: "`qty` < 1000".into(),
            columns: s(&["qty"]),
            definition: format!("CONSTRAINT items_chk_2 CHECK (`qty` < 1000) {}", CheckConstraint::MYSQL_NOT_ENFORCED),
        },
    ];
    t.triggers = vec![
        trigger("items_au", TriggerTiming::After, TriggerEvent::Update),
        trigger("items_bi", TriggerTiming::Before, TriggerEvent::Insert),
    ];
    t
}

/// A MySQL profile (its databases are the schemas), the explorer focused, `shop` open with
/// `items`.
fn mysql_shop_open(icons: bool) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    p.name = "local-my".into();
    p.driver = "mysql".into();
    let cfg = datarig_core::config::Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.app.icons = if icons { IconsSetting::On } else { IconsSetting::Off };
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["datarig".into(), "shop".into()])));
    h.explore("local-my");
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["items".into()], vec![]).into()) });
    h.sent();
    h
}

/// On MySQL an open table shows its structure as on PostgreSQL (no "columns only" notice):
/// an `AUTO_INCREMENT` column says so, a trigger names no function (it calls none, so `F` says
/// there is no function's DDL and the menu does not offer one), a check the server keeps
/// without enforcing it says so, and Enter on a foreign key goes to its table in another
/// database. Triggers the server would not list to the user are unknown, not none.
#[test]
fn a_mysql_table_shows_its_structure() {
    let mut h = mysql_shop_open(false);
    h.goto("items");
    h.key(KeyCode::Char('l'));
    assert_eq!(asked(&h.sent()), ["shop.items"]);
    assert!(!h.status(160, 45).contains("Columns only"), "{}", h.status(160, 45));
    h.db(structure("shop", "items", mysql_items_structure()));
    assert!(line_of(&mut h, "items").ends_with("▾ items  ~1.2k rows · 80 KB"), "{}", line_of(&mut h, "items"));
    for g in ["Columns", "Indexes", "Check Constraints", "Triggers"] {
        h.goto(g);
        h.key(KeyCode::Char('l'));
    }
    for want in [
        "PK id  bigint unsigned, not null, auto_increment",
        "FK order_id  int",
        "created  timestamp, not null, default CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP",
        "twice  int, generated (`qty` * 2)",
        "ix_qty  (qty DESC) BTREE",
        "ix_sku_prefix  (sku(8)) BTREE",
        "ix_upper  ((upper(`sku`))) BTREE",
        "uq_sku  (sku) UNIQUE BTREE · constraint",
        "items_chk_1  `qty` >= 0",
        "items_chk_2  `qty` < 1000 · not enforced",
    ] {
        assert!(lines(&mut h).iter().any(|l| l.ends_with(want)), "{want:?}:\n{}", lines(&mut h).join("\n"));
    }
    assert!(line_of(&mut h, "items_chk_1").ends_with("`qty` >= 0"), "an enforced check says nothing more");
    let bi = line_of(&mut h, "items_bi");
    assert!(bi.ends_with("items_bi  BEFORE INSERT · FOR EACH ROW"), "{bi}");
    assert!(line_of(&mut h, "items_au").ends_with("items_au  AFTER UPDATE · FOR EACH ROW"));
    // No function to show.
    h.goto("items_au");
    {
        use datarig_tui::app::action::{Action, ExplorerAction};
        let items = h.app.menu_items();
        assert!(items.contains(&Action::Explorer(ExplorerAction::ShowDdl)), "{items:?}");
        assert!(!items.contains(&Action::Explorer(ExplorerAction::ShowFunctionDdl)), "{items:?}");
    }
    h.key(KeyCode::Char('F'));
    assert!(h.status(160, 45).contains("This trigger calls no function"), "{}", h.status(160, 45));
    // Enter on the foreign key: the table of another database, read once it is listed.
    h.explore("local-my");
    h.goto("Foreign Keys");
    h.key(KeyCode::Char('l'));
    h.goto("items_order_fk");
    h.sent();
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "datarig")), "{sent:?}");
    h.db(DbEvent::Objects { schema: "datarig".into(), result: Ok((vec!["orders".into()], vec![]).into()) });
    assert_eq!(h.rows()[h.selected()].trim(), "orders");
    // Without the privilege to list triggers: unknown, and not opened as empty.
    let mut hidden = mysql_items_structure();
    hidden.triggers.clear();
    hidden.hidden = vec![datarig_core::driver::structure::StructureGroup::Triggers];
    h.db(structure("shop", "items", hidden));
    let l = line_of(&mut h, "Triggers");
    assert!(l.ends_with("Triggers · unknown: listed only to a user with a privilege this one lacks"), "{l}");
}

/// The MySQL table drawn with its columns, indexes, checks and triggers open, English, icons on
/// and off, at 80x24 and 120x40.
#[test]
fn mysql_structure_icons_on_and_off() {
    for icons in [false, true] {
        for (w, hh) in [(80, 24), (120, 40)] {
            let mut h = mysql_shop_open(icons);
            h.goto("items");
            h.key(KeyCode::Char('l'));
            h.db(structure("shop", "items", mysql_items_structure()));
            for g in ["Columns", "Indexes", "Check Constraints", "Triggers"] {
                h.goto(g);
                h.key(KeyCode::Char('l'));
            }
            h.explore("local-my");
            h.goto("items");
            let name = format!("mysql_structure_{}_en_{w}x{hh}", if icons { "icons" } else { "text" });
            insta::assert_snapshot!(name, h.draw(w, hh).backend());
        }
    }
}
