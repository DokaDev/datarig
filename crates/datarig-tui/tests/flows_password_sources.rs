//! Per-profile password sources through the real `App` event path: the form's storage selector and its fields, the default preselection, connecting
//! and testing with each source, and changing a profile's source. No terminal, no database;
//! every store is in memory unless a test puts a secrets file in a temporary directory.

mod common;

use common::*;
use datarig_core::config::{self, Config};
use datarig_core::driver::DbEvent;
use datarig_core::i18n::Lang;
use datarig_core::secret::{DefaultSource, MemoryStore, PasswordSource, SecretStore, SourceKind, Stores};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::profiles::{Field, FieldError};
use datarig_tui::app::{NodeState, PromptPurpose, Startup, TestState};
use ratatui::crossterm::event::KeyCode;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn temp_config(tag: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-sources-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.toml");
    std::fs::write(&p, body).unwrap();
    p
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Launched with the three sample profiles, the first one (`local-pg`, where the explorer's
/// cursor starts) using `source`.
fn launched_with(source: PasswordSource) -> Harness {
    let mut cfg = sample_config(None);
    cfg.connections[0].set_source(source);
    Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal)
}

/// Move the form's focus to the storage selector.
fn to_selector(h: &mut Harness) {
    while h.form().focus != Field::Source {
        h.key(KeyCode::Tab);
    }
}

fn select(h: &mut Harness, kind: SourceKind) {
    to_selector(h);
    while h.form().source != kind {
        h.key(KeyCode::Right);
    }
}

fn env_with(pairs: &'static [(&'static str, &'static str)]) -> datarig_tui::app::EnvLookup {
    Arc::new(move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()))
}

// ── the form ──────────────────────────────────────────────────────────────────

#[test]
fn form_fields_follow_the_storage_selector() {
    let mut h = Harness::connected(Lang::En);
    h.command("conn.new");
    to_selector(&mut h);
    let shown = |h: &Harness| h.form().fields();
    assert_eq!(h.form().source, SourceKind::Keychain);
    assert!(shown(&h).contains(&Field::Password) && !shown(&h).contains(&Field::Command));
    h.key(KeyCode::Right);
    assert_eq!((h.form().source, shown(&h).contains(&Field::Password)), (SourceKind::File, true));
    h.key(KeyCode::Char(' ')); // Space cycles too
    assert_eq!(h.form().source, SourceKind::Command);
    assert!(shown(&h).contains(&Field::Command) && !shown(&h).contains(&Field::Password));
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::Command, "the field under the selector is the command");
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Right);
    assert_eq!(h.form().source, SourceKind::Env);
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::Env);
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Right);
    assert_eq!(h.form().source, SourceKind::Prompt);
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::Database, "prompt: nothing to enter");
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Right);
    assert_eq!(h.form().source, SourceKind::Keychain, "wraps around");
    h.key(KeyCode::Left);
    assert_eq!(h.form().source, SourceKind::Prompt);
}

#[test]
fn command_and_env_fields_are_validated() {
    let mut h = Harness::connected(Lang::En);
    h.command("conn.new");
    h.type_text("prod");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("app");
    select(&mut h, SourceKind::Command);
    h.ctrl('s');
    let errors = |h: &Harness| h.form().errors(|_| false);
    assert!(errors(&h).contains(&(Field::Command, FieldError::Required)), "{:?}", errors(&h));
    assert!(h.form_open(), "not saved");
    h.key(KeyCode::Tab);
    h.type_text("pass show 'db/prod");
    assert!(errors(&h).contains(&(Field::Command, FieldError::CommandSyntax)));
    h.type_text("'");
    assert!(errors(&h).is_empty(), "{:?}", errors(&h));
    // Env: a bad name is an error at once, an empty one on save.
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Right);
    h.key(KeyCode::Tab);
    h.type_text("DB-PW");
    assert!(errors(&h).contains(&(Field::Env, FieldError::EnvName)));
    assert!(h.screen(160, 45).contains("letters, digits and _ only"));
    h.ctrl('u');
    assert!(errors(&h).contains(&(Field::Env, FieldError::Required)));
    h.type_text("PROD_DB_PW");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    let p = h.app.profiles.iter().find(|p| p.name == "prod").unwrap();
    assert_eq!(p.source(), PasswordSource::Env("PROD_DB_PW".into()));
    assert_eq!(p.password_command, None, "the command typed before is not kept");
}

