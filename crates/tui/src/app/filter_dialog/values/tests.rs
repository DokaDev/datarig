use super::*;
use datarig_core::driver::{SessionValues, ValueKind};

fn meta(name: &str) -> ColumnMeta {
    ColumnMeta::new(name.to_string(), "text".to_string(), ValueKind::Text, None)
}

fn key(table: &str) -> TableKey {
    TableKey { profile: ProfileId::new(), database: None, schema: "shop".into(), table: table.into() }
}

fn rows(values: &[&[Option<&str>]]) -> Vec<Vec<Option<String>>> {
    values.iter().map(|r| r.iter().map(|v| v.map(str::to_string)).collect()).collect()
}

#[test]
fn values_are_counted_most_rows_first_with_null_among_them() {
    let t = tally([Some("paid"), Some("shipped"), None, Some("shipped"), Some("paid"), Some("shipped")], false);
    let listed: Vec<(Option<&str>, u64)> = t.values.iter().map(|c| (c.value.as_deref(), c.rows)).collect();
    assert_eq!(listed, vec![(Some("shipped"), 3), (Some("paid"), 2), (None, 1)]);
    assert_eq!((t.rows, t.not_null, t.long, t.others), (6, 5, 0, 0));
    assert!(!t.more() && !t.mostly_distinct());
    // Ties by text, NULL first among them.
    let t = tally([Some("b"), Some("a"), None], false);
    let listed: Vec<Option<&str>> = t.values.iter().map(|c| c.value.as_deref()).collect();
    assert_eq!(listed, vec![None, Some("a"), Some("b")]);
}

#[test]
fn a_long_value_is_counted_never_kept() {
    let long = "x".repeat(VALUE_BYTES + 1);
    let edge = "y".repeat(VALUE_BYTES);
    let t = tally([Some(long.as_str()), Some(long.as_str()), Some(edge.as_str())], false);
    assert_eq!(t.long, 2);
    assert_eq!(t.values, vec![Counted { value: Some(edge), rows: 1 }]);
    assert_eq!(t.not_null, 3);
}

#[test]
fn a_count_keeps_a_bounded_number_of_values() {
    let many: Vec<String> = (0..TALLY_DISTINCT + 5).map(|i| format!("v{i}")).collect();
    let t = tally(many.iter().map(|v| Some(v.as_str())), false);
    assert_eq!(t.values.len(), TALLY_DISTINCT);
    assert_eq!(t.others, 5);
    assert!(t.more());
    // And a bounded number of bytes.
    let big = "z".repeat(VALUE_BYTES);
    let values: Vec<String> =
        (0..TALLY_BYTES / VALUE_BYTES + 3).map(|i| format!("{i}{}", &big[1 + i.to_string().len()..])).collect();
    let t = tally(values.iter().map(|v| Some(v.as_str())), false);
    assert!(t.values.iter().map(|c| c.value.as_ref().map_or(0, String::len)).sum::<usize>() <= TALLY_BYTES);
    assert!(t.others > 0);
}

#[test]
fn numbers_mostly_distinct_have_a_range_not_a_list() {
    let values: Vec<String> = (0..60).map(|i| format!("{}.50", 1000 - i * 3)).collect();
    let t = tally(values.iter().map(|v| Some(v.as_str())).chain([None, Some("-2.5")]), true);
    assert!(t.mostly_distinct());
    assert_eq!(t.range, Some(("-2.5".to_string(), "1000.50".to_string())));
    // Fewer than 50 values, or values repeated: listed.
    let few: Vec<String> = (0..49).map(|i| i.to_string()).collect();
    assert!(!tally(few.iter().map(|v| Some(v.as_str())), true).mostly_distinct());
    let repeated: Vec<String> = (0..100).map(|i| (i % 10).to_string()).collect();
    assert!(!tally(repeated.iter().map(|v| Some(v.as_str())), true).mostly_distinct());
    // Text is never a range.
    assert!(!tally(values.iter().map(|v| Some(v.as_str())), false).mostly_distinct());
}

#[test]
fn mentions_finds_the_column_quoted_or_not() {
    let pg = Dialect::Postgres;
    assert!(mentions("status = 'x' OR id > 1", "status", pg));
    assert!(mentions("\"Status\" IS NULL OR 1 = 1", "Status", pg));
    assert!(mentions("\"a\"\"b\" = 1", "a\"b", pg));
    assert!(mentions("STATUS <> 'x'", "status", pg));
    assert!(!mentions("note = 'status'", "status", pg));
    assert!(!mentions("statuses = 1", "status", pg));
    let my = Dialect::MySql(Default::default());
    assert!(mentions("`status` = 'x' OR 1", "status", my));
}

