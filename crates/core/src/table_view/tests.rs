use super::*;
use std::fs;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("datarig-table-view-{tag}-{}", std::process::id()));
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

fn names(ns: &[&str]) -> Vec<String> {
    ns.iter().map(|n| n.to_string()).collect()
}

#[test]
fn a_missing_file_hides_nothing() {
    let d = dir("missing");
    assert_eq!(load(&d.join(FILE)), Ok(TableView::default()));
    let _ = fs::remove_dir_all(&d);
}

/// A file that is there but cannot be used is never an empty view: each of these is
/// `Unreadable`, and the bytes stay as they were.
#[test]
fn a_file_that_cannot_be_read_hides_nothing_and_is_kept() {
    let d = dir("unreadable");
    let path = d.join(FILE);
    let table = "[[tables]]\nprofile = \"3f0b8f5e-0b1c-4d2e-9f3a-1b2c3d4e5f60\"\ndatabase = \"d\"\nschema = \"s\"\ntable = \"t\"\n";
    let cases: Vec<(String, Unreadable)> = vec![
        ("version = 1\n[[tables]\n".into(), Unreadable::Malformed { line: Some(2) }),
        ("version = \"1\"\n".into(), Unreadable::Malformed { line: None }),
        ("tables = []\n".into(), Unreadable::Malformed { line: None }),
        ("version = 2\n".into(), Unreadable::Newer(2)),
        ("version = 1\nwidths = []\n".into(), Unreadable::Malformed { line: None }),
        (format!("version = 1\n{table}hidden = \"a\"\nused = 1\n"), Unreadable::Malformed { line: None }),
        (format!("version = 1\n{table}hidden = [1]\nused = 1\n"), Unreadable::Malformed { line: None }),
        (format!("version = 1\n{table}hidden = [\"a\"]\n"), Unreadable::Malformed { line: None }),
        (format!("version = 1\n{table}hidden = [\"a\"]\nused = 1\nshown = []\n"), Unreadable::Malformed { line: None }),
        (
            format!("version = 1\n{table}hidden = [\"{}\"]\nused = 1\n", "x".repeat(NAME_MAX_BYTES + 1)),
            Unreadable::Malformed { line: None },
        ),
    ];
    for (text, want) in cases {
        fs::write(&path, &text).unwrap();
        assert_eq!(load(&path), Err(want), "{text}");
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "kept as it was");
    }
    fs::write(&path, vec![b'#'; READ_MAX as usize + 1]).unwrap();
    assert_eq!(load(&path), Err(Unreadable::TooLarge));
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn hidden_names_round_trip_with_any_text() {
    let d = dir("round");
    let path = d.join(FILE);
    let mut v = TableView::default();
    let odd = names(&["memo", "a \"quoted\" name", "line\nbreak", "Ünïcode", "  "]);
    assert!(v.set(key("orders"), &odd, 10));
    assert!(v.set(key("users"), &names(&["id"]), 20));
    write(&path, &v.to_toml()).unwrap();
    let back = load(&path).unwrap();
    assert_eq!(back, v);
    assert_eq!(back.hidden(&key("orders")), odd.as_slice());
    assert_eq!(back.hidden(&key("users")), ["id"]);
    assert!(back.hidden(&key("nope")).is_empty());
    let _ = fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn the_file_is_always_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("mode");
    let path = d.join(FILE);
    fs::write(&path, "version = 1\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    write(&path, &TableView::default().to_toml()).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn names_are_kept_once_within_their_bounds() {
    let mut v = TableView::default();
    let long = "x".repeat(NAME_MAX_BYTES + 1);
    assert!(v.set(key("t"), &names(&["a", "b", "a", &long]), 1));
    assert_eq!(v.hidden(&key("t")), ["a", "b"]);
    assert!(!v.set(key("t"), &names(&["a", "b"]), 2), "the same names: no change");
    let many: Vec<String> = (0..NAMES + 5).map(|i| format!("c{i}")).collect();
    v.set(key("t"), &many, 3);
    assert_eq!(v.hidden(&key("t")).len(), NAMES);
    // Nothing hidden: the table goes.
    assert!(v.set(key("t"), &[], 4));
    assert_eq!(v.table_count(), 0);
    // A table whose name is too long keeps nothing.
    let mut k = key("t");
    k.schema = long;
    assert!(!v.set(k, &names(&["a"]), 5));
    assert_eq!(v.table_count(), 0);
}

#[test]
fn the_table_changed_longest_ago_goes_first() {
    let mut v = TableView::default();
    for i in 0..TABLES + 3 {
        v.set(key(&format!("t{i}")), &names(&["a"]), i as i64);
    }
    assert_eq!(v.table_count(), TABLES);
    for i in 0..3 {
        assert!(v.hidden(&key(&format!("t{i}"))).is_empty(), "t{i} went first");
    }
    assert_eq!(v.hidden(&key(&format!("t{}", TABLES + 2))), ["a"]);
    // A change moves a table to the front: it stays when others come.
    v.set(key("t3"), &names(&["b"]), 1000);
    v.set(key("new"), &names(&["a"]), 1001);
    assert_eq!(v.hidden(&key("t3")), ["b"]);
    assert!(v.hidden(&key("t4")).is_empty());
}

#[test]
fn the_file_stays_within_its_budget_leaving_out_the_oldest_tables() {
    let mut v = TableView::default();
    let wide: Vec<String> = (0..NAMES).map(|i| format!("{}{i}", "n".repeat(200))).collect();
    for i in 0..10 {
        v.set(key(&format!("t{i}")), &wide, i);
    }
    let text = v.to_toml();
    assert!(text.len() <= FILE_BUDGET, "{}", text.len());
    let back = parse(&text).unwrap();
    assert_eq!(back.hidden(&key("t9")).len(), NAMES, "the newest kept");
    assert!(back.hidden(&key("t0")).is_empty(), "the oldest left out");
}

#[test]
fn pruning_keeps_only_the_columns_named() {
    let mut v = TableView::default();
    v.set(key("t"), &names(&["a", "gone", "b"]), 1);
    assert!(!v.prune(&key("other"), &["a"]));
    assert!(v.prune(&key("t"), &["a", "b", "c"]));
    assert_eq!(v.hidden(&key("t")), ["a", "b"]);
    assert!(!v.prune(&key("t"), &["a", "b"]));
    assert!(v.prune(&key("t"), &["c"]));
    assert_eq!(v.table_count(), 0, "nothing left hidden: the table goes");
}

#[cfg(unix)]
#[test]
fn a_link_at_the_path_is_never_read_nor_written_through() {
    let d = dir("link");
    let path = d.join(FILE);
    let target = d.join("elsewhere.toml");
    fs::write(&target, "version = 1\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert_eq!(load(&path), Err(Unreadable::NotAFile));
    let _ = fs::remove_file(&path);
    std::os::unix::fs::symlink(d.join("nothing"), &path).unwrap();
    assert_eq!(load(&path), Err(Unreadable::NotAFile));
    let _ = fs::remove_dir_all(&d);
}
