use super::*;
use std::fs;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("datarig-filter-history-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn key(table: &str) -> TableKey {
    TableKey {
        profile: ProfileId::parse("3f0b8f5e-0b1c-4d2e-9f3a-1b2c3d4e5f60").unwrap(),
        database: "shop".into(),
        schema: "public".into(),
        table: table.into(),
    }
}

#[test]
fn a_missing_file_is_an_empty_history() {
    let d = dir("missing");
    assert_eq!(load(&d.join(FILE)), Ok(History::default()));
    let _ = fs::remove_dir_all(&d);
}

/// A file that is there but cannot be used is never an empty history: each of these is
/// `Unreadable`, and the bytes stay as they were.
#[test]
fn a_file_that_cannot_be_read_is_not_an_empty_history() {
    let d = dir("unreadable");
    let path = d.join(FILE);
    let cases: &[(&str, Unreadable)] = &[
        ("version = 1\n[[tables]\n", Unreadable::Malformed { line: Some(2) }),
        ("version = \"1\"\n", Unreadable::Malformed { line: None }),
        ("tables = []\n", Unreadable::Malformed { line: None }),
        ("version = 2\n", Unreadable::Newer(2)),
        ("version = 1\nfavourites = []\n", Unreadable::Malformed { line: None }),
        (
            "version = 1\n[[tables]]\nprofile = \"3f0b8f5e-0b1c-4d2e-9f3a-1b2c3d4e5f60\"\ndatabase = \"d\"\nschema = \"s\"\ntable = \"t\"\n[[tables.entries]]\nwhere = \"a = 1\"\norder = \"\"\nused = \"yesterday\"\n",
            Unreadable::Malformed { line: None },
        ),
        (
            "version = 1\n[[tables]]\nprofile = \"not an id\"\ndatabase = \"d\"\nschema = \"s\"\ntable = \"t\"\nentries = []\n",
            Unreadable::Malformed { line: None },
        ),
    ];
    for (text, want) in cases {
        fs::write(&path, text).unwrap();
        assert_eq!(load(&path), Err(*want), "{text}");
        assert_eq!(fs::read_to_string(&path).unwrap(), *text, "left as it was");
    }
    fs::write(&path, [0xff, 0xfe, b'\n']).unwrap();
    assert_eq!(load(&path), Err(Unreadable::Malformed { line: None }), "not UTF-8");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(load(&path), Err(Unreadable::Io(_))), "a folder at the path");
    let _ = fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn a_file_without_read_permission_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("mode000");
    let path = d.join(FILE);
    let mut h = History::default();
    h.record(key("orders"), "id = 1", "", 1);
    save(&path, &h).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads anything: then there is nothing to check.
    if fs::read(&path).is_err() {
        assert_eq!(load(&path), Err(Unreadable::Io(io::ErrorKind::PermissionDenied)));
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn a_file_over_the_read_limit_is_unreadable() {
    let d = dir("large");
    let path = d.join(FILE);
    let mut text = String::from("version = 1\n");
    text.push_str(&"#".repeat(READ_MAX as usize));
    fs::write(&path, &text).unwrap();
    assert_eq!(load(&path), Err(Unreadable::TooLarge));
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn entries_round_trip_with_any_text() {
    let d = dir("round");
    let path = d.join(FILE);
    let mut h = History::default();
    assert!(h.record(key("orders"), "note = 'a\nb' AND x = \"q\" AND y = '\\'", "id DESC", 10));
    assert!(h.record(key("orders"), "name = '東京' AND t = 'tab\there' AND c = 'cr\r\nlf'", "", 11));
    assert!(h.record(key("users"), "", "email", 12));
    assert!(h.record(key("users"), "'''", "", 13));
    save(&path, &h).unwrap();
    assert_eq!(load(&path), Ok(h.clone()));
    let users = h.entries(&key("users"));
    assert_eq!(users.len(), 2);
    assert_eq!(users[0], Entry { filter: "'''".into(), order: String::new(), used: 13 });
    assert_eq!(users[1], Entry { filter: String::new(), order: "email".into(), used: 12 });
    let _ = fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn the_file_is_always_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("private");
    let path = d.join(FILE);
    let mut h = History::default();
    h.record(key("orders"), "email = 'a@b.c'", "", 1);
    save(&path, &h).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "a new file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    save(&path, &h).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "never the old mode");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn the_same_texts_move_to_the_front_and_another_order_is_another_entry() {
    let mut h = History::default();
    h.record(key("orders"), "a = 1", "id", 1);
    h.record(key("orders"), "b = 2", "", 2);
    h.record(key("orders"), "a = 1", "id DESC", 3);
    assert!(h.record(key("orders"), "  a = 1 ", "id", 4));
    let e = h.entries(&key("orders"));
    assert_eq!(e.len(), 3);
    assert_eq!(e[0], Entry { filter: "a = 1".into(), order: "id".into(), used: 4 }, "moved, its time updated");
    assert_eq!((e[1].filter.as_str(), e[1].order.as_str()), ("a = 1", "id DESC"));
    assert_eq!(e[2].filter, "b = 2");
}

#[test]
fn blank_and_overlong_texts_are_not_kept() {
    let mut h = History::default();
    assert!(!h.record(key("orders"), "  ", "\n", 1));
    assert!(!h.record(key("orders"), &"x".repeat(ENTRY_MAX_BYTES + 1), "", 1));
    assert!(!h.record(key("orders"), "a = 1", &"y".repeat(ENTRY_MAX_BYTES + 1), 1));
    assert!(h.record(key("orders"), &"x".repeat(ENTRY_MAX_BYTES), "", 1));
    assert_eq!(h.entries(&key("orders")).len(), 1);
}

#[test]
fn a_table_keeps_its_most_recent_entries() {
    let mut h = History::default();
    for i in 0..=PER_TABLE {
        h.record(key("orders"), &format!("id = {i}"), "", i as i64);
    }
    let e = h.entries(&key("orders"));
    assert_eq!(e.len(), PER_TABLE);
    assert_eq!(e[0].filter, format!("id = {PER_TABLE}"));
    assert!(!e.iter().any(|e| e.filter == "id = 0"), "the oldest went");
}

#[test]
fn the_table_used_longest_ago_goes_first() {
    let mut h = History::default();
    for i in 0..TABLES {
        h.record(key(&format!("t{i}")), "a = 1", "", i as i64);
    }
    // Used again: the most recent now, so t1 is the one used longest ago.
    h.record(key("t0"), "a = 2", "", 1000);
    h.record(key("new"), "a = 1", "", 1001);
    assert_eq!(h.table_count(), TABLES);
    assert_eq!(h.entries(&key("t0")).len(), 2);
    assert!(h.entries(&key("t1")).is_empty());
    assert_eq!(h.entries(&key("t2")).len(), 1);
}

#[test]
fn the_file_stays_within_its_budget_leaving_out_the_oldest_tables() {
    let mut h = History::default();
    let long = "x".repeat(ENTRY_MAX_BYTES - 2);
    for t in 0..10 {
        for i in 0..PER_TABLE {
            h.record(key(&format!("t{t}")), &format!("{long}{i:02}"), "", (t * 100 + i) as i64);
        }
    }
    let text = h.to_toml();
    assert!(text.len() <= FILE_BUDGET, "{}", text.len());
    let read = parse(&text).unwrap();
    assert_eq!(read.entries(&key("t9")).len(), PER_TABLE, "the most recent table is there");
    assert!(read.entries(&key("t0")).is_empty(), "the oldest is left out");
    assert!(read.table_count() < 10);
}

#[test]
fn clearing_a_table_or_everything() {
    let mut h = History::default();
    h.record(key("orders"), "a = 1", "", 1);
    h.record(key("orders"), "a = 2", "", 2);
    h.record(key("users"), "b = 1", "", 3);
    assert_eq!(h.clear_table(&key("orders")), 2);
    assert_eq!(h.clear_table(&key("orders")), 0);
    assert_eq!(h.entries(&key("users")).len(), 1);
    assert_eq!(h.clear_all(), 1);
    assert_eq!(h, History::default());
    assert_eq!(parse(&h.to_toml()), Ok(History::default()));
}
