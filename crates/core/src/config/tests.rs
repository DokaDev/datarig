use super::*;
use crate::profile::color::ProfileColor;
use crate::secret::{DefaultSource, PasswordSource, SourceKind};

#[test]
fn debug_never_leaks_password() {
    let mut c = ConnectionConfig::test_db();
    c.password = "hunter2".into();
    let out = format!("{c:?}");
    assert!(!out.contains("hunter2"), "{out}");
    assert!(out.contains("<redacted>"), "{out}");

    c.password.clear();
    assert!(format!("{c:?}").contains("<unset>"));

    // A raw DSN string can also carry `user:<password>@host` (kept as-is when it has extra
    // parameters); that must be masked too.
    c.dsn = Some("postgres://u:hunter2@h/db?application_name=x".into());
    let out = format!("{c:?}");
    assert!(!out.contains("hunter2"), "{out}");
    assert!(out.contains("<redacted>"), "{out}");
}

#[test]
fn parses_spec_example() {
    let cfg = parse(
        r#"
language = "ko"
page_size = 200
[[connections]]
name = "local-pg"
driver = "postgres"
host = "127.0.0.1"
port = 55432
user = "datarig"
password = "datarig"
database = "datarig"
sslmode = "disable"
"#,
    )
    .unwrap();
    assert_eq!(cfg.language, "ko");
    assert_eq!(cfg.page_size, 200);
    assert_eq!(cfg.connections[0].port, 55432);
    assert_eq!(cfg.connections[0].origin.as_deref(), Some("local-pg"));
    assert_eq!(cfg.version, 1, "no `version`: the v0.6 format");
    assert!(cfg.ids_assigned, "the profile got an id in memory");
}

#[test]
fn empty_file_has_no_profiles() {
    // No built-in test DB any more: zero profiles is a real state.
    let cfg = parse("").unwrap();
    assert!(cfg.connections.is_empty());
    assert_eq!(cfg.page_size, 500);
    assert!(Config::default().connections.is_empty());
    assert_eq!(Config::default().version, CONFIG_VERSION, "a new file is written as version 2");
}

#[test]
fn errors_are_reported() {
    assert!(parse("language = \"fr\"").is_err());
    assert!(parse("page_size = 0").is_err());
    assert!(parse("[[connections]]\nport = \"x\"").is_err());
}

#[test]
fn url_dsn_becomes_fields_other_dsn_kept() {
    let cfg = parse(
        "[[connections]]\nname = \"a\"\ndsn = \"postgres://u:p%40w@[::1]/db?sslmode=require\"\n\
             [[connections]]\nname = \"b\"\ndsn = \"postgres://h/db?application_name=x\"\n\
             [[connections]]\nname = \"c\"\ndsn = \"host=h port=1\"\n",
    )
    .unwrap();
    let a = &cfg.connections[0];
    assert_eq!((a.host.as_str(), a.port, a.user.as_str(), a.password.as_str()), ("::1", 5432, "u", "p@w"));
    assert_eq!((a.database.as_str(), a.sslmode.as_str(), a.dsn.as_deref()), ("db", "require", None));
    assert!(cfg.connections[1].dsn.is_some());
    assert_eq!(cfg.connections[2].dsn.as_deref(), Some("host=h port=1"));
    assert_eq!(a.display_dsn(), "postgres://u@[::1]:5432/db?sslmode=require");
    assert_eq!(a.endpoint(), ("[::1]:5432".into(), "db".into(), "require".into()));
    assert_eq!(cfg.connections[1].endpoint(), ("h".into(), "db".into(), String::new()));
    assert_eq!(cfg.connections[2].endpoint(), ("host=h port=1".into(), String::new(), String::new()));
}

#[test]
fn mysql_url_dsn_becomes_fields_of_a_mysql_profile_only() {
    let cfg = parse(
        "[[connections]]\nname = \"a\"\ndriver = \"mysql\"\ndsn = \"mysql://u:pw@db.example/shop\"\n\
             [[connections]]\nname = \"b\"\ndriver = \"mysql\"\ndsn = \"mysql://h/db?sslmode=require\"\n\
             [[connections]]\nname = \"c\"\ndsn = \"mysql://h/db\"\n\
             [[connections]]\nname = \"d\"\ndriver = \"mysql\"\ndsn = \"postgres://h/db\"\n",
    )
    .unwrap();
    let a = &cfg.connections[0];
    assert_eq!((a.host.as_str(), a.port, a.user.as_str(), a.password.as_str()), ("db.example", 3306, "u", "pw"));
    assert_eq!((a.database.as_str(), a.dsn.as_deref()), ("shop", None));
    assert_eq!(a.display_dsn(), "mysql://u@db.example:3306/shop");
    // MySQL has no `sslmode`; a URL of another database stays as it was written (the driver
    // says what is wrong with it).
    for c in &cfg.connections[1..] {
        assert!(c.dsn.is_some(), "{}", c.name);
    }
}

#[test]
fn xdg_path_precedence() {
    let p = default_path(|k| match k {
        "XDG_CONFIG_HOME" => Some("/x".into()),
        "HOME" => Some("/h".into()),
        _ => None,
    });
    assert_eq!(p.unwrap(), Path::new("/x").join("datarig").join("config.toml"));
    let p = default_path(|k| (k == "HOME").then(|| "/h".to_string()));
    assert_eq!(p.unwrap(), Path::new("/h").join(".config").join("datarig").join("config.toml"));
    let p = default_path(|k| (k == "USERPROFILE").then(|| "C:/u".to_string()));
    assert_eq!(p.unwrap(), Path::new("C:/u").join(".config").join("datarig").join("config.toml"));
}

fn settings(language: &str) -> Settings<'_> {
    Settings {
        version: 1,
        language,
        icons: IconsSetting::Auto,
        theme: crate::theme::DEFAULT,
        default_source: DefaultSource::Auto,
        prefs: Prefs::default(),
    }
}

fn profiles<'a>(c: &'a [ConnectionConfig], folders: &'a Folders, last_used: Option<ProfileId>) -> Profiles<'a> {
    Profiles { connections: c, tunnels: &[], environments: None, default_rules: None, folders, last_used }
}

/// `<tmp>/datarig-cfg-<pid>-<tag>/sub/config.toml`, not created; the directory is removed when
/// the value is dropped, so a failing test leaves nothing behind either.
struct TempFile {
    dir: PathBuf,
    path: PathBuf,
}

impl std::ops::Deref for TempFile {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.path
    }
}

