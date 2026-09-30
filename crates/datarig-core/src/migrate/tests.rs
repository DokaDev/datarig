use super::*;
use crate::secret::MemoryStore;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// A store whose `set` fails for some accounts, or gives back other values, or that counts writes.
#[derive(Default)]
struct Store {
    inner: MemoryStore,
    fail_set: Vec<String>,
    lie_on_get: Vec<String>,
    sets: Mutex<HashMap<String, u32>>,
}

impl SecretStore for Store {
    fn get(&self, a: &str) -> Result<Option<String>, Unavailable> {
        if self.lie_on_get.iter().any(|x| x == a) && self.sets.lock().unwrap().contains_key(a) {
            return Ok(Some("garbled".into()));
        }
        self.inner.get(a)
    }
    fn set(&self, a: &str, s: &str) -> Result<(), Unavailable> {
        if self.fail_set.iter().any(|x| x == a) {
            return Err(Unavailable("access denied".into()));
        }
        *self.sets.lock().unwrap().entry(a.to_string()).or_default() += 1;
        self.inner.set(a, s)
    }
    fn delete(&self, a: &str) -> Result<bool, Unavailable> {
        self.inner.delete(a)
    }
}

const V1: &str = "# my config\nlanguage = \"ko\"      # ui\nlast_used = \"b\"\n\n[[connections]]\nname = \"a\" # first\nhost = \"h1\"\n\n[[connections]]\nname = \"b\"\nhost = \"h2\"\npassword = \"plain-b\" # legacy\n";

fn setup(tag: &str, body: &str) -> (PathBuf, Config) {
    let dir = std::env::temp_dir().join(format!("datarig-migrate-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, body).unwrap();
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    (path, cfg)
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

fn account(cfg: &Config, name: &str) -> String {
    cfg.connections.iter().find(|c| c.name == name).unwrap().id.account()
}

#[test]
fn migrates_a_v1_file_and_its_keychain_entries() {
    let (path, cfg) = setup("ok", V1);
    assert!(cfg.needs_migration());
    let store = Store::default();
    store.inner.set("a", "kc-a").unwrap(); // name-keyed entry of v0.6
    let r = run(&store, &MemoryStore::new(), Input::new(&cfg));
    assert_eq!(r.keychain, Keychain::Done, "{r:?}");
    assert_eq!((r.rekeyed, r.version, r.failures.len(), r.conflicts.len()), (1, 2, 0, 0));
    assert_eq!(r.backup, Some(backup_path(&path)));
    assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), V1, "backup is the original");
    // Keychain: re-keyed by id, old entry removed after the check; plaintext moved.
    assert_eq!(store.get("a").unwrap(), None);
    assert_eq!(store.get(&account(&cfg, "a")).unwrap().as_deref(), Some("kc-a"));
    assert_eq!(store.get(&account(&cfg, "b")).unwrap().as_deref(), Some("plain-b"));
    assert_eq!(r.moved, [cfg.connections[1].id]);
    // File: version 2, ids, last_used as an id, no password, comments and order kept.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# my config\nlanguage = \"ko\"      # ui\n"), "{text}");
    assert!(text.contains(&format!("last_used = \"{}\"", cfg.connections[1].id)), "{text}");
    assert!(
        text.contains(&format!(
            "[[connections]]\nid = \"{}\"\nname = \"a\" # first\nhost = \"h1\"",
            cfg.connections[0].id
        )),
        "{text}"
    );
    assert!(text.contains("version = 2") && !text.contains("password"), "{text}");
    let (again, err) = config::load(Some(path.clone()));
    assert!(err.is_none() && !again.needs_migration(), "{err:?}");
    assert_eq!(again.connections[0].id, cfg.connections[0].id);
    // Another v1 file at the same place (e.g. restored by hand) keeps the first backup.
    std::fs::write(&path, V1.replace("h1", "h9")).unwrap();
    let (cfg2, _) = config::load(Some(path.clone()));
    let r2 = run(&store, &MemoryStore::new(), Input::new(&cfg2));
    assert_eq!(r2.backup, None, "an existing backup is never overwritten");
    assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), V1);
    cleanup(&path);
}

