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
    let load = |result: u64, status_free: bool| Load {
        table: k.clone(),
        tab: TabId(1),
        result,
        held: 3,
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
fn the_pages_of_one_result_are_counted_once_and_fewer_rows_held_start_afresh() {
    let c = meta("c");
    let read_in = ReadIn::Unknown;
    let k = key("orders");
    let mut m = Memory::default();
    let load = |result: u64, held: usize| Load {
        table: k.clone(),
        tab: TabId(1),
        result,
        held,
        columns: vec![(&c, true)],
        read_in: &read_in,
    };
    let page = rows(&[&[Some("a")], &[Some("b")]]);
    // Each page of result 7 (2 rows each) counts only its own rows.
    for p in 1..=5 {
        let l = load(7, 2 * p);
        assert_eq!(m.start(&l), 2 * (p - 1), "page {p}");
        m.add(&l, &page);
    }
    let a = Counted { value: Some("a".into()), rows: 5 };
    let b = Counted { value: Some("b".into()), rows: 5 };
    assert_eq!(m.column(&k, "c").unwrap().values(), vec![a, b]);
    // The same result holding fewer rows than were counted: counted again from its first row.
    let l = load(7, 2);
    assert_eq!(m.start(&l), 0);
    m.add(&l, &page);
    assert_eq!(m.column(&k, "c").unwrap().values().iter().map(|v| v.rows).collect::<Vec<_>>(), vec![1, 1]);
    // Another result starts afresh.
    let l = load(8, 2);
    assert_eq!(m.start(&l), 0);
}

#[test]
fn the_memory_is_bounded_per_column_table_and_session() {
    let c = meta("c");
    let read_in = ReadIn::Unknown;
    let mut m = Memory::default();
    let k = key("t");
    let l = Load { table: k.clone(), tab: TabId(1), result: 1, held: 1, columns: vec![(&c, true)], read_in: &read_in };
    m.start(&l);
    let many: Vec<Vec<Option<String>>> = (0..MEMORY_VALUES + 50).map(|i| vec![Some(format!("v{i}"))]).collect();
    m.add(&l, &many);
    assert_eq!(m.column(&k, "c").unwrap().values().len(), MEMORY_VALUES);
    // A value past the length kept is not remembered.
    let long = "x".repeat(VALUE_BYTES + 1);
    let l2 = Load {
        result: 2,
        held: 1,
        ..Load { table: k.clone(), tab: TabId(1), result: 1, held: 1, columns: vec![(&c, true)], read_in: &read_in }
    };
    m.start(&l2);
    m.add(&l2, &[vec![Some(long)]]);
    assert!(m.column(&k, "c").is_none(), "a column with no value kept is not kept");
    // At most so many columns of a table.
    let metas: Vec<ColumnMeta> = (0..MEMORY_COLUMNS + 3).map(|i| meta(&format!("c{i}"))).collect();
    let wide = key("wide");
    let l = Load {
        table: wide.clone(),
        tab: TabId(1),
        result: 1,
        held: 1,
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
            result: 1,
            held: 1,
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

#[test]
fn null_and_empty_values_count_toward_the_memorys_bounds() {
    let metas: Vec<ColumnMeta> = (0..MEMORY_COLUMNS).map(|i| meta(&format!("c{i}"))).collect();
    let read_in = ReadIn::Unknown;
    let mut m = Memory::default();
    let tables: Vec<TableKey> = (0..MEMORY_TABLES * 4).map(|t| key(&format!("t{t}"))).collect();
    for k in &tables {
        let l = Load {
            table: k.clone(),
            tab: TabId(1),
            result: 1,
            held: 1,
            columns: metas.iter().map(|m| (m, true)).collect(),
            read_in: &read_in,
        };
        m.start(&l);
        m.add(&l, &[metas.iter().map(|_| None).collect(), metas.iter().map(|_| Some(String::new())).collect()]);
        assert!(m.tables() <= MEMORY_TABLES, "{}", m.tables());
        assert!(m.bytes() <= MEMORY_BYTES, "{}", m.bytes());
        assert!(m.tables.iter().all(|(_, tm)| tm.bytes <= MEMORY_TABLE_BYTES));
    }
    // Each value kept costs: the tables used least recently went.
    assert!(m.bytes() > 0);
    assert!(m.column(&tables[tables.len() - 1], "c0").is_some_and(|c| c.values().len() == 2));
    assert!(m.column(&tables[0], "c0").is_none());
}

#[test]
fn a_load_of_no_rows_leaves_nothing_in_the_memory() {
    let c = meta("c");
    let read_in = ReadIn::Unknown;
    let mut m = Memory::default();
    for t in 0..MEMORY_TABLES * 4 {
        let l = Load {
            table: key(&format!("t{t}")),
            tab: TabId(1),
            result: 1,
            held: 1,
            columns: vec![(&c, true)],
            read_in: &read_in,
        };
        assert_eq!(m.start(&l), 0);
        m.add(&l, &[]);
        // Only the table counted last is held, while it is counted.
        assert_eq!(m.tables(), 1);
    }
    // Started only (its first page not come yet): bounded by the tables too.
    for t in 0..MEMORY_TABLES * 4 {
        let l = Load {
            table: key(&format!("s{t}")),
            tab: TabId(1),
            result: 1,
            held: 1,
            columns: vec![(&c, true)],
            read_in: &read_in,
        };
        m.start(&l);
        assert_eq!(m.tables(), 1);
    }
}

#[test]
fn the_range_of_whole_and_decimal_numbers_is_exact() {
    // Sixty adjacent BIGINT values at the top of their range: one f64 for all of them.
    let big: Vec<String> = (0..60).map(|i| (i64::MAX - i).to_string()).collect();
    let t = tally(big.iter().map(|v| Some(v.as_str())), true);
    assert!(t.mostly_distinct());
    assert_eq!(t.range, Some(("9223372036854775748".to_string(), "9223372036854775807".to_string())));
    // Negative ones, and a value past what f64 tells apart.
    let low: Vec<String> = (0..60).map(|i| (i64::MIN + i).to_string()).collect();
    let t = tally(low.iter().map(|v| Some(v.as_str())).chain([Some("-0"), Some("18446744073709551616")]), true);
    assert_eq!(t.range, Some(("-9223372036854775808".to_string(), "18446744073709551616".to_string())));
    // High-precision DECIMAL: digits far past an f64's, leading and trailing zeros.
    let dec: Vec<String> = (0..60).map(|i| format!("1.0000000000000000000000{i:02}")).collect();
    let more = [Some("0001.000000000000000000000000"), Some("1.00000000000000000000006000"), Some("-0.5")];
    let t = tally(dec.iter().map(|v| Some(v.as_str())).chain(more), true);
    assert_eq!(t.range, Some(("-0.5".to_string(), "1.00000000000000000000006000".to_string())));
    // A value not written in plain digits (`NaN`, `Infinity`, money): no range, the values listed.
    for odd in ["NaN", "Infinity", "$1.00", "1e3"] {
        let t = tally(big.iter().map(|v| Some(v.as_str())).chain([Some(odd)]), true);
        assert_eq!(t.range, None, "{odd}");
        assert!(!t.mostly_distinct());
    }
}
