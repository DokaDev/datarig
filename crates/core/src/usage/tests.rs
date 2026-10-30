use super::*;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("datarig-usage-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_missing_file_is_empty_usage() {
    let d = dir("missing");
    assert_eq!(load(&d.join(FILE)), Ok(Usage::default()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn counts_nudges_and_the_onboarding_round_trip() {
    let d = dir("round");
    let path = d.join(FILE);
    let mut u = Usage::default();
    assert_eq!(u.bump(Via::Key, "query.execute_current"), 1);
    assert_eq!(u.bump(Via::Key, "query.execute_current"), 2);
    u.bump(Via::Menu, "explorer.show_ddl");
    u.bump(Via::Command, ":theme");
    u.nudged.insert("explorer.show_ddl".into());
    u.steps.insert("menu.open".into());
    u.onboarding_done = true;
    save(&path, &u).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("onboarding = \"done\""), "{text}");
    assert!(text.contains("steps = [\"menu.open\"]"), "{text}");
    assert!(text.contains("\"query.execute_current\" = 2"), "{text}");
    assert_eq!(load(&path), Ok(u.clone()));
    assert_eq!(u.total(), 4);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_count_stops_at_the_maximum() {
    let mut u = Usage::default();
    u.keys.insert("a".into(), MAX_COUNT);
    assert_eq!(u.bump(Via::Key, "a"), MAX_COUNT);
}

#[test]
fn an_unreadable_file_is_never_written_over() {
    let d = dir("damaged");
    let path = d.join(FILE);
    let damaged = "[keys]\n\"a\" = 1\nnudged = [\n";
    std::fs::write(&path, damaged).unwrap();
    assert_eq!(load(&path), Err(Unreadable));
    let mut u = Usage::default();
    u.bump(Via::Key, "b");
    u.onboarding_done = true;
    let e = save(&path, &u).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    // A folder in the file's place cannot be read either.
    let folder = d.join("folder");
    std::fs::create_dir_all(folder.join(FILE)).unwrap();
    assert_eq!(load(&folder.join(FILE)), Err(Unreadable));
    assert!(save(&folder.join(FILE), &u).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unknown_entries_are_kept_and_counts_only_grow() {
    let d = dir("merge");
    let path = d.join(FILE);
    let before = "# mine\nlater = \"kept\"\nonboarding = \"later-value\"\nnudged = [\"old.action\"]\n\
                  steps = \"odd\"\n\n[keys]\n\"future.action\" = 7\n\"query.cancel\" = 5\n\"odd\" = \"x\"\n";
    std::fs::write(&path, before).unwrap();
    let mut u = load(&path).unwrap();
    assert!(!u.onboarding_done, "an onboarding value this version does not know is not done");
    assert!(!u.keys.contains_key("odd"), "not a count: left out");
    // Another instance counted less meanwhile: the larger count stays.
    u.keys.insert("query.cancel".into(), 2);
    u.bump(Via::Key, "editor.complete");
    u.nudged.insert("explorer.show_ddl".into());
    u.steps.insert("menu.open".into());
    save(&path, &u).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for kept in [
        "# mine",
        "later = \"kept\"",
        "onboarding = \"later-value\"",
        "steps = \"odd\"",
        "\"future.action\" = 7",
        "\"odd\" = \"x\"",
    ] {
        assert!(text.contains(kept), "{kept}: {text}");
    }
    assert!(text.contains("\"query.cancel\" = 5"), "{text}");
    assert!(text.contains("\"editor.complete\" = 1"), "{text}");
    let back = load(&path).unwrap();
    assert_eq!(back.nudged, ["explorer.show_ddl".to_string(), "old.action".to_string()].into());
    let _ = std::fs::remove_dir_all(&d);
}
