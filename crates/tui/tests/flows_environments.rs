//! Environments, headless: the explorer's "Environments" section (the presets while there is
//! none, new, edit, rename, delete), the environment form, the profile form's environment
//! picker (a preset written with the profile, "New environment…" and back), a profile naming an
//! environment that is not defined (an error of that profile: it never connects, never runs
//! anything under the default rules), and the badge on the tab and in the status bar.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::policy::{Confirm, Environment, EnvironmentId, Policy};
use datarig_core::secret::MemoryStore;
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::profiles::{EnvChoice, Field};
use datarig_tui::app::{Focus, NodeState, Startup};
use datarig_tui::widgets::editor::Editor;
use datarig_tui::widgets::list_picker::{ListPicker, Row};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-envs-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

impl Scratch {
    fn config(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap_or_default()
    }
}

/// The sample profiles; `local-pg` names `prod`; the config file written to `dir` (when given).
fn config(dir: Option<&Scratch>, envs: Vec<Environment>) -> Config {
    let mut cfg = sample_config(None);
    cfg.connections[0].policy = Some("prod".into());
    cfg.policies.environments_mut().extend(envs);
    if let Some(d) = dir {
        cfg.path = Some(d.config());
    }
    cfg
}

/// `prod` (read-only, every write asks, red, `PROD`) and `dev`, as the file had them.
fn two() -> Vec<Environment> {
    let mut prod = Environment::new("prod", Policy { read_only: true, confirm: Confirm::Writes, ..Policy::default() });
    (prod.color, prod.badge, prod.origin) = (Some("red".into()), Some("PROD".into()), Some("prod".into()));
    let mut dev = Environment::new("dev", Policy::default());
    (dev.color, dev.origin) = (Some("green".into()), Some("dev".into()));
    vec![prod, dev]
}

fn harness_with(cfg: &Config, lang: Lang) -> Harness {
    Harness::launched(cfg, lang, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver()
}

fn env_id(h: &Harness, name: &str) -> EnvironmentId {
    h.app.environments().iter().find(|e| e.name == name).expect("environment").id
}

/// Focus the explorer on environment `name`.
fn on_env(h: &mut Harness, name: &str) {
    let id = env_id(h, name);
    h.app.focus = Focus::Tree;
    h.app.explorer.select_kind(RowKind::Environment(id));
}

/// Focus the explorer on the row of kind `kind`.
fn on_row(h: &mut Harness, kind: RowKind) {
    h.app.focus = Focus::Tree;
    h.app.explorer.select_kind(kind);
}

fn names(h: &Harness) -> Vec<String> {
    h.app.environments().iter().map(|e| e.name.clone()).collect()
}

fn said(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

fn env_list(h: &Harness) -> &ListPicker<EnvChoice> {
    h.app.overlays.picker().and_then(|p| p.environment()).expect("the environment list is open")
}

fn env_rows(h: &Harness) -> Vec<String> {
    let l = env_list(h);
    l.rows().iter().map(|r| l.label(*r).to_string()).collect()
}

/// The profile form of `name` on its environment field.
fn env_field(h: &mut Harness, name: &str) {
    h.explore(name);
    h.keys("e");
    h.ctrl('n');
    h.ctrl('n');
    while h.form().focus != Field::Environment {
        h.key(KeyCode::Tab);
    }
}

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

/// A profile naming an environment that is not defined does not connect: nothing is asked or
/// sent, its node says why, and its explorer line carries the environment mark in the error
/// color. It is never connected under the default rules instead.
#[test]
fn an_unknown_environment_is_an_error_of_the_profile() {
    let cfg = config(None, Vec::new());
    let mut h = harness_with(&cfg, Lang::En);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing is opened");
    let id = h.app.profiles[0].id;
    assert_eq!(h.app.conns.state(id), NodeState::Failed);
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some(
            "Environment “prod” not found for local-pg: pick another in the profile's form (it never runs under the default rules instead)"
        )
    );
    assert!(h.rows().contains(&"  !".to_string()), "the error line under it: {:?}", h.rows());
    h.explore("local-pg");
    let line = h.explorer_line();
    assert!(line.contains("local-pg ENV") && !line.contains(" RO"), "{line}");
    // Its form shows the name, marked missing.
    env_field(&mut h, "local-pg");
    assert_eq!(h.form().env_choice(), EnvChoice::Named("prod".into()));
    assert!(h.screen(100, 30).contains("prod (not found) ▾"), "{}", h.screen(100, 30));
}

/// An environment deleted while its profile is connected: nothing runs on that connection
/// (every statement is refused, not run under the default rules) until another is picked.
#[test]
fn a_connected_profile_whose_environment_goes_runs_nothing() {
    let dir = scratch("gone-connected");
    let mut envs = two();
    envs[0].policy = Policy::default();
    let mut h = Harness::with_config(&config(Some(&dir), envs), Lang::En);
    h.db(DbEvent::Connected);
    on_env(&mut h, "prod");
    h.keys("d");
    let screen = h.screen(120, 40);
    assert!(screen.contains("A connected profile that uses it runs nothing until"), "{screen}");
    h.keys("y");
    assert_eq!(names(&h), ["dev"]);
    h.app.focus = Focus::Editor;
    h.app.tab_mut().editor = Editor::new("SELECT 1");
    h.sent();
    h.ctrl('e');
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::Execute { .. })), "nothing is sent");
    // (The notice of the delete aside.)
    h.app.transient = None;
    let status = h.status(200, 40);
    assert!(status.contains("Environment “prod” not found"), "{status}");
}

