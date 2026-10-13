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
                RowKind::TunnelsHeader => "[tunnels]".to_string(),
                RowKind::Tunnel(id) => format!("~{}", a.preset(id).unwrap().name),
                RowKind::TunnelError(_) => "!".to_string(),
                RowKind::TunnelUser(_, id) => format!("@{}", a.profile(id).unwrap().name),
                RowKind::TunnelsEmpty => "(no tunnels)".to_string(),
                RowKind::EnvironmentsHeader => "[environments]".to_string(),
                RowKind::Environment(id) => format!("%{}", a.environment(id).unwrap().name),
                RowKind::EnvironmentUser(_, id) => format!("@{}", a.profile(id).unwrap().name),
                RowKind::EnvironmentPresets => "(add presets)".to_string(),
                RowKind::DefaultRules => "(default rules)".to_string(),
                RowKind::DefaultRulesUser(id) => format!("@{}", a.profile(id).unwrap().name),
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
    // An empty filter (spaces only too) is no filter: every row is listed.
    assert!(Needle::new("").is_empty() && Needle::new("  ").is_empty());
    assert!(Needle::new("PROD").profile(&profile("orders-prod", None)));
    assert!(Needle::new("分析").profile(&profile("分析-replica", None)));
    assert!(!Needle::new("ord prod").profile(&profile("orders-prod", None)), "a part, not a fuzzy match");
    assert!(!Needle::new("zzz").profile(&profile("orders-prod", None)));
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

#[test]
fn the_needle_matches_a_part_ignoring_case_without_folding_each_name() {
    let n = Needle::new("  Order ");
    assert!(n.matches("ORDER_ITEMS") && n.matches("big_orders") && !n.matches("ord"));
    assert!(Needle::new("").matches("anything"));
    // Non-ASCII on either side.
    assert!(Needle::new("分析").matches("分析-replica"));
    assert!(Needle::new("ÉTÉ").matches("table_été") && !Needle::new("été").matches("table_ete"));
    assert!(Needle::new("straße").matches("STRASSE_straße"));
    assert!(!Needle::new("é").matches("plain ascii"));
}

#[test]
fn a_folder_whose_name_matches_shows_all_it_has() {
    let mut a = app(vec![
        profile("alpha", Some("reports")),
        profile("beta", Some("reports/2024")),
        profile("gamma", None),
        profile("reports-db", Some("other")),
    ]);
    open(&mut a, "other");
    a.explorer.filter.set("report");
    // `reports` matches: every profile and subfolder in it, the subfolder as it is (closed);
    // `other` opens for the profile that matches in it.
    assert_eq!(rows(&a), ["+", "other/", "  reports-db", "reports/", "  2024/", "  alpha"]);
    let shown = a.explorer_rows();
    assert!(shown[3].hit && shown[3].opened, "the folder matched and shows open");
    assert!(!shown[4].opened && a.explorer_arrow(&shown[4]) == Some(false), "its subfolder as it is");
    assert!(shown[1].opened && !shown[1].hit, "opened for a match inside");
    // The filter changed no folder.
    assert!(!a.folders.is_expanded(&FolderPath::parse("reports").unwrap()));
    a.explorer.filter.set("");
    assert_eq!(rows(&a), ["+", "other/", "  reports-db", "reports/", "gamma"]);
}