impl AsRef<Path> for TempFile {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_file(tag: &str) -> TempFile {
    let dir = std::env::temp_dir().join(format!("datarig-cfg-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("sub").join("config.toml");
    TempFile { dir, path }
}

#[test]
fn save_preserves_comments_and_drops_passwords() {
    let path = temp_file("save");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "# my config\nlanguage = \"auto\"      # auto | en | ko\npage_size = 500 # rows\n\n\
             [[connections]]\nname = \"local-pg\" # main\ndriver = \"postgres\"\nhost = \"127.0.0.1\"\n\
             port = 55432\nuser = \"datarig\"\npassword = \"datarig\" # plaintext\ndatabase = \"datarig\"\n\
             sslmode = \"disable\"\n";
    std::fs::write(&path, original).unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none());
    cfg.connections[0].password.clear(); // moved to the keychain
    let extra = ConnectionConfig { name: "prod".into(), host: "db.example.com".into(), ..ConnectionConfig::default() };
    let prod = extra.id;
    cfg.connections.push(extra);
    save(&path, settings("ko"), Some(profiles(&cfg.connections, &cfg.folders, Some(prod)))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# my config\n"), "{text}");
    assert!(text.contains("language = \"ko\"      # auto | en | ko"), "{text}");
    assert!(text.contains("page_size = 500 # rows"), "{text}");
    assert!(text.contains("name = \"local-pg\" # main"), "{text}");
    assert!(!text.contains("password"), "{text}");
    assert!(text.contains(&format!("last_used = \"{prod}\"\n")), "{text}");
    assert!(
        text.contains(&format!("[[connections]]\nid = \"{}\"\nname = \"local-pg\" # main", cfg.connections[0].id)),
        "{text}"
    );
    let (again, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(again.language, "ko");
    assert_eq!(again.last_used, Some(prod));
    assert_eq!(again.connections[0].id, cfg.connections[0].id, "the id is kept");
    assert!(!again.ids_assigned);
    save(&path, settings("ko"), Some(profiles(&again.connections, &again.folders, None))).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("last_used"));
    assert_eq!(again.connections.len(), 2);
    assert_eq!(again.connections[1].host, "db.example.com");
    assert_eq!(again.connections[0].port, 55432);
}

#[test]
fn save_creates_missing_file_with_language_only() {
    let path = temp_file("create");
    save(&path, settings("ko"), None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.trim(), "language = \"ko\"");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none());
    assert!(cfg.connections.is_empty());
    // English is the default: a new file does not get the key.
    let path = temp_file("create-en");
    save(&path, settings("en"), None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "");
}

#[test]
fn save_refuses_unparseable_file() {
    let path = temp_file("broken");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "language = \n").unwrap();
    assert!(save(&path, settings("en"), None).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "language = \n", "left untouched");
}

#[test]
fn keymap_is_read() {
    assert!(parse("").unwrap().keymap.is_empty());
    let cfg = parse(
        "[keymap.explorer]\n\"x\" = \"conn.disconnect\"\n\"ctrl+w\" = \"none\"\n\
         [keymap.nav]\n\"space t d\" = \"tab.close\"\n",
    )
    .unwrap();
    assert_eq!(cfg.keymap["explorer"]["x"], "conn.disconnect");
    assert_eq!(cfg.keymap["explorer"]["ctrl+w"], "none");
    assert_eq!(cfg.keymap["nav"]["space t d"], "tab.close");
    // Content is not validated by core (unknown contexts and actions are the TUI's business),
    // but the shape is: an unknown [editor] key or a non-string action fail.
    assert!(parse("[keymap.nowhere]\n\"q\" = \"no.such.action\"\n").is_ok());
    assert!(parse("[editor]\nmodee = \"vim\"\n").is_err());
    assert!(parse("[keymap.explorer]\n\"x\" = 1\n").is_err());
}

/// `[editor] mode` (the retired choice between vim and standard keys) loads with any value,
/// changes nothing and is gone after the next save; the comments and keys around it stay.
#[test]
fn the_retired_editor_mode_is_ignored_and_dropped_on_save() {
    for value in ["\"vim\"", "\"standard\"", "\"bogus\"", "3"] {
        let cfg = parse(&format!("[editor]\nmode = {value}\n")).unwrap();
        assert_eq!(cfg.prefs, Prefs::default(), "{value}");
    }
    let cfg = parse("[editor]\nmode = \"standard\"\ncursor_shape = \"off\"\n").unwrap();
    assert_eq!(cfg.prefs.cursor_shape, CursorShape::Off, "the other keys of the table still count");
    assert!(parse("[editor]\nmode = \"vim\"\nshape = \"off\"\n").is_err(), "unknown keys still fail");

    let path = temp_file("retired_mode");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let cases = [
        // The table it leaves empty goes.
        ("language = \"ko\"\n\n[editor]\nmode = \"standard\"\n", "language = \"ko\"\n"),
        ("editor = { mode = \"vim\" }\nlanguage = \"ko\"\n", "language = \"ko\"\n"),
        // Comments and the order of the rest survive.
        (
            "language = \"ko\"\n\n[editor]\n# keys\nmode = \"bogus\" # old\ncursor_shape = \"off\" # bar\n\n\
             [commands]\nposition = \"bottom\"\n",
            "language = \"ko\"\n\n[editor]\n# keys\n# old\ncursor_shape = \"off\" # bar\n\n\
             [commands]\nposition = \"bottom\"\n",
        ),
        (
            "[editor] # mine\nmode = \"vim\"\n\n[keymap.explorer]\n\"x\" = \"explorer.refresh\" # keep\n",
            "[editor] # mine\n\n[keymap.explorer]\n\"x\" = \"explorer.refresh\" # keep\n",
        ),
        ("editor = { mode = \"vim\", cursor_shape = \"off\" }\n", "editor = { cursor_shape = \"off\" }\n"),
    ];
    for (before, after) in cases {
        std::fs::write(&path, before).unwrap();
        let (cfg, err) = load(Some(path.clone()));
        assert!(err.is_none(), "{err:?}");
        let s = Settings { language: &cfg.language, prefs: cfg.prefs, ..settings("en") };
        save(&path, s, None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after, "{before}");
        let (_, err) = load(Some(path.clone()));
        assert!(err.is_none(), "{err:?}");
    }
}

const ID_A: &str = "3f0b8f5e-6a57-4f7e-9a53-0c1c2b8f9d11";
const ID_B: &str = "7d7c0a5e-1111-4a2b-8c3d-000000000002";

#[test]
fn version_2_keys_are_read_and_checked() {
    let cfg = parse(&format!(
        "version = 2\nicons = \"on\"\nlast_used = \"{ID_A}\"\nfolders = [\"work\", \"local/empty\"]\n\
         [[connections]]\nid = \"{ID_A}\"\nname = \"shard-1\"\nfolder = \"work/prod\"\ncolor = \"red\"\n\
         icon = \"database\"\npolicy = \"default\"\n\
         [[connections]]\nid = \"{ID_B}\"\nname = \"b\"\ncolor = \"#00ff7f\"\n"
    ))
    .unwrap();
    assert_eq!((cfg.version, cfg.icons, cfg.ids_assigned), (2, IconsSetting::On, false));
    let a = &cfg.connections[0];
    assert_eq!(a.id, ProfileId::parse(ID_A).unwrap());
    assert_eq!(cfg.last_used, Some(a.id));
    assert_eq!(
        (a.folder.as_deref(), a.icon.as_deref(), a.policy.as_deref()),
        (Some("work/prod"), Some("database"), Some("default"))
    );
    assert_eq!(a.display_color(), ProfileColor::parse("red").unwrap());
    assert_eq!(cfg.connections[1].display_color(), ProfileColor::Hex(0, 0xff, 0x7f));
    let folders: Vec<&str> = cfg.folders.iter().map(FolderPath::as_str).collect();
    assert_eq!(folders, ["local", "local/empty", "work", "work/prod"], "a profile's folder is added");
    // deny_unknown_fields still catches typos, at the top level and in a profile.
    assert!(parse("versoin = 2\n").is_err());
    assert!(parse("[[connections]]\nname = \"a\"\nfolderr = \"x\"\n").is_err());
    // Bad values are config errors.
    for (body, needle) in [
        ("[[connections]]\nid = \"nope\"\n".to_string(), "not a UUID"),
        ("[[connections]]\nid = \"00000000-0000-0000-0000-000000000000\"\n".to_string(), "not a UUID"),
        (format!("[[connections]]\nid = \"{ID_A}\"\n[[connections]]\nid = \"{ID_A}\"\n"), "DuplicateId"),
        ("[[connections]]\nfolder = \"a//b\"\n".to_string(), "folder"),
        ("folders = [\"..\"]\n".to_string(), "folders"),
        ("[[connections]]\ncolor = \"reddish\"\n".to_string(), "color"),
        ("icons = \"maybe\"\n".to_string(), "icons"),
        ("version = 3\n".to_string(), "Version(3)"),
    ] {
        let e = parse(&body).unwrap_err();
        assert!(format!("{e:?}").contains(needle), "{body}: {e:?}");
    }
}

#[test]
fn ids_are_assigned_and_last_used_names_resolve() {
    let cfg = parse("last_used = \"b\"\n[[connections]]\nname = \"a\"\n[[connections]]\nname = \"b\"\n").unwrap();
    assert!(cfg.ids_assigned);
    assert_ne!(cfg.connections[0].id, cfg.connections[1].id);
    assert_eq!(cfg.last_used, Some(cfg.connections[1].id), "a v1 name becomes the id");
    let cfg = parse(&format!("last_used = \"{ID_B}\"\n[[connections]]\nid = \"{ID_A}\"\nname = \"a\"\n")).unwrap();
    assert_eq!(cfg.last_used, None, "an id of no profile");
}

/// `auto` (the old default, or no `icons` key) is "not decided yet": text marks
/// until the user answers the app's question, whatever the terminal is called. The answer is
/// saved in place of the old value, comments, order and unknown keys kept.
#[test]
fn icons_auto_is_not_decided_yet_and_the_answer_replaces_it() {
    assert!(!IconsSetting::Auto.on() && IconsSetting::On.on() && !IconsSetting::Off.on());
    assert_eq!(parse("").unwrap().icons, IconsSetting::Auto, "no key: not decided");
    assert_eq!(parse("icons = \"auto\"").unwrap().icons, IconsSetting::Auto);
    assert_eq!(parse("icons = \"on\"").unwrap().icons, IconsSetting::On, "kept as it is");
    assert_eq!(parse("icons = \"off\"").unwrap().icons, IconsSetting::Off);
    let path = temp_file("icons-answer");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let before = "# mine\nicons = \"auto\" # the old default\nmystery = 1\n\n[keymap.global]\n\"ctrl+y\" = \"ui.icons.toggle\"\n";
    std::fs::write(&path, before).unwrap();
    save(&path, Settings { icons: IconsSetting::On, ..settings(DEFAULT_LANGUAGE) }, None).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, before.replace("icons = \"auto\"", "icons = \"on\""), "{after}");
}

#[test]
fn theme_is_one_top_level_name_written_only_when_not_the_default() {
    assert_eq!(parse("").unwrap().theme, "terminal", "no key: the default");
    assert_eq!(parse("theme = \"catppuccin\"").unwrap().theme, "catppuccin");
    // Any name is read: the UI resolves it (an unknown one never makes the file unusable).
    assert_eq!(parse("theme = \"no-such-theme\"").unwrap().theme, "no-such-theme");
    assert!(parse("theme = 3").is_err(), "not a name");
    let path = temp_file("theme");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let before = "# mine\nlanguage = \"ko\"\n";
    std::fs::write(&path, before).unwrap();
    save(&path, Settings { theme: "terminal", ..settings("ko") }, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "the default is not written");
    save(&path, Settings { theme: "gruvbox-light", ..settings("ko") }, None).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, format!("{before}theme = \"gruvbox-light\"\n"));
    assert_eq!(load(Some(path.to_path_buf())).0.theme, "gruvbox-light");
    save(&path, Settings { theme: "terminal", ..settings("ko") }, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "back to the default: the key goes");
}

#[test]
fn save_matches_tables_by_id_then_by_name_and_writes_version_2_keys() {
    let path = temp_file("v2");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# top\n[[connections]] # first\nname = \"a\" # keep\nhost = \"h\"\n\n[[connections]]\nname = \"b\"\n",
    )
    .unwrap();
    let (mut cfg, _) = load(Some(path.clone()));
    let (a, b) = (cfg.connections[0].id, cfg.connections[1].id);
    // First save after the migration: tables found by name, ids added first.
    cfg.connections[0].name = "a-renamed".into();
    cfg.connections[0].color = Some("teal".into());
    cfg.connections[0].folder = Some("work/prod".into());
    cfg.folders.insert(&FolderPath::parse("work/prod").unwrap());
    let v2 = Settings { version: 2, icons: IconsSetting::Off, ..settings("auto") };
    save(&path, v2, Some(profiles(&cfg.connections, &cfg.folders, Some(b)))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    // The comment belongs to the first table header; new top-level keys go above it.
    assert!(text.contains("# top\n[[connections]] # first\n"), "{text}");
    assert!(text.contains("version = 2") && text.contains("icons = \"off\""), "{text}");
    assert!(text.contains("folders = [\"work\", \"work/prod\"]"), "{text}");
    assert!(
        text.contains(&format!("[[connections]] # first\nid = \"{a}\"\nname = \"a-renamed\" # keep\nhost = \"h\"")),
        "{text}"
    );
    assert!(text.contains("color = \"teal\"") && text.contains("folder = \"work/prod\""), "{text}");
    // Later saves find the table by id (the name in the file no longer matches `origin`).
    let (mut again, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!((again.connections[0].id, again.connections[1].id, again.version), (a, b, 2));
    again.connections.swap(0, 1);
    again.connections[1].name = "renamed-again".into();
    again.connections[1].origin = Some("something else".into());
    again.connections[1].color = None;
    save(&path, v2, Some(profiles(&again.connections, &again.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("name = \"renamed-again\" # keep"), "matched by id, comment kept: {text}");
    assert!(!text.contains("color"), "{text}");
    let (third, _) = load(Some(path.clone()));
    assert_eq!(third.connections.iter().map(|c| c.id).collect::<Vec<_>>(), [b, a]);
}

#[test]
fn missing_and_broken_files_need_no_migration() {
    let path = temp_file("nomigrate");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_some(), "an explicit --config that does not exist is an error");
    assert!(!cfg.needs_migration() && cfg.path.is_none());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[[connections]]\nname = \"a\"\npassword = \"pw\"\nnope = 1\n").unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_some() && !cfg.needs_migration(), "broken: nothing is migrated");
    std::fs::write(&path, "version = 2\n").unwrap();
    assert!(!load(Some(path.clone())).0.needs_migration(), "up to date");
    std::fs::write(&path, "language = \"en\"\n").unwrap();
    assert!(load(Some(path.clone())).0.needs_migration(), "version 1");
    std::fs::write(&path, "version = 2\n[[connections]]\nname = \"new\"\n").unwrap();
    assert!(load(Some(path.clone())).0.needs_migration(), "a profile without id");
}