/// The section while there is no environment: one row adds the presets (one write, said);
/// without a file to write it is not shown at all.
#[test]
fn the_empty_section_adds_the_presets() {
    let dir = scratch("presets");
    let mut h = harness_with(&config(Some(&dir), Vec::new()), Lang::En);
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "[environments]").expect("the section");
    assert_eq!((rows[at + 1].as_str(), rows[at + 2].as_str()), ("  (default rules)", "  (add presets)"));
    on_row(&mut h, RowKind::EnvironmentPresets);
    assert!(h.explorer_line().contains("＋ Add presets: dev, test, staging, prod"), "{}", h.explorer_line());
    h.key(KeyCode::Enter);
    assert_eq!(names(&h), ["dev", "test", "staging", "prod"]);
    assert!(said(&h).contains("Added the environments dev, test, staging, prod"), "{}", said(&h));
    let text = dir.text();
    for t in ["[policy.dev]", "[policy.test]", "[policy.staging]", "[policy.prod]", "badge = \"PROD\""] {
        assert!(text.contains(t), "{t}: {text}");
    }
    // Staging and prod commit by hand; dev and test by themselves (nothing written for them).
    let table =
        |name: &str| text.split(&format!("[policy.{name}]")).nth(1).unwrap().split("\n[").next().unwrap().to_string();
    assert!(
        table("staging").contains("commit = \"manual\"") && table("prod").contains("commit = \"manual\""),
        "{text}"
    );
    assert!(!table("dev").contains("commit") && !table("test").contains("commit"), "{text}");
    let manual = |name: &str| h.app.policies.get(Some(name)).unwrap().commit.manual();
    assert_eq!([manual("dev"), manual("test"), manual("staging"), manual("prod")], [false, false, true, true]);
    assert!(h.rows().iter().any(|r| r == "  %prod"), "{:?}", h.rows());
    // The profile that named `prod` before it existed now has it.
    assert!(h.app.policy_of(h.app.profiles[0].id).is_ok_and(|p| p.read_only));
    // No file to write: no section, nothing offered.
    let h = harness_with(&config(None, Vec::new()), Lang::En);
    assert!(!h.rows().iter().any(|r| r.contains("environments")), "{:?}", h.rows());
}

/// The presets' row once the file cannot be written: said, nothing added.
#[test]
fn presets_that_cannot_be_written_are_not_added() {
    let dir = scratch("presets-fail");
    let mut h = harness_with(&config(Some(&dir), Vec::new()), Lang::En);
    // The file's place is a directory: the write fails.
    std::fs::create_dir_all(dir.config()).unwrap();
    on_row(&mut h, RowKind::EnvironmentPresets);
    h.key(KeyCode::Enter);
    assert!(names(&h).is_empty());
    assert!(said(&h).contains("Could not save config"), "{}", said(&h));
}

