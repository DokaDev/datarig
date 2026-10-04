use super::*;
use crate::secret::MemoryStore;
use std::sync::Arc;

const T: Duration = Duration::from_secs(10);

fn stores() -> Stores {
    Stores::with_keychain(Arc::new(MemoryStore::new()))
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn kinds_and_defaults() {
    for k in SourceKind::ALL {
        assert_eq!(SourceKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(SourceKind::parse(" File "), Some(SourceKind::File));
    assert_eq!(SourceKind::parse("vault"), None);
    assert!(SourceKind::Keychain.stores_secret() && SourceKind::File.stores_secret());
    assert!(!SourceKind::Command.stores_secret() && !SourceKind::Env.stores_secret());
    for d in DefaultSource::ALL {
        assert_eq!(DefaultSource::parse(d.as_str()), Some(d));
    }
    // `auto`: the keychain when it works, else a prompt; an explicit keychain likewise.
    assert_eq!(DefaultSource::Auto.resolve(true), SourceKind::Keychain);
    assert_eq!(DefaultSource::Auto.resolve(false), SourceKind::Prompt);
    assert_eq!(DefaultSource::Kind(SourceKind::Keychain).resolve(false), SourceKind::Prompt);
    assert_eq!(DefaultSource::Kind(SourceKind::File).resolve(false), SourceKind::File);
    assert_eq!(DefaultSource::Kind(SourceKind::Env).resolve(true), SourceKind::Env);
}

#[test]
fn env_names() {
    for ok in ["PGPASSWORD", "_x", "db_prod_2"] {
        assert!(valid_env_name(ok), "{ok}");
    }
    for bad in ["", "2DB", "DB-PASS", "DB PASS", "$DB", "秘密"] {
        assert!(!valid_env_name(bad), "{bad}");
    }
}

#[test]
fn debug_never_prints_a_command() {
    let s = format!("{:?}", PasswordSource::Command("echo hunter2".into()));
    assert!(!s.contains("hunter2"), "{s}");
    assert!(!format!("{:?}", Lookup::Found("hunter2".into())).contains("hunter2"));
}

#[test]
fn lookup_per_source() {
    let s = stores();
    s.keychain.set("profile:a", "kc").unwrap();
    s.file.set("profile:a", "fi").unwrap();
    let env = |k: &str| (k == "DB_PW").then(|| "from-env".to_string());
    let look = |src: PasswordSource| lookup(&src, "profile:a", &s, &env, T);
    assert_eq!(look(PasswordSource::Keychain), Ok(Lookup::Found("kc".into())));
    assert_eq!(look(PasswordSource::File), Ok(Lookup::Found("fi".into())));
    assert_eq!(look(PasswordSource::Env("DB_PW".into())), Ok(Lookup::Found("from-env".into())));
    assert_eq!(look(PasswordSource::Env("NOPE".into())), Err(SourceError::EnvMissing("NOPE".into())));
    assert_eq!(look(PasswordSource::Prompt), Ok(Lookup::Prompt));
    assert_eq!(lookup(&PasswordSource::Keychain, "profile:b", &s, &no_env, T), Ok(Lookup::Missing));
    let empty = |_: &str| Some(String::new());
    assert_eq!(
        lookup(&PasswordSource::Env("E".into()), "profile:a", &s, &empty, T),
        Err(SourceError::EnvMissing("E".into())),
        "an empty variable is no password"
    );
    let down = Stores::with_keychain(Arc::new(MemoryStore::unavailable("locked")));
    let e = lookup(&PasswordSource::Keychain, "profile:a", &down, &no_env, T).unwrap_err();
    assert_eq!((e.kind(), e), (SourceKind::Keychain, SourceError::Keychain(nk("locked"))));
    let e = lookup(&PasswordSource::Command(String::new()), "profile:a", &s, &no_env, T).unwrap_err();
    assert_eq!((e.kind(), e), (SourceKind::Command, SourceError::Command(CommandError::Empty)));
}

#[cfg(unix)]
#[test]
fn lookup_runs_the_command_and_checks_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let s = stores();
    let cmd = PasswordSource::Command("printf 'from-cmd\\n'".into());
    assert_eq!(lookup(&cmd, "profile:a", &s, &no_env, T), Ok(Lookup::Found("from-cmd".into())));
    let dir = std::env::temp_dir().join(format!("datarig-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("secrets.toml");
    let file = crate::secret::FileStore::new(path.clone());
    file.set("profile:a", "fi").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let s = Stores { file: Arc::new(file), file_path: Some(path.clone()), ..stores() };
    assert_eq!(
        lookup(&PasswordSource::File, "profile:a", &s, &no_env, T),
        Err(SourceError::File(FileError::Insecure { path: path.clone(), mode: 0o644 }))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn ok() -> Result<(), crate::fault::Fault> {
    Ok(())
}

#[test]
fn switching_moves_the_password_then_removes_the_old_copy() {
    let s = stores();
    s.keychain.set("profile:a", "pw").unwrap();
    let r = switch(&s, SourceKind::Keychain, SourceKind::File, "profile:a", None, ok).unwrap();
    assert_eq!(r, Switched { copied: true, removed: true, remove_failed: None });
    assert_eq!(s.file.get("profile:a").unwrap().as_deref(), Some("pw"));
    assert_eq!(s.keychain.get("profile:a").unwrap(), None);
    // Back, with a password typed in the form: the typed one wins.
    let r = switch(&s, SourceKind::File, SourceKind::Keychain, "profile:a", Some("typed"), ok).unwrap();
    assert!(r.copied && r.removed);
    assert_eq!(s.keychain.get("profile:a").unwrap().as_deref(), Some("typed"));
    assert_eq!(s.file.get("profile:a").unwrap(), None);
    // To a source that stores nothing: saved, then removed.
    let r = switch(&s, SourceKind::Keychain, SourceKind::Env, "profile:a", None, ok).unwrap();
    assert_eq!(r, Switched { copied: false, removed: true, remove_failed: None });
    assert_eq!(s.keychain.get("profile:a").unwrap(), None);
    // From a source that stores nothing, with nothing typed: nothing to copy, nothing removed.
    let r = switch(&s, SourceKind::Prompt, SourceKind::File, "profile:a", None, ok).unwrap();
    assert_eq!(r, Switched::default());
    assert_eq!(s.file.get("profile:a").unwrap(), None);
}

/// Accepts writes but gives back something else.
struct Forgetful;
impl SecretStore for Forgetful {
    fn get(&self, _: &str) -> Result<Option<String>, Unavailable> {
        Ok(Some("garbled".into()))
    }
    fn set(&self, _: &str, _: &str) -> Result<(), Unavailable> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<bool, Unavailable> {
        Ok(true)
    }
}

#[test]
fn a_failed_switch_never_loses_the_password() {
    // The new store is down: nothing saved, the old copy stays.
    let s = Stores { file: Arc::new(MemoryStore::unavailable("disk full")), ..stores() };
    s.keychain.set("profile:a", "pw").unwrap();
    let mut saved = false;
    let e = switch(&s, SourceKind::Keychain, SourceKind::File, "profile:a", None, || {
        saved = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(e, SwitchError::Write(SourceError::File(FileError::Io(nk("disk full")))));
    assert!(!saved, "the config is not changed");
    assert_eq!(s.keychain.get("profile:a").unwrap().as_deref(), Some("pw"));
    // The read back differs: stop.
    let s = Stores { file: Arc::new(Forgetful), ..stores() };
    s.keychain.set("profile:a", "pw").unwrap();
    let e = switch(&s, SourceKind::Keychain, SourceKind::File, "profile:a", None, ok).unwrap_err();
    assert_eq!(e, SwitchError::Mismatch(SourceKind::File));
    assert_eq!(s.keychain.get("profile:a").unwrap().as_deref(), Some("pw"));
    // The old store cannot be read and nothing was typed: stop before writing anything.
    let s = Stores { keychain: Arc::new(MemoryStore::unavailable("locked")), ..stores() };
    let e = switch(&s, SourceKind::Keychain, SourceKind::File, "profile:a", None, ok).unwrap_err();
    assert_eq!(e, SwitchError::Read(SourceError::Keychain(nk("locked"))));
    assert_eq!(s.file.get("profile:a").unwrap(), None);
    // Saving the config fails: both copies stay.
    let s = stores();
    s.keychain.set("profile:a", "pw").unwrap();
    let e = switch(&s, SourceKind::Keychain, SourceKind::File, "profile:a", None, || Err("read-only".into()));
    assert_eq!(e, Err(SwitchError::Config("read-only".into())));
    assert_eq!(s.keychain.get("profile:a").unwrap().as_deref(), Some("pw"));
    assert_eq!(s.file.get("profile:a").unwrap().as_deref(), Some("pw"));
    // Removing the old copy fails after the save: reported, the change stands.
    let s = Stores { keychain: Arc::new(MemoryStore::unavailable("locked")), ..stores() };
    let r = switch(&s, SourceKind::Keychain, SourceKind::Prompt, "profile:a", None, ok).unwrap();
    assert_eq!(r.remove_failed, Some(SourceError::Keychain(nk("locked"))));
}

/// The fault a `MemoryStore::unavailable(detail)` gives.
fn nk(detail: &str) -> crate::fault::Fault {
    crate::fault::Fault::new(crate::fault::FaultKind::Keychain(crate::fault::KeychainFault::NoStore), detail)
}

#[test]
fn rekey_moves_a_secret_to_another_account_without_losing_it() {
    // The old copy, written under the new account and read back; removed only in step 3.
    let s = stores();
    s.keychain.set("profile:a:ssh", "pass").unwrap();
    assert_eq!(rekey_copy(&s, SourceKind::Keychain, "profile:a:ssh", "tunnel:t", None), Ok(true));
    assert_eq!(s.keychain.get("tunnel:t").unwrap().as_deref(), Some("pass"));
    assert_eq!(s.keychain.get("profile:a:ssh").unwrap().as_deref(), Some("pass"), "kept until step 3");
    let r = rekey_remove(&s, SourceKind::Keychain, "profile:a:ssh");
    assert!(r.removed && r.remove_failed.is_none());
    assert_eq!(s.keychain.get("profile:a:ssh").unwrap(), None);
    // A typed one wins over the stored one; the secrets file works the same way.
    let s = stores();
    s.file.set("profile:a:ssh", "old").unwrap();
    assert_eq!(rekey_copy(&s, SourceKind::File, "profile:a:ssh", "tunnel:t", Some("new")), Ok(true));
    assert_eq!(s.file.get("tunnel:t").unwrap().as_deref(), Some("new"));
    // Nothing stored and nothing typed: nothing written.
    let s = stores();
    assert_eq!(rekey_copy(&s, SourceKind::Keychain, "profile:a:ssh", "tunnel:t", None), Ok(false));
    assert_eq!(s.keychain.get("tunnel:t").unwrap(), None);
    // A source that stores nothing has nothing to move.
    assert_eq!(rekey_copy(&s, SourceKind::Prompt, "profile:a:ssh", "tunnel:t", Some("x")), Ok(false));
    // Unknown is not absent: a keychain that cannot be read stops the move before anything is
    // written (and the old copy is never removed).
    let s = Stores { keychain: Arc::new(MemoryStore::unavailable("locked")), ..stores() };
    assert_eq!(
        rekey_copy(&s, SourceKind::Keychain, "profile:a:ssh", "tunnel:t", None),
        Err(SwitchError::Read(SourceError::Keychain(nk("locked"))))
    );
    // The new copy reads back different: stop.
    let s = Stores { keychain: Arc::new(Forgetful), ..stores() };
    assert_eq!(
        rekey_copy(&s, SourceKind::Keychain, "profile:a:ssh", "tunnel:t", Some("x")),
        Err(SwitchError::Mismatch(SourceKind::Keychain))
    );
    // Removing the old copy fails: said, nothing else changes.
    let s = Stores { keychain: Arc::new(MemoryStore::unavailable("locked")), ..stores() };
    assert_eq!(
        rekey_remove(&s, SourceKind::Keychain, "profile:a:ssh").remove_failed,
        Some(SourceError::Keychain(nk("locked")))
    );
}
