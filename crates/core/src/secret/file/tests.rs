use super::*;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-secfile-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(FILE_NAME)
}

#[test]
fn roundtrip_keyed_by_profile_id() {
    let path = temp("roundtrip");
    let store = FileStore::new(path.clone());
    assert_eq!(store.get("profile:abc"), Ok(None), "no file yet");
    store.set("profile:abc", "pw \"quoted\" 🐘").unwrap();
    store.set("profile:def", "other").unwrap();
    assert_eq!(store.get("profile:abc").unwrap().as_deref(), Some("pw \"quoted\" 🐘"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# datarig passwords"), "{text}");
    assert!(text.contains("[passwords]\nabc = ") && !text.contains("profile:"), "{text}");
    assert_eq!(store.delete("profile:abc"), Ok(true));
    assert_eq!(store.delete("profile:abc"), Ok(false), "already gone");
    assert_eq!(store.get("profile:abc"), Ok(None));
    assert_eq!(store.get("profile:def").unwrap().as_deref(), Some("other"), "the others stay");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn next_to_the_config_file() {
    let p = FileStore::next_to(Path::new("/home/u/.config/datarig/config.toml"));
    assert_eq!(p, Path::new("/home/u/.config/datarig").join("secrets.toml"));
}

#[test]
fn a_broken_file_is_an_error_not_an_empty_store() {
    let path = temp("broken");
    std::fs::write(&path, "[passwords\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let store = FileStore::new(path.clone());
    assert!(store.get("profile:a").is_err());
    assert!(store.set("profile:a", "x").is_err(), "never overwrites what it cannot read");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[passwords\n");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn created_0600_and_refused_when_others_have_access() {
    use std::os::unix::fs::PermissionsExt;
    let path = temp("perms");
    let store = FileStore::new(path.clone());
    store.set("profile:a", "pw").unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(check_permissions(&path), Ok(()));
    for bad in [0o644, 0o640, 0o604, 0o660, 0o606] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(bad)).unwrap();
        assert_eq!(check_permissions(&path), Err(FileError::Insecure { path: path.clone(), mode: bad }));
        let e = store.get("profile:a").unwrap_err();
        assert!(e.0.detail.contains("chmod 600"), "{e:?}");
        assert!(store.set("profile:a", "new").is_err() && store.delete("profile:a").is_err());
        assert_eq!(mode(&path), bad, "not touched");
    }
    // Owner-only modes other than 0600 are fine (0400: read-only).
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(store.get("profile:a").unwrap().as_deref(), Some("pw"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