/// `n` in the section makes one, `e` edits it, `R` renames it (the profiles naming it follow),
/// `c` copies it; each saved in the file at once, only where it changed.
#[test]
fn new_edit_rename_and_copy() {
    let dir = scratch("crud");
    std::fs::write(
        dir.config(),
        "version = 2\n\n# hand-written\n[policy.old]\npaging = \"hold\"\npaging_idle_timeout = \"5m\" # five minutes\n\n\
         [policy.prod]\nread_only = true\nconfirm = \"writes\"\ncolor = \"red\"\nbadge = \"PROD\"\n\n\
         [[connections]]\nname = \"local-pg\"\nhost = \"127.0.0.1\"\nuser = \"datarig\"\npolicy = \"prod\"\n",
    )
    .unwrap();
    let (cfg, err) = datarig_core::config::load(Some(dir.config()));
    assert!(err.is_none(), "{err:?}");
    let mut h = harness_with(&cfg, Lang::En);
    let old_table = "# hand-written\n[policy.old]\npaging = \"hold\"\npaging_idle_timeout = \"5m\" # five minutes\n";
    // New.
    on_env(&mut h, "prod");
    h.keys("n");
    assert!(h.form().is_environment());
    h.type_text("qa");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right); // color: none -> red
    h.key(KeyCode::Right); // -> orange
    h.key(KeyCode::Tab);
    h.type_text("QA");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right); // confirm: all writes
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(names(&h), ["old", "prod", "qa"], "the file's order, the new one last");
    let text = dir.text();
    assert!(text.contains("[policy.qa]\ncolor = \"orange\"\nbadge = \"QA\"\nconfirm = \"writes\"\n"), "{text}");
    assert!(text.contains(old_table), "{text}");
    assert_eq!(h.explorer_line().trim(), "● qa  QA   asks before every write", "the cursor on it");
    // Edit: the badge goes, read-only on.
    h.keys("e");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.ctrl('u');
    h.key(KeyCode::Tab);
    h.key(KeyCode::Char(' '));
    h.ctrl('s');
    let qa = h.app.environments().iter().find(|e| e.name == "qa").unwrap().clone();
    assert_eq!((qa.badge.as_deref(), qa.policy.read_only), (None, true));
    let text = dir.text();
    assert!(text.contains("[policy.qa]\ncolor = \"orange\"\nconfirm = \"writes\"\nread_only = true\n"), "{text}");
    // Rename `prod`: the profile naming it follows; its table moves.
    on_env(&mut h, "prod");
    h.keys("R");
    assert_eq!(h.form().focus, Field::Name);
    h.type_text("uction");
    h.ctrl('s');
    assert_eq!(h.app.profiles[0].policy.as_deref(), Some("production"));
    let text = dir.text();
    assert!(text.contains("[policy.production]") && !text.contains("[policy.prod]"), "{text}");
    assert!(text.contains("policy = \"production\""), "{text}");
    assert!(text.contains(old_table), "{text}");
    // Copy.
    on_env(&mut h, "production");
    h.keys("c");
    assert_eq!(h.form().name.text(), "production-copy");
    h.ctrl('s');
    assert_eq!(names(&h), ["old", "production", "qa", "production-copy"]);
    // A name taken (case aside), `default`, and a bad badge or duration are field errors.
    on_env(&mut h, "qa");
    h.keys("e");
    h.ctrl('u');
    h.type_text("OLD");
    h.ctrl('s');
    assert!(h.form_open() && h.screen(100, 30).contains("name already used"), "{}", h.screen(100, 30));
    for _ in 0..3 {
        h.key(KeyCode::Backspace);
    }
    h.type_text("Default");
    let screen = h.screen(120, 40);
    assert!(screen.contains("reserved for the default rules"), "{screen}");
    h.key(KeyCode::Esc);
    assert_eq!(names(&h), ["old", "production", "qa", "production-copy"], "nothing changed");
}