#[test]
fn a_new_profile_starts_on_the_default_source() {
    // Keychain available, `auto`: the keychain.
    let mut h = Harness::launched(&sample_config(None), Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.keys("n");
    assert_eq!(h.form().source, SourceKind::Keychain);
    // No keychain, `auto` (or `keychain`): prompt; the keychain cannot be picked.
    for default in [DefaultSource::Auto, DefaultSource::Kind(SourceKind::Keychain)] {
        let cfg = Config { default_source: default, ..sample_config(None) };
        let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::unavailable("no dbus")), Startup::Normal);
        assert_eq!(h.app.secrets.unavailable.as_ref().map(|f| f.detail.as_str()), Some("no dbus"), "probed at launch");
        h.keys("n");
        assert_eq!(h.form().source, SourceKind::Prompt, "{default:?}");
        assert!(!h.form().keychain_ok);
        select(&mut h, SourceKind::Command);
        for _ in 0..5 {
            h.key(KeyCode::Right);
            assert_ne!(h.form().source, SourceKind::Keychain, "skipped");
        }
    }
    // An explicit default.
    let cfg = Config { default_source: DefaultSource::Kind(SourceKind::File), ..sample_config(None) };
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.keys("n");
    assert_eq!(h.form().source, SourceKind::File);
    // An existing profile keeps its own source.
    h.key(KeyCode::Esc);
    h.keys("e");
    assert_eq!(h.form().source, SourceKind::Keychain);
}

#[test]
fn set_default_source_is_saved_and_used() {
    let path = temp_config("default", "[[connections]]\nname = \"a\"\n");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.command("set secrets.default_source=env");
    assert_eq!(h.app.default_source, DefaultSource::Kind(SourceKind::Env));
    assert!(h.cmdline().is_none(), "applied");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[secrets]\ndefault_source = \"env\""), "{text}");
    assert!(h.status(120, 24).contains("New profiles start with: environment variable"));
    h.keys("n");
    assert_eq!(h.form().source, SourceKind::Env);
    h.key(KeyCode::Esc);
    // The action does the same.
    h.command("secrets.default.auto");
    assert_eq!(h.app.default_source, DefaultSource::Auto);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("default_source") && !text.contains("[secrets]"), "back to the default: {text}");
    h.command("set secrets.default_source=vault");
    assert!(h.cmdline().is_some_and(|c| c.error.is_some()), "a bad value is an error");
    cleanup(&path);
}

// ── connecting and testing ────────────────────────────────────────────────────

#[test]
fn connect_with_stored_sources() {
    // Keychain.
    let mut h = launched_with(PasswordSource::Keychain);
    h.app.secrets.stores().keychain.set(&h.account("local-pg"), "kc").unwrap();
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    // File (the in-memory secrets file of the harness).
    let mut h = launched_with(PasswordSource::File);
    h.key(KeyCode::Enter);
    assert!(!h.app.conns.attempt().unwrap().had_password, "nothing stored yet");
    h.db(DbEvent::ConnectFailed { error: "password authentication failed".into(), auth: true });
    let p = h.prompt().expect("the prompt opens");
    assert_eq!(p.save_to, Some(SourceKind::File), "saves to the secrets file");
    assert!(h.screen(80, 24).contains("Save to the secrets file"));
    h.type_text("fi");
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    h.db(DbEvent::Connected);
    let account = h.account("local-pg");
    assert_eq!(h.app.secrets.stores().file.get(&account).unwrap().as_deref(), Some("fi"));
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap(), None, "not the keychain");
}

