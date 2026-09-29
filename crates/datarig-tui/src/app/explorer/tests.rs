use super::*;
use datarig_core::config::Config;
use datarig_core::i18n::Lang;

fn profile(name: &str, folder: Option<&str>) -> ConnectionConfig {
    ConnectionConfig { name: name.into(), folder: folder.map(String::from), ..ConnectionConfig::default() }
}

fn app(profiles: Vec<ConnectionConfig>) -> App {
    App::new(&Config { connections: profiles, ..Config::default() }, None, Lang::En)
}

/// Rows as text: `+`, `name/` for folders, profile names, `!` for error lines, indented.
fn rows(a: &App) -> Vec<String> {
    a.explorer_rows()
        .into_iter()
        .map(|r| {
            let t = match r.kind {
                RowKind::NewConnection => "+".to_string(),
                RowKind::Folder(f) => format!("{}/", f.name()),
                RowKind::Profile(id) => a.profile(id).unwrap().name.clone(),
                RowKind::ProfileError(_) => "!".to_string(),
                RowKind::Database(_, db) => format!("db {}", db.unwrap_or_default()),
                RowKind::DatabasesNote(_) | RowKind::DatabaseNote(..) => "note".to_string(),
                RowKind::Node(..) | RowKind::AuxNode(..) => "node".to_string(),
                RowKind::ScriptsHeader => "[saved queries]".to_string(),
                RowKind::ScriptFolder(p) => format!("{p}/"),
                RowKind::Script(p) => p,
                RowKind::ScriptsEmpty => "(none)".to_string(),
            };
            format!("{}{t}", "  ".repeat(r.depth))
        })
        .collect()
}

fn open(a: &mut App, path: &str) {
    let f = FolderPath::parse(path).unwrap();
    if !a.folders.is_expanded(&f) {
        a.folders.toggle(&f);
    }
}

#[test]
fn the_filter_matches_a_part_of_the_name_ignoring_case() {
    assert!(filter_matches("", &profile("x", None)));
    assert!(filter_matches("  ", &profile("x", None)));
    assert!(filter_matches("PROD", &profile("orders-prod", None)));
    assert!(filter_matches("分析", &profile("分析-replica", None)));
    assert!(!filter_matches("ord prod", &profile("orders-prod", None)), "a part, not a fuzzy match");
    assert!(!filter_matches("zzz", &profile("orders-prod", None)));
}

#[test]
fn folders_come_first_then_profiles_each_by_name() {
    let mut a = app(vec![
        profile("zeta", None),
        profile("Alpha", None),
        profile("b-prod", Some("work/prod")),
        profile("a-stage", Some("work")),
        profile("beta", None),
    ]);
    assert_eq!(rows(&a), ["+", "work/", "Alpha", "beta", "zeta"], "folders start closed");
    open(&mut a, "work");
    assert_eq!(rows(&a), ["+", "work/", "  prod/", "  a-stage", "Alpha", "beta", "zeta"]);
    open(&mut a, "work/prod");
    assert_eq!(rows(&a), ["+", "work/", "  prod/", "    b-prod", "  a-stage", "Alpha", "beta", "zeta"]);
    // An empty folder is listed too (the config keeps it).
    a.folders.insert(&FolderPath::parse("archive").unwrap());
    assert_eq!(rows(&a)[1], "archive/");
}

#[test]
fn the_filter_shows_matching_profiles_inside_their_folders() {
    let mut a = app(vec![
        profile("orders-prod", Some("work/prod")),
        profile("orders-stage", Some("work")),
        profile("scratch", Some("local")),
        profile("orders-local", None),
    ]);
    a.explorer.filter.set("orders");
    assert_eq!(
        rows(&a),
        ["+", "work/", "  prod/", "    orders-prod", "  orders-stage", "orders-local"],
        "folders of matches open, others hidden"
    );
    a.explorer.filter.set("zzz");
    assert_eq!(rows(&a), ["+"]);
}

#[test]
fn the_cursor_stays_on_its_row_when_rows_change_around_it() {
    let mut a = app(vec![profile("a", Some("f")), profile("b", None), profile("c", None)]);
    let c = a.profiles[2].id;
    a.reveal_profile(c);
    assert_eq!(a.explorer_selected(), 3);
    // A folder above opens: the cursor follows its profile.
    open(&mut a, "f");
    assert_eq!(a.explorer_selected(), 4);
    assert_eq!(a.selected_profile(), Some(c));
    // Its row disappears (deleted): the cursor stays where it was, clamped.
    a.profiles.remove(2);
    assert_eq!(a.explorer_selected(), 3);
    // Revealing a profile in a closed folder opens the folder.
    let mut a = app(vec![profile("deep", Some("x/y")), profile("top", None)]);
    let deep = a.profiles[0].id;
    a.reveal_profile(deep);
    assert_eq!(rows(&a), ["+", "x/", "  y/", "    deep", "top"]);
    assert_eq!(a.selected_profile(), Some(deep));
}

#[test]
fn a_profile_in_an_unknown_folder_shows_at_the_top() {
    // The loader adds the folders of profiles; a bad path is kept but shown at the top level.
    let a = app(vec![profile("bad", Some("a//b"))]);
    assert_eq!(rows(&a), ["+", "bad"]);
}