/// A rename the file cannot take: nothing changes (the environment, the profiles naming it),
/// the form stays open.
#[test]
fn a_rename_that_cannot_be_written_changes_nothing() {
    let dir = scratch("rename-fail");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    std::fs::create_dir_all(dir.config()).unwrap();
    on_env(&mut h, "prod");
    h.keys("R");
    h.type_text("uction");
    h.ctrl('s');
    assert!(h.form_open(), "the form stays");
    assert!(said(&h).contains("Could not save config"), "{}", said(&h));
    assert_eq!(names(&h), ["prod", "dev"]);
    assert_eq!(h.app.profiles[0].policy.as_deref(), Some("prod"));
}

/// `d` on an environment some profiles name: asked first, listing them; `Enter` keeps it, `y`
/// deletes it. The profiles keep the name, so they show the error and do not connect.
#[test]
fn deleting_an_environment_in_use_asks_and_leaves_the_name() {
    let dir = scratch("delete");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    on_env(&mut h, "prod");
    h.keys("d");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let c = h.app.overlays.confirm().unwrap();
    assert_eq!(c.action, ConfirmAction::DeleteEnvironment(env_id(&h, "prod")));
    let screen = h.screen(100, 30);
    assert!(screen.contains("Delete the environment “prod”?"), "{screen}");
    assert!(screen.contains("local-pg uses it: it will not connect until you pick"), "{screen}");
    h.key(KeyCode::Enter);
    assert_eq!(names(&h), ["prod", "dev"], "Enter keeps it");
    h.keys("d");
    h.keys("y");
    assert_eq!(names(&h), ["dev"]);
    assert!(said(&h).contains("the profile that used it needs another environment"), "{}", said(&h));
    assert!(!dir.text().contains("[policy.prod]"));
    assert_eq!(h.app.profiles[0].policy.as_deref(), Some("prod"), "the name stays");
    h.explore("local-pg");
    assert!(h.explorer_line().contains("local-pg ENV"), "{}", h.explorer_line());
    h.key(KeyCode::Enter);
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("Environment “prod” not found")));
}

/// Profiles under an environment, opened; `Enter` on one goes to it; the menu of an
/// environment's row has its own actions, worded for it.
#[test]
fn an_environment_lists_its_profiles_and_has_its_menu() {
    let dir = scratch("menu");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    on_env(&mut h, "prod");
    h.key(KeyCode::Char('l'));
    assert!(h.rows().iter().any(|r| r == "    @local-pg"), "{:?}", h.rows());
    h.right_click_row("%prod");
    assert_eq!(
        h.menu_labels(),
        [
            "Explorer: open or toggle",
            "New environment",
            "Edit environment",
            "Duplicate environment",
            "Delete environment",
            "Rename environment"
        ]
    );
    h.key(KeyCode::Esc);
    h.right_click_row("[environments]");
    assert_eq!(h.menu_labels(), ["Explorer: open or toggle", "New environment"]);
    h.key(KeyCode::Esc);
    h.right_click_row("@local-pg");
    assert_eq!(h.menu_labels(), ["Explorer: open or toggle"]);
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    assert!(h.explorer_line().contains("local-pg") && h.app.selected_profile().is_some());
}

/// The profile form's environment: `Enter` lists the default rules, each environment (its
/// rules as the hint) and "New environment…", the current one selected; a pick is saved with
/// the profile.
#[test]
fn the_environment_list_picks_one() {
    let dir = scratch("picker");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    env_field(&mut h, "local-pg");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
    assert_eq!(env_rows(&h), ["None (default rules)", "prod", "dev", "New environment…"]);
    assert_eq!(env_list(&h).selected(), Some(Row::Entry(1)), "the current one");
    let e = &env_list(&h).entries()[1];
    assert_eq!(e.hint, "read-only · asks before every write");
    assert_eq!(e.badge.as_ref().map(|b| b.0.as_str()), Some(" PROD "));
    h.type_text("dev");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().env_choice(), EnvChoice::Named("dev".into()));
    h.key(KeyCode::Enter);
    h.key(KeyCode::Up);
    h.key(KeyCode::Up);
    h.key(KeyCode::Enter);
    assert_eq!(h.form().env_choice(), EnvChoice::Default);
    h.ctrl('s');
    assert_eq!(h.app.profiles[0].policy, None);
    assert!(!dir.text().contains("policy = \"prod\""), "{}", dir.text());
}