#[test]
fn connect_with_env_and_its_errors() {
    let mut h = launched_with(PasswordSource::Env("PROD_DB_PW".into()));
    h.app.set_env_lookup(env_with(&[("PROD_DB_PW", "from-env")]));
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    // Read at connect time: a missing variable fails before the server is asked.
    let mut h = launched_with(PasswordSource::Env("PROD_DB_PW".into()));
    h.app.set_env_lookup(env_with(&[]));
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().is_none() && h.prompt().is_none());
    assert_eq!(h.node_error("local-pg").as_deref(), Some("Environment variable PROD_DB_PW is not set"));
    assert!(h.screen(160, 45).contains("Environment variable PROD_DB_PW is not set"));
    // A wrong password from the environment does not open the prompt (fix it at the source).
    let mut h = launched_with(PasswordSource::Env("PROD_DB_PW".into()));
    h.app.set_env_lookup(env_with(&[("PROD_DB_PW", "wrong")]));
    h.key(KeyCode::Enter);
    h.db(DbEvent::ConnectFailed { error: "password authentication failed".into(), auth: true });
    assert!(h.prompt().is_none());
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("password authentication failed")));
}

#[cfg(unix)]
#[test]
fn connect_with_a_command_and_its_errors() {
    let mut h = launched_with(PasswordSource::Command("printf 'from-cmd\\n'".into()));
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    let mut h =
        launched_with(PasswordSource::Command("sh -c 'printf secret-out; echo \"item not found\" >&2; exit 4'".into()));
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().is_none() && h.prompt().is_none());
    let error = h.node_error("local-pg").unwrap_or_default();
    assert_eq!(error, "Password command failed (exit 4): item not found");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Password command failed (exit 4): item not found"), "{screen}");
    assert!(!screen.contains("secret-out"), "stdout is never shown");
    let mut h = launched_with(PasswordSource::Command("datarig-no-such-tool read".into()));
    h.key(KeyCode::Enter);
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("Password command: cannot run datarig-no-such-tool")));
}

#[test]
fn a_prompt_profile_asks_on_every_connect() {
    let mut h = launched_with(PasswordSource::Prompt);
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().is_none(), "asks first");
    let p = h.prompt().expect("the prompt opens before connecting");
    assert_eq!((p.save_to, p.save), (None, false), "no checkbox");
    let screen = h.screen(80, 24);
    assert!(!screen.contains("[x]") && !screen.contains("[ ]"), "{screen}");
    assert!(screen.contains("Asked on every connect — never saved"), "{screen}");
    h.type_text("typed");
    h.key(KeyCode::Tab); // no checkbox to focus
    assert!(!h.prompt().unwrap().save_focus);
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    // Wrong: asked again.
    h.db(DbEvent::ConnectFailed { error: "password authentication failed".into(), auth: true });
    assert!(h.prompt().is_some_and(|p| p.save_to.is_none()));
    h.type_text("right");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    let account = h.account("local-pg");
    let stores = h.app.secrets.stores();
    assert_eq!(stores.keychain.get(&account).unwrap(), None, "never stored");
    assert_eq!(stores.file.get(&account).unwrap(), None);
    assert_eq!(h.app.secrets.session(&account), None);
    // The next connect asks again.
    h.keys("x");
    h.key(KeyCode::Enter);
    assert!(h.prompt().is_some() && h.app.conns.attempt().is_none());
    // Also after a cancelled attempt: the typed password served that attempt only.
    h.type_text("once");
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().is_some());
    h.key(KeyCode::Esc);
    assert!(h.app.conns.attempt().is_none());
    h.key(KeyCode::Enter);
    assert!(h.prompt().is_some() && h.app.conns.attempt().is_none(), "asked again");
}

