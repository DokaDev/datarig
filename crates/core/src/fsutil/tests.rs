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

/// A link at a name a same-user process could guess, pointing at `victim`.
#[cfg(unix)]
fn plant(link: &Path, victim: &Path) {
    std::os::unix::fs::symlink(victim, link).unwrap();
}

/// The temporary names a write of `name` could be expected to use, derived from what is known
/// about the process: none of them may be written through.
#[cfg(unix)]
fn guessable(dir: &Path, name: &str) -> Vec<PathBuf> {
    let pid = std::process::id();
    [format!(".{name}.tmp-{pid}"), format!(".{name}.new-{pid}"), format!(".tmp-{pid}")]
        .into_iter()
        .map(|n| dir.join(n))
        .collect()
}

#[cfg(unix)]
#[test]
fn a_link_at_a_guessable_temporary_name_is_not_followed() {
    let dir = temp_dir("guess");
    let victim = dir.join("victim");
    fs::write(&victim, "keep").unwrap();
    let links = guessable(&dir, "a.toml");
    for link in &links {
        plant(link, &victim);
    }
    atomic_write(&dir.join("a.toml"), b"one").unwrap();
    atomic_write_private(&dir.join("a.toml"), b"two").unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep", "the link's target is untouched");
    assert_eq!(fs::read_to_string(dir.join("a.toml")).unwrap(), "two");
    create_new(&dir.join("b.toml"), b"new").unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep", "create_new leaves it too");
    for link in &links {
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink(), "someone else's link stays");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_taken_temporary_name_is_passed_over_never_opened() {
    let dir = temp_dir("taken-name");
    let victim = dir.join("victim");
    fs::write(&victim, "keep").unwrap();
    // A link to a file, a link to nowhere, a plain file: each name is taken.
    plant(&dir.join(".tmp-a"), &victim);
    plant(&dir.join(".tmp-b"), &dir.join("nowhere"));
    fs::write(dir.join(".tmp-c"), "theirs").unwrap();
    let mut names = ["a", "b", "c", "d"].map(String::from).into_iter();
    write_atomically_named(&dir.join("a.toml"), b"new", false, &mut names).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.toml")).unwrap(), "new");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert!(!dir.join("nowhere").exists(), "a dangling link is not created through");
    assert_eq!(fs::read_to_string(dir.join(".tmp-c")).unwrap(), "theirs");
    assert!(!dir.join(".tmp-d").exists(), "renamed over the target");

    let mut names = ["a", "b", "c"].map(String::from).into_iter();
    assert_eq!(
        create_new_named(&dir.join("b.toml"), b"new", &mut names).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists,
        "no free name: an error"
    );
    assert!(!dir.join("b.toml").exists());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn temporary_names_are_unpredictable_and_short() {
    let dir = temp_dir("names");
    let (one, _a) = create_temp(&dir, TempAccess::OwnerOnly).unwrap();
    let (two, _b) = create_temp(&dir, TempAccess::OwnerOnly).unwrap();
    assert_ne!(one, two);
    let name = one.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name.len(), ".tmp-".len() + 32, "{name}");
    let _ = fs::remove_dir_all(&dir);
}

/// A file name as long as a file system takes still fits: the temporary name does not grow
/// with it.
#[cfg(unix)]
#[test]
fn a_file_name_of_the_longest_length_can_be_written() {
    let dir = temp_dir("long");
    let path = dir.join("x".repeat(255));
    atomic_write(&path, b"one").unwrap();
    atomic_write(&path, b"two").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_temporary_file_is_never_more_open_than_the_file_it_replaces() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let dir = temp_dir("modes");
    let (owner, _f) = create_temp(&dir, TempAccess::OwnerOnly).unwrap();
    assert_eq!(mode(&owner), 0o600, "owner-only from the moment it exists");

    // Owner-only stays so, a wider mode is kept, a new file takes the umask's.
    let secret = dir.join("secret");
    fs::write(&secret, "old").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    atomic_write(&secret, b"new").unwrap();
    assert_eq!(mode(&secret), 0o600);
    let shared = dir.join("shared");
    fs::write(&shared, "old").unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o640)).unwrap();
    atomic_write(&shared, b"new").unwrap();
    assert_eq!(mode(&shared), 0o640);
    let plain = dir.join("plain");
    fs::write(&plain, "as a new file").unwrap();
    let usual = mode(&plain);
    let fresh = dir.join("fresh");
    atomic_write(&fresh, b"new").unwrap();
    assert_eq!(mode(&fresh), usual, "a new file has the usual permissions");
    let _ = fs::remove_dir_all(&dir);
}

