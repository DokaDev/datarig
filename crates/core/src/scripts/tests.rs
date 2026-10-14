use super::name::*;
use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-scripts-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ── names ────────────────────────────────────────────────────────────────────

#[test]
fn names_get_the_language_extension_and_may_make_folders() {
    assert_eq!(parse_name("daily", SQL), Ok("daily.sql".into()));
    assert_eq!(parse_name("  reports/daily  ", SQL), Ok("reports/daily.sql".into()));
    assert_eq!(parse_name("a/b/c", SQL), Ok("a/b/c.sql".into()));
    assert_eq!(parse_name("daily.sql", SQL), Ok("daily.sql".into()), "a typed .sql is the extension");
    assert_eq!(parse_name("Daily.SQL", SQL), Ok("Daily.sql".into()));
    assert_eq!(parse_name("report.v2", SQL), Ok("report.v2.sql".into()), "other dots belong to the name");
    assert_eq!(parse_name("売上/日別 集計", SQL), Ok("売上/日別 集計.sql".into()), "CJK and inner spaces");
    assert_eq!(parse_name("🐘 elephant", SQL), Ok("🐘 elephant.sql".into()));
}

#[test]
fn names_that_are_not_portable_file_paths_are_refused() {
    assert_eq!(parse_name("", SQL), Err(NameError::Empty));
    assert_eq!(parse_name("   ", SQL), Err(NameError::Empty));
    assert_eq!(parse_name("a//b", SQL), Err(NameError::EmptyPart));
    assert_eq!(parse_name("/a", SQL), Err(NameError::EmptyPart));
    assert_eq!(parse_name("a/", SQL), Err(NameError::EmptyPart));
    assert_eq!(parse_name("../x", SQL), Err(NameError::Dots));
    assert_eq!(parse_name("a/./x", SQL), Err(NameError::Dots));
    assert_eq!(parse_name(".hidden", SQL), Err(NameError::Hidden));
    assert_eq!(parse_name(".sql", SQL), Err(NameError::Hidden));
    assert_eq!(parse_name("a /b", SQL), Err(NameError::Edges), "a folder ending in a space");
    assert_eq!(parse_name("a/ b", SQL), Err(NameError::Edges));
    assert_eq!(parse_name("dir./b", SQL), Err(NameError::Edges), "Windows drops a trailing dot");
    for c in ['<', '>', ':', '"', '\\', '|', '?', '*', '\t', '\u{7}'] {
        assert_eq!(parse_name(&format!("a{c}b"), SQL), Err(NameError::Char(c)), "{c:?}");
    }
    for n in ["CON", "con", "nul", "Aux", "PRN", "COM1", "lpt9", "con.txt", "nul/x", "x/COM3"] {
        assert!(matches!(parse_name(n, SQL), Err(NameError::Reserved(_))), "{n}");
    }
    for n in ["console", "COM0", "COM10", "lpt", "connection"] {
        assert!(parse_name(n, SQL).is_ok(), "{n} is fine");
    }
    assert_eq!(parse_name(&"x".repeat(252), SQL), Err(NameError::TooLong), "with .sql over 255 bytes");
    assert!(parse_name(&"x".repeat(251), SQL).is_ok());
    assert_eq!(parse_folder("a/b/"), Ok("a/b".into()));
    assert_eq!(parse_folder("a/../b"), Err(NameError::Dots));
}

#[test]
fn fold_ignores_case_and_decomposed_hangul() {
    assert_eq!(fold("Daily.SQL"), fold("daily.sql"));
    // U+D55C (a Hangul syllable) = U+1112 + U+1161 + U+11AB (decomposed, how macOS may store it)
    assert_eq!(fold("\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}"), "\u{D55C}\u{AE00}");
    assert_eq!(fold("\u{1112}\u{1161}"), "\u{D558}");
    assert_ne!(fold("\u{D55C}"), fold("\u{D558}"));
}

// ── the store ────────────────────────────────────────────────────────────────