#[test]
fn run_twice_is_idempotent() {
    let (path, cfg) = setup("twice", V1);
    let store = Store::default();
    store.inner.set("a", "kc-a").unwrap();
    run(&store, &MemoryStore::new(), Input::new(&cfg));
    let text = std::fs::read_to_string(&path).unwrap();
    let (cfg2, _) = config::load(Some(path.clone()));
    assert!(!cfg2.needs_migration());
    let r = run(&store, &MemoryStore::new(), Input::new(&cfg2));
    assert_eq!((r.rekeyed, r.moved.len(), r.version), (0, 0, 2));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "same file");
    assert_eq!(store.get(&account(&cfg, "a")).unwrap().as_deref(), Some("kc-a"));
    cleanup(&path);
}

#[test]
fn saving_the_ids_fails_so_the_keychain_is_not_touched() {
    let (path, cfg) = setup("savefail", V1);
    // The config path becomes a directory: every save fails.
    let mut input = Input::new(&cfg);
    input.config.path = Some(path.parent().unwrap().join("dir.toml"));
    std::fs::create_dir_all(path.parent().unwrap().join("dir.toml").join("x")).unwrap();
    std::fs::write(backup_path(input.config.path.as_ref().unwrap()), "").ok();
    let store = Store::default();
    store.inner.set("a", "kc-a").unwrap();
    let r = run(&store, &MemoryStore::new(), input);
    assert!(r.save_failed.is_some(), "{r:?}");
    assert_eq!(store.get("a").unwrap().as_deref(), Some("kc-a"), "old entry untouched");
    assert!(store.sets.lock().unwrap().is_empty(), "nothing written to the keychain");
    assert_eq!(r.version, 1);
    cleanup(&path);
}

#[test]
fn a_mismatch_or_a_failed_write_keeps_the_old_entry_and_the_version() {
    let (path, cfg) = setup("partial", V1);
    let a = account(&cfg, "a");
    let b = account(&cfg, "b");
    let store = Store { lie_on_get: vec![a.clone()], fail_set: vec![b.clone()], ..Store::default() };
    store.inner.set("a", "kc-a").unwrap();
    let r = run(&store, &MemoryStore::new(), Input::new(&cfg));
    assert_eq!(r.keychain, Keychain::Partial);
    assert_eq!(r.failures.len(), 2, "{r:?}");
    assert_eq!(store.get("a").unwrap().as_deref(), Some("kc-a"), "not removed without a good read back");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("password = \"plain-b\""), "plaintext stays: {text}");
    assert!(!text.contains("version"), "version stays 1 so the next launch retries: {text}");
    assert!(text.contains("id = "), "the ids are saved already: {text}");
    // Next launch, with a working keychain: the same ids, and it finishes.
    let (cfg2, _) = config::load(Some(path.clone()));
    assert_eq!(cfg2.connections[0].id, cfg.connections[0].id);
    let good = Store::default();
    good.inner.set("a", "kc-a").unwrap();
    let r = run(&good, &MemoryStore::new(), Input::new(&cfg2));
    assert_eq!((r.keychain, r.version), (Keychain::Done, 2));
    cleanup(&path);
}

#[test]
fn an_existing_id_entry_is_never_overwritten() {
    let (path, cfg) = setup("exists", V1);
    let (a, b) = (account(&cfg, "a"), account(&cfg, "b"));
    // Same value: the old entry goes. Different value: both stay (maybe a newer password).
    let store = Store::default();
    store.inner.set("a", "same").unwrap();
    store.inner.set(&a, "same").unwrap();
    store.inner.set("b", "old-b").unwrap();
    store.inner.set(&b, "newer-b").unwrap();
    let r = run(&store, &MemoryStore::new(), Input::new(&cfg));
    assert_eq!(store.get("a").unwrap(), None);
    assert_eq!(store.get("b").unwrap().as_deref(), Some("old-b"));
    assert_eq!(store.get(&b).unwrap().as_deref(), Some("newer-b"), "not overwritten");
    // b's plaintext differs from its id entry too: it stays in the file.
    assert_eq!(r.conflicts, ["b", "b"]);
    assert!(std::fs::read_to_string(&path).unwrap().contains("plain-b"));
    assert!(store.sets.lock().unwrap().is_empty(), "no write at all");
    assert_eq!((r.keychain, r.version), (Keychain::Done, 2));
    cleanup(&path);
}