/// `Esc` in the password prompt cancels the attempt cleanly: the node is back to "not
/// connected" (no spinner, no error line) and the status bar says the connection was cancelled
/// instead of still "Connecting…". Both ways the prompt opens: a `prompt` profile, and a
/// keychain profile without a saved password that the server turned down.
#[test]
fn esc_in_the_password_prompt_cancels_the_connection() {
    let mut h = launched_with(PasswordSource::Prompt);
    let id = h.app.profiles[0].id;
    h.key(KeyCode::Enter);
    assert!(h.prompt().is_some());
    assert!(h.status(120, 30).contains("Connecting to local-pg"), "{}", h.status(120, 30));
    h.key(KeyCode::Esc);
    assert!(h.prompt().is_none());
    assert_eq!(h.app.conns.state(id), NodeState::Disconnected);
    let status = h.status(120, 30);
    assert!(status.contains("Connection to local-pg cancelled") && !status.contains("Connecting"), "{status}");
    assert!(h.rows().iter().all(|r| r.trim() != "!"), "no error line: {:?}", h.rows());

    let mut h = launched_with(PasswordSource::Keychain);
    let id = h.app.profiles[0].id;
    h.key(KeyCode::Enter);
    assert_eq!(h.connecting(), Some(("local-pg".into(), false)));
    h.db(DbEvent::ConnectFailed { error: "fe_sendauth: no password supplied".into(), auth: true });
    assert!(h.prompt().is_some(), "no saved password: asked");
    let old = h.app.conns.get(id).unwrap().generation;
    h.key(KeyCode::Esc);
    assert_eq!(h.app.conns.state(id), NodeState::Disconnected, "○, not ✕ or a spinner");
    assert!(h.node_error("local-pg").is_none());
    let status = h.status(120, 30);
    assert!(status.contains("Connection to local-pg cancelled"), "{status}");
    // A late answer of the cancelled attempt changes nothing.
    let target = datarig_tui::app::EventTarget::Meta(id);
    h.app.on_app_event(datarig_tui::app::AppEvent::Db { target, generation: old, ev: DbEvent::Connected });
    assert_eq!(h.app.conns.state(id), NodeState::Disconnected);
}

#[test]
fn test_connection_goes_through_the_source() {
    let mut h = launched_with(PasswordSource::Env("PROD_DB_PW".into()));
    h.app.set_env_lookup(env_with(&[]));
    h.keys("t");
    let state = h.app.conn_test.as_ref().map(|t| t.state.clone());
    assert!(matches!(state, Some(TestState::Source(_))), "{state:?}");
    assert!(h.screen(80, 24).contains("Environment variable PROD_DB_PW is not set"));
    h.app.set_env_lookup(env_with(&[("PROD_DB_PW", "x")]));
    h.keys("t");
    assert_eq!(h.app.conn_test.as_ref().map(|t| t.state.clone()), Some(TestState::Running));
    // A prompt profile asks first, then tests; nothing is saved.
    let mut h = launched_with(PasswordSource::Prompt);
    h.keys("t");
    assert!(matches!(h.prompt().map(|p| &p.purpose), Some(PromptPurpose::Test(_))));
    assert!(h.screen(80, 24).contains("Enter test · Esc cancel"));
    h.type_text("pw");
    h.key(KeyCode::Enter);
    assert!(h.prompt().is_none());
    assert_eq!(h.app.conn_test.as_ref().map(|t| t.state.clone()), Some(TestState::Running));
    assert!(h.app.conns.attempt().is_none(), "a test, not a connection");
    // From the form: the source being edited.
    let mut h = launched_with(PasswordSource::Keychain);
    h.keys("e");
    select(&mut h, SourceKind::Env);
    h.key(KeyCode::Tab);
    h.type_text("NOT_SET_ANYWHERE");
    h.app.set_env_lookup(env_with(&[]));
    h.ctrl('t');
    assert!(matches!(h.app.conn_test.as_ref().map(|t| &t.state), Some(TestState::Source(_))));
}

