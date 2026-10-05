use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-ws-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample() -> WorkspaceState {
    let p = ProfileId::new();
    WorkspaceState {
        active: 1,
        explorer: ExplorerState {
            expanded_folders: vec!["work".into(), "work/prod".into()],
            scripts_expanded: true,
            script_folders: vec!["売上".into()],
        },
        tabs: vec![
            TabState {
                id: new_id(),
                kind: TabKind::Console,
                script: None,
                profile: Some(p),
                cursor: (12, 4),
                top: 3,
                console: 2,
                table: None,
                ddl: None,
                results: Some(PaneState { share: 35, hidden: true, maximized: false }),
                database: Some("sales".into()),
                schema: Some("shop".into()),
            },
            TabState {
                id: new_id(),
                kind: TabKind::Script,
                script: Some("売上/daily.sql".into()),
                profile: None,
                cursor: (0, 0),
                top: 0,
                console: 0,
                table: None,
                ddl: None,
                results: None,
                database: None,
                schema: Some("Sales 2026".into()),
            },
            TabState {
                id: new_id(),
                kind: TabKind::Table,
                script: None,
                profile: Some(p),
                cursor: (0, 0),
                top: 0,
                console: 0,
                table: Some(("shop".into(), "Order Items".into())),
                ddl: None,
                results: Some(PaneState { share: 50, hidden: false, maximized: true }),
                database: None,
                schema: None,
            },
        ],
        unknown_tabs: Vec::new(),
    }
}