/// What the rename does for a crash: until it, the target is the old file, whole.
#[test]
fn the_target_is_whole_until_the_rename() {
    let dir = temp_dir("whole");
    let path = dir.join("a.toml");
    atomic_write(&path, b"old").unwrap();
    let (tmp, mut f) = create_temp(&dir, TempAccess::Usual).unwrap();
    f.write_all(b"half").unwrap();
    // A crash here: the temporary file stays behind, and the target is untouched.
    drop(f);
    assert_eq!(fs::read_to_string(&path).unwrap(), "old");
    assert!(tmp.exists());
    let _ = fs::remove_dir_all(&dir);
}

/// What is already at a name the new file could take is left alone and another name used: a
/// link is not followed, a second name of the file is not truncated, a file keeps its bytes
/// and its mode. With every name taken nothing is written.
#[cfg(unix)]
#[test]
fn a_private_replacement_leaves_what_is_at_its_new_names_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("replace-private");
    let path = dir.join("h.toml");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let victim = dir.join("victim");
    fs::write(&victim, "victim").unwrap();
    std::os::unix::fs::symlink(&victim, dir.join(".h.toml.tmp-a")).unwrap();
    fs::hard_link(&path, dir.join(".h.toml.tmp-b")).unwrap();
    fs::write(dir.join(".h.toml.tmp-c"), "c").unwrap();
    fs::set_permissions(dir.join(".h.toml.tmp-c"), fs::Permissions::from_mode(0o644)).unwrap();
    let mut names = ["a", "b", "c", "d"].into_iter().map(String::from);
    replace_private_named(&path, b"new", &mut names).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "victim", "the link is not followed");
    assert_eq!(fs::read_to_string(dir.join(".h.toml.tmp-b")).unwrap(), "old", "the old file is not truncated");
    assert_eq!(fs::read_to_string(dir.join(".h.toml.tmp-c")).unwrap(), "c");
    assert_eq!(fs::metadata(dir.join(".h.toml.tmp-c")).unwrap().permissions().mode() & 0o777, 0o644);
    assert!(fs::symlink_metadata(dir.join(".h.toml.tmp-d")).is_err(), "renamed into place");
    let mut taken = ["a", "b", "c"].into_iter().map(String::from);
    let e = replace_private_named(&path, b"newer", &mut taken).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&path).unwrap(), "new", "untouched");
    let _ = fs::remove_dir_all(&dir);
}

/// The new file is 0600 from the moment it exists (not made so after its bytes are in).
#[cfg(unix)]
#[test]
fn a_private_new_file_is_0600_when_created() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("create-private");
    let path = dir.join("new");
    let f = create_private(&path).unwrap();
    assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&path).unwrap().len(), 0, "nothing written yet");
    drop(f);
    assert_eq!(create_private(&path).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    let _ = fs::remove_dir_all(&dir);
}

/// A link at the path, one to nothing too, is refused and left as it is.
#[cfg(unix)]
#[test]
fn a_private_replacement_refuses_a_link_at_the_path() {
    let dir = temp_dir("replace-link");
    let real = dir.join("real");
    fs::write(&real, "real").unwrap();
    std::os::unix::fs::symlink(&real, dir.join("to-real")).unwrap();
    std::os::unix::fs::symlink(dir.join("none"), dir.join("to-none")).unwrap();
    for link in ["to-real", "to-none"] {
        let e = replace_private(&dir.join(link), b"x").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{link}");
        assert!(fs::symlink_metadata(dir.join(link)).unwrap().file_type().is_symlink());
    }
    assert_eq!(fs::read_to_string(&real).unwrap(), "real");
    assert!(fs::symlink_metadata(dir.join("none")).is_err());
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    let _ = fs::remove_dir_all(&dir);
}