#[cfg(unix)]
#[test]
fn an_insecure_secrets_file_is_refused_with_the_fix() {
    use std::os::unix::fs::PermissionsExt;
    let path = temp_config("insecure", "");
    let file = path.with_file_name("secrets.toml");
    let store = datarig_core::secret::FileStore::new(file.clone());
    let mut h = launched_with(PasswordSource::File);
    store.set(&h.account("local-pg"), "fi").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    h.app.set_secret_stores(Stores {
        keychain: Arc::new(MemoryStore::new()),
        file: Arc::new(store),
        file_path: Some(file.clone()),
    });
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().is_none());
    let notice = h.node_error("local-pg").unwrap_or_default();
    assert!(notice.contains("mode 644") && notice.contains(&format!("chmod 600 {}", file.display())), "{notice}");
    // The form cannot load it either, and saving the form keeps it.
    h.keys("e");
    assert!(h.form().password_unread);
    h.ctrl('s');
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let back = datarig_core::secret::FileStore::new(file.clone());
    assert_eq!(back.get(&h.account("local-pg")).unwrap().as_deref(), Some("fi"), "not removed");
    cleanup(&path);
}

// ── changing the source of a profile ───────────────────────────────────────────

const ONE: &str = "# mine\n[[connections]]\nname = \"db\"\nhost = \"h\"\nuser = \"u\"\n";

#[test]
fn changing_the_source_asks_then_moves_the_password() {
    let path = temp_config("move", ONE);
    let (cfg, _) = config::load(Some(path.clone()));
    let store = Arc::new(MemoryStore::new());
    let mut h = Harness::launched(&cfg, Lang::En, store.clone(), Startup::Normal);
    let account = h.account("db");
    store.set(&account, "pw").unwrap();
    h.keys("e");
    select(&mut h, SourceKind::File);
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(80, 24).contains("Move the saved password of “db” from the OS"));
    // No: nothing changes, the form stays.
    h.key(KeyCode::Char('n'));
    assert!(h.form_open());
    assert_eq!(store.get(&account).unwrap().as_deref(), Some("pw"));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("password_source"));
    // Yes: written to the file, checked, saved, then removed from the keychain.
    h.ctrl('s');
    h.key(KeyCode::Char('y'));
    assert!(!h.form_open());
    let file = h.app.secrets.stores().file.clone();
    assert_eq!(file.get(&account).unwrap().as_deref(), Some("pw"));
    assert_eq!(store.get(&account).unwrap(), None);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# mine") && text.contains("password_source = \"file\""), "{text}");
    assert!(h.status(120, 24).contains("Moved the password of db to the secrets file"));
    // To a source that stores nothing: removed from the file after the confirmation.
    h.keys("e");
    select(&mut h, SourceKind::Env);
    h.key(KeyCode::Tab);
    h.type_text("DB_PW");
    h.ctrl('s');
    let screen = h.screen(80, 24);
    assert!(screen.contains("Switch “db” to the environment variable?"), "{screen}");
    assert!(screen.contains("removed from the secrets file."), "{screen}");
    // Enter keeps the stored password: nothing changes, the form stays.
    h.key(KeyCode::Enter);
    assert!(h.form_open());
    assert_eq!(file.get(&account).unwrap().as_deref(), Some("pw"));
    h.ctrl('s');
    h.key(KeyCode::Char('y'));
    assert_eq!(file.get(&account).unwrap(), None);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("password_source = \"env\"\npassword_env = \"DB_PW\""), "{text}");
    // From a source that stores nothing: no question; a typed password goes to the new store.
    h.keys("e");
    select(&mut h, SourceKind::Keychain);
    h.key(KeyCode::Tab);
    h.type_text("new");
    h.ctrl('s');
    assert!(!h.form_open(), "saved without asking");
    assert_eq!(store.get(&account).unwrap().as_deref(), Some("new"));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("password_"));
    cleanup(&path);
}

