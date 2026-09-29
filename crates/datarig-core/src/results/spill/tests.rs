//! The spill directory's permissions.

#[cfg(unix)]
#[test]
fn a_spill_dir_of_another_user_is_refused_and_left_alone() {
    use super::{create_fault, make_private};
    use crate::fault::FaultKind;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = std::env::temp_dir().join(format!("datarig-spill-owner-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mode = || std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    let owner = std::fs::metadata(&dir).unwrap().uid();
    // Owned by someone else (here: the directory's owner is not the user asking).
    let e = make_private(&dir, owner.wrapping_add(1)).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    let fault = create_fault(&e, &dir);
    assert_eq!(fault.kind, FaultKind::NotPrivate);
    assert!(fault.detail.contains("owned by user"), "{}", fault.detail);
    assert_eq!(mode(), 0o755, "another user's directory is not touched");
    // Ours: restricted.
    make_private(&dir, owner).unwrap();
    assert_eq!(mode(), 0o700);
    // Any other failure keeps its io kind.
    let other = create_fault(&std::io::Error::from(std::io::ErrorKind::StorageFull), &dir);
    assert_eq!(other.kind, FaultKind::Io(std::io::ErrorKind::StorageFull));
    let _ = std::fs::remove_dir_all(&dir);
}