/// While there is no environment the list offers the presets; one picked is written with the
/// profile, only then (cancelled: nothing), and both or neither when the file cannot be
/// written.
#[test]
fn a_preset_picked_in_the_profile_form_is_written_with_the_profile() {
    let dir = scratch("picker-preset");
    let mut cfg = config(Some(&dir), Vec::new());
    cfg.connections[0].policy = None;
    let mut h = harness_with(&cfg, Lang::En);
    env_field(&mut h, "v6");
    h.key(KeyCode::Enter);
    assert_eq!(
        env_rows(&h),
        ["None (default rules)", "dev", "test", "staging", "prod", "New environment…"],
        "the presets"
    );
    assert_eq!(env_list(&h).entries()[4].hint, "preset · read-only · asks before every write");
    h.type_text("prod");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().env_choice(), EnvChoice::Preset("prod".into()));
    assert!(h.screen(100, 30).contains("prod (new)"), "{}", h.screen(100, 30));
    // Cancelled: nothing written.
    h.key(KeyCode::Esc);
    assert!(names(&h).is_empty() && !dir.text().contains("[policy.prod]"));
    // Picked again, the file cannot be written: neither the profile nor the preset.
    env_field(&mut h, "v6");
    h.key(KeyCode::Enter);
    h.type_text("prod");
    h.key(KeyCode::Enter);
    let _ = std::fs::remove_file(dir.config());
    std::fs::create_dir_all(dir.config()).unwrap();
    h.ctrl('s');
    assert!(h.form_open() && names(&h).is_empty());
    assert_eq!(h.app.profiles[2].policy, None);
    // Saved: the preset and the profile naming it, one write.
    std::fs::remove_dir_all(dir.config()).unwrap();
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(names(&h), ["prod"]);
    assert_eq!(h.app.profiles[2].policy.as_deref(), Some("prod"));
    let text = dir.text();
    assert!(text.contains("[policy.prod]") && text.contains("policy = \"prod\""), "{text}");
    assert!(!text.contains("[policy.dev]"), "only the one picked: {text}");
    // There is one now: the presets are no longer offered.
    env_field(&mut h, "v6");
    h.key(KeyCode::Enter);
    assert_eq!(env_rows(&h), ["None (default rules)", "prod", "New environment…"]);
}

/// "New environment…" opens the environment form over the profile form; saved, the profile
/// form comes back as it was with the new one picked (cancelled: unchanged).
#[test]
fn a_new_environment_from_the_list_comes_back_picked() {
    let dir = scratch("picker-new");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    h.explore("v6");
    h.keys("e");
    h.key(KeyCode::Tab);
    h.type_text("-edited");
    let host = h.form().host.text().to_string();
    h.ctrl('n');
    h.ctrl('n');
    while h.form().focus != Field::Environment {
        h.key(KeyCode::Tab);
    }
    h.key(KeyCode::Enter);
    h.key(KeyCode::Up);
    assert_eq!(env_list(&h).selected(), Some(Row::New));
    h.key(KeyCode::Enter);
    assert!(h.form().is_environment());
    h.type_text("qa");
    h.key(KeyCode::Esc);
    assert!(!h.form().is_environment());
    assert_eq!((h.form().env_choice(), h.form().host.text()), (EnvChoice::Default, host.as_str()));
    assert_eq!(h.form().focus, Field::Environment);
    h.key(KeyCode::Enter);
    h.key(KeyCode::Up);
    h.key(KeyCode::Enter);
    h.type_text("qa");
    h.ctrl('s');
    assert!(!h.form().is_environment());
    assert_eq!(h.form().env_choice(), EnvChoice::Named("qa".into()));
    assert_eq!(h.form().host.text(), host);
    let text = dir.text();
    assert!(text.contains("[policy.qa]") && !text.contains("-edited"), "{text}");
    h.ctrl('s');
    assert_eq!(h.app.profiles[2].policy.as_deref(), Some("qa"));
}