#[test]
fn list_scans_folders_and_scripts_ignoring_other_files() {
    let dir = temp_dir("list");
    let s = ScriptStore::open(&dir);
    assert!(s.list().is_empty(), "no directory yet");
    let root = s.root().to_path_buf();
    fs::create_dir_all(root.join("reports").join("old")).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(root.join("b.sql"), "b").unwrap();
    fs::write(root.join("A.SQL"), "a").unwrap();
    fs::write(root.join("notes.txt"), "no").unwrap();
    fs::write(root.join(".b.sql.tmp-1"), "tmp").unwrap();
    fs::write(root.join("reports").join("daily.sql"), "d").unwrap();
    fs::write(root.join("reports").join("old").join("x.sql"), "x").unwrap();
    let got: Vec<(String, bool)> = s.list().into_iter().map(|e| (e.path, e.folder)).collect();
    assert_eq!(
        got,
        [
            ("empty".into(), true),
            ("reports".into(), true),
            ("reports/old".into(), true),
            ("reports/old/x.sql".into(), false),
            ("reports/daily.sql".into(), false),
            ("A.SQL".into(), false),
            ("b.sql".into(), false),
        ]
    );
    let e = Entry { path: "reports/daily.sql".into(), folder: false, unreadable: false };
    assert_eq!((e.name(), e.depth()), ("daily", 1));
    assert_eq!(stem_path("reports/daily.sql"), "reports/daily");
    assert_eq!(parent("reports/daily.sql"), Some("reports"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn new_names_collide_ignoring_case_and_reuse_existing_folders() {
    let dir = temp_dir("collide");
    let s = ScriptStore::open(&dir);
    fs::create_dir_all(s.root().join("Reports")).unwrap();
    fs::write(s.root().join("Reports").join("Daily.sql"), "d").unwrap();
    assert_eq!(s.resolve_new("reports/daily", None), Err(NameError::Exists("Reports/Daily.sql".into())));
    assert_eq!(s.resolve_new("REPORTS/weekly", None), Ok("Reports/weekly.sql".into()), "the folder's spelling");
    assert_eq!(s.resolve_new("reports/DAILY", Some("Reports/Daily.sql")), Ok("Reports/DAILY.sql".into()), "itself");
    assert_eq!(s.resolve_new("reports", None), Ok("reports.sql".into()), "a folder and a script may share a name");
    assert!(matches!(s.resolve_new("Reports/Daily.sql/x", None), Err(NameError::Exists(_))), "a file is no folder");
    assert_eq!(s.find("reports/daily.SQL"), Ok(Some("Reports/Daily.sql".into())));
    assert_eq!(s.resolve_folder("reports", None), Err(NameError::Exists("Reports".into())));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn saves_check_the_stamp_and_never_overwrite_a_changed_file() {
    let dir = temp_dir("stamp");
    let s = ScriptStore::open(&dir);
    let st = s.create("a/q.sql", "select 1").unwrap();
    assert!(matches!(s.create("A/Q.sql", "x"), Err(SaveError::Exists(_))));
    let (text, read) = s.read("a/q.sql").unwrap();
    assert_eq!((text.as_str(), read), ("select 1", st));
    let st2 = s.save("a/q.sql", "select 2", Some(st)).unwrap();
    // Someone else writes the file.
    fs::write(s.file("a/q.sql"), "select 'theirs'").unwrap();
    assert_eq!(s.save("a/q.sql", "select 3", Some(st2)), Err(SaveError::Conflict));
    assert_eq!(fs::read_to_string(s.file("a/q.sql")).unwrap(), "select 'theirs'", "kept");
    s.save("a/q.sql", "select 3", None).expect("overwrite when asked to");
    let st3 = s.stamp("a/q.sql").unwrap();
    fs::remove_file(s.file("a/q.sql")).unwrap();
    assert_eq!(s.save("a/q.sql", "select 4", Some(st3)), Err(SaveError::Missing));
    let leftovers: Vec<_> = fs::read_dir(s.root().join("a")).unwrap().flatten().collect();
    assert!(leftovers.is_empty(), "no temporary files: {leftovers:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_index_binds_by_profile_id_and_follows_renames() {
    let dir = temp_dir("index");
    let (a, b) = (ProfileId::new(), ProfileId::new());
    let mut s = ScriptStore::open(&dir);
    s.create("r/daily.sql", "select 1").unwrap();
    s.create("r/weekly.sql", "select 2").unwrap();
    s.create("top.sql", "select 3").unwrap();
    s.bind("r/daily.sql", Some(a)).unwrap();
    s.bind("r/weekly.sql", Some(b)).unwrap();
    s.bind("top.sql", Some(a)).unwrap();
    assert_eq!(fs::read_to_string(s.file("r/daily.sql")).unwrap(), "select 1", "the SQL file stays pure");
    let index = fs::read_to_string(dir.join("scripts.toml")).unwrap();
    assert!(index.contains("version = 1") && index.contains(&a.to_string()), "{index}");
    // Reopened: the same bindings.
    let mut s = ScriptStore::open(&dir);
    assert_eq!((s.binding("r/daily.sql"), s.binding("r/weekly.sql")), (Some(a), Some(b)));
    // Rename a script, then move a whole folder: the bindings follow.
    s.rename("top.sql", "r/top.sql").unwrap();
    assert_eq!((s.binding("top.sql"), s.binding("r/top.sql")), (None, Some(a)));
    s.rename("r", "reports").unwrap();
    assert_eq!(s.binding("reports/daily.sql"), Some(a));
    assert_eq!(s.binding("reports/weekly.sql"), Some(b));
    assert_eq!(s.binding("r/daily.sql"), None);
    assert!(s.file("reports/top.sql").is_file());
    // A file removed outside the app: its binding is dropped on the next save of the index.
    fs::remove_file(s.file("reports/weekly.sql")).unwrap();
    s.bind("reports/daily.sql", None).unwrap();
    let s2 = ScriptStore::open(&dir);
    assert_eq!((s2.binding("reports/weekly.sql"), s2.binding("reports/daily.sql")), (None, None));
    assert_eq!(s2.binding("reports/top.sql"), Some(a));
    // Delete.
    let mut s = s2;
    s.delete("reports/top.sql").unwrap();
    assert_eq!(s.binding("reports/top.sql"), None);
    assert!(!s.file("reports/top.sql").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn bindings_survive_a_scripts_directory_that_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unreadable-dir");
    let mut s = ScriptStore::open(&dir);
    let (a, b) = (ProfileId::new(), ProfileId::new());
    s.create("a.sql", "x").unwrap();
    s.create("b.sql", "y").unwrap();
    s.bind("a.sql", Some(a)).unwrap();
    s.bind("b.sql", Some(b)).unwrap();
    fs::set_permissions(s.root(), fs::Permissions::from_mode(0o000)).unwrap();
    let r = s.bind("a.sql", Some(b));
    fs::set_permissions(s.root(), fs::Permissions::from_mode(0o755)).unwrap();
    r.unwrap();
    let s = ScriptStore::open(&dir);
    assert_eq!((s.binding("a.sql"), s.binding("b.sql")), (Some(b), Some(b)), "not taken for gone");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_broken_index_is_backed_up_and_rebuilt_keeping_what_can_be_read() {
    let dir = temp_dir("broken");
    let (a, b, c) = (ProfileId::new(), ProfileId::new(), ProfileId::new());
    let s = ScriptStore::open(&dir);
    for f in ["a.sql", "reports/売上 b.sql", "c.sql"] {
        s.create(f, "x").unwrap();
    }
    let broken = format!(
        "version = 1\n[bindings.\"a.sql\"]\nprofile = \"{a}\"\n@@@ not toml\n[bindings.\"reports/売上 b.sql\"]\nprofile = \"{b}\"\n\
         [bindings.\"gone.sql\"]\nprofile = \"{c}\"\n[bindings]\n\"c.sql\" = {{ profile = \"{c}\" }}\n[bindings.\"a.sql\"\n"
    );
    fs::write(dir.join("scripts.toml"), &broken).unwrap();
    let s = ScriptStore::open(&dir);
    let p = s.index_problem.clone().expect("reported");
    assert_eq!((p.backup, p.kept), (Some(dir.join("scripts.toml.bak")), 3));
    assert_eq!(fs::read_to_string(dir.join("scripts.toml.bak")).unwrap(), broken, "backed up as it was");
    // Rewritten at once, with the bindings of the scripts that exist.
    let s = ScriptStore::open(&dir);
    assert!(s.index_problem.is_none());
    assert_eq!(
        (s.binding("a.sql"), s.binding("reports/売上 b.sql"), s.binding("c.sql"), s.binding("gone.sql")),
        (Some(a), Some(b), Some(c), None)
    );
    // A second broken index does not replace the first backup.
    fs::write(dir.join("scripts.toml"), "not [toml").unwrap();
    let p = ScriptStore::open(&dir).index_problem.unwrap();
    assert_eq!((p.backup, p.kept), (Some(dir.join("scripts.toml.bak.2")), 0));
    assert_eq!(fs::read_to_string(dir.join("scripts.toml.bak")).unwrap(), broken);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn empty_folders_can_be_deleted() {
    let dir = temp_dir("folders");
    let mut s = ScriptStore::open(&dir);
    s.create("a/q.sql", "x").unwrap();
    assert!(!s.folder_empty("a").unwrap());
    assert!(s.delete_folder("a").is_err(), "not empty");
    s.delete("a/q.sql").unwrap();
    assert!(s.folder_empty("a").unwrap());
    s.delete_folder("a").unwrap();
    assert!(s.list().is_empty());
    let _ = fs::remove_dir_all(&dir);
}

// ── what cannot be seen is never taken for "not there" ──────────────────────

/// Running as root ignores permissions: the unreadable-folder tests have nothing to check.
#[cfg(unix)]
fn permissions_apply(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let probe = dir.join("probe");
    fs::create_dir_all(&probe).unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o300)).unwrap();
    let applies = fs::read_dir(&probe).is_err();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir(&probe).unwrap();
    applies
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_an_error_not_an_empty_one() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unreadable");
    let mut s = ScriptStore::open(&dir);
    s.create("d/x.sql", "ORIGINAL precious").unwrap();
    s.create("mine.sql", "select 'mine'").unwrap();
    if !permissions_apply(&dir) {
        return;
    }
    // Write and search, but not list: `d/x.sql` is there and cannot be seen.
    fs::set_permissions(s.root().join("d"), fs::Permissions::from_mode(0o300)).unwrap();
    assert_eq!(s.resolve_new("d/x", None), Err(NameError::Unreadable("d".into())));
    assert_eq!(s.resolve_new("d/other", None), Err(NameError::Unreadable("d".into())));
    assert!(matches!(s.find("d/x.sql"), Err(Unreadable { kind: io::ErrorKind::PermissionDenied, .. })));
    assert!(matches!(s.create("d/x.sql", "clobbered"), Err(SaveError::Io(io::ErrorKind::PermissionDenied, _))));
    assert!(s.folder_empty("d").is_err(), "unknown, not empty");
    assert!(s.rename("mine.sql", "d/x.sql").is_err(), "never over a file that is there");
    fs::set_permissions(s.root().join("d"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(fs::read_to_string(s.file("d/x.sql")).unwrap(), "ORIGINAL precious");
    assert_eq!(fs::read_to_string(s.file("mine.sql")).unwrap(), "select 'mine'");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rename_and_create_never_replace_what_is_there() {
    let dir = temp_dir("noclobber");
    let mut s = ScriptStore::open(&dir);
    s.create("a.sql", "select 'a'").unwrap();
    s.create("b.sql", "select 'b'").unwrap();
    s.create("f/x.sql", "select 'fx'").unwrap();
    s.create("g/y.sql", "select 'gy'").unwrap();
    let e = s.rename("a.sql", "b.sql").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(s.file("b.sql")).unwrap(), "select 'b'", "the target is kept");
    assert_eq!(fs::read_to_string(s.file("a.sql")).unwrap(), "select 'a'", "the source is kept");
    let e = s.rename("f", "g").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "a folder onto a folder");
    assert!(s.file("f/x.sql").is_file() && s.file("g/y.sql").is_file());
    // The same file in another case is itself, not a collision.
    s.rename("a.sql", "A.sql").unwrap();
    assert_eq!(s.find("a.sql"), Ok(Some("A.sql".into())));
    s.rename("A.sql", "c.sql").unwrap();
    assert!(s.file("c.sql").is_file());
    assert!(matches!(s.create("B.SQL", "x"), Err(SaveError::Exists(_))));
    let leftovers: Vec<_> = fs::read_dir(s.root())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "no temporary files: {leftovers:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// A folder is made exclusively (a name taken ignoring case is refused, never
/// taken over), and a folder whose contents cannot be listed is marked, never listed as empty.
#[test]
fn folders_are_made_exclusively_and_unreadable_ones_are_marked() {
    let dir = temp_dir("mkfolder");
    let store = ScriptStore::open(&dir);
    store.create_folder("reports/weekly").unwrap();
    assert!(store.file("reports/weekly").is_dir());
    let e = store.create_folder("REPORTS/Weekly").unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        store.create_folder("locked/inner").unwrap();
        let locked = store.file("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let list = store.list();
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        let l = list.iter().find(|e| e.path == "locked").unwrap();
        assert!(l.folder && l.unreadable, "{list:?}");
        assert!(list.iter().all(|e| e.path == "locked" || !e.unreadable));
        assert!(!list.iter().any(|e| e.path.starts_with("locked/")));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