#[test]
fn password_sources_are_read_and_checked() {
    let cfg = parse(
        "[secrets]\ndefault_source = \"file\"\n\
         [[connections]]\nname = \"kc\"\n\
         [[connections]]\nname = \"f\"\npassword_source = \"file\"\n\
         [[connections]]\nname = \"c\"\npassword_source = \"command\"\npassword_command = \"op read op://v/db/pw\"\n\
         [[connections]]\nname = \"e\"\npassword_source = \"env\"\npassword_env = \"PROD_DB_PW\"\n\
         [[connections]]\nname = \"p\"\npassword_source = \"prompt\"\n",
    )
    .unwrap();
    assert_eq!(cfg.default_source, DefaultSource::Kind(SourceKind::File));
    let sources: Vec<PasswordSource> = cfg.connections.iter().map(ConnectionConfig::source).collect();
    assert_eq!(
        sources,
        [
            PasswordSource::Keychain,
            PasswordSource::File,
            PasswordSource::Command("op read op://v/db/pw".into()),
            PasswordSource::Env("PROD_DB_PW".into()),
            PasswordSource::Prompt,
        ]
    );
    assert_eq!(parse("").unwrap().default_source, DefaultSource::Auto);
    for (bad, what) in [
        ("[secrets]\ndefault_source = \"vault\"\n", "secrets.default_source"),
        ("[secrets]\ndefault = \"file\"\n", "unknown field"),
        ("[[connections]]\nname = \"x\"\npassword_source = \"vault\"\n", "password_source"),
        ("[[connections]]\nname = \"x\"\npassword_source = \"command\"\n", "Missing { key: \"password_command\""),
        (
            "[[connections]]\nname = \"x\"\npassword_source = \"command\"\npassword_command = \" \"\n",
            "password_command",
        ),
        ("[[connections]]\nname = \"x\"\npassword_source = \"env\"\n", "Missing { key: \"password_env\""),
        (
            "[[connections]]\nname = \"x\"\npassword_source = \"env\"\npassword_env = \"DB-PW\"\n",
            "key: \"password_env\", value: \"DB-PW\"",
        ),
    ] {
        let e = parse(bad).unwrap_err();
        assert!(format!("{e:?}").contains(what), "{bad}: {e:?}");
    }
    // A command or env profile never uses a legacy plaintext password: nothing to migrate.
    let mut c = parse("version = 2\n[[connections]]\nid = \"3f0b8f5e-6a57-4f7e-9a53-0c1c2b8f9d11\"\nname = \"c\"\npassword = \"x\"\npassword_source = \"env\"\npassword_env = \"E\"\n").unwrap();
    c.path = Some(PathBuf::from("/nowhere/config.toml"));
    c.exists = true;
    assert!(!c.needs_migration());
    c.connections[0].set_source(PasswordSource::File);
    assert!(c.needs_migration(), "a file profile's plaintext moves to the secrets file");
}

#[test]
fn save_writes_the_source_keys_and_default_source() {
    let path = temp_file("sources");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[[connections]]\nname = \"a\" # mine\nhost = \"h\"\n").unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    // The keychain (the default) writes no key.
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("password") && !text.contains("secrets"), "{text}");
    cfg.connections[0].set_source(PasswordSource::Command("pass show db".into()));
    let with_default = Settings { default_source: DefaultSource::Kind(SourceKind::Prompt), ..settings("auto") };
    save(&path, with_default, Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("name = \"a\" # mine\n"), "{text}");
    assert!(text.contains("password_source = \"command\"\npassword_command = \"pass show db\""), "{text}");
    assert!(text.contains("[secrets]\ndefault_source = \"prompt\""), "{text}");
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(back.connections[0].source(), PasswordSource::Command("pass show db".into()));
    assert_eq!(back.default_source, DefaultSource::Kind(SourceKind::Prompt));
    // Switching to env drops the command; back to the keychain drops every source key.
    cfg.connections[0].set_source(PasswordSource::Env("DB_PW".into()));
    save(&path, with_default, Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("password_env = \"DB_PW\"") && !text.contains("password_command"), "{text}");
    cfg.connections[0].set_source(PasswordSource::Keychain);
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("password_"), "{text}");
    assert!(!text.contains("default_source") && !text.contains("[secrets]"), "back to the default: gone: {text}");
}

#[test]
fn macos_keychain_access_is_read_written_and_checked() {
    assert_eq!(parse("").unwrap().prefs.keychain_access, KeychainAccess::Direct, "the default");
    let cfg = parse("[secrets]\nmacos_keychain_access = \"security_tool\"\n").unwrap();
    assert_eq!(cfg.prefs.keychain_access, KeychainAccess::SecurityTool);
    assert_eq!(cfg.default_source, DefaultSource::Auto, "the other key keeps its default");
    let direct = parse("[secrets]\ndefault_source = \"file\"\nmacos_keychain_access = \"direct\"\n").unwrap();
    assert_eq!(direct.prefs.keychain_access, KeychainAccess::Direct);
    let e = parse("[secrets]\nmacos_keychain_access = \"cli\"\n").unwrap_err();
    assert_eq!(
        e,
        ConfigError::Value {
            key: "secrets.macos_keychain_access".into(),
            value: "cli".into(),
            profile: None,
            allowed: Some("direct, security_tool"),
        }
    );
    // Written only when it is not the default; comments and the other key stay.
    let path = temp_file("keychain-access");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# mine\n[secrets] # passwords\ndefault_source = \"prompt\"\n").unwrap();
    let tool = Prefs { keychain_access: KeychainAccess::SecurityTool, ..Prefs::default() };
    let with = |prefs| Settings { default_source: DefaultSource::Kind(SourceKind::Prompt), prefs, ..settings("en") };
    save(&path, with(tool), None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(
            "# mine\n[secrets] # passwords\ndefault_source = \"prompt\"\nmacos_keychain_access = \"security_tool\"\n"
        ),
        "{text}"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(back.prefs.keychain_access, KeychainAccess::SecurityTool);
    save(&path, with(Prefs::default()), None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("macos_keychain_access") && text.contains("default_source = \"prompt\""), "{text}");
}

#[test]
fn policy_tables_are_read() {
    let cfg = parse(
        r#"
[policy.careful]
paging_idle_timeout = "10s"

[policy.local]
paging = "hold"
paging_idle_timeout = "off"

[policy.plain]

[policy.prod]
read_only = true
confirm = "writes"
"#,
    )
    .unwrap();
    let prod = cfg.policies.get(Some("prod")).unwrap();
    assert_eq!((prod.read_only, prod.confirm), (true, crate::policy::Confirm::Writes));
    let plain = cfg.policies.get(Some("plain")).unwrap();
    assert_eq!((plain.read_only, plain.confirm), (false, crate::policy::Confirm::Destructive));
    let get = |n: &str| cfg.policies.get(Some(n)).unwrap().paging_idle_timeout;
    assert_eq!(get("careful"), Some(std::time::Duration::from_secs(10)));
    assert_eq!(get("local"), None);
    assert_eq!(get("plain"), Some(crate::policy::PAGING_IDLE_TIMEOUT), "a missing item has its default");
    assert_eq!(cfg.policies.get(None).unwrap().paging_idle_timeout, Some(crate::policy::PAGING_IDLE_TIMEOUT));

    let paging = |n: &str| cfg.policies.get(Some(n)).unwrap().paging;
    assert_eq!(paging("local"), crate::policy::PagingMode::Hold);
    assert_eq!(paging("plain"), crate::policy::PagingMode::NoHold, "no-hold is the default");
    assert_eq!(cfg.policies.get(None).unwrap().paging, crate::policy::PagingMode::NoHold);
    let e = parse("[policy.x]\npaging = \"keep\"").unwrap_err();
    assert!(format!("{e:?}").contains("policy.x.paging"), "{e:?}");
    let e = parse("[policy.x]\npaging_idle_timeout = \"soon\"").unwrap_err();
    assert!(format!("{e:?}").contains("policy.x.paging_idle_timeout"), "{e:?}");
    let e = parse("[policy.x]\nconfirm = \"never\"").unwrap_err();
    assert!(format!("{e:?}").contains("policy.x.confirm"), "{e:?}");
    let e = parse("[policy.x]\nread_only = \"yes\"").unwrap_err();
    assert!(format!("{e:?}").contains("policy.x.read_only"), "a boolean: {e:?}");
    let e = parse("[policy.x]\nrow_limit = 5").unwrap_err();
    assert!(format!("{e:?}").contains("row_limit"), "unknown items are typos: {e:?}");
}

#[test]
fn step_2_settings_are_read_checked_and_saved_only_when_set() {
    let cfg = parse("").unwrap();
    assert_eq!(cfg.prefs, Prefs::default());
    assert_eq!(
        (cfg.prefs.commands_position, cfg.prefs.detail_view, cfg.prefs.clipboard, cfg.prefs.copy_header),
        (CommandsPosition::Popup, DetailView::Panel, ClipboardSetting::Auto, CopyHeader::Auto)
    );
    let cfg = parse(
        "detail_view = \"statusbar\"\nclipboard = \"OSC52\"\ncopy_header = \"off\"\n[commands]\nposition = \"bottom\"\n",
    )
    .unwrap();
    assert_eq!(
        cfg.prefs,
        Prefs {
            commands_position: CommandsPosition::Bottom,
            detail_view: DetailView::Statusbar,
            clipboard: ClipboardSetting::Osc52,
            copy_header: CopyHeader::Off,
            osc52_max_bytes: OSC52_MAX_BYTES,
            cursor_shape: CursorShape::On,
            editor_clipboard: EditorClipboard::On,
            format_case: KeywordCase::Preserve,
            format_indent: FormatIndent::Four,
            auto_pairs: AutoPairs::Off,
            soft_tab_backspace: SoftTabBackspace::On,
            run_hints: RunHints::On,
            keychain_access: KeychainAccess::Direct,
            size_heat: SizeHeat::Three,
            yank_flash: YankFlash::On,
            idle_disconnect: None,
            lock_wait: None,
        }
    );
    assert_eq!(parse("osc52_max_bytes = 5000\n").unwrap().prefs.osc52_max_bytes, 5000);
    for bad in ["0", "-1"] {
        assert_eq!(
            parse(&format!("osc52_max_bytes = {bad}\n")).unwrap_err(),
            ConfigError::Value { key: "osc52_max_bytes".into(), value: bad.into(), profile: None, allowed: None }
        );
    }
    // A wrong value is a typed error naming the key, the value and what it takes.
    for (text, key, value, allowed) in [
        ("clipboard = \"pbcopy\"\n", "clipboard", "pbcopy", "auto, system, osc52"),
        ("detail_view = \"side\"\n", "detail_view", "side", "panel, statusbar"),
        ("copy_header = \"yes\"\n", "copy_header", "yes", "auto, on, off"),
        ("[commands]\nposition = \"top\"\n", "commands.position", "top", "popup, bottom"),
    ] {
        assert_eq!(
            parse(text).unwrap_err(),
            ConfigError::Value { key: key.into(), value: value.into(), profile: None, allowed: Some(allowed) },
            "{text}"
        );
    }
    assert!(parse("[commands]\nplace = \"top\"\n").is_err(), "unknown key");
    assert!(parse("clipboard = 1\n").is_err(), "wrong type");

    let path = temp_file("prefs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "# mine\nlanguage = \"en\" # keep\n";
    std::fs::write(&path, original).unwrap();
    // Defaults are not added to a file that never had them.
    save(&path, settings("en"), None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let prefs =
        Prefs { commands_position: CommandsPosition::Bottom, clipboard: ClipboardSetting::Osc52, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(original), "comments and order kept: {text}");
    assert!(text.contains("clipboard = \"osc52\"") && text.contains("[commands]\nposition = \"bottom\""), "{text}");
    assert!(!text.contains("detail_view") && !text.contains("copy_header"), "{text}");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(cfg.prefs, prefs);
    // Back to the defaults, the keys go.
    save(&path, settings("en"), None).unwrap();
    let (cfg, _) = load(Some(path.clone()));
    assert_eq!(cfg.prefs, Prefs::default());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

/// `[editor] format_keyword_case` and `format_indent`: read, checked, written only when not
/// the default (the indent as a number).
#[test]
fn formatter_settings() {
    let cfg = parse("[editor]\nformat_keyword_case = \"UPPER\"\nformat_indent = 2\n").unwrap();
    assert_eq!((cfg.prefs.format_case, cfg.prefs.format_indent), (KeywordCase::Upper, FormatIndent::Two));
    for (text, key, value, allowed) in [
        (
            "[editor]\nformat_keyword_case = \"title\"\n",
            "editor.format_keyword_case",
            "title",
            "preserve, upper, lower",
        ),
        ("[editor]\nformat_indent = 3\n", "editor.format_indent", "3", "4, 2"),
    ] {
        assert_eq!(
            parse(text).unwrap_err(),
            ConfigError::Value { key: key.into(), value: value.into(), profile: None, allowed: Some(allowed) },
            "{text}"
        );
    }
    assert!(parse("[editor]\nformat_indent = \"2\"\n").is_err(), "a number, not a string");
    assert_eq!(parse("[editor]\nauto_pairs = \"on\"\n").unwrap().prefs.auto_pairs, AutoPairs::On);
    assert_eq!(
        parse("[editor]\nauto_pairs = \"yes\"\n").unwrap_err(),
        ConfigError::Value {
            key: "editor.auto_pairs".into(),
            value: "yes".into(),
            profile: None,
            allowed: Some("off, on")
        }
    );
    assert_eq!(parse("").unwrap().prefs.run_hints, RunHints::On, "hints are on by default");
    assert_eq!(parse("[editor]\nrun_hints = \"off\"\n").unwrap().prefs.run_hints, RunHints::Off);
    assert_eq!(
        parse("[editor]\nrun_hints = \"no\"\n").unwrap_err(),
        ConfigError::Value {
            key: "editor.run_hints".into(),
            value: "no".into(),
            profile: None,
            allowed: Some("on, off")
        }
    );
    let path = temp_file("format");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    let prefs = Prefs { format_case: KeywordCase::Lower, format_indent: FormatIndent::Two, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[editor]\nformat_keyword_case = \"lower\"\nformat_indent = 2\n"), "{text}");
    assert_eq!(load(Some(path.clone())).0.prefs, prefs);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("format"), "back at the defaults, the keys go");
    let prefs = Prefs { run_hints: RunHints::Off, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[editor]\nrun_hints = \"off\"\n"), "{text}");
    assert_eq!(load(Some(path.clone())).0.prefs, prefs);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("run_hints"), "on is the default: the key goes");
}

