use super::*;

#[test]
fn toml_faults_know_their_line() {
    let text = "a = 1\nb = [\n";
    let e = text.parse::<toml::Table>().unwrap_err();
    let f = Fault::toml_de(text, &e);
    assert!(matches!(f.kind, FaultKind::Toml { line: Some(2 | 3) }), "{f:?}");
    assert!(!f.detail.is_empty());
    let e = "x = = 1".parse::<toml_edit::DocumentMut>().unwrap_err();
    assert_eq!(Fault::toml_edit("x = = 1", &e).kind, FaultKind::Toml { line: Some(1) });
}

#[test]
fn the_log_keeps_the_raw_detail_on_one_line() {
    let dir = std::env::temp_dir().join(format!("datarig-fault-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let log = ErrorLog::new(Some(dir.join("errors.log")));
    let e = io::Error::new(io::ErrorKind::PermissionDenied, "Permission denied (os error 13)");
    log.record("workspace.broken", &Fault::io_at(&e, Path::new("/x/workspace.toml")));
    log.record("config.error", &Fault::other("line one\nline two"));
    log.record("db.transport", &Fault::other(""));
    let text = std::fs::read_to_string(dir.join("errors.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[2].ends_with("db.transport: Other (no detail given)"), "{text}");
    assert!(lines[0].contains("workspace.broken: Io(PermissionDenied): /x/workspace.toml: Permission denied"));
    assert!(lines[1].ends_with("config.error: Other: line one | line two"));
    ErrorLog::default().record("nowhere", &Fault::other("x"));
    let _ = std::fs::remove_dir_all(&dir);
}
