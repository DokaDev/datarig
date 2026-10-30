//! Tips (`[ui] tips`): the welcome panel's first steps, one tip about a key in an empty place
//! (the workspace without tabs, an empty console, an empty or running result, the status bar's
//! spare room), the status line that names a key after a few runs from a menu or the command
//! line, and the usage file they read and write.

mod common;

use common::*;
use datarig_core::config::{Config, Tips};
use datarig_core::driver::DbEvent;
use datarig_core::i18n::{Lang, Msg};
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::usage::Via;
use datarig_tui::app::Startup;
use datarig_tui::app::action::Action;
use datarig_tui::app::tips::{Place, STEPS};
use datarig_tui::keymap::{Bound, Ctx, Keymap, Target, parse_keys};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn lang_tag(l: Lang) -> &'static str {
    match l {
        Lang::En => "en",
        Lang::Ko => "ko",
    }
}

fn no_profiles() -> Config {
    Config { connections: Vec::new(), ..Default::default() }
}

/// Launched with `cfg` and tips on (the harness keeps them off).
fn launched(cfg: &Config, lang: Lang) -> Harness {
    let mut h = Harness::launched(cfg, lang, Arc::new(MemoryStore::new()), Startup::Normal);
    h.app.tips.paused = false;
    h
}

/// Connected to the test profile, tips on, the first steps out of the way.
fn connected() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.tips.paused = false;
    h.app.dispatch(Action::DismissOnboarding);
    h
}

/// A state directory of its own, removed at the end.
struct State(PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-tips-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }

    fn usage(&self) -> PathBuf {
        self.0.join(datarig_core::usage::FILE)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn launch_at(cfg: &Config, state: &State) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    app.tips.paused = false;
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    let h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    let mut h = h.with_fake_driver();
    h.app.launch(Startup::Normal);
    h
}