#[test]
fn backspace_back_to_the_tab_stop_is_on_by_default_and_can_be_turned_off() {
    assert_eq!(parse("").unwrap().prefs.soft_tab_backspace, SoftTabBackspace::On);
    assert_eq!(
        parse("[editor]\nsoft_tab_backspace = \"off\"\n").unwrap().prefs.soft_tab_backspace,
        SoftTabBackspace::Off
    );
    assert_eq!(
        parse("[editor]\nsoft_tab_backspace = \"4\"\n").unwrap_err(),
        ConfigError::Value {
            key: "editor.soft_tab_backspace".into(),
            value: "4".into(),
            profile: None,
            allowed: Some("on, off")
        }
    );
    let path = temp_file("soft-tab");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    let prefs = Prefs { soft_tab_backspace: SoftTabBackspace::Off, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[editor]\nsoft_tab_backspace = \"off\"\n"), "{text}");
    assert_eq!(load(Some(path.clone())).0.prefs, prefs);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("soft_tab"), "on is the default: the key goes");
}

#[test]
fn the_size_heat_of_the_explorer_is_three_levels_by_default_five_or_off() {
    assert_eq!(parse("").unwrap().prefs.size_heat, SizeHeat::Three);
    assert_eq!(parse("[explorer]\nsize_heat = \"off\"\n").unwrap().prefs.size_heat, SizeHeat::Off);
    assert_eq!(parse("[explorer]\nsize_heat = \"3\"\n").unwrap().prefs.size_heat, SizeHeat::Three);
    assert_eq!(parse("[explorer]\nsize_heat = \"5\"\n").unwrap().prefs.size_heat, SizeHeat::Five);
    // The number of levels without quotes too.
    assert_eq!(parse("[explorer]\nsize_heat = 5\n").unwrap().prefs.size_heat, SizeHeat::Five);
    assert_eq!(parse("[explorer]\nsize_heat = 3\n").unwrap().prefs.size_heat, SizeHeat::Three);
    assert!(parse("[explorer]\nsize_heat = 4\n").is_err());
    // `on`, from before there were levels, keeps working: three levels.
    assert_eq!(parse("[explorer]\nsize_heat = \"on\"\n").unwrap().prefs.size_heat, SizeHeat::Three);
    assert_eq!(
        parse("[explorer]\nsize_heat = \"warm\"\n").unwrap_err(),
        ConfigError::Value {
            key: "explorer.size_heat".into(),
            value: "warm".into(),
            profile: None,
            allowed: Some("3, 5, off")
        }
    );
    assert!(matches!(parse("[explorer]\nhidden = true\n"), Err(ConfigError::Syntax(_))), "only its own keys");
    let path = temp_file("size_heat");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    let prefs = Prefs { size_heat: SizeHeat::Off, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[explorer]\nsize_heat = \"off\"\n"), "{text}");
    assert_eq!(load(Some(path.clone())).0.prefs, prefs);
    let five = Prefs { size_heat: SizeHeat::Five, ..Prefs::default() };
    save(&path, Settings { prefs: five, ..settings("en") }, None).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("[explorer]\nsize_heat = \"5\"\n"));
    assert_eq!(load(Some(path.clone())).0.prefs, five);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("size_heat"), "3 is the default: the key goes");
}

#[test]
fn the_yank_flash_is_on_by_default_and_can_be_turned_off() {
    assert_eq!(parse("").unwrap().prefs.yank_flash, YankFlash::On);
    assert_eq!(parse("[ui]\nyank_flash = \"off\"\n").unwrap().prefs.yank_flash, YankFlash::Off);
    assert_eq!(parse("[ui]\nyank_flash = \"on\"\n").unwrap().prefs.yank_flash, YankFlash::On);
    assert_eq!(
        parse("[ui]\nyank_flash = \"blink\"\n").unwrap_err(),
        ConfigError::Value {
            key: "ui.yank_flash".into(),
            value: "blink".into(),
            profile: None,
            allowed: Some("on, off")
        }
    );
    assert!(matches!(parse("[ui]\nflash = true\n"), Err(ConfigError::Syntax(_))), "only its own keys");
    let path = temp_file("yank_flash");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    let prefs = Prefs { yank_flash: YankFlash::Off, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[ui]\nyank_flash = \"off\"\n"), "{text}");
    assert_eq!(load(Some(path.clone())).0.prefs, prefs);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("yank_flash"), "on is the default: the key goes");
}

#[test]
fn english_is_the_default_language_and_an_explicit_one_is_kept() {
    assert_eq!(Config::default().language, "en");
    assert_eq!(parse("").unwrap().language, "en", "no key: English, not the locale");
    for lang in ["auto", "ko", "en"] {
        assert_eq!(parse(&format!("language = \"{lang}\"\n")).unwrap().language, lang);
    }
}

#[test]
fn a_setting_back_at_its_default_leaves_the_file_with_its_comments() {
    let path = temp_file("defaults");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "# my settings\nclipboard = \"osc52\" # over ssh\ncopy_header = \"off\"\nicons = \"auto\" # said on purpose\n\
                    osc52_max_bytes = 5000\n\n[editor] # keys\ncursor_shape = \"off\"\n\n[commands]\n# where : opens\nposition = \"bottom\"\n";
    std::fs::write(&path, original).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    // clipboard, osc52_max_bytes, editor.cursor_shape and commands.position go back to their
    // defaults.
    let prefs = Prefs { copy_header: CopyHeader::Off, ..Prefs::default() };
    assert_ne!(cfg.prefs, prefs);
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "# my settings\n# over ssh\ncopy_header = \"off\"\nicons = \"auto\" # said on purpose\n\n\
         [editor] # keys\n# where : opens\n",
        "the keys went, every comment stayed (one without a key after it goes to the end), a setting \
         already at its default was left alone"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!((back.prefs, back.icons), (prefs, IconsSetting::Auto));
    // Nothing changes: the file stays byte for byte.
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn result_window_and_spill_limit() {
    let cfg = parse("").unwrap();
    assert_eq!((cfg.result_window_rows, cfg.spill_limit), (10_000, crate::policy::SpillLimit(Some(1 << 30))));
    let cfg = parse(
        "result_window_rows = 5000\nspill_limit = \"256MB\"\n[policy.big]\nspill_limit = \"off\"\n[policy.small]\nspill_limit = 1048576\n",
    )
    .unwrap();
    assert_eq!(cfg.result_window_rows, 5_000);
    assert_eq!(cfg.spill_limit, crate::policy::SpillLimit(Some(256 << 20)));
    assert_eq!(cfg.policies.get(Some("big")).unwrap().spill_limit, Some(crate::policy::SpillLimit(None)));
    assert_eq!(cfg.policies.get(Some("small")).unwrap().spill_limit, Some(crate::policy::SpillLimit(Some(1 << 20))));
    assert_eq!(cfg.policies.get(None).unwrap().spill_limit, None, "the default policy uses the config's");
    for (bad, key) in [
        ("result_window_rows = 10", "result_window_rows"),
        ("result_window_rows = 99999999", "result_window_rows"),
        ("spill_limit = \"huge\"", "spill_limit"),
        ("[policy.x]\nspill_limit = \"1XB\"", "policy.x.spill_limit"),
    ] {
        let e = parse(bad).unwrap_err();
        assert!(format!("{e:?}").contains(key), "{bad}: {e:?}");
    }
}

/// `[editor] cursor_shape`: on unless the file says off; checked and saved like the other
/// settings.
#[test]
fn cursor_shape_is_read_checked_and_saved() {
    assert_eq!(parse("").unwrap().prefs.cursor_shape, CursorShape::On);
    let cfg = parse("[editor]\ncursor_shape = \"OFF\"\n").unwrap();
    assert_eq!(cfg.prefs.cursor_shape, CursorShape::Off);
    assert_eq!(
        parse("[editor]\ncursor_shape = \"bar\"\n").unwrap_err(),
        ConfigError::Value {
            key: "editor.cursor_shape".into(),
            value: "bar".into(),
            profile: None,
            allowed: Some("on, off")
        }
    );
    let path = temp_file("cursor_shape");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "[editor] # mine\n";
    std::fs::write(&path, original).unwrap();
    save(&path, settings("en"), None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "the default is not written");
    let prefs = Prefs { cursor_shape: CursorShape::Off, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[editor] # mine\ncursor_shape = \"off\"\n"), "{text}");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(cfg.prefs.cursor_shape, CursorShape::Off);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("cursor_shape"), "back at the default, the key goes");
}

/// `[editor] clipboard`: on unless the file says off; next to `cursor_shape` in the same
/// table, checked, and written only when off.
#[test]
fn editor_clipboard_is_read_checked_and_saved() {
    assert_eq!(parse("").unwrap().prefs.editor_clipboard, EditorClipboard::On);
    let cfg = parse("[editor]\ncursor_shape = \"off\"\nclipboard = \"Off\"\n").unwrap();
    assert_eq!((cfg.prefs.editor_clipboard, cfg.prefs.cursor_shape), (EditorClipboard::Off, CursorShape::Off));
    assert_eq!(parse("editor = { clipboard = \"off\" }\n").unwrap().prefs.editor_clipboard, EditorClipboard::Off);
    assert_eq!(
        parse("[editor]\nclipboard = \"unnamedplus\"\n").unwrap_err(),
        ConfigError::Value {
            key: "editor.clipboard".into(),
            value: "unnamedplus".into(),
            profile: None,
            allowed: Some("on, off")
        }
    );
    let path = temp_file("editor_clipboard");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "[editor]\ncursor_shape = \"off\"\n";
    std::fs::write(&path, original).unwrap();
    let shape = Prefs { cursor_shape: CursorShape::Off, ..Prefs::default() };
    save(&path, Settings { prefs: shape, ..settings("en") }, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "the default is not written");
    let prefs = Prefs { editor_clipboard: EditorClipboard::Off, ..shape };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "[editor]\ncursor_shape = \"off\"\nclipboard = \"off\"\n");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(cfg.prefs, prefs);
    save(&path, Settings { prefs: shape, ..settings("en") }, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "back at the default, the key goes");
}

/// The server-side statement cache: on unless the profile says `false`, and
/// written to the file only when off.
#[test]
fn statement_cache_is_read_and_written_only_when_off() {
    let path = temp_file("stmtcache");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[[connections]]\nname = \"a\"\nhost = \"h\"\n\n[[connections]]\nname = \"b\"\nstatement_cache = false # pgbouncer\n").unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert!(cfg.connections[0].statement_cache, "on by default");
    assert!(!cfg.connections[1].statement_cache);
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("statement_cache").count(), 1, "{text}");
    assert!(text.contains("statement_cache = false # pgbouncer"), "{text}");
    cfg.connections[0].statement_cache = false;
    cfg.connections[1].statement_cache = true;
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert!(!back.connections[0].statement_cache && back.connections[1].statement_cache);
    assert_eq!(std::fs::read_to_string(&path).unwrap().matches("statement_cache").count(), 1);
}