#[test]
fn a_load_is_remembered_for_its_unfiltered_columns() {
    let (status, amount) = (meta("status"), meta("amount"));
    let read_in = ReadIn::Values(SessionValues { time_zone: "+00:00".into(), charset: "utf8mb4".into() });
    let k = key("orders");
    let mut m = Memory::default();
    let load = |run: u64, status_free: bool| Load {
        table: k.clone(),
        tab: TabId(1),
        run,
        columns: vec![(&status, status_free), (&amount, true)],
        read_in: &read_in,
    };
    // The first load: no condition on either column.
    let l = load(1, true);
    assert_eq!(m.start(&l), 0);
    m.add(&l, &rows(&[&[Some("paid"), Some("1")], &[Some("shipped"), Some("2")]]));
    // Its next page adds to it.
    assert_eq!(m.start(&l), 2);
    m.add(&l, &rows(&[&[Some("paid"), None]]));
    let v = m.column(&k, "status").unwrap();
    assert_eq!(
        v.values(),
        vec![Counted { value: Some("paid".into()), rows: 2 }, Counted { value: Some("shipped".into()), rows: 1 }]
    );
    assert_eq!(v.read_in, read_in);
    // A load filtered on `status` keeps the status values of the earlier one, not its own.
    let l = load(2, false);
    assert_eq!(m.start(&l), 0);
    m.add(&l, &rows(&[&[Some("shipped"), Some("9")]]));
    assert_eq!(m.column(&k, "status").unwrap().values().len(), 2);
    assert_eq!(m.column(&k, "amount").unwrap().values(), vec![Counted { value: Some("9".into()), rows: 1 }]);
    // Forgotten with its profile's structure.
    m.forget(k.profile, Some(None));
    assert!(m.column(&k, "status").is_none());
}

#[test]
fn the_memory_is_bounded_per_column_table_and_session() {
    let c = meta("c");
    let read_in = ReadIn::Unknown;
    let mut m = Memory::default();
    let k = key("t");
    let l = Load { table: k.clone(), tab: TabId(1), run: 1, columns: vec![(&c, true)], read_in: &read_in };
    m.start(&l);
    let many: Vec<Vec<Option<String>>> = (0..MEMORY_VALUES + 50).map(|i| vec![Some(format!("v{i}"))]).collect();
    m.add(&l, &many);
    assert_eq!(m.column(&k, "c").unwrap().values().len(), MEMORY_VALUES);
    // A value past the length kept is not remembered.
    let long = "x".repeat(VALUE_BYTES + 1);
    let l2 = Load {
        run: 2,
        ..Load { table: k.clone(), tab: TabId(1), run: 1, columns: vec![(&c, true)], read_in: &read_in }
    };
    m.start(&l2);
    m.add(&l2, &[vec![Some(long)]]);
    assert!(m.column(&k, "c").unwrap().values().is_empty());
    // At most so many columns of a table.
    let metas: Vec<ColumnMeta> = (0..MEMORY_COLUMNS + 3).map(|i| meta(&format!("c{i}"))).collect();
    let wide = key("wide");
    let l = Load {
        table: wide.clone(),
        tab: TabId(1),
        run: 1,
        columns: metas.iter().map(|m| (m, true)).collect(),
        read_in: &read_in,
    };
    m.start(&l);
    m.add(&l, &[metas.iter().map(|_| Some("x".to_string())).collect()]);
    let kept = metas.iter().filter(|c| m.column(&wide, &c.name).is_some()).count();
    assert_eq!(kept, MEMORY_COLUMNS);
    // A table keeps at most so many bytes, the session too: the tables used least recently go.
    let mut m = Memory::default();
    let value = |i: usize, t: usize| format!("{t:03}{i:05}{}", "v".repeat(VALUE_BYTES - 8));
    let metas: Vec<ColumnMeta> = (0..8).map(|i| meta(&format!("c{i}"))).collect();
    let tables: Vec<TableKey> = (0..8).map(|t| key(&format!("t{t}"))).collect();
    for (t, k) in tables.iter().enumerate() {
        let l = Load {
            table: k.clone(),
            tab: TabId(1),
            run: 1,
            columns: metas.iter().map(|m| (m, true)).collect(),
            read_in: &read_in,
        };
        m.start(&l);
        let rows: Vec<Vec<Option<String>>> =
            (0..100).map(|i| metas.iter().enumerate().map(|(c, _)| Some(value(i * 8 + c, t))).collect()).collect();
        m.add(&l, &rows);
        assert!(m.tables.iter().all(|(_, tm)| tm.bytes <= MEMORY_TABLE_BYTES));
        assert!(m.bytes() <= MEMORY_BYTES, "{}", m.bytes());
    }
    assert!(m.tables() < tables.len(), "the oldest tables were dropped");
    assert!(m.column(&tables[7], "c0").is_some(), "the last one counted stays");
    assert!(m.column(&tables[0], "c0").is_none(), "the first one went");
}