#[test]
fn the_welcome_panel_shows_the_first_steps_until_they_are_hidden() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = launched(&no_profiles(), lang);
        // The first run still opens the new-connection form at once.
        assert!(h.form_open());
        h.key(KeyCode::Esc);
        for (w, hh) in [(80, 24), (160, 45)] {
            assert_screen!(format!("welcome_first_steps_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
        // The help opened by its key: that step is taken.
        h.key(KeyCode::F(1));
        h.key(KeyCode::Esc);
        let screen = h.screen(80, 24);
        let help = screen.lines().find(|l| l.contains(" F1 ")).unwrap().to_string();
        assert!(help.contains("[x]"), "{help}");
        assert!(screen.lines().filter(|l| l.contains("[ ]")).count() == 3, "{screen}");
        // Its key hides them for good: the welcome panel's keys are back.
        h.keys("X");
        assert!(!h.app.onboarding_shown());
        assert!(h.app.tips.usage.onboarding_done);
        let screen = h.screen(80, 24);
        assert!(!screen.contains("[ ]"), "{screen}");
        if lang == Lang::En {
            assert!(screen.contains("n   New connection"), "{screen}");
        }
    }
}

#[test]
fn icons_mark_the_steps_and_the_tips_with_glyphs() {
    let mut h = launched(&no_profiles(), Lang::En);
    h.key(KeyCode::Esc);
    h.app.icons = datarig_core::config::IconsSetting::On;
    let screen = h.screen(80, 24);
    assert!(screen.contains(datarig_tui::icons::STEP_TODO) && !screen.contains("[ ]"), "{screen}");
    let mut h = connected();
    h.app.icons = datarig_core::config::IconsSetting::On;
    h.ctrl('t');
    let screen = h.screen(80, 24);
    assert!(screen.contains(datarig_tui::icons::TIP) && !screen.contains("Tip:"), "{screen}");
}

#[test]
fn tips_off_shows_the_welcome_panel_as_before_and_no_tip() {
    let mut h = launched(&no_profiles(), Lang::En);
    h.key(KeyCode::Esc);
    h.command("set ui.tips=off");
    assert_eq!(h.app.prefs.tips, Tips::Off);
    let screen = h.screen(80, 24);
    assert!(screen.contains("n   New connection") && !screen.contains("[ ]"), "{screen}");
    assert!(!h.app.onboarding_shown());
    let mut h = connected();
    h.ctrl('t');
    h.app.prefs.tips = Tips::Off;
    for p in [Place::NoTab, Place::EmptyEditor, Place::EmptyResults, Place::Running, Place::StatusIdle] {
        assert_eq!(h.app.tip(p), None, "{p:?}");
    }
    h.command("set ui.tips=on");
    assert_eq!(h.app.prefs.tips, Tips::On);
    assert!(h.app.tip(Place::EmptyEditor).is_some());
}

/// The status line's text (all of it, however wide).
fn status_text(h: &Harness) -> String {
    h.app.status.as_ref().map(|s| h.app.i18n.msg(&s.msg).to_string()).unwrap_or_default()
}

#[test]
fn the_empty_workspace_shows_the_next_step_then_a_tip() {
    let mut h = launched(&sample_config(None), Lang::En);
    // The first connection exists, and without a tab nothing runs: the next step is another.
    let screen = h.screen(160, 45);
    let line = screen.lines().find(|l| l.contains("Next step: ")).unwrap_or_else(|| panic!("{screen}"));
    assert!(line.contains("[ ] Next step: ") && line.contains("(X hides the steps)"), "{line}");
    assert!(!line.contains("Run your first query"), "{line}");
    h.keys("X");
    assert_screen!("no_tab_tip_en_80x24", Lang::En, h.draw(80, 24));
}

#[test]
fn the_next_step_takes_turns_with_the_tips_of_the_empty_editor() {
    let mut h = Harness::connected(Lang::En);
    h.app.tips.paused = false;
    h.ctrl('t');
    assert!(h.app.onboarding_shown());
    let (mut steps, mut tips) = (0, 0);
    for e in 0..10 {
        h.app.tips.epoch = e;
        match h.app.tip(Place::EmptyEditor) {
            // The key that hides the steps types or edits in the editor: not named there.
            Some(Msg::OnboardingNext { .. }) => panic!("the editor has no key that hides the steps"),
            Some(m @ Msg::OnboardingNextStep { .. }) => {
                let text = h.app.i18n.msg(&m).to_string();
                assert_eq!(text, "Next step: Run your first query with Ctrl+E");
                steps += 1;
            }
            Some(_) => tips += 1,
            None => panic!("epoch {e}: no tip"),
        }
        // One line, never two.
        let screen = h.screen(80, 24);
        let lines = screen.lines().filter(|l| l.contains("Next step: ") || l.contains("Tip: ")).count();
        assert_eq!(lines, 1, "{screen}");
    }
    assert!(steps > 0 && tips > 0, "{steps} steps, {tips} tips");
}

#[test]
fn a_tip_is_back_after_a_popup_a_menu_or_the_command_line_closes() {
    let mut h = connected();
    h.ctrl('t');
    let tip = h.app.tip(Place::EmptyEditor).expect("a tip");
    let line = h.app.tip_line(Place::EmptyEditor).unwrap();
    let start: String = line.chars().take(24).collect();
    assert!(h.screen(80, 24).contains(&start), "{line}");
    // The command line, the keyboard help, the leader's popup, the action menu.
    for (name, open) in [(":", ":"), ("help", ""), ("which-key", " "), ("menu", "  ")] {
        if open.is_empty() {
            h.key(KeyCode::F(1));
        } else {
            h.keys(open);
            // The leader's popup comes after a moment.
            h.settle();
        }
        assert!(h.app.overlays.top().is_some(), "{name} open");
        assert_eq!(h.app.tip(Place::EmptyEditor), None, "{name}: no tip under it");
        assert!(!h.screen(80, 24).contains(&start), "{name}");
        h.key(KeyCode::Esc);
        assert!(h.app.overlays.top().is_none(), "{name} closed");
        assert_eq!(h.app.tip(Place::EmptyEditor), Some(tip.clone()), "{name}: the same tip again");
        assert!(h.screen(80, 24).contains(&start), "{name}");
    }
}

#[test]
fn an_empty_console_has_a_tip_until_its_first_character() {
    let mut h = connected();
    h.ctrl('t');
    assert_screen!("empty_editor_tip_en_80x24", Lang::En, h.draw(80, 24));
    let tip = h.app.i18n.msg(&h.app.tip(Place::EmptyEditor).unwrap()).to_string();
    h.keys("i");
    h.type_text("s");
    assert!(!h.screen(80, 24).contains(&tip), "{tip}");
}

#[test]
fn a_running_and_an_empty_result_have_a_tip() {
    let mut h = connected();
    h.ctrl('e');
    // Without the status bar: it says how long the run takes, by the wall clock.
    let screen = h.screen(80, 24);
    let panes: Vec<&str> = screen.lines().take(23).collect();
    insta::assert_snapshot!("running_tip_en_80x24", panes.join("\n"));
    h.tab_db(0, DbEvent::Lost { error: "server closed the connection unexpectedly".into() });
    assert_screen!("empty_results_tip_en_80x24", Lang::En, h.draw(80, 24));
}

#[test]
fn the_status_bar_has_a_tip_only_where_every_hint_fits_beside_it() {
    let mut h = connected();
    h.app.status = None;
    let status = h.status(160, 45);
    insta::assert_snapshot!("status_idle_tip_en_160x45", status);
    assert!(status.contains("Tip: "), "{status}");
    assert!(!h.status(80, 24).contains("Tip: "), "no room at 80 columns");
    // A message takes the place.
    h.app.status =
        Some(datarig_tui::app::Notice::new(datarig_core::i18n::Label::LangSwitched, datarig_tui::app::Level::Info));
    assert!(!h.status(160, 45).contains("Tip: "));
}

#[test]
fn a_tip_stays_while_the_page_does_and_a_known_key_drops_out() {
    let mut h = connected();
    h.ctrl('t');
    let first = h.app.tip(Place::EmptyEditor).unwrap();
    // Frames, time and a key that turns no page change nothing (no timer, no wakeup).
    for _ in 0..5 {
        h.draw(80, 24);
        h.advance(Duration::from_secs(60));
    }
    h.keys("j");
    assert_eq!(h.app.tip(Place::EmptyEditor), Some(first.clone()));
    assert_eq!(h.app.next_tick(h.clock.now()), None, "tips wake nothing up");
    // Another tab is another page: the next tip.
    let epoch = h.app.tips.epoch;
    h.ctrl('t');
    assert_eq!(h.app.tips.epoch, epoch + 1);
    assert_ne!(h.app.tip(Place::EmptyEditor), Some(first));
    // The run key used once: its tip never shows again.
    h.ctrl('e');
    assert_eq!(h.app.tips.usage.count(Via::Key, "query.execute_current"), 1);
    for e in 0..12 {
        h.app.tips.epoch = e;
        assert!(!matches!(h.app.tip(Place::EmptyEditor), Some(Msg::TipsRun { .. })), "epoch {e}");
    }
}

#[test]
fn the_third_command_line_run_names_the_key_once() {
    let mut h = connected();
    let named = "Tip: Ctrl+T runs “New console tab” here without the command line";
    for _ in 0..2 {
        h.command("tabnew");
        assert!(!h.status(160, 45).contains(named));
    }
    h.command("tabnew");
    assert!(h.status(160, 45).contains(named), "{}", h.status(160, 45));
    assert!(h.app.tips.usage.nudged.contains("tab.new_console"));
    // Once ever.
    h.app.status = None;
    h.command("tabnew");
    assert!(!h.status(160, 45).contains(named));
    // Not once the key was used.
    let mut h = connected();
    h.ctrl('t');
    for _ in 0..4 {
        h.command("tabnew");
        assert!(!h.status(160, 45).contains("without the command line"));
    }
    // Not with tips off, and nothing counted then.
    let mut h = connected();
    h.app.prefs.tips = Tips::Off;
    for _ in 0..3 {
        h.command("tabnew");
    }
    assert!(!h.status(160, 45).contains("without the command line"));
    assert_eq!(h.app.tips.usage.count(Via::Command, "tab.new_console"), 0);
    assert_eq!(h.app.tips.usage.count(Via::Key, "commands.open"), 0);
}

#[test]
fn the_third_menu_run_names_the_key_unless_the_key_was_used() {
    let label = "Comment the line or the selected lines out with -- (or back in)";
    let mut h = connected();
    h.ctrl('t');
    let key = h.app.key_for(Action::ToggleComment, Ctx::VimNormal);
    let named = format!("Tip: {key} runs “{label}” here without the menu");
    for i in 0..3 {
        h.keys("  ");
        h.menu_pick(label);
        assert_eq!(status_text(&h).contains(&named), i == 2, "run {}: {}", i + 1, status_text(&h));
    }
    assert!(h.app.tips.usage.nudged.contains("editor.comment_toggle"));
    // Its key used once before (an action no tip is about): never named.
    let mut h = connected();
    h.ctrl('t');
    h.keys(" ec");
    assert_eq!(h.app.tips.usage.count(Via::Key, "editor.comment_toggle"), 1);
    for _ in 0..4 {
        h.keys("  ");
        h.menu_pick(label);
        assert!(!status_text(&h).contains("without the menu"), "{}", status_text(&h));
    }
}

#[test]
fn a_remapped_key_is_the_one_a_nudge_names() {
    let mut h = connected();
    let mut bindings: Vec<Bound> =
        h.app.keymap.bindings().iter().filter(|b| b.target != Target::Action(Action::NewTab)).cloned().collect();
    bindings.push(Bound {
        ctx: Ctx::Workspace,
        keys: parse_keys("ctrl+f9").unwrap(),
        target: Target::Action(Action::NewTab),
        user: true,
    });
    h.app.keymap = Keymap::from_bindings(bindings);
    for _ in 0..3 {
        h.command("tabnew");
    }
    let status = status_text(&h);
    assert!(status.contains("Ctrl+F9 runs “New console tab”") && !status.contains("Ctrl+T"), "{status}");
}

#[test]
fn a_first_step_is_taken_by_any_way_to_it() {
    let mut h = Harness::connected(Lang::En);
    h.app.tips.paused = false;
    let step = |h: &Harness, a: Action| h.app.step_done(STEPS.iter().find(|s| s.action == a).unwrap());
    assert!(step(&h, Action::NewProfile));
    assert!(!step(&h, Action::OpenMenu) && !step(&h, Action::RunStatement));
    // A right click opens the menu.
    h.right_click_row("local-pg");
    assert!(h.app.overlays.top().is_some());
    h.key(KeyCode::Esc);
    assert!(step(&h, Action::OpenMenu));
    // A run that is not the run key's: the plan of the statement.
    h.ctrl('t');
    h.keys("i");
    h.type_text("select 1");
    h.key(KeyCode::Esc);
    h.command("explain");
    assert!(h.app.tab().exec.running.is_some());
    assert!(step(&h, Action::RunStatement));
    assert_eq!(h.app.tips.usage.count(Via::Key, "query.execute_current"), 0);
}

#[test]
fn usage_is_written_with_the_workspace_and_read_at_the_next_launch() {
    let state = State::new("written");
    // Not a first launch (that writes the file at its first save).
    std::fs::write(state.usage(), "").unwrap();
    let mut h = launch_at(&test_db_config(), &state);
    assert_eq!(std::fs::read_to_string(state.usage()).unwrap(), "", "nothing counted, nothing written");
    h.key(KeyCode::F(1));
    h.key(KeyCode::Esc);
    assert_eq!(std::fs::read_to_string(state.usage()).unwrap(), "", "not per key");
    h.ctrl('q');
    assert!(h.app.quit);
    let text = std::fs::read_to_string(state.usage()).unwrap();
    assert!(text.contains("\"help.context\" = 1"), "{text}");
    for absent in ["local-pg", "select", "SELECT", "127.0.0.1"] {
        assert!(!text.contains(absent), "{absent}: {text}");
    }
    drop(h);
    let h = launch_at(&test_db_config(), &state);
    assert_eq!(h.app.tips.usage.count(Via::Key, "help.context"), 1);
}

#[test]
fn dismissing_the_first_steps_is_saved_at_once() {
    let state = State::new("dismiss");
    let mut h = launch_at(&no_profiles(), &state);
    h.key(KeyCode::Esc);
    h.keys("X");
    let text = std::fs::read_to_string(state.usage()).unwrap();
    assert!(text.contains("onboarding = \"done\""), "{text}");
    drop(h);
    let h = launch_at(&no_profiles(), &state);
    assert!(!h.app.onboarding_shown());
}

#[test]
fn an_unreadable_usage_file_is_never_written_over() {
    let state = State::new("unreadable");
    let damaged = "[keys]\n\"help.context\" = \n";
    std::fs::write(state.usage(), damaged).unwrap();
    let mut h = launch_at(&test_db_config(), &state);
    h.key(KeyCode::F(1));
    h.key(KeyCode::Esc);
    // Counted in memory all the same.
    assert_eq!(h.app.tips.usage.count(Via::Key, "help.context"), 1);
    h.ctrl('q');
    assert_eq!(std::fs::read_to_string(state.usage()).unwrap(), damaged);
}

#[test]
fn ids_the_usage_file_has_and_this_version_does_not_know_are_kept() {
    let state = State::new("unknown");
    std::fs::write(state.usage(), "later = 1\n\n[keys]\n\"future.action\" = 4\n").unwrap();
    let mut h = launch_at(&test_db_config(), &state);
    h.key(KeyCode::F(1));
    h.key(KeyCode::Esc);
    h.ctrl('q');
    let text = std::fs::read_to_string(state.usage()).unwrap();
    for kept in ["later = 1", "\"future.action\" = 4", "\"help.context\" = 1"] {
        assert!(text.contains(kept), "{kept}: {text}");
    }
}

#[test]
fn without_a_writable_state_directory_usage_stays_in_memory() {
    // No state directory at all.
    let mut h = connected();
    h.keys(" ?");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tips.usage.count(Via::Key, "help.context"), 1);
    // A second instance (read-only workspace) writes nothing either.
    let state = State::new("read-only");
    std::fs::write(state.usage(), "").unwrap();
    let mut h = launch_at(&test_db_config(), &state);
    h.app.read_only = true;
    h.key(KeyCode::F(1));
    h.key(KeyCode::Esc);
    h.ctrl('q');
    assert_eq!(std::fs::read_to_string(state.usage()).unwrap(), "");
}