/// MySQL's public key options: read, written only when set, and kept for a profile whose
/// `dsn` stays a URL (and for one whose URL becomes fields).
#[test]
fn mysql_key_options_are_read_and_written_only_when_set() {
    let path = temp_file("mysqlkey");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "[[connections]]\nname = \"a\"\ndriver = \"mysql\"\nhost = \"h\"\n\n\
         [[connections]]\nname = \"b\"\ndriver = \"mysql\"\nserver_public_key_file = \"keys/db.pem\"\n\
         allow_public_key_retrieval = true # trusted network\n\n\
         [[connections]]\nname = \"c\"\ndriver = \"mysql\"\ndsn = \"mysql://u@h:3307/shop\"\n\
         server_public_key_file = \"c.pem\"\n\n\
         [[connections]]\nname = \"d\"\ndriver = \"mysql\"\ndsn = \"mysql://u@h/shop?x=1\"\n\
         allow_public_key_retrieval = true\n",
    )
    .unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let c = &cfg.connections;
    assert_eq!(
        (c[0].server_public_key_file.as_deref(), c[0].allow_public_key_retrieval),
        (None, false),
        "off by default"
    );
    assert_eq!((c[1].server_public_key_file.as_deref(), c[1].allow_public_key_retrieval), (Some("keys/db.pem"), true));
    assert_eq!((c[2].dsn.as_deref(), c[2].port), (None, 3307), "the URL became fields");
    assert_eq!(c[2].server_public_key_file.as_deref(), Some("c.pem"));
    assert!(c[3].dsn.is_some() && c[3].allow_public_key_retrieval, "{:?}", c[3]);
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("server_public_key_file").count(), 2, "{text}");
    assert_eq!(text.matches("allow_public_key_retrieval").count(), 2, "{text}");
    assert!(text.contains("allow_public_key_retrieval = true # trusted network"), "{text}");
    cfg.connections[0].server_public_key_file = Some("a.pem".into());
    cfg.connections[1].server_public_key_file = None;
    cfg.connections[1].allow_public_key_retrieval = false;
    cfg.connections[3].allow_public_key_retrieval = false;
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let keys: Vec<_> =
        back.connections.iter().map(|c| (c.server_public_key_file.clone(), c.allow_public_key_retrieval)).collect();
    assert_eq!(keys, [(Some("a.pem".into()), false), (None, false), (Some("c.pem".into()), false), (None, false)]);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("allow_public_key_retrieval"));
}

/// The TLS settings: read from the fields and from a PostgreSQL URL (which becomes fields),
/// written back as they were (no mode stays no mode), and removed when cleared.
#[test]
fn tls_settings_are_read_and_written_back() {
    let path = temp_file("tls");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "[[connections]]\nname = \"a\"\nhost = \"h\"\n\n\
         [[connections]]\nname = \"b\"\nhost = \"h\"\nsslmode = \"verify-full\"\n\
         sslrootcert = \"/etc/ca.pem\" # the company CA\nsslcert = \"~/me.pem\"\nsslkey = \"~/me.key\"\n\n\
         [[connections]]\nname = \"c\"\ndsn = \"postgres://u@h/db?sslmode=require&sslrootcert=%2Fca.pem\"\n\n\
         [[connections]]\nname = \"d\"\ndriver = \"mysql\"\nhost = \"h\"\nsslmode = \"VERIFY_IDENTITY\"\n",
    )
    .unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let c = &cfg.connections;
    assert_eq!((c[0].sslmode.as_str(), c[0].tls().unwrap().mode), ("", crate::tls::TlsMode::Disable));
    assert_eq!(
        (c[1].sslrootcert.as_deref(), c[1].sslcert.as_deref(), c[1].sslkey.as_deref()),
        (Some("/etc/ca.pem"), Some("~/me.pem"), Some("~/me.key"))
    );
    assert_eq!(
        (c[2].dsn.as_deref(), c[2].sslmode.as_str(), c[2].sslrootcert.as_deref()),
        (None, "require", Some("/ca.pem"))
    );
    assert_eq!(c[3].tls().unwrap().mode, crate::tls::TlsMode::VerifyFull);
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("sslmode").count(), 3, "no mode stays no mode: {text}");
    assert!(text.contains("sslrootcert = \"/etc/ca.pem\" # the company CA"), "{text}");
    assert!(text.contains("sslmode = \"VERIFY_IDENTITY\""), "{text}");
    cfg.connections[1].sslcert = None;
    cfg.connections[1].sslkey = None;
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!((back.connections[1].sslcert.as_deref(), back.connections[1].sslkey.as_deref()), (None, None));
    assert_eq!(back.connections[1].sslrootcert.as_deref(), Some("/etc/ca.pem"));
    assert_eq!(back.connections[2].sslrootcert.as_deref(), Some("/ca.pem"));
}

/// The SSH tunnel's table: read, checked when on, written back with its
/// comments, left out when never set up, kept when turned off.
#[test]
fn the_ssh_table_is_read_checked_and_written() {
    use crate::profile::ssh::{SshAuth, SshSettings};
    let path = temp_file("ssh");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "[[connections]]\nname = \"prod\"\nhost = \"orders-db.internal\"\n\n[connections.ssh]\nenabled = true\n\
                host = \"bastion.example.com\" # the jump host\nuser = \"ec2-user\"\nkey_file = \"~/.ssh/prod.pem\"\n\n\
                [[connections]]\nname = \"local\"\n";
    std::fs::write(&path, text).unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let ssh = cfg.connections[0].inline_ssh().expect("on");
    assert_eq!(
        (ssh.host.as_str(), ssh.port, ssh.user.as_str(), ssh.auth),
        ("bastion.example.com", 22, "ec2-user", SshAuth::Key)
    );
    assert_eq!(ssh.key_file.as_deref(), Some("~/.ssh/prod.pem"));
    assert!(cfg.connections[1].ssh.is_none());
    // Turned off and changed: the table stays with its comment; nothing is added to the other.
    let s = cfg.connections[0].ssh.as_mut().unwrap();
    s.enabled = false;
    s.port = 2222;
    s.keepalive = Some(0);
    save(&path, settings("auto"), Some(profiles(&cfg.connections, &cfg.folders, None))).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("host = \"bastion.example.com\" # the jump host"), "{written}");
    assert!(
        written.contains("enabled = false") && written.contains("port = 2222") && written.contains("keepalive = 0")
    );
    assert_eq!(written.matches("[connections.ssh]").count(), 1, "{written}");
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(back.connections[0].ssh, cfg.connections[0].ssh);
    assert!(back.connections[0].inline_ssh().is_none(), "off");
    // Never set up: no table.
    let mut plain = back.connections.clone();
    plain[0].ssh = None;
    save(&path, settings("auto"), Some(profiles(&plain, &back.folders, None))).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("ssh"));
    // An enabled tunnel with a missing key, or an unknown key in the table, is an error.
    std::fs::write(&path, "[[connections]]\nname = \"p\"\n[connections.ssh]\nenabled = true\nhost = \"b\"\n").unwrap();
    assert_eq!(load(Some(path.clone())).1, Some(ConfigError::SshMissing { key: "user", profile: "p".into() }));
    std::fs::write(&path, "[[connections]]\nname = \"p\"\n[connections.ssh]\nhots = \"b\"\n").unwrap();
    assert!(matches!(load(Some(path.clone())).1, Some(ConfigError::Syntax(_))));
    let off = SshSettings { enabled: false, ..SshSettings::default() };
    assert_eq!(off.problem(), None);
}

