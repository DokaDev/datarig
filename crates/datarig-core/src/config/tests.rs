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
        editor_mode: EditorMode::Vim,
        default_source: DefaultSource::Auto,
        prefs: Prefs::default(),
    }
}

fn profiles<'a>(c: &'a [ConnectionConfig], folders: &'a Folders, last_used: Option<ProfileId>) -> Profiles<'a> {
    Profiles { connections: c, folders, last_used }
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
fn editor_mode_and_keymap_are_read() {
    let cfg = parse("").unwrap();
    assert_eq!(cfg.editor_mode, EditorMode::Vim, "vim is the default");
    assert!(cfg.keymap.is_empty());
    let cfg = parse(
        "[editor]\nmode = \"standard\"\n\n[keymap.explorer]\n\"x\" = \"conn.disconnect\"\n\"ctrl+w\" = \"none\"\n\
         [keymap.nav]\n\"space t d\" = \"tab.close\"\n",
    )
    .unwrap();
    assert_eq!(cfg.editor_mode, EditorMode::Standard);
    assert_eq!(cfg.keymap["explorer"]["x"], "conn.disconnect");
    assert_eq!(cfg.keymap["explorer"]["ctrl+w"], "none");
    assert_eq!(cfg.keymap["nav"]["space t d"], "tab.close");
    // Content is not validated by core (unknown contexts and actions are the TUI's business),
    // but the shape is: a wrong mode, an unknown [editor] key or a non-string action fail.
    assert!(parse("[keymap.nowhere]\n\"q\" = \"no.such.action\"\n").is_ok());
    assert!(format!("{:?}", parse("[editor]\nmode = \"emacs\"\n").unwrap_err()).contains("editor.mode"));
    assert!(parse("[editor]\nmodee = \"vim\"\n").is_err());
    assert!(parse("[keymap.explorer]\n\"x\" = 1\n").is_err());
}

#[test]
fn save_writes_editor_mode_and_keeps_keymap_tables() {
    let path = temp_file("editor");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "language = \"en\"\n\n[keymap.explorer] # mine\n\"x\" = \"explorer.refresh\" # keep\n";
    std::fs::write(&path, original).unwrap();
    // `vim` is the default: not added to a file that never had it.
    save(&path, settings("en"), None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    save(&path, Settings { editor_mode: EditorMode::Standard, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[keymap.explorer] # mine\n\"x\" = \"explorer.refresh\" # keep\n"), "{text}");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(cfg.editor_mode, EditorMode::Standard);
    assert_eq!(cfg.keymap["explorer"]["x"], "explorer.refresh");
    // Back to the default, the key goes (and the table it emptied).
    save(&path, settings("en"), None).unwrap();
    let (cfg, _) = load(Some(path.clone()));
    assert_eq!(cfg.editor_mode, EditorMode::Vim);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    // An inline table works too.
    std::fs::write(&path, "editor = { mode = \"vim\" }\n").unwrap();
    save(&path, Settings { editor_mode: EditorMode::Standard, ..settings("en") }, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "editor = { mode = \"standard\" }\n");
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
fn policy_tables_are_read() {
    let cfg = parse(
        r#"
[policy.careful]
paging_idle_timeout = "10s"

[policy.local]
paging_idle_timeout = "off"

[policy.plain]

[policy.prod]
read_only = true
confirm = "writes"
"#,
    )
    .unwrap();
    let prod = cfg.policies.get(Some("prod"));
    assert_eq!((prod.read_only, prod.confirm), (true, crate::policy::Confirm::Writes));
    let plain = cfg.policies.get(Some("plain"));
    assert_eq!((plain.read_only, plain.confirm), (false, crate::policy::Confirm::Destructive));
    let get = |n: &str| cfg.policies.get(Some(n)).paging_idle_timeout;
    assert_eq!(get("careful"), Some(std::time::Duration::from_secs(10)));
    assert_eq!(get("local"), None);
    assert_eq!(get("plain"), Some(crate::policy::PAGING_IDLE_TIMEOUT), "a missing item has its default");
    assert_eq!(cfg.policies.get(None).paging_idle_timeout, Some(crate::policy::PAGING_IDLE_TIMEOUT));

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
                    osc52_max_bytes = 5000\n\n[editor] # keys\nmode = \"standard\"\n\n[commands]\n# where : opens\nposition = \"bottom\"\n";
    std::fs::write(&path, original).unwrap();
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    // clipboard, osc52_max_bytes, editor.mode and commands.position go back to their defaults.
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
    assert_eq!((back.prefs, back.editor_mode, back.icons), (prefs, EditorMode::Vim, IconsSetting::Auto));
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
    assert_eq!(cfg.policies.get(Some("big")).spill_limit, Some(crate::policy::SpillLimit(None)));
    assert_eq!(cfg.policies.get(Some("small")).spill_limit, Some(crate::policy::SpillLimit(Some(1 << 20))));
    assert_eq!(cfg.policies.get(None).spill_limit, None, "the default policy uses the config's");
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

/// `[editor] cursor_shape`: on unless the file says off, next to the editor
/// mode in the same table; checked and saved like the other settings.
#[test]
fn cursor_shape_is_read_checked_and_saved_next_to_the_editor_mode() {
    assert_eq!(parse("").unwrap().prefs.cursor_shape, CursorShape::On);
    let cfg = parse("[editor]\nmode = \"standard\"\ncursor_shape = \"OFF\"\n").unwrap();
    assert_eq!((cfg.editor_mode, cfg.prefs.cursor_shape), (EditorMode::Standard, CursorShape::Off));
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
    let original = "[editor] # mine\nmode = \"vim\"\n";
    std::fs::write(&path, original).unwrap();
    save(&path, settings("en"), None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "the default is not written");
    let prefs = Prefs { cursor_shape: CursorShape::Off, ..Prefs::default() };
    save(&path, Settings { prefs, ..settings("en") }, None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[editor] # mine\nmode = \"vim\"\ncursor_shape = \"off\"\n"), "{text}");
    let (cfg, err) = load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(cfg.prefs.cursor_shape, CursorShape::Off);
    save(&path, settings("en"), None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("cursor_shape"), "back at the default, the key goes");
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
    let ssh = cfg.connections[0].tunnel().expect("on");
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
    assert!(back.connections[0].tunnel().is_none(), "off");
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