#[test]
fn the_state_file_round_trips() {
    let dir = temp_dir("round");
    let l = load(&dir);
    assert!(!l.found && l.broken.is_none() && l.state == WorkspaceState::default(), "a first run");
    let s = sample();
    save(&dir, &s).unwrap();
    let l = load(&dir);
    assert!(l.found && l.broken.is_none());
    assert_eq!(l.state, s);
    let text = fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(text.contains("version = 3") && text.contains("kind = \"script\""), "{text}");
    assert!(text.contains("context = { database = \"sales\", schema = \"shop\" }"), "{text}");
    assert!(text.contains("kind = \"table\"") && text.contains("table = \"Order Items\""), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ddl_tabs_round_trip_with_their_objects() {
    use crate::driver::ddl::DdlObject;
    let dir = temp_dir("ddl");
    let p = ProfileId::new();
    let tab = |object: DdlObject, database: Option<&str>| TabState {
        id: new_id(),
        kind: TabKind::Ddl,
        script: None,
        profile: Some(p),
        cursor: (3, 1),
        top: 0,
        console: 0,
        table: None,
        ddl: Some(object),
        results: None,
        database: database.map(str::to_string),
        schema: None,
    };
    let s = WorkspaceState {
        tabs: vec![
            tab(DdlObject::Relation { schema: "shop".into(), name: "Order Items".into() }, None),
            tab(DdlObject::Index { schema: "shop".into(), name: "users_pkey".into() }, Some("sales")),
            tab(DdlObject::Trigger { schema: "shop".into(), table: "users".into(), name: "touch".into() }, None),
            tab(DdlObject::TriggerFunction { schema: "s".into(), table: "t".into(), trigger: "x".into() }, None),
            tab(DdlObject::Named { name: "\"Mixed\".f(int)".into(), schema: Some("shop".into()) }, None),
            tab(DdlObject::Named { name: "users".into(), schema: None }, None),
        ],
        ..WorkspaceState::default()
    };
    save(&dir, &s).unwrap();
    let text = fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(text.contains("kind = \"ddl\"") && text.contains("object = \"trigger_function\""), "{text}");
    assert_eq!(load(&dir).state, s);
    // Saved again over itself: nothing is lost or doubled.
    save(&dir, &s).unwrap();
    assert_eq!(load(&dir).state, s);
    // A DDL tab whose object cannot be read back is left out, like a table tab without a table.
    fs::write(dir.join(FILE), "version = 3\n[[tabs]]\nkind = \"ddl\"\nobject = \"trigger\"\nschema = \"s\"\n").unwrap();
    assert!(load(&dir).state.tabs.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_broken_state_file_is_moved_aside() {
    let dir = temp_dir("broken");
    fs::write(dir.join(FILE), "active = [").unwrap();
    let l = load(&dir);
    let (_, bak) = l.broken.clone().expect("reported");
    assert_eq!(bak, Some(dir.join(BACKUP)));
    assert_eq!(l.state, WorkspaceState::default());
    assert!(!dir.join(FILE).exists() && dir.join(BACKUP).exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unknown_or_incomplete_tabs_are_skipped_and_bad_ids_replaced() {
    let dir = temp_dir("lenient");
    let text = r#"
version = 1
active = 7
[[tabs]]
kind = "notebook"
[[tabs]]
kind = "script"
[[tabs]]
id = "../../etc/passwd"
kind = "console"
profile = "not-a-uuid"
cursor = [-3]
"#;
    fs::write(dir.join(FILE), text).unwrap();
    let s = load(&dir).state;
    assert_eq!(s.tabs.len(), 1, "{s:?}");
    let t = &s.tabs[0];
    assert!(t.id.chars().all(|c| c.is_ascii_alphanumeric()), "a safe file name: {}", t.id);
    assert_eq!((t.profile, t.cursor), (None, (0, 0)));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_console_listed_twice_opens_in_the_first_tab_only() {
    let dir = temp_dir("dup");
    let tab = |id: &str, kind: &str, extra: &str| format!("[[tabs]]\nid = \"{id}\"\nkind = \"{kind}\"\n{extra}");
    let text = [
        "version = 1\nactive = 0\n".to_string(),
        tab("aaa1", "console", "cursor = [0, 1]\n"),
        tab("aaa1", "script", "script = \"x.sql\"\n"),
        tab("aaa1", "console", "cursor = [0, 2]\n"),
        tab("ccc3", "console", ""),
        tab("ccc3", "console", ""),
    ]
    .concat();
    fs::write(dir.join(FILE), text).unwrap();
    let l = load(&dir);
    assert!(l.broken.is_none());
    assert_eq!(l.duplicates, 2, "later console tabs of an id are left out");
    let tabs: Vec<(&str, TabKind, (usize, usize))> =
        l.state.tabs.iter().map(|t| (t.id.as_str(), t.kind, t.cursor)).collect();
    assert_eq!(
        tabs,
        [("aaa1", TabKind::Console, (0, 1)), ("aaa1", TabKind::Script, (0, 0)), ("ccc3", TabKind::Console, (0, 0))],
        "the first one wins"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn an_unreadable_console_is_an_error_not_an_empty_text() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unreadable-console");
    write_console(&dir, "a1", "select 1").unwrap();
    let path = console_path(&dir, "a1");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let r = read_console(&dir, "a1");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(r.is_err(), "{r:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn console_files_are_written_read_and_only_trashed_when_closed() {
    let dir = temp_dir("consoles");
    write_console(&dir, "a1", "select 1").unwrap();
    write_console(&dir, "b2", "select 2").unwrap();
    assert_eq!(read_console(&dir, "a1").unwrap().as_deref(), Some("select 1"));
    fs::write(dir.join(CONSOLES).join("notes.txt"), "mine").unwrap();
    assert_eq!(orphan_consoles(&dir, &["b2".into()]).unwrap(), ["a1"]);
    // Closing a1 (its tab holds newer text than its file).
    let t = trash_console(&dir, "a1", "select 1 -- edited").unwrap();
    assert_eq!(read_console(&dir, "a1").unwrap(), None, "the console's own file goes");
    assert_eq!(orphan_consoles(&dir, &["b2".into()]).unwrap(), Vec::<String>::new());
    assert_eq!(list_trash(&dir).unwrap(), std::slice::from_ref(&t));
    assert_eq!(fs::read_to_string(dir.join(CONSOLES).join(TRASH).join(&t.name)).unwrap(), "select 1 -- edited");
    // Names are unique and increasing, even within one millisecond and for the same id.
    let now = SystemTime::now();
    let u = trash_console_at(&dir, "a1", "again", now).unwrap();
    let v = trash_console_at(&dir, "a1", "and again", now).unwrap();
    assert!(t.millis < u.millis && u.millis < v.millis, "{t:?} {u:?} {v:?}");
    assert_eq!(
        list_trash(&dir).unwrap().iter().map(|x| x.name.clone()).collect::<Vec<_>>(),
        [v.name.clone(), u.name.clone(), t.name.clone()]
    );
    // Back from the trash: a new console file with the text; the trashed file goes.
    let (id, text) = untrash(&dir, &v.name).unwrap();
    assert_eq!((read_console(&dir, &id).unwrap().as_deref(), text.as_str()), (Some("and again"), "and again"));
    assert_eq!(list_trash(&dir).unwrap().len(), 2);
    assert!(dir.join(CONSOLES).join("notes.txt").exists(), "other files are not touched");
    assert_eq!(read_console(&dir, "b2").unwrap().as_deref(), Some("select 2"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_trash_is_trimmed_only_past_the_newest_files_and_the_age_limit() {
    let dir = temp_dir("trash");
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    let now = SystemTime::now();
    let old = now - day * (TRASH_DAYS as u32 + 10);
    // 59 closed at once (one launch, one moment): nothing is trimmed then, and not at the
    // next launch either while they are young.
    for i in 0..59 {
        trash_console_at(&dir, &format!("n{i:02}"), "x", now).unwrap();
    }
    assert_eq!(list_trash(&dir).unwrap().len(), 59, "trashing never trims");
    assert_eq!(trim_trash(&dir, now).unwrap(), 0, "young files all stay");
    // 59 old ones next to 3 young ones and a file the trash did not name.
    let dir2 = temp_dir("trash-old");
    for i in 0..59 {
        trash_console_at(&dir2, &format!("o{i:02}"), "old", old).unwrap();
    }
    for i in 0..3 {
        trash_console_at(&dir2, &format!("y{i}"), "young", now).unwrap();
    }
    let trash = dir2.join(CONSOLES).join(TRASH);
    fs::write(trash.join("keep-me.txt"), "not ours").unwrap();
    fs::write(trash.join("12-abc.sql"), "not ours either").unwrap();
    assert_eq!(trim_trash(&dir2, now).unwrap(), 12, "62 files, 50 kept by count, the 3 young ones among them");
    let left = list_trash(&dir2).unwrap();
    assert_eq!(left.len(), TRASH_KEEP);
    assert!(left.iter().filter(|t| t.id.starts_with('y')).count() == 3, "the young ones stay");
    let ids: Vec<&str> = left.iter().map(|t| t.id.as_str()).collect();
    assert!(!ids.contains(&"o00") && !ids.contains(&"o11") && ids.contains(&"o12"), "the oldest go first: {ids:?}");
    assert!(trash.join("keep-me.txt").exists() && trash.join("12-abc.sql").exists(), "only its own files");
    assert_eq!(trim_trash(&dir2, now).unwrap(), 0, "the same result the next time");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&dir2);
}

#[cfg(unix)]
#[test]
fn a_consoles_folder_that_cannot_be_read_is_an_error_not_an_empty_one() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("consoles-unreadable");
    write_console(&dir, "a1", "select 1").unwrap();
    trash_console(&dir, "b2", "closed").unwrap();
    let consoles = dir.join(CONSOLES);
    fs::set_permissions(&consoles, fs::Permissions::from_mode(0o300)).unwrap();
    let applies = fs::read_dir(&consoles).is_err();
    let (orphans, trash) = (orphan_consoles(&dir, &[]), list_trash(&dir));
    fs::set_permissions(&consoles, fs::Permissions::from_mode(0o755)).unwrap();
    if applies {
        assert!(orphans.is_err(), "{orphans:?}");
        assert!(trash.is_ok(), "the trash itself is readable");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_older_backup_is_never_replaced() {
    let dir = temp_dir("backups");
    fs::write(dir.join(FILE), "active = [").unwrap();
    assert_eq!(load(&dir).broken.unwrap().1, Some(dir.join(BACKUP)));
    fs::write(dir.join(FILE), "tabs = [").unwrap();
    let second = dir.join(format!("{BACKUP}.2"));
    assert_eq!(load(&dir).broken.unwrap().1, Some(second.clone()));
    assert_eq!(fs::read_to_string(dir.join(BACKUP)).unwrap(), "active = [");
    assert_eq!(fs::read_to_string(second).unwrap(), "tabs = [");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn an_unreadable_state_file_is_moved_aside_not_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unreadable");
    save(&dir, &sample()).unwrap();
    fs::set_permissions(dir.join(FILE), fs::Permissions::from_mode(0o000)).unwrap();
    let l = load(&dir);
    let bak = dir.join(BACKUP);
    let _ = fs::set_permissions(&bak, fs::Permissions::from_mode(0o600));
    let (_, moved) = l.broken.expect("reported");
    assert_eq!(moved, Some(bak.clone()));
    assert!(!dir.join(FILE).exists());
    assert!(fs::read_to_string(bak).unwrap().contains("version = 3"), "kept as it was");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_second_lock_fails_until_the_first_is_released() {
    let dir = temp_dir("lock");
    let first = match acquire(&dir).unwrap() {
        Acquired::Owned(l) => l,
        Acquired::Held { .. } => panic!("the first instance owns it"),
    };
    assert!(!first.pid_only);
    // Windows' file locks are mandatory: while held, no other handle reads the file, so a second
    // instance cannot say who holds it.
    let holder = if cfg!(windows) { None } else { Some(std::process::id()) };
    let pid = fs::read_to_string(dir.join(LOCK)).ok().map(|s| s.trim().to_string());
    assert_eq!(pid, holder.map(|p| p.to_string()));
    match acquire(&dir).unwrap() {
        Acquired::Held { pid } => assert_eq!(pid, holder, "says who holds it"),
        Acquired::Owned(_) => panic!("a second lock must fail"),
    }
    drop(first);
    assert_eq!(fs::read_to_string(dir.join(LOCK)).unwrap().trim(), std::process::id().to_string());
    // Released. (A process another test forks in the meantime shares the file until it runs
    // its program, which closes it; so allow a moment.)
    let owned = (0..100).any(|_| {
        let ok = matches!(acquire(&dir).unwrap(), Acquired::Owned(_));
        if !ok {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        ok
    });
    assert!(owned, "released");
    let _ = fs::remove_dir_all(&dir);
}

/// A process that is gone.
fn dead_pid() -> u32 {
    let mut c = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "exit"]).spawn().unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let pid = c.id();
    c.wait().unwrap();
    pid
}

#[test]
fn a_stale_lock_of_a_crashed_process_is_taken_over() {
    let dir = temp_dir("stale");
    let dead = dead_pid();
    assert!(!pid_alive(dead), "{dead} ended");
    assert!(pid_alive(std::process::id()));
    // The lock file a crashed instance left: its process is gone and nobody holds the lock.
    fs::write(dir.join(LOCK), format!("{dead}\n")).unwrap();
    match acquire(&dir).unwrap() {
        Acquired::Owned(_) => {}
        Acquired::Held { pid } => panic!("stale lock of {pid:?} was not taken over"),
    }
    assert_eq!(fs::read_to_string(dir.join(LOCK)).unwrap().trim(), std::process::id().to_string());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn absurd_times_in_trash_names_are_left_alone_and_never_overflow() {
    let dir = temp_dir("trash-absurd");
    let trash = dir.join(CONSOLES).join(TRASH);
    fs::create_dir_all(&trash).unwrap();
    let absurd = [
        format!("{}-aaa1.sql", u128::MAX),
        format!("{}-aaa2.sql", TRASH_MAX_MILLIS + 1),
        "99999999999999999999999999999999999999999999-aaa3.sql".to_string(),
    ];
    for name in &absurd {
        fs::write(trash.join(name), "kept").unwrap();
    }
    let last = format!("{TRASH_MAX_MILLIS}-aaa4.sql");
    fs::write(trash.join(&last), "the last possible time").unwrap();
    let now = UNIX_EPOCH + std::time::Duration::from_millis(1_790_000_000_000);
    write_console(&dir, "c1", "closed").unwrap();
    let t = trash_console_at(&dir, "c1", "closed", now).expect("no overflow, no panic");
    assert_eq!(t.millis, 1_790_000_000_000, "no later time than the last one: now");
    let listed: Vec<String> = list_trash(&dir).unwrap().into_iter().map(|t| t.name).collect();
    assert_eq!(listed, [last.clone(), t.name.clone()], "absurd names are not the trash's");
    assert_eq!(trim_trash(&dir, now).unwrap(), 0);
    for name in &absurd {
        assert_eq!(fs::read_to_string(trash.join(name)).unwrap(), "kept", "{name} left alone");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A version 1 file (no console numbers, no table tabs, no results pane)
/// reads with the defaults and loses nothing.
#[test]
fn a_version_1_file_reads_with_the_defaults() {
    let dir = temp_dir("v1");
    let p = ProfileId::new();
    let text = format!(
        "version = 1\nactive = 0\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\nprofile = \"{p}\"\ncursor = [1, 2]\ntop = 0\n"
    );
    fs::write(dir.join(FILE), text).unwrap();
    let l = load(&dir);
    assert!(l.broken.is_none());
    let t = &l.state.tabs[0];
    assert_eq!((t.id.as_str(), t.kind, t.profile, t.cursor), ("aaaa", TabKind::Console, Some(p), (1, 2)));
    assert_eq!((t.console, &t.table, t.results), (0, &None, None));
    let _ = fs::remove_dir_all(&dir);
}

/// A tab of a kind this version does not know is kept and written back; a table tab without
/// its table, or a table without a kind, is no tab.
#[test]
fn tabs_of_an_unknown_kind_are_written_back() {
    let dir = temp_dir("unknown");
    let text = "version = 2\nactive = 0\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\n\n\
                [[tabs]]\nid = \"bbbb\"\nkind = \"notebook\"\ncells = [\"a\", \"b\"]\n\n\
                [[tabs]]\nid = \"cccc\"\nkind = \"table\"\nschema = \"s\"\n\n\
                [[tabs]]\nid = \"dddd\"\n";
    fs::write(dir.join(FILE), text).unwrap();
    let l = load(&dir);
    assert_eq!(l.state.tabs.len(), 1);
    assert_eq!(l.state.unknown_tabs.len(), 1);
    save(&dir, &l.state).unwrap();
    let again = load(&dir);
    assert_eq!(again.state.unknown_tabs, l.state.unknown_tabs);
    let written = fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(written.contains("kind = \"notebook\"") && written.contains("cells = [\"a\", \"b\"]"), "{written}");
    let _ = fs::remove_dir_all(&dir);
}

/// Unknown is not absent: every key this version does not know is written back as it was, at
/// the top (a `[layout]` table too), in `[explorer]`, in a tab (found again by its console, its
/// saved query or its table) and in its `results`.
#[test]
fn keys_this_version_does_not_know_are_written_back() {
    let dir = temp_dir("keys");
    let p = ProfileId::new();
    let text = format!(
        "version = 2\nactive = 0\nfuture_top = 7\n\n[layout]\nsplit = \"v\"\n\n[explorer]\nexpanded_folders = []\n\
         future = \"x\"\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\npinned = true\n\
         results = {{ share = 40, hidden = false, maximized = false, split = \"v\" }}\n\n\
         [[tabs]]\nid = \"bbbb\"\nkind = \"script\"\nscript = \"r/daily.sql\"\ncolor = \"red\"\n\n\
         [[tabs]]\nid = \"cccc\"\nkind = \"table\"\nprofile = \"{p}\"\nschema = \"s\"\ntable = \"t\"\n\
         filter = \"id > 3\"\nresults = {{ zoom = 2 }}\n"
    );
    fs::write(dir.join(FILE), &text).unwrap();
    let mut l = load(&dir);
    // A restored saved query's tab gets a new id: it is found by its path.
    l.state.tabs[1].id = new_id();
    l.state.tabs[2].results = None;
    save(&dir, &l.state).unwrap();
    let check = || {
        let written = fs::read_to_string(dir.join(FILE)).unwrap();
        let doc: toml::Table = written.parse().unwrap();
        assert_eq!(doc["version"].as_integer(), Some(3), "{written}");
        assert_eq!(doc["future_top"].as_integer(), Some(7), "{written}");
        assert_eq!(doc["layout"]["split"].as_str(), Some("v"), "{written}");
        assert_eq!(doc["explorer"]["future"].as_str(), Some("x"), "{written}");
        let tabs = doc["tabs"].as_array().unwrap();
        assert_eq!(tabs[0]["pinned"].as_bool(), Some(true), "{written}");
        assert_eq!(tabs[0]["results"]["split"].as_str(), Some("v"), "{written}");
        assert_eq!(tabs[0]["results"]["share"].as_integer(), Some(40), "{written}");
        assert_eq!(tabs[1]["color"].as_str(), Some("red"), "{written}");
        assert_eq!(tabs[2]["filter"].as_str(), Some("id > 3"), "{written}");
        assert_eq!(tabs[2]["results"]["zoom"].as_integer(), Some(2), "{written}");
    };
    check();
    // And again: they stay (a results table without a share reads with the default one).
    let again = load(&dir);
    assert_eq!(again.state.tabs[2].results.map(|r| r.share), Some(DEFAULT_SHARE));
    save(&dir, &again.state).unwrap();
    check();
    let _ = fs::remove_dir_all(&dir);
}

/// A file of a later version is read for what this version knows, and never replaced.
#[test]
fn a_later_versions_file_is_never_written() {
    let dir = temp_dir("newer");
    let text = "version = 4\nactive = 0\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\n";
    fs::write(dir.join(FILE), text).unwrap();
    let l = load(&dir);
    assert_eq!(l.newer, Some(4));
    assert_eq!(l.state.tabs.len(), 1);
    assert!(save(&dir, &l.state).is_err());
    assert_eq!(fs::read_to_string(dir.join(FILE)).unwrap(), text, "untouched");
    assert!(!dir.join(V1_BACKUP).exists());
    let v3 = "version = 3\nactive = 0\n";
    fs::write(dir.join(FILE), v3).unwrap();
    assert_eq!(load(&dir).newer, None);
    let _ = fs::remove_dir_all(&dir);
}

/// The file's `active` counts every tab, those of kinds this version does not know too: the
/// tab it names opens (the one before it when it is not one this version shows).
#[test]
fn active_counts_the_tabs_of_unknown_kinds() {
    let dir = temp_dir("active");
    for (active, want) in [(0, 0), (1, 0), (2, 1), (3, 1), (9, 1)] {
        let text = format!(
            "version = 2\nactive = {active}\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\n\n\
             [[tabs]]\nid = \"xxxx\"\nkind = \"chart\"\n\n[[tabs]]\nid = \"bbbb\"\nkind = \"console\"\n\n\
             [[tabs]]\nid = \"yyyy\"\nkind = \"chart\"\n"
        );
        fs::write(dir.join(FILE), text).unwrap();
        let l = load(&dir);
        assert_eq!(l.state.active, want, "active = {active}");
        assert_eq!(l.state.tabs[l.state.active].id, if want == 0 { "aaaa" } else { "bbbb" });
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A version 1 file is copied to `workspace.toml.v1.bak` before it is first written as version
/// 2, once: a later save, or one when a copy is there already, leaves the copy alone.
#[test]
fn a_version_1_file_is_kept_before_it_is_first_rewritten() {
    let dir = temp_dir("v1bak");
    let v1 = "version = 1\nactive = 0\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\n";
    fs::write(dir.join(FILE), v1).unwrap();
    let l = load(&dir);
    save(&dir, &l.state).unwrap();
    assert_eq!(fs::read_to_string(dir.join(V1_BACKUP)).unwrap(), v1);
    assert!(fs::read_to_string(dir.join(FILE)).unwrap().contains("version = 3"));
    save(&dir, &load(&dir).state).unwrap();
    assert_eq!(fs::read_to_string(dir.join(V1_BACKUP)).unwrap(), v1, "once");
    // Another version 1 file with a copy there already: the copy stays as it is.
    let other = "version = 1\nactive = 0\n";
    fs::write(dir.join(FILE), other).unwrap();
    save(&dir, &load(&dir).state).unwrap();
    assert_eq!(fs::read_to_string(dir.join(V1_BACKUP)).unwrap(), v1, "never replaced");
    // A file without a version is a version 1 file.
    let _ = fs::remove_file(dir.join(V1_BACKUP));
    fs::write(dir.join(FILE), "active = 0\n").unwrap();
    save(&dir, &load(&dir).state).unwrap();
    assert_eq!(fs::read_to_string(dir.join(V1_BACKUP)).unwrap(), "active = 0\n");
    let _ = fs::remove_dir_all(&dir);
}

/// Version 3 keeps a query tab's database and schema. A version 2 file reads with
/// the profile's defaults and is copied to `workspace.toml.v2.bak` (once) before it is first
/// written as version 3; keys of `context` this version does not know are written back, also
/// when the tab went back to the defaults.
#[test]
fn a_version_2_file_is_kept_and_contexts_round_trip() {
    let dir = temp_dir("v2bak");
    let v2 = "version = 2\nactive = 0\n\n[[tabs]]\nid = \"aaaa\"\nkind = \"console\"\n";
    fs::write(dir.join(FILE), v2).unwrap();
    let mut l = load(&dir);
    assert_eq!((l.state.tabs[0].database.clone(), l.state.tabs[0].schema.clone()), (None, None));
    l.state.tabs[0].schema = Some("shop".into());
    save(&dir, &l.state).unwrap();
    assert_eq!(fs::read_to_string(dir.join(backup_of(2))).unwrap(), v2);
    let l = load(&dir);
    assert_eq!(l.state.tabs[0].schema.as_deref(), Some("shop"));
    assert_eq!(l.state.tabs[0].database, None);
    // A later version's key inside `context` stays, with the tab's context or without one.
    let text =
        fs::read_to_string(dir.join(FILE)).unwrap().replace("schema = \"shop\"", "schema = \"shop\", role = \"ro\"");
    fs::write(dir.join(FILE), &text).unwrap();
    let mut l = load(&dir);
    save(&dir, &l.state).unwrap();
    let doc: toml::Table = fs::read_to_string(dir.join(FILE)).unwrap().parse().unwrap();
    assert_eq!(doc["tabs"][0]["context"]["role"].as_str(), Some("ro"));
    l.state.tabs[0].schema = None;
    save(&dir, &l.state).unwrap();
    let doc: toml::Table = fs::read_to_string(dir.join(FILE)).unwrap().parse().unwrap();
    let ctx = doc["tabs"][0]["context"].as_table().unwrap();
    assert_eq!((ctx.get("role").and_then(|v| v.as_str()), ctx.get("schema")), (Some("ro"), None));
    assert_eq!(fs::read_to_string(dir.join(backup_of(2))).unwrap(), v2, "the copy is made once");
    let _ = fs::remove_dir_all(&dir);
}