/// Tunnel presets: read with their ids (a new one when missing), checked, named by profiles,
/// written back with their comments, moved to a new name when renamed, removed when gone.
#[test]
fn tunnel_presets_are_read_checked_and_written() {
    use crate::profile::ssh::SshAuth;
    use crate::profile::tunnel::{RouteError, TunnelPreset, route};
    let path = temp_file("tunnels");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "version = 2\n\n[tunnels.office] # the office bastion\nhost = \"bastion.example.com\" # jump host\n\
                user = \"ec2-user\"\nkey_file = \"~/.ssh/office.pem\"\n\n[tunnels.lab]\nhost = \"lab\"\nport = 2222\n\
                user = \"me\"\nauth = \"password\"\nsecret_source = \"file\"\n\n\
                [[connections]]\nname = \"orders\"\nhost = \"orders-db\"\ntunnel = \"office\"\n\n\
                [[connections]]\nname = \"local\"\n";
    std::fs::write(&path, text).unwrap();
    let (mut cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert!(cfg.ids_assigned && cfg.needs_migration(), "presets without ids get them written");
    let names: Vec<&str> = cfg.tunnels.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["lab", "office"]);
    let office = &cfg.tunnels[1];
    assert!(office.settings.enabled);
    assert_eq!(
        (office.settings.host.as_str(), office.settings.port, office.settings.auth),
        ("bastion.example.com", 22, SshAuth::Key)
    );
    assert_eq!(cfg.tunnels[0].settings.port, 2222);
    assert_eq!(cfg.connections[0].tunnel.as_deref(), Some("office"));
    assert_eq!(route(&cfg.connections[0], &cfg.tunnels).unwrap().settings(), Some(&office.settings));
    // Saved: the ids are written first; the comments stay.
    save(
        &path,
        settings("auto"),
        Some(Profiles {
            connections: &cfg.connections,
            tunnels: &cfg.tunnels,
            environments: None,
            default_rules: None,
            folders: &cfg.folders,
            last_used: None,
        }),
    )
    .unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("[tunnels.office] # the office bastion"), "{written}");
    assert!(written.contains("host = \"bastion.example.com\" # jump host"), "{written}");
    assert!(written.contains(&format!("id = \"{}\"", cfg.tunnels[1].id)), "{written}");
    assert!(written.contains("tunnel = \"office\""), "{written}");
    assert!(!written.contains("enabled"), "a preset has no switch: {written}");
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert!(!back.ids_assigned);
    assert_eq!(
        back.tunnels,
        cfg.tunnels.iter().map(|t| TunnelPreset { origin: Some(t.name.clone()), ..t.clone() }).collect::<Vec<_>>()
    );
    // Renamed (the profile follows, as the app does it), changed and one removed: the table
    // moves to its new name with its comments; the removed one is gone.
    let id = cfg.tunnels[1].id;
    cfg.tunnels.remove(0);
    cfg.tunnels[0].name = "hq".into();
    cfg.tunnels[0].settings.keepalive = Some(30);
    cfg.connections[0].tunnel = Some("hq".into());
    save(
        &path,
        settings("auto"),
        Some(Profiles {
            connections: &cfg.connections,
            tunnels: &cfg.tunnels,
            environments: None,
            default_rules: None,
            folders: &cfg.folders,
            last_used: None,
        }),
    )
    .unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("[tunnels.hq] # the office bastion"), "{written}");
    assert!(
        written.contains("keepalive = 30") && !written.contains("[tunnels.lab]") && !written.contains("office]"),
        "{written}"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!((back.tunnels.len(), back.tunnels[0].id, back.tunnels[0].name.as_str()), (1, id, "hq"));
    assert_eq!(back.connections[0].tunnel.as_deref(), Some("hq"));
    // None left: no table at all, and a profile without a preset has no key.
    let mut conns = back.connections.clone();
    conns[0].tunnel = None;
    save(
        &path,
        settings("auto"),
        Some(Profiles {
            connections: &conns,
            tunnels: &[],
            environments: None,
            default_rules: None,
            folders: &back.folders,
            last_used: None,
        }),
    )
    .unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(!written.contains("tunnel"), "{written}");
    // A new preset in a file that had none.
    let new = TunnelPreset::new("new one", back.tunnels[0].settings.clone());
    save(
        &path,
        settings("auto"),
        Some(Profiles {
            connections: &conns,
            tunnels: std::slice::from_ref(&new),
            environments: None,
            default_rules: None,
            folders: &back.folders,
            last_used: None,
        }),
    )
    .unwrap();
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!((back.tunnels[0].name.as_str(), back.tunnels[0].id), ("new one", new.id));
    // Both a preset and its own tunnel on, or a name no preset has: an error of that profile
    // only (the file loads; the profile does not connect).
    let both = "[tunnels.office]\nhost = \"b\"\nuser = \"u\"\nauth = \"agent\"\n[[connections]]\nname = \"p\"\ntunnel = \"office\"\n\
                [connections.ssh]\nenabled = true\nhost = \"b\"\nuser = \"u\"\nauth = \"agent\"\n";
    let cfg = parse(both).unwrap();
    assert_eq!(route(&cfg.connections[0], &cfg.tunnels), Err(RouteError::Both { tunnel: "office".into() }));
    let cfg = parse("[[connections]]\nname = \"p\"\ntunnel = \"nowhere\"\n").unwrap();
    assert_eq!(route(&cfg.connections[0], &cfg.tunnels), Err(RouteError::NotFound("nowhere".into())));
}

#[test]
fn bad_tunnel_presets_are_errors_of_the_file() {
    let ok = "host = \"b\"\nuser = \"u\"\nauth = \"agent\"\n";
    assert!(parse(&format!("[tunnels.a]\n{ok}")).is_ok());
    assert_eq!(parse(&format!("[tunnels.\" a\"]\n{ok}")).unwrap_err(), ConfigError::TunnelName(" a".into()));
    assert_eq!(
        parse("[tunnels.a]\nhost = \"b\"\nauth = \"agent\"\n").unwrap_err(),
        ConfigError::TunnelMissing { key: "user", tunnel: "a".into() }
    );
    assert_eq!(
        parse("[tunnels.a]\nhost = \"b\"\nuser = \"u\"\n").unwrap_err(),
        ConfigError::TunnelMissing { key: "key_file", tunnel: "a".into() },
        "a key file, unless it logs in another way"
    );
    assert!(matches!(
        parse(&format!("[tunnels.a]\n{ok}port = 0\n")).unwrap_err(),
        ConfigError::Value { key, .. } if key == "tunnels.a.port"
    ));
    // `enabled` belongs to a profile's own table only; unknown keys are refused.
    assert!(matches!(parse(&format!("[tunnels.a]\n{ok}enabled = true\n")).unwrap_err(), ConfigError::Syntax(_)));
    let id = crate::profile::tunnel::TunnelId::new();
    let twice = format!("[tunnels.a]\nid = \"{id}\"\n{ok}[tunnels.b]\nid = \"{id}\"\n{ok}");
    assert_eq!(parse(&twice).unwrap_err(), ConfigError::DuplicateTunnelId(id.to_string()));
    assert!(matches!(
        parse("[[connections]]\nname = \"p\"\ntunnel = \" \"\n").unwrap_err(),
        ConfigError::Value { key, profile: Some(p), .. } if key == "tunnel" && p == "p"
    ));
    // `tunnels` must be a table of tables when the app saves.
    let path = temp_file("tunnels-shape");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "tunnels = 3\n").unwrap();
    let t = a_preset();
    assert!(
        save(
            &path,
            settings("auto"),
            Some(Profiles {
                connections: &[],
                tunnels: std::slice::from_ref(&t),
                environments: None,
                default_rules: None,
                folders: &Folders::default(),
                last_used: None
            })
        )
        .is_err()
    );
}

fn a_preset() -> crate::profile::tunnel::TunnelPreset {
    crate::profile::tunnel::TunnelPreset::new(
        "a",
        crate::profile::ssh::SshSettings {
            host: "b".into(),
            user: "u".into(),
            auth: crate::profile::ssh::SshAuth::Agent,
            ..Default::default()
        },
    )
}

/// Presets written as inline tables (under `[tunnels]`, or a `tunnels = { … }` key) are found,
/// updated, renamed and removed like the others: a deleted one never comes back, a renamed one
/// never stays under its old name with the same id.
#[test]
fn inline_tunnel_tables_are_rewritten_not_left_behind() {
    let path = temp_file("tunnels-inline");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for text in [
        "[tunnels]\noffice = { host = \"b\", user = \"u\", auth = \"agent\" }\nlab = { host = \"l\", user = \"u\", auth = \"agent\" }\n",
        "tunnels = { office = { host = \"b\", user = \"u\", auth = \"agent\" }, lab = { host = \"l\", user = \"u\", auth = \"agent\" } }\n",
        "tunnels.office.host = \"b\"\ntunnels.office.user = \"u\"\ntunnels.office.auth = \"agent\"\n[tunnels.lab]\nhost = \"l\"\nuser = \"u\"\nauth = \"agent\"\n",
    ] {
        std::fs::write(&path, text).unwrap();
        let (cfg, err) = load(Some(path.clone()));
        assert!(err.is_none(), "{err:?}\n{text}");
        // `office` renamed, `lab` deleted.
        let mut tunnels: Vec<_> = cfg.tunnels.iter().filter(|t| t.name == "office").cloned().collect();
        tunnels[0].name = "hq".into();
        let save_with = |t: &[crate::profile::tunnel::TunnelPreset]| {
            save(
                &path,
                settings("auto"),
                Some(Profiles {
                    connections: &[],
                    tunnels: t,
                    environments: None,
                    default_rules: None,
                    folders: &cfg.folders,
                    last_used: None,
                }),
            )
        };
        save_with(&tunnels).unwrap();
        let (back, err) = load(Some(path.clone()));
        assert!(err.is_none(), "{err:?}\n{}", std::fs::read_to_string(&path).unwrap());
        let names: Vec<&str> = back.tunnels.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["hq"], "{}", std::fs::read_to_string(&path).unwrap());
        assert_eq!(back.tunnels[0].id, tunnels[0].id);
        // None left: nothing of them stays.
        save_with(&[]).unwrap();
        let (back, err) = load(Some(path.clone()));
        assert!(err.is_none(), "{err:?}");
        assert!(back.tunnels.is_empty(), "{}", std::fs::read_to_string(&path).unwrap());
    }
}

/// Environments: read in the file's order with their color and badge (checked), and a
/// profile's environment that is not defined still loads (it is an error of that profile).
#[test]
fn environments_are_read_in_file_order_with_color_and_badge() {
    let cfg = parse(
        "[policy.zeta]\ncolor = \"Red\"\nbadge = \"PROD\"\nread_only = true\n\n[policy.default]\nconfirm = \"writes\"\n\n\
         [policy.alpha]\n\n[[connections]]\nname = \"p\"\npolicy = \"gone\"\n",
    )
    .unwrap();
    let names: Vec<&str> = cfg.policies.environments().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["zeta", "alpha"], "the file's order; `default` is no environment");
    let z = &cfg.policies.environments()[0];
    assert_eq!((z.color.as_deref(), z.badge.as_deref(), z.policy.read_only), (Some("Red"), Some("PROD"), true));
    assert_eq!(z.origin.as_deref(), Some("zeta"));
    assert!(cfg.policies.default_rules().confirm.writes());
    assert_eq!(cfg.connections[0].policy.as_deref(), Some("gone"));
    assert_eq!(
        cfg.policies.get(cfg.connections[0].policy.as_deref()),
        Err(crate::policy::UnknownEnvironment("gone".into())),
        "not the default rules"
    );
    for (text, key) in [
        ("[policy.x]\ncolor = \"mauve\"", "policy.x.color"),
        ("[policy.x]\ncolor = 3", "policy.x.color"),
        ("[policy.x]\nbadge = \" PROD\"", "policy.x.badge"),
        ("[policy.x]\nbadge = \"WAYTOOLONG\"", "policy.x.badge"),
        ("[policy.x]\nbadge = \"\"", "policy.x.badge"),
        ("[policy.x]\nbadge = true", "policy.x.badge"),
    ] {
        match parse(text) {
            Err(ConfigError::Value { key: k, .. }) => assert_eq!(k, key, "{text}"),
            other => panic!("{text}: {other:?}"),
        }
    }
    // Names are not checked when read: an old file keeps loading.
    assert!(parse("[policy.\" odd \"]\n").is_ok());
}

