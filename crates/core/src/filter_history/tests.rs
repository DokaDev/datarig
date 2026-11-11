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
    assert_eq!(load(&path), Err(Unreadable::NotAFile), "a folder at the path");
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

/// Only ASCII blanks are left out around a text: a space outside ASCII can be part of a name.
#[test]
fn a_space_outside_ascii_stays_in_what_is_kept() {
    let mut h = History::default();
    assert!(h.record(key("t"), " \t active\u{a0} ", " name\u{a0}\n", 1));
    let e = &h.entries(&key("t"))[0];
    assert_eq!((e.filter.as_str(), e.order.as_str()), ("active\u{a0}", "name\u{a0}"));
}

#[test]
fn blank_and_overlong_texts_are_not_kept() {
    let mut h = History::default();
    assert!(!h.record(key("orders"), "  ", "\n", 1));
    assert!(!h.record(key("orders"), &"x".repeat(ENTRY_MAX_BYTES + 1), "", 1));
    assert!(!h.record(key("orders"), "a = 1", &"y".repeat(ENTRY_MAX_BYTES + 1), 1));
    let long_name = TableKey { table: "t".repeat(ENTRY_MAX_BYTES + 1), ..key("orders") };
    assert!(!h.record(long_name, "a = 1", "", 1), "a name the file could not hold");
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

/// A valid file of one entry, as text.
#[cfg(unix)]
fn one_entry() -> String {
    let mut h = History::default();
    h.record(key("orders"), "id = 1", "", 1);
    h.to_toml()
}

/// A symbolic link at the path, to a file or to nothing, is never followed: it cannot be read
/// (never an empty history), a write is refused, and the link and what it points to stay.
#[cfg(unix)]
#[test]
fn a_link_at_the_path_is_never_read_nor_written_through() {
    let d = dir("link");
    let path = d.join(FILE);
    let elsewhere = d.join("elsewhere.toml");
    fs::write(&elsewhere, one_entry()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert_eq!(load(&path), Err(Unreadable::NotAFile), "a link to a valid file");
    assert!(write(&path, "version = 1\n").is_err());
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), one_entry(), "the file it points to is untouched");
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink(), "the link stays");
    fs::remove_file(&path).unwrap();
    let nowhere = d.join("nowhere.toml");
    std::os::unix::fs::symlink(&nowhere, &path).unwrap();
    assert_eq!(load(&path), Err(Unreadable::NotAFile), "a link to nothing is not a missing file");
    assert!(write(&path, "version = 1\n").is_err());
    assert!(fs::symlink_metadata(&nowhere).is_err(), "nothing is created where it points");
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink(), "the link stays");
    let _ = fs::remove_dir_all(&d);
}

/// A FIFO at the path is not opened for reading (which would wait for a writer forever).
#[cfg(unix)]
#[test]
fn a_fifo_at_the_path_is_not_a_file() {
    let d = dir("fifo");
    let path = d.join(FILE);
    let made = std::process::Command::new("mkfifo").arg(&path).status();
    if !made.is_ok_and(|s| s.success()) {
        return; // No mkfifo here: nothing to check.
    }
    assert_eq!(load(&path), Err(Unreadable::NotAFile));
    assert!(write(&path, "version = 1\n").is_err());
    let _ = fs::remove_dir_all(&d);
}

/// Whatever is at the name a write used to put its bytes in first (`.<name>.tmp-<pid>`) is
/// left alone: a link there is not written through, a second name of the file itself is not
/// truncated before the new file replaces it, a file there keeps its bytes.
#[cfg(unix)]
#[test]
fn a_write_never_goes_into_what_is_already_there() {
    let d = dir("planted");
    let path = d.join(FILE);
    write(&path, "version = 1\n").unwrap();
    let planted = d.join(format!(".{FILE}.tmp-{}", std::process::id()));
    let victim = d.join("victim");
    fs::write(&victim, "victim").unwrap();
    std::os::unix::fs::symlink(&victim, &planted).unwrap();
    write(&path, &one_entry()).unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "victim", "not written through the link");
    assert_eq!(fs::read_to_string(&path).unwrap(), one_entry());
    assert!(fs::symlink_metadata(&planted).unwrap().file_type().is_symlink(), "the link is left alone");
    fs::remove_file(&planted).unwrap();
    // A second name of the file: the old bytes stay under it, the new ones come in a new file.
    fs::hard_link(&path, &planted).unwrap();
    write(&path, "version = 1\n").unwrap();
    assert_eq!(fs::read_to_string(&planted).unwrap(), one_entry(), "the old file was not truncated");
    assert_eq!(fs::read_to_string(&path).unwrap(), "version = 1\n");
    fs::remove_file(&planted).unwrap();
    fs::write(&planted, "someone else's").unwrap();
    write(&path, &one_entry()).unwrap();
    assert_eq!(fs::read_to_string(&planted).unwrap(), "someone else's");
    assert_eq!(fs::read_to_string(&path).unwrap(), one_entry());
    let _ = fs::remove_dir_all(&d);
}

/// The file's text of one table `t` with one entry of `filter` (a hand edit can put anything).
fn file_with(table: &str, filter: &str) -> String {
    let s = |v: &str| toml_edit::Value::from(v).to_string();
    format!(
        "version = 1\n[[tables]]\nprofile = \"3f0b8f5e-0b1c-4d2e-9f3a-1b2c3d4e5f60\"\ndatabase = \"d\"\nschema = \"s\"\ntable = {}\n[[tables.entries]]\nwhere = {}\norder = \"\"\nused = 1\n",
        s(table),
        s(filter)
    )
}

/// A text longer than an entry may be (one the app would never keep) makes the file
/// unreadable, read as strictly as the rest: never loaded as it is, never cut.
#[test]
fn a_file_with_an_overlong_text_is_unreadable() {
    let fits = "x".repeat(ENTRY_MAX_BYTES);
    assert_eq!(parse(&file_with("t", &fits)).map(|h| h.table_count()), Ok(1));
    let long = "x".repeat(ENTRY_MAX_BYTES + 1);
    assert_eq!(parse(&file_with("t", &long)), Err(Unreadable::Malformed { line: None }), "a long condition");
    let order = file_with("t", "a = 1").replace("order = \"\"", &format!("order = \"{long}\""));
    assert_eq!(parse(&order), Err(Unreadable::Malformed { line: None }), "a long order");
    assert_eq!(parse(&file_with(&long, "a = 1")), Err(Unreadable::Malformed { line: None }), "a long name");
}

/// One table alone can be larger than the budget (texts of characters written escaped): its
/// oldest entries are left out of the file then.
#[test]
fn a_single_table_stays_within_the_budget() {
    let mut h = History::default();
    let control = "\u{1}".repeat(ENTRY_MAX_BYTES - 2);
    for i in 0..PER_TABLE {
        assert!(h.record(key("orders"), &format!("{control}{i:02}"), &control, i as i64));
    }
    let text = h.to_toml();
    assert!(text.len() <= FILE_BUDGET, "{}", text.len());
    let read = parse(&text).unwrap();
    let kept = read.entries(&key("orders"));
    assert!(!kept.is_empty() && kept.len() < PER_TABLE, "{}", kept.len());
    assert_eq!(kept, &h.entries(&key("orders"))[..kept.len()], "the most recent ones");
}