#[test]
fn returning_users_get_no_first_steps_and_newcomers_do() {
    // Connections from an earlier version, no usage file yet.
    let state = State::new("returning");
    let mut h = launch_at(&test_db_config(), &state);
    assert!(h.app.tips.usage.onboarding_done && !h.app.onboarding_shown());
    h.ctrl('q');
    let text = std::fs::read_to_string(state.usage()).unwrap();
    assert!(text.contains("onboarding = \"done\""), "{text}");
    drop(h);
    // A workspace from an earlier version, no connection left.
    let state = State::new("returning-workspace");
    std::fs::write(state.0.join(datarig_core::workspace::FILE), "").unwrap();
    let mut h = launch_at(&no_profiles(), &state);
    h.key(KeyCode::Esc);
    assert!(!h.app.onboarding_shown());
    assert!(!h.screen(80, 24).contains("First steps"));
    drop(h);
    // A first launch: the first steps, and a usage file, so the next launch (with the connection
    // added meanwhile) still shows them.
    let state = State::new("newcomer");
    let mut h = launch_at(&no_profiles(), &state);
    h.key(KeyCode::Esc);
    assert!(h.app.onboarding_shown());
    assert!(h.screen(80, 24).contains("First steps"));
    h.ctrl('q');
    assert!(state.usage().exists());
    drop(h);
    let h = launch_at(&test_db_config(), &state);
    assert!(h.app.onboarding_shown());
}

#[test]
fn a_first_launch_that_ends_without_a_save_still_counts_as_new_the_next_time() {
    let state = State::new("newcomer-crash");
    let h = launch_at(&no_profiles(), &state);
    // Written at launch, before any save: a crash after a connection is added keeps the steps.
    assert!(state.usage().exists());
    // A crash: nothing runs on the way out.
    std::mem::forget(h);
    let h = launch_at(&test_db_config(), &state);
    assert!(h.app.onboarding_shown());
}