/// `idle_disconnect` and `lock_wait`: off without the keys; the top-level ones are the config's,
/// an environment's own win (`"off"` too) and one that leaves them out takes the config's
/// (`None`); a value out of range or not a duration is an error naming the key (never read as
/// absent); a save writes them only when set and reads them back the same.
#[test]
fn idle_disconnect_and_lock_wait_are_read_overridden_and_written() {
    use std::time::Duration;
    let cfg = parse("").unwrap();
    assert_eq!((cfg.prefs.idle_disconnect, cfg.prefs.lock_wait), (None, None));
    let cfg = parse(
        r#"
idle_disconnect = "300s"
lock_wait = 5

[policy.prod]
idle_disconnect = "off"
lock_wait = "30s"

[policy.dev]
"#,
    )
    .unwrap();
    assert_eq!(cfg.prefs.idle_disconnect, Some(Duration::from_secs(300)));
    assert_eq!(cfg.prefs.lock_wait, Some(Duration::from_secs(5)));
    let prod = cfg.policies.get(Some("prod")).unwrap();
    assert_eq!((prod.idle_disconnect, prod.lock_wait), (Some(None), Some(Some(Duration::from_secs(30)))));
    let dev = cfg.policies.get(Some("dev")).unwrap();
    assert_eq!((dev.idle_disconnect, dev.lock_wait), (None, None), "left out: the config's");
    assert_eq!(cfg.policies.get(None).unwrap().idle_disconnect, None);
    for (text, key) in [
        ("idle_disconnect = \"30s\"", "idle_disconnect"),
        ("idle_disconnect = \"2d\"", "idle_disconnect"),
        ("idle_disconnect = \"25h\"", "idle_disconnect"),
        ("idle_disconnect = true", "idle_disconnect"),
        ("lock_wait = \"soon\"", "lock_wait"),
        ("lock_wait = -1", "lock_wait"),
        ("[policy.x]\nidle_disconnect = \"10s\"", "policy.x.idle_disconnect"),
        ("[policy.x]\nlock_wait = \"500ms\"", "policy.x.lock_wait"),
        ("[policy.x]\nlock_wait = \"48h\"", "policy.x.lock_wait"),
    ] {
        let e = parse(text).unwrap_err();
        assert!(format!("{e:?}").contains(key), "{text}: {e:?}");
    }
    // Zero is off, as for the other durations.
    assert_eq!(parse("lock_wait = 0").unwrap().prefs.lock_wait, None);

    // Written: the top-level keys while set, an environment's own when it has them.
    let path = temp_file("session-limits");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "version = 2\n\n[policy.prod]\nconfirm = \"writes\"\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let prefs = Prefs {
        idle_disconnect: Some(Duration::from_secs(300)),
        lock_wait: Some(Duration::from_secs(5)),
        ..Prefs::default()
    };
    let mut envs = cfg.policies.environments().to_vec();
    envs[0].policy.idle_disconnect = Some(None);
    envs[0].policy.lock_wait = Some(Some(Duration::from_secs(90)));
    let save_all = |prefs: Prefs, envs: &[crate::policy::Environment]| {
        save(
            &path,
            Settings { version: 2, prefs, ..settings("en") },
            Some(Profiles {
                connections: &cfg.connections,
                tunnels: &cfg.tunnels,
                environments: Some(envs),
                default_rules: None,
                folders: &cfg.folders,
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    let out = save_all(prefs, &envs);
    assert!(out.contains("idle_disconnect = \"5m\"\n"), "{out}");
    assert!(out.contains("lock_wait = \"5s\"\n"), "{out}");
    assert!(
        out.contains("[policy.prod]\nconfirm = \"writes\"\nidle_disconnect = \"off\"\nlock_wait = \"90s\"\n"),
        "{out}"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!((back.prefs.idle_disconnect, back.prefs.lock_wait), (prefs.idle_disconnect, prefs.lock_wait));
    assert!(back.policies.environments()[0].same_settings(&envs[0]));
    // Back to the config's and off: the keys go.
    envs[0].policy.idle_disconnect = None;
    envs[0].policy.lock_wait = None;
    assert_eq!(save_all(Prefs::default(), &envs), text);
}

/// `commit`, `tx_idle_warning` and `tx_idle_rollback`: read with their defaults when left out,
/// garbage and too-short times are errors naming the item, and a save writes them only when
/// they differ from the default (an existing file without them stays as it is).
#[test]
fn commit_rules_are_read_and_written() {
    use crate::policy::{CommitRule, TX_IDLE_ROLLBACK, TX_IDLE_WARNING};
    use std::time::Duration;
    let cfg = parse(
        r#"
[policy.careful]
commit = "manual"
tx_idle_warning = "2m"
tx_idle_rollback = "off"

[policy.plain]

[policy.default]
commit = "manual"
"#,
    )
    .unwrap();
    let careful = cfg.policies.get(Some("careful")).unwrap();
    assert_eq!(
        (careful.commit, careful.tx_idle_warning, careful.tx_idle_rollback),
        (CommitRule::Manual, Some(Duration::from_secs(120)), None)
    );
    let plain = cfg.policies.get(Some("plain")).unwrap();
    assert_eq!(
        (plain.commit, plain.tx_idle_warning, plain.tx_idle_rollback),
        (CommitRule::Auto, Some(TX_IDLE_WARNING), Some(TX_IDLE_ROLLBACK)),
        "missing keys: automatic commit, 5m, 30m"
    );
    assert_eq!(cfg.policies.get(None).unwrap().commit, CommitRule::Manual, "the default rules take it too");
    for (text, key) in [
        ("[policy.x]\ncommit = \"yes\"", "policy.x.commit"),
        ("[policy.x]\ncommit = true", "policy.x.commit"),
        ("[policy.x]\ntx_idle_warning = \"soon\"", "policy.x.tx_idle_warning"),
        ("[policy.x]\ntx_idle_warning = \"5s\"", "policy.x.tx_idle_warning"),
        ("[policy.x]\ntx_idle_rollback = 3", "policy.x.tx_idle_rollback"),
        ("[policy.x]\ntx_idle_timeout = 3", "policy.x.tx_idle_timeout"),
        ("[policy.x]\ntx_idle_timeout = \"1 day\"", "policy.x.tx_idle_timeout"),
    ] {
        let e = parse(text).unwrap_err();
        assert!(format!("{e:?}").contains(key), "{text}: {e:?}");
    }

    // Written: only what differs from the default, in the file's key order; read back the same.
    let path = temp_file("commit-rules");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "version = 2\n\n[policy.old]\nconfirm = \"writes\"\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let save_envs = |envs: &[crate::policy::Environment]| {
        save(
            &path,
            Settings { version: 2, ..settings("en") },
            Some(Profiles {
                connections: &cfg.connections,
                tunnels: &cfg.tunnels,
                environments: Some(envs),
                default_rules: None,
                folders: &cfg.folders,
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    assert_eq!(save_envs(cfg.policies.environments()), text, "an environment without the keys is untouched");
    let mut envs = cfg.policies.environments().to_vec();
    envs[0].policy.commit = CommitRule::Manual;
    envs[0].policy.tx_idle_warning = None;
    envs[0].policy.tx_idle_rollback = Some(Duration::from_secs(3600));
    let out = save_envs(&envs);
    assert!(
        out.contains(
            "[policy.old]\nconfirm = \"writes\"\ncommit = \"manual\"\ntx_idle_warning = \"off\"\ntx_idle_rollback = \"1h\"\n"
        ),
        "{out}"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert!(back.policies.environments()[0].same_settings(&envs[0]));
    // Back at the defaults: the keys go.
    envs[0].policy.commit = CommitRule::Auto;
    envs[0].policy.tx_idle_warning = Some(TX_IDLE_WARNING);
    envs[0].policy.tx_idle_rollback = Some(TX_IDLE_ROLLBACK);
    let out = save_envs(&envs);
    assert!(!out.contains("commit") && !out.contains("tx_idle"), "{out}");
}

/// `tx_idle_timeout`, the old name of `tx_idle_rollback`: read the same; with both in one table,
/// an error naming both (nothing is chosen); loading never rewrites it, an unchanged table keeps
/// it on a save, and a table saved with a change says `tx_idle_rollback` instead (its comment
/// kept).
#[test]
fn tx_idle_timeout_is_read_as_tx_idle_rollback() {
    use crate::policy::TX_IDLE_ROLLBACK;
    use std::time::Duration;
    let cfg = parse("[policy.a]\ntx_idle_timeout = \"1h\"\n\n[policy.b]\ntx_idle_rollback = \"off\"\n").unwrap();
    assert_eq!(cfg.policies.get(Some("a")).unwrap().tx_idle_rollback, Some(Duration::from_secs(3600)));
    assert_eq!(cfg.policies.get(Some("b")).unwrap().tx_idle_rollback, None);
    let cfg = parse("[policy.default]\ntx_idle_timeout = \"2m\"\n").unwrap();
    assert_eq!(cfg.policies.get(None).unwrap().tx_idle_rollback, Some(Duration::from_secs(120)));
    // Both: an error that names the table and both keys, whatever their values.
    for text in [
        "[policy.a]\ntx_idle_rollback = \"5m\"\ntx_idle_timeout = \"5m\"\n",
        "[policy.a]\ntx_idle_timeout = \"off\"\ntx_idle_rollback = \"1h\"\n",
    ] {
        let e = parse(text).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Alias { table, key: "tx_idle_rollback", alias: "tx_idle_timeout" } if table == "policy.a"),
            "{text}: {e:?}"
        );
    }

    let path = temp_file("tx-idle-alias");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "version = 2\n\n[policy.old]\n# keep this\ntx_idle_timeout = \"1h\"\n\n[policy.other]\ntx_idle_timeout = \"2h\"\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "loading writes nothing");
    let save_envs = |envs: &[crate::policy::Environment]| {
        save(
            &path,
            Settings { version: 2, ..settings("en") },
            Some(Profiles {
                connections: &cfg.connections,
                tunnels: &cfg.tunnels,
                environments: Some(envs),
                default_rules: None,
                folders: &cfg.folders,
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    let mut envs = cfg.policies.environments().to_vec();
    assert_eq!(save_envs(&envs), text, "unchanged: kept as written");
    envs[0].policy.tx_idle_rollback = Some(Duration::from_secs(5400));
    let out = save_envs(&envs);
    assert!(out.contains("[policy.old]\n# keep this\ntx_idle_rollback = \"90m\"\n"), "{out}");
    assert!(out.contains("[policy.other]\ntx_idle_timeout = \"2h\"\n"), "the other table is untouched: {out}");
    assert_eq!(out.matches("tx_idle_timeout").count(), 1, "{out}");
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert!(back.policies.environments()[0].same_settings(&envs[0]));
    assert_eq!(back.policies.environments()[1].policy.tx_idle_rollback, Some(Duration::from_secs(7200)));
    // A change of another item renames it in place too; back at the default, the key goes.
    envs[0].policy.tx_idle_rollback = Some(TX_IDLE_ROLLBACK);
    envs[1].policy.read_only = true;
    let out = save_envs(&envs);
    assert!(out.contains("[policy.other]\ntx_idle_rollback = \"2h\"\nread_only = true\n"), "{out}");
    assert!(!out.contains("tx_idle_timeout") && out.matches("tx_idle_rollback").count() == 1, "{out}");
    assert!(out.contains("# keep this"), "a removed key's comment stays: {out}");
}

/// Saving environments writes only what the user changed: an unchanged table and an
/// unchanged value keep their text (`"5m"` stays `"5m"`), a changed value is set in place with
/// its comment, a value back at its default goes (its comment kept), a rename moves the table
/// with its comments, a delete removes it, a new one is added after the others, and
/// `[policy.default]` and everything else stay as they are.
#[test]
fn environments_are_written_only_where_they_changed() {
    use crate::policy::{Confirm, Environment, presets};
    let path = temp_file("environments");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "version = 2\n# my settings\n\n# the old rules, by hand\n[policy.old]\npaging = \"hold\"\n\
                paging_idle_timeout = \"5m\" # five minutes\n\n[policy.default]\nconfirm = \"writes\"\n\n\
                # production\n[policy.prod] # careful\nread_only = true # never write\nconfirm = \"writes\"\n\
                color = \"Red\"\n\n[policy.gone]\nread_only = true\n\n[tunnels.lab]\n\
                id = \"6f7c1a52-0d3b-4c8e-9a51-0b5f3e7a2d10\"\nhost = \"lab\"\nuser = \"me\"\nauth = \"agent\"\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let save_envs = |envs: &[Environment]| {
        save(
            &path,
            Settings { version: 2, ..settings("en") },
            Some(Profiles {
                connections: &cfg.connections,
                tunnels: &cfg.tunnels,
                environments: Some(envs),
                default_rules: None,
                folders: &cfg.folders,
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    // Nothing changed: not a byte.
    assert_eq!(save_envs(cfg.policies.environments()), text);

    let mut envs = cfg.policies.environments().to_vec();
    // prod: renamed, one value changed, one back at its default, a badge added.
    let prod = envs.iter_mut().find(|e| e.name == "prod").unwrap();
    prod.name = "production".into();
    prod.policy.confirm = Confirm::Destructive;
    prod.color = Some("red".into());
    prod.badge = Some("PROD".into());
    prod.policy.read_only = false;
    envs.retain(|e| e.name != "gone");
    envs.push(presets().pop().map(|p| Environment { name: "qa".into(), ..p }).unwrap());
    let out = save_envs(&envs);
    let old =
        "# the old rules, by hand\n[policy.old]\npaging = \"hold\"\npaging_idle_timeout = \"5m\" # five minutes\n";
    assert!(out.contains(old), "untouched, byte for byte: {out}");
    assert!(out.contains("[policy.default]\nconfirm = \"writes\"\n"), "{out}");
    assert!(out.contains("# production\n[policy.production] # careful\n"), "moved with its comments: {out}");
    assert!(!out.contains("[policy.prod]") && !out.contains("[policy.gone]"), "{out}");
    assert!(out.contains("# never write\n"), "the comment of a removed item stays: {out}");
    assert!(!out.contains("read_only = true # never write"), "{out}");
    assert!(out.contains("color = \"Red\""), "\"Red\" is red: left as written: {out}");
    assert!(out.contains("badge = \"PROD\""), "{out}");
    let production = out.split("[policy.production]").nth(1).unwrap().split("\n[").next().unwrap();
    assert!(!production.contains("confirm"), "back at its default: gone: {production}");
    assert!(out.contains("# my settings") && out.contains("[tunnels.lab]"), "{out}");
    assert!(out.find("[policy.qa]").unwrap() > out.find("[policy.production]").unwrap(), "{out}");
    assert!(
        out.contains("[policy.qa]\ncolor = \"red\"\nbadge = \"PROD\"\nread_only = true\nconfirm = \"writes\"\n"),
        "{out}"
    );
    // Read back: what was saved.
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let names: Vec<&str> = back.policies.environments().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["old", "production", "qa"]);
    for (a, b) in back.policies.environments().iter().zip(&envs) {
        assert!(a.same_settings(b) || a.color.as_deref() == Some("Red"), "{a:?} {b:?}");
    }
    assert_eq!(back.policies.default_rules().confirm, Confirm::Writes);
    // Edited again with a new idle timeout: written in its largest exact unit.
    let mut again = back.policies.environments().to_vec();
    again[2].policy.paging_idle_timeout = Some(std::time::Duration::from_secs(90));
    again[2].policy.spill_limit = Some(crate::policy::SpillLimit(Some(512 << 20)));
    let out = save_envs(&again);
    assert!(out.contains("paging_idle_timeout = \"90s\"\nspill_limit = \"512MB\"\n"), "{out}");
    assert!(out.contains(old), "{out}");
    // Every environment gone: `[policy.default]` stays.
    let out = save_envs(&[]);
    assert!(out.contains("[policy.default]\nconfirm = \"writes\"\n") && !out.contains("[policy.old]"), "{out}");
}

/// A file without environments gets the presets as new tables; an inline table stays inline.
#[test]
fn new_environments_and_inline_tables() {
    use crate::policy::presets;
    let path = temp_file("environments-new");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "version = 2\n").unwrap();
    let (cfg, _) = load(Some(path.clone()));
    let p = |envs: &[crate::policy::Environment]| {
        save(
            &path,
            Settings { version: 2, ..settings("en") },
            Some(Profiles {
                connections: &[],
                tunnels: &[],
                environments: Some(envs),
                default_rules: None,
                folders: &cfg.folders,
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    let out = p(&presets());
    assert_eq!(
        out,
        "version = 2\n\n[policy.dev]\ncolor = \"green\"\n\n[policy.test]\ncolor = \"blue\"\n\n\
         [policy.staging]\ncolor = \"orange\"\nbadge = \"STG\"\nconfirm = \"writes\"\ncommit = \"manual\"\n\n\
         [policy.prod]\ncolor = \"red\"\nbadge = \"PROD\"\nread_only = true\nconfirm = \"writes\"\n\
         commit = \"manual\"\n"
    );
    let (back, _) = load(Some(path.clone()));
    let names: Vec<&str> = back.policies.environments().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["dev", "test", "staging", "prod"]);
    std::fs::write(&path, "version = 2\npolicy = { dev = { color = \"green\" }, prod = { read_only = true } }\n")
        .unwrap();
    let (cfg2, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(p(cfg2.policies.environments()), text, "unchanged: as written");
    let mut envs = cfg2.policies.environments().to_vec();
    envs[1].badge = Some("PROD".into());
    let out = p(&envs);
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}: {out}");
    assert_eq!(back.policies.environments()[1].badge.as_deref(), Some("PROD"), "{out}");
    assert_eq!(back.policies.environments()[0].color.as_deref(), Some("green"), "{out}");
}

/// The `policy` item keeps its place and its form: dotted `policy.<name>.<key>` keys at the top
/// and an inline `policy = { … }` table are edited where they are; the lines around them, and
/// the environments that did not change, keep their text and their place.
#[test]
fn dotted_and_inline_environments_stay_where_they_are() {
    let path = temp_file("environments-placed");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let save_envs = |envs: &[crate::policy::Environment]| {
        save(
            &path,
            Settings { version: 2, ..settings("en") },
            Some(Profiles {
                connections: &[],
                tunnels: &[],
                environments: Some(envs),
                default_rules: None,
                folders: &Folders::default(),
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    // Dotted keys between two other settings.
    let text = "version = 2\n# before\npolicy.prod.read_only = true # never write\npolicy.prod.badge = \"PROD\"\n\
                policy.dev.color = \"green\"\npage_size = 300 # after\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(save_envs(cfg.policies.environments()), text, "unchanged: not a byte");
    let mut envs = cfg.policies.environments().to_vec();
    envs[1].color = Some("teal".into());
    let out = save_envs(&envs);
    assert_eq!(
        out,
        "version = 2\n# before\npolicy.prod.read_only = true # never write\npolicy.prod.badge = \"PROD\"\n\
         policy.dev.color = \"teal\"\npage_size = 300 # after\n",
        "only that value, in its place"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(back.policies.environments()[1].color.as_deref(), Some("teal"));
    // An inline table between two other settings.
    let text = "version = 2\n# before\npolicy = { prod = { read_only = true, badge = \"PROD\" }, dev = { color = \"green\" } }\n\
                page_size = 300 # after\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(save_envs(cfg.policies.environments()), text, "unchanged: not a byte");
    let mut envs = cfg.policies.environments().to_vec();
    envs[1].color = Some("teal".into());
    let out = save_envs(&envs);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert_eq!((lines[0], lines[1], lines[3]), ("version = 2", "# before", "page_size = 300 # after"), "{out}");
    assert!(lines[2].starts_with("policy = { prod = { read_only = true, badge = \"PROD\" }, dev = {"), "{out}");
    assert!(lines[2].contains("teal"), "{out}");
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    let names: Vec<&str> = back.policies.environments().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["prod", "dev"]);
    assert_eq!(back.policies.environments()[1].color.as_deref(), Some("teal"));
    // Renamed and one more: the inline table stays in its place, the renamed one at its place.
    let mut envs = back.policies.environments().to_vec();
    envs[0].name = "production".into();
    envs.push(crate::policy::Environment::new("qa", crate::policy::Policy::default()));
    let out = save_envs(&envs);
    assert_eq!(
        out,
        "version = 2\n# before\npolicy = { production = { read_only = true, badge = \"PROD\" }, \
         dev = { color = \"teal\" }, qa = {} }\npage_size = 300 # after\n"
    );
    let (back, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}: {out}");
    let names: Vec<&str> = back.policies.environments().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["production", "dev", "qa"], "{out}");
}

/// The default rules go to `[policy.default]` only where they changed: the table is made once a
/// rule differs from the built-in default, an untouched table is not touched, and the `color`
/// and `badge` a hand-written one has stay.
#[test]
fn the_default_rules_are_written_only_where_they_changed() {
    use crate::policy::{Confirm, Policy};
    let path = temp_file("default-rules");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let save_rules = |rules: &Policy| {
        save(
            &path,
            Settings { version: 2, ..settings("en") },
            Some(Profiles {
                connections: &[],
                tunnels: &[],
                environments: Some(&[]),
                default_rules: Some(rules),
                folders: &Folders::default(),
                last_used: None,
            }),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    };
    std::fs::write(&path, "version = 2\n").unwrap();
    assert_eq!(save_rules(&Policy::default()), "version = 2\n", "the built-in default: no table");
    let writes = Policy { confirm: Confirm::Writes, ..Policy::default() };
    assert_eq!(save_rules(&writes), "version = 2\n\n[policy.default]\nconfirm = \"writes\"\n");
    let text = "version = 2\n\n# mine\n[policy.default]\nbadge = \"X\" # not shown\npaging_idle_timeout = \"90s\"\n";
    std::fs::write(&path, text).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(save_rules(cfg.policies.default_rules()), text, "unchanged: not a byte");
    let mut rules = cfg.policies.default_rules().clone();
    rules.read_only = true;
    assert_eq!(
        save_rules(&rules),
        "version = 2\n\n# mine\n[policy.default]\nbadge = \"X\" # not shown\npaging_idle_timeout = \"90s\"\nread_only = true\n"
    );
}