#[test]
fn without_a_keychain_the_migration_is_not_applicable() {
    let (path, cfg) = setup("nokeychain", V1);
    let r = run(&MemoryStore::unavailable("no secret service"), &MemoryStore::new(), Input::new(&cfg));
    assert_eq!(r.keychain, Keychain::Unavailable(nk("no secret service")));
    assert_eq!(r.version, 2, "done: nothing name-keyed can exist there");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("version = 2") && text.contains("password = \"plain-b\""), "{text}");
    cleanup(&path);
}

#[test]
fn a_new_profile_after_version_2_never_sees_an_old_name_entry() {
    let (path, cfg) = setup("v2new", "version = 2\n[[connections]]\nname = \"a\"\n");
    assert!(cfg.needs_migration() && cfg.ids_assigned);
    let store = Store::default();
    store.inner.set("a", "someone else's").unwrap();
    let r = run(&store, &MemoryStore::new(), Input::new(&cfg));
    assert_eq!((r.rekeyed, r.backup.clone(), r.version), (0, None, 2), "{r:?}");
    assert_eq!(store.get(&account(&cfg, "a")).unwrap(), None);
    assert_eq!(store.get("a").unwrap().as_deref(), Some("someone else's"), "left alone");
    assert!(std::fs::read_to_string(&path).unwrap().contains(&format!("id = \"{}\"", cfg.connections[0].id)));
    cleanup(&path);
}

#[test]
fn a_file_profile_moves_its_plaintext_to_the_secrets_file() {
    let body = "version = 2\n[[connections]]\nname = \"f\"\npassword = \"plain-f\"\npassword_source = \"file\"\n";
    let (path, cfg) = setup("file", body);
    assert!(cfg.needs_migration());
    let (keychain, file) = (Store::default(), MemoryStore::new());
    let r = run(&keychain, &file, Input::new(&cfg));
    assert_eq!(r.moved, [cfg.connections[0].id], "{r:?}");
    assert_eq!(file.get(&account(&cfg, "f")).unwrap().as_deref(), Some("plain-f"));
    assert!(keychain.sets.lock().unwrap().is_empty(), "not the keychain");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("plain-f") && text.contains("password_source = \"file\""), "{text}");
    // Even without a keychain (the file does not depend on it).
    let (path2, cfg2) = setup("file-nokc", body);
    let file = MemoryStore::new();
    let r = run(&MemoryStore::unavailable("no secret service"), &file, Input::new(&cfg2));
    assert_eq!(r.moved.len(), 1, "{r:?}");
    assert_eq!(file.get(&account(&cfg2, "f")).unwrap().as_deref(), Some("plain-f"));
    cleanup(&path);
    cleanup(&path2);
}

/// The fault a `MemoryStore::unavailable(detail)` gives.
fn nk(detail: &str) -> crate::fault::Fault {
    crate::fault::Fault::new(crate::fault::FaultKind::Keychain(crate::fault::KeychainFault::NoStore), detail)
}

#[test]
fn the_theme_survives_the_migration() {
    let (path, cfg) = setup("theme", &V1.replace("language = \"ko\"", "language = \"ko\"\ntheme = \"nord\""));
    let r = run(&Store::default(), &MemoryStore::new(), Input::new(&cfg));
    assert_eq!(r.version, 2, "{r:?}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("theme = \"nord\""), "{text}");
    cleanup(&path);
}