/// The badge: a chip on the tab and in the status bar; read-only still says so.
#[test]
fn the_badge_on_the_tab_and_in_the_status_bar() {
    let mut h = Harness::with_config(&config(None, two()), Lang::En);
    h.db(DbEvent::Connected);
    let screen = h.screen(120, 40);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[0].contains("console 1  PROD  RO ×"), "tab: {}", lines[0]);
    assert!(lines[39].contains("local-pg  PROD   READ-ONLY "), "status: {}", lines[39]);
    assert!(lines[2].contains("127.0.0.1:55432  PROD   READ-ONLY "), "connection: {}", lines[2]);
    // Without a badge: the name, in the status bar only.
    h.app.profiles[0].policy = Some("dev".into());
    let screen = h.screen(120, 40);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[0].contains("console 1 ×"), "tab: {}", lines[0]);
    assert!(lines[39].contains("local-pg · dev │"), "status: {}", lines[39]);
}

/// The section, the form, the delete question, the profile form's list and a tab with its
/// badge, in English (pinned) and Korean (checked against the catalog), at 80x24 and 120x40.
#[test]
fn environment_screens() {
    for lang in [Lang::En, Lang::Ko] {
        let tag = lang_tag(lang);
        for (w, ht) in [(80, 24), (120, 40)] {
            let dir = scratch(&format!("screens-{tag}-{w}"));
            let mut h = harness_with(&config(Some(&dir), Vec::new()), lang);
            on_row(&mut h, RowKind::EnvironmentPresets);
            assert_screen!(format!("environments_empty_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
            h.key(KeyCode::Enter);
            on_env(&mut h, "prod");
            h.key(KeyCode::Char('l'));
            assert_screen!(format!("environments_section_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
            h.keys("e");
            assert_screen!(format!("environment_form_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
            h.key(KeyCode::Esc);
            on_row(&mut h, RowKind::DefaultRules);
            h.keys("e");
            assert_screen!(format!("default_rules_form_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
            h.key(KeyCode::Esc);
            on_env(&mut h, "prod");
            h.key(KeyCode::Esc);
            h.keys("d");
            assert_screen!(format!("environment_delete_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
            h.key(KeyCode::Enter);
            env_field(&mut h, "local-pg");
            assert_screen!(format!("profile_form_environment_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
            h.key(KeyCode::Enter);
            assert_screen!(format!("environment_list_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
        }
        let mut h = Harness::with_config(&config(None, two()), lang);
        h.db(DbEvent::Connected);
        for (w, ht) in [(80, 24), (120, 40)] {
            assert_screen!(format!("environment_badge_{tag}_{w}x{ht}"), lang, h.draw(w, ht));
        }
    }
}

/// An environment renamed while its profile is connected: the sessions the profile opens from
/// then on (a new tab's) take its rules under the new name, so a writable environment stays
/// writable on the server too, and a write is sent.
#[test]
fn renaming_the_environment_of_a_connected_profile_keeps_its_rules() {
    let dir = scratch("rename-connected");
    let mut envs = two();
    envs[0].policy = Policy::default();
    let mut h = Harness::with_config(&config(Some(&dir), envs), Lang::En);
    h.db(DbEvent::Connected);
    on_env(&mut h, "prod");
    h.keys("R");
    h.type_text("uction");
    h.ctrl('s');
    assert_eq!(h.app.profiles[0].policy.as_deref(), Some("production"));
    h.app.focus = Focus::Editor;
    h.app.tab_mut().editor = Editor::new("INSERT INTO t VALUES (1)");
    h.sent();
    h.ctrl('e');
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Execute { .. })), "{sent:?}");
    let opened: Vec<(SessionRole, bool)> =
        h.driver.sessions.lock().unwrap().iter().map(|s| (s.role, s.opts.read_only)).collect();
    assert_eq!(opened.last(), Some(&(SessionRole::Query, false)), "{opened:?}");
}

/// Testing a profile whose environment is not defined tries nothing and says why; in the form,
/// a preset picked there (written with the profile) counts as defined.
#[test]
fn a_test_of_a_profile_with_an_unknown_environment_tries_nothing() {
    let dir = scratch("test-unknown");
    let mut h = harness_with(&config(Some(&dir), Vec::new()), Lang::En);
    h.explore("local-pg");
    h.keys("t");
    let state = h.app.conn_test.as_ref().map(|t| format!("{:?}", t.state)).unwrap_or_default();
    assert!(state.contains("not found for"), "{state}");
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    // In the form: the name alone is refused; the preset `prod` picked is tried.
    env_field(&mut h, "local-pg");
    h.ctrl('t');
    let state = h.app.conn_test.as_ref().map(|t| format!("{:?}", t.state)).unwrap_or_default();
    assert!(state.contains("not found for"), "{state}");
    h.key(KeyCode::Enter);
    h.type_text("prod");
    let rows = env_rows(&h);
    assert!(rows.contains(&"prod".to_string()), "{rows:?}");
    // The preset, not the missing name.
    let i = env_list(&h).rows().iter().position(|r| env_list(&h).label(*r) == "prod").unwrap();
    while env_list(&h).selected() != Some(env_list(&h).rows()[i]) {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Enter);
    assert_eq!(h.form().env_choice(), EnvChoice::Preset("prod".into()));
    h.ctrl('t');
    let state = h.app.conn_test.as_ref().map(|t| format!("{:?}", t.state)).unwrap_or_default();
    assert!(!state.contains("not found for"), "{state}");
}

/// A preset picked, then another environment made from the list and the preset picked back
/// (the form keeps it to write with the profile): the list still has its row, current.
#[test]
fn a_picked_preset_keeps_its_row_once_an_environment_exists() {
    let dir = scratch("preset-row");
    let mut cfg = config(Some(&dir), Vec::new());
    cfg.connections[2].policy = None;
    let mut h = harness_with(&cfg, Lang::En);
    env_field(&mut h, "v6");
    h.key(KeyCode::Enter);
    h.type_text("prod");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().env_choice(), EnvChoice::Preset("prod".into()));
    // "New environment…" (the only row a filter of nothing leaves): `qa` made and picked.
    h.key(KeyCode::Enter);
    h.type_text("zz");
    h.key(KeyCode::Enter);
    assert!(h.form().is_environment());
    h.type_text("qa");
    h.ctrl('s');
    assert_eq!(h.form().env_choice(), EnvChoice::Named("qa".into()));
    assert_eq!(names(&h), ["qa"]);
    // The preset back, as the form had it.
    h.app.overlays.form_mut().unwrap().set_env_choice(EnvChoice::Preset("prod".into()));
    h.key(KeyCode::Enter);
    assert_eq!(env_rows(&h), ["None (default rules)", "qa", "prod (new)", "New environment…"]);
    assert_eq!(env_list(&h).selected(), Some(Row::Entry(2)), "the current one");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().env_choice(), EnvChoice::Preset("prod".into()));
    h.ctrl('s');
    assert_eq!(names(&h), ["qa", "prod"]);
    assert_eq!(h.app.profiles[2].policy.as_deref(), Some("prod"));
}

/// The default rules: first in the section, always; `Enter` lists the profiles without an
/// environment; `e` edits them (no name, color or badge), saved to `[policy.default]` only where
/// they changed; rename, copy and delete refuse with a notice and are not in its menu.
#[test]
fn the_default_rules_are_a_row_of_their_own() {
    let dir = scratch("default-rules");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "[environments]").expect("the section");
    assert_eq!(rows[at + 1], "  (default rules)", "{rows:?}");
    on_row(&mut h, RowKind::DefaultRules);
    assert_eq!(h.explorer_line().trim(), "▸ ○ Default rules (no environment)  standard rules · 2 profiles");
    h.key(KeyCode::Enter);
    let rows = h.rows();
    assert_eq!((rows[at + 2].as_str(), rows[at + 3].as_str()), ("    @分析-replica", "    @v6"), "{rows:?}");
    // Not renamed, copied or deleted.
    for k in ["R", "c", "d"] {
        h.app.transient = None;
        h.keys(k);
        assert!(!h.form_open() && h.overlay_kind().is_none(), "{k}");
        assert!(said(&h).contains("cannot be renamed, copied or deleted"), "{k}: {}", said(&h));
    }
    h.right_click_row("(default rules)");
    assert_eq!(h.menu_labels(), ["Explorer: open or toggle", "New environment", "Edit the default rules"]);
    h.key(KeyCode::Esc);
    // Edited: read-only for every profile without an environment.
    on_row(&mut h, RowKind::DefaultRules);
    h.keys("e");
    assert!(h.form().is_default_rules());
    assert_eq!(
        h.form().fields(),
        [
            Field::EnvReadOnly,
            Field::EnvConfirm,
            Field::EnvCommit,
            Field::EnvTxWarning,
            Field::EnvTxTimeout,
            Field::EnvPaging,
            Field::EnvIdle,
            Field::EnvSpill,
            Field::Save,
            Field::Cancel
        ]
    );
    let screen = h.screen(100, 30);
    assert!(
        screen.contains("Default rules") && screen.contains("Used by") && screen.contains("分析-replica, v6"),
        "{screen}"
    );
    h.key(KeyCode::Char(' '));
    h.ctrl('s');
    assert!(!h.form_open());
    assert!(said(&h).contains("Saved the default rules"), "{}", said(&h));
    assert!(h.app.policies.default_rules().read_only);
    let v6 = h.app.profiles.iter().find(|p| p.name == "v6").unwrap().id;
    assert!(h.app.read_only(v6));
    let text = dir.text();
    assert!(text.contains("[policy.default]\nread_only = true\n"), "{text}");
    // Back to the built-in default: the key goes.
    on_row(&mut h, RowKind::DefaultRules);
    h.keys("e");
    h.key(KeyCode::Char(' '));
    h.ctrl('s');
    assert!(!dir.text().contains("[policy.default]"), "empty again: gone: {}", dir.text());
}

/// The profile form's environment says "None (default rules)" without one.
#[test]
fn no_environment_reads_none_default_rules() {
    let dir = scratch("none-label");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    env_field(&mut h, "v6");
    let screen = h.screen(100, 30);
    assert!(screen.contains("Environment       None (default rules) ▾"), "{screen}");
}

/// The commit rule and the idle times of an open transaction are edited in the form: a time
/// shorter than 10 seconds (or not a time) is a field error and nothing is saved; saved, only
/// what differs from the defaults is written, and the rules say "manual commit".
#[test]
fn the_commit_rule_and_the_idle_times_are_edited_in_the_form() {
    let dir = scratch("commit");
    let mut h = harness_with(&config(Some(&dir), two()), Lang::En);
    on_env(&mut h, "dev");
    h.keys("e");
    while h.form().focus != Field::EnvCommit {
        h.key(KeyCode::Tab);
    }
    let screen = h.screen(120, 40);
    assert!(screen.contains("‹automatic (default)›") && screen.contains("PostgreSQL only"), "{screen}");
    h.key(KeyCode::Right);
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::EnvTxWarning);
    h.ctrl('u');
    h.type_text("5s");
    assert!(h.screen(120, 40).contains("at least 10s, or off"), "{}", h.screen(120, 40));
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    h.ctrl('u');
    h.type_text("2m");
    h.key(KeyCode::Tab);
    h.ctrl('u');
    h.type_text("off");
    h.ctrl('s');
    assert!(!h.form_open());
    let dev = h.app.environments().iter().find(|e| e.name == "dev").unwrap().policy.clone();
    assert_eq!(
        (dev.commit, dev.tx_idle_warning, dev.tx_idle_timeout),
        (datarig_core::policy::CommitRule::Manual, Some(std::time::Duration::from_secs(120)), None)
    );
    let text = dir.text();
    assert!(
        text.contains("[policy.dev]\ncolor = \"green\"\ncommit = \"manual\"\ntx_idle_warning = \"2m\"\ntx_idle_timeout = \"off\"\n"),
        "{text}"
    );
    assert!(h.explorer_line().contains("manual commit"), "{}", h.explorer_line());
}
