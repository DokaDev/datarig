use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-fs-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect()
}

#[test]
fn writes_new_and_replaces_existing_files() {
    let dir = temp_dir("write");
    let path = dir.join("sub").join("a.toml");
    atomic_write(&path, b"one").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "one", "parent directories are created");
    atomic_write(&path, b"two").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    assert!(leftovers(&dir.join("sub")).is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn failure_leaves_no_temporary_file_and_keeps_the_target() {
    let dir = temp_dir("fail");
    // A directory where the file should go: the rename fails.
    let path = dir.join("taken");
    fs::create_dir_all(path.join("inner")).unwrap();
    assert!(atomic_write(&path, b"x").is_err());
    assert!(path.join("inner").is_dir(), "target untouched");
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn keeps_permissions_and_writes_through_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unix");
    let real = dir.join("real.toml");
    fs::write(&real, "old").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("link.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    atomic_write(&link, b"new").unwrap();
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
    assert_eq!(fs::read_to_string(&real).unwrap(), "new", "the target got the bytes");
    assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600, "mode kept");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn private_files_are_always_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("private");
    let path = dir.join("secrets.toml");
    atomic_write_private(&path, b"one").unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "a new file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    atomic_write_private(&path, b"two").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "never the old mode");
    assert!(leftovers(&dir).is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn create_new_and_rename_new_never_replace_a_file() {
    let dir = temp_dir("noclobber");
    let path = dir.join("sub").join("a.sql");
    create_new(&path, b"first").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first", "parent directories are created");
    assert_eq!(create_new(&path, b"second").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&path).unwrap(), "first");
    let other = dir.join("sub").join("b.sql");
    fs::write(&other, "other").unwrap();
    assert_eq!(rename_new(&other, &path).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        (fs::read_to_string(&path).unwrap(), fs::read_to_string(&other).unwrap()),
        ("first".into(), "other".into())
    );
    let free = dir.join("sub").join("c.sql");
    rename_new(&other, &free).unwrap();
    assert!(!other.exists() && fs::read_to_string(&free).unwrap() == "other");
    let names: Vec<String> = fs::read_dir(dir.join("sub"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.starts_with('.')), "no temporary files: {names:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_link_is_written_through_never_replaced() {
    let dir = temp_dir("links");
    // A dotfiles link to a file that does not exist yet: the file is created there.
    std::os::unix::fs::symlink("real/config.toml", dir.join("config.toml")).unwrap();
    fs::create_dir_all(dir.join("real")).unwrap();
    atomic_write(&dir.join("config.toml"), b"one").unwrap();
    assert!(fs::symlink_metadata(dir.join("config.toml")).unwrap().file_type().is_symlink(), "still a link");
    assert_eq!(fs::read_to_string(dir.join("real").join("config.toml")).unwrap(), "one");
    // A link that cannot be resolved (a loop) is an error, not replaced by a file.
    std::os::unix::fs::symlink("loop-b", dir.join("loop-a")).unwrap();
    std::os::unix::fs::symlink("loop-a", dir.join("loop-b")).unwrap();
    assert!(atomic_write(&dir.join("loop-a"), b"x").is_err());
    assert!(fs::symlink_metadata(dir.join("loop-a")).unwrap().file_type().is_symlink());
    let _ = fs::remove_dir_all(&dir);
}
