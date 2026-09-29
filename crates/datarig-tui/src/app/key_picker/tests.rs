use super::*;

#[test]
fn likely_keys_stand_out_and_public_ones_are_dimmed() {
    for n in ["id_ed25519", "id_rsa", "bastion.pem", "prod.KEY", "ID_ECDSA"] {
        assert_eq!(key_look(n), KeyLook::Likely, "{n}");
    }
    for n in ["id_ed25519.pub", "known_hosts", "known_hosts.old", "config", "authorized_keys", "k.pem.pub"] {
        assert_eq!(key_look(n), KeyLook::Dim, "{n}");
    }
    for n in ["notes.txt", "deploy", "work.ppk"] {
        assert_eq!(key_look(n), KeyLook::Other, "{n}");
    }
}

#[test]
fn a_path_is_told_from_a_filter_and_tilde_is_home() {
    assert!(is_path("~/.ssh/id") && is_path("/etc/ssh") && is_path("keys/a") && is_path("~"));
    assert!(!is_path("id_") && !is_path(".pem"));
    let home = Path::new("/home/u");
    assert_eq!(expand("~/.ssh/id", Some(home)), PathBuf::from("/home/u/.ssh/id"));
    assert_eq!(expand("~", Some(home)), PathBuf::from("/home/u"));
    assert_eq!(expand("/k.pem", Some(home)), PathBuf::from("/k.pem"));
    assert_eq!(shown(Path::new("/home/u/.ssh/id"), Some(home)), "~/.ssh/id");
    assert_eq!(shown(Path::new("/k.pem"), Some(home)), "/k.pem");
    assert_eq!(shown_dir(home, Some(home)), "~");
    assert_eq!(shown_dir(Path::new("/home/u/.ssh"), Some(home)), "~/.ssh");
}