#[test]
fn a_failed_source_change_keeps_everything() {
    let path = temp_config("movefail", ONE);
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    let keychain: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let account = h.account("db");
    keychain.set(&account, "pw").unwrap();
    h.app.set_secret_stores(Stores {
        keychain: keychain.clone(),
        file: Arc::new(MemoryStore::failing(datarig_core::fault::Fault::new(
            datarig_core::fault::FaultKind::Io(std::io::ErrorKind::StorageFull),
            "No space left on device (os error 28)",
        ))),
        file_path: None,
    });
    h.keys("e");
    select(&mut h, SourceKind::File);
    h.ctrl('s');
    h.key(KeyCode::Char('y'));
    assert!(h.form_open(), "the form stays open");
    assert!(h.status(120, 24).contains("Nothing changed: Secrets file: the disk is full"));
    assert_eq!(keychain.get(&account).unwrap().as_deref(), Some("pw"), "not removed");
    assert_eq!(h.app.profiles[0].source(), PasswordSource::Keychain);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("password_source"));
    // The config cannot be saved: both copies stay, the profile keeps its old source.
    h.key(KeyCode::Esc);
    h.app.set_secret_stores(Stores::with_keychain(keychain.clone()));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(path.join("blocked")).unwrap();
    h.keys("e");
    select(&mut h, SourceKind::File);
    h.ctrl('s');
    h.key(KeyCode::Char('y'));
    assert!(h.status(160, 24).contains("Nothing changed:"));
    assert_eq!(keychain.get(&account).unwrap().as_deref(), Some("pw"));
    assert_eq!(h.app.profiles[0].source(), PasswordSource::Keychain);
    cleanup(&path);
}

#[test]
fn deleting_a_profile_removes_its_stored_password() {
    let mut cfg = sample_config(None);
    cfg.connections[1].set_source(PasswordSource::File);
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    let account = h.account("分析-replica");
    h.app.secrets.stores().file.set(&account, "fi").unwrap();
    h.app.secrets.stores().keychain.set(&account, "other").unwrap();
    h.explore("分析-replica");
    h.keys("d");
    h.key(KeyCode::Char('y'));
    assert!(h.app.profiles.iter().all(|p| p.name != "分析-replica"));
    assert_eq!(h.app.secrets.stores().file.get(&account).unwrap(), None);
    assert_eq!(
        h.app.secrets.stores().keychain.get(&account).unwrap().as_deref(),
        Some("other"),
        "only the profile's own store"
    );
}

/// The SSH tunnel's secret goes with its profile, from its own store (it may
/// be another than the database password's).
#[test]
fn deleting_a_profile_removes_its_tunnels_secret_too() {
    use datarig_core::profile::ssh::SshSettings;
    let mut cfg = sample_config(None);
    let mut ssh = SshSettings { enabled: true, host: "b".into(), user: "u".into(), ..SshSettings::default() };
    ssh.set_source(PasswordSource::File);
    cfg.connections[1].ssh = Some(ssh);
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    let id = h.app.profiles.iter().find(|p| p.name == "分析-replica").unwrap().id;
    let (db, tunnel) = (id.account(), SshSettings::account(id));
    h.app.secrets.stores().keychain.set(&db, "db-pw").unwrap();
    h.app.secrets.stores().file.set(&tunnel, "passphrase").unwrap();
    h.app.secrets.stores().keychain.set(&tunnel, "not its store").unwrap();
    h.explore("分析-replica");
    h.keys("d");
    h.key(KeyCode::Char('y'));
    assert!(h.app.profiles.iter().all(|p| p.id != id));
    assert_eq!(h.app.secrets.stores().keychain.get(&db).unwrap(), None);
    assert_eq!(h.app.secrets.stores().file.get(&tunnel).unwrap(), None);
    assert_eq!(h.app.secrets.stores().keychain.get(&tunnel).unwrap().as_deref(), Some("not its store"));
}
