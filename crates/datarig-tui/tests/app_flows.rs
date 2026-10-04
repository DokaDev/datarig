//! Behaviour tests through the real `App` event path (no terminal, no DB): key-enhancement
//! event filtering, the command line, completion triggers, connection profiles with the secret
//! store, the startup flow (the explorer, `datarig <profile>`, password migration at launch),
//! connecting and disconnecting in the explorer, and the password prompt.

mod common;

use common::*;
use datarig_core::config::{self, Config};
use datarig_core::driver::{DbCommand, DbEvent, PingError, PingInfo};
use datarig_core::i18n::Lang;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::action::LangSetting;
use datarig_tui::app::{AppEvent, CommandItem, EventTarget, NodeState, Startup, TestState};
use datarig_tui::widgets::editor::Mode;
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const NONE: KeyModifiers = KeyModifiers::NONE;

fn executes(h: &mut Harness) -> usize {
    h.sent().iter().filter(|c| matches!(c, DbCommand::Execute { .. })).count()
}

// ── keyboard enhancement (kitty protocol) ───────────────────────────────────

#[test]
fn ctrl_enter_runs_statement_release_and_repeat_do_not_double_fire() {
    let mut h = Harness::connected(Lang::En);
    h.key_kind(KeyCode::Enter, CTRL, KeyEventKind::Press);
    h.key_kind(KeyCode::Enter, CTRL, KeyEventKind::Repeat);
    h.key_kind(KeyCode::Enter, CTRL, KeyEventKind::Release);
    let sent = h.sent();
    assert!(
        matches!(&sent[..], [DbCommand::Execute { statements, .. }] if statements == &["SELECT * FROM shop.users WHERE id <= 8"]),
        "exactly one execution: {sent:?}"
    );
    h.db(DbEvent::Done { id: 1, outcome: datarig_core::driver::Outcome::Affected(0), elapsed: Duration::ZERO });
    // Ctrl+E fallback always works, and a release alone never acts.
    h.key_kind(KeyCode::Char('e'), CTRL, KeyEventKind::Release);
    assert_eq!(executes(&mut h), 0);
    h.ctrl('e');
    assert_eq!(executes(&mut h), 1);
}

#[test]
fn ctrl_enter_in_insert_and_visual_mode() {
    let mut h = Harness::connected(Lang::En);
    h.keys("A");
    h.key_kind(KeyCode::Enter, CTRL, KeyEventKind::Press);
    assert_eq!(executes(&mut h), 1, "runs from Insert mode");
    assert_eq!(h.app.tab().editor.text(), SAMPLE_SQL, "Ctrl+Enter inserts no newline");
    h.db(DbEvent::Failed { id: 1, error: "x".into(), cancelled: false });
    h.key(KeyCode::Esc);
    h.keys("0vjjjjj$");
    h.key_kind(KeyCode::Enter, CTRL, KeyEventKind::Press);
    let sent = h.sent();
    let [DbCommand::Execute { statements, .. }] = &sent[..] else { panic!("{sent:?}") };
    assert_eq!(statements.len(), 2, "Visual selection");
}

#[test]
fn other_keys_still_work_with_enhancement() {
    let mut h = Harness::connected(Lang::En);
    h.app.set_keyboard_enhanced(true);
    // Korean IME commits arrive as text; release events for them must not type twice.
    h.keys("A");
    for c in "こんにちは 🐘".chars() {
        h.key_kind(KeyCode::Char(c), NONE, KeyEventKind::Press);
        h.key_kind(KeyCode::Char(c), NONE, KeyEventKind::Release);
    }
    assert!(h.app.tab().editor.lines[1].ends_with("id <= 8;こんにちは 🐘"), "{}", h.app.tab().editor.lines[1]);
    // Auto-repeat still types / deletes (editing is not an action).
    h.key_kind(KeyCode::Backspace, NONE, KeyEventKind::Press);
    h.key_kind(KeyCode::Backspace, NONE, KeyEventKind::Repeat);
    assert!(h.app.tab().editor.lines[1].ends_with("こんにちは"), "{}", h.app.tab().editor.lines[1]);
    // Esc (disambiguated) leaves Insert; Ctrl+C when idle does nothing.
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab().editor.mode, Mode::Normal);
    h.ctrl('c');
    assert!(!h.app.quit);
    // Tab and Shift+Tab (kitty reports Tab + SHIFT) cycle panes: before a run the tab has no
    // results pane, so the explorer and the editor.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Tree);
    h.key_mod(KeyCode::Tab, KeyModifiers::SHIFT);
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Editor);
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Tree);
    // Held Tab (repeat) is an action key: ignored.
    h.key_kind(KeyCode::Tab, NONE, KeyEventKind::Repeat);
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Tree);
    // Held j in the explorer moves (navigation, not an action): from the profile to its first
    // schema.
    h.key_kind(KeyCode::Char('j'), NONE, KeyEventKind::Repeat);
    assert_eq!(h.selected(), 2);
}

#[test]
fn status_hint_shows_active_run_key() {
    let mut h = Harness::connected(Lang::En);
    assert!(h.status(160, 45).contains("Ctrl+E run"));
    h.app.set_keyboard_enhanced(true);
    assert!(h.status(160, 45).contains("Ctrl+Enter run"));
    // Before a run the editor has the whole tab: no results pane yet.
    h.draw(160, 45);
    assert_eq!(h.app.layout.results.height, 0);
}

// ── `:` command line ─────────────────────────────────────────────────────────

#[test]
fn command_line_opens_with_colon_outside_text_and_ctrl_k_everywhere() {
    let mut h = Harness::connected(Lang::En);
    // ':' in editor Normal mode
    h.keys(":");
    assert!(h.cmdline().is_some());
    h.key(KeyCode::Esc);
    // ':' in Insert mode types a colon; Ctrl+K opens the command line.
    h.keys("A:");
    assert!(h.cmdline().is_none());
    assert!(h.app.tab().editor.lines[1].ends_with(':'));
    h.ctrl('k');
    assert!(h.cmdline().is_some());
    h.ctrl('k');
    assert!(h.cmdline().is_none(), "Ctrl+K toggles");
    h.key(KeyCode::Esc);
    // explorer and grid
    h.key(KeyCode::BackTab);
    h.keys(":");
    assert!(h.cmdline().is_some());
    h.key(KeyCode::Esc);
    h.key(KeyCode::BackTab);
    h.keys(":");
    assert!(h.cmdline().is_some());
    // Inside the command line ':' is text; Backspace deletes it, then closes the empty line.
    h.keys(":");
    assert_eq!(h.cmdline().unwrap().input.text(), ":");
    h.key(KeyCode::Backspace);
    assert!(h.cmdline().is_some_and(|c| c.input.text().is_empty()));
    h.key(KeyCode::Backspace);
    assert!(h.cmdline().is_none(), "Backspace on an empty line closes it");
    // The keyboard help's filter is text input too: ':' is typed there.
    h.key(KeyCode::F(1));
    h.keys("/:");
    assert!(h.cmdline().is_none());
    assert_eq!(h.app.overlays.help().unwrap().filter.text(), ":");
    h.ctrl('k');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands), "Ctrl+K opens over a text input");
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    assert!(h.overlay_kind().is_none());
    // The command line runs the same action as the key: run statement.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab); // back to editor
    h.command("run statement");
    assert_eq!(executes(&mut h), 1);
    // Tab / Shift+Tab, Ctrl+N / Ctrl+P and arrows move the selection; Enter runs the selected one.
    h.db(DbEvent::Failed { id: 1, error: "x".into(), cancelled: false });
    h.ctrl('k');
    h.type_text("quit");
    for k in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Down, KeyCode::Up] {
        h.key(k);
    }
    h.ctrl('n');
    h.ctrl('p');
    assert_eq!(h.cmdline().unwrap().selected, 0);
    h.key(KeyCode::Enter);
    assert!(h.app.quit);
}

#[test]
fn colon_opens_the_command_line_in_the_explorer_and_from_which_key() {
    let mut h = launch(&sample_config(None), Startup::Normal);
    h.keys(":");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands), "the explorer is not text input");
    h.key(KeyCode::Esc);
    // The which-key footer names `:`; pressing it there opens the command line.
    let mut h = Harness::connected(Lang::En);
    h.keys(" ");
    h.settle();
    assert_eq!(h.overlay_kind(), Some(OverlayKind::WhichKey));
    h.keys(":");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands));
    assert!(h.app.transient.is_none(), "no \"no binding\" notice");
}

#[test]
fn conn_command_goes_to_the_profile() {
    let mut h = Harness::connected(Lang::En);
    h.app.profiles.extend(sample_profiles().into_iter().skip(1));
    // Completion lists the profiles; Enter on `:conn` completes the name first.
    h.keys(":conn");
    assert_eq!(h.cmdline().unwrap().items.first(), Some(&CommandItem::Command(0)));
    h.key(KeyCode::Enter);
    assert_eq!(h.cmdline().unwrap().input.text(), "conn ");
    assert_eq!(h.cmdline().unwrap().items.len(), 3, "every profile");
    h.type_text("zzz");
    h.key(KeyCode::Enter);
    let err = h.screen(80, 24);
    assert!(err.contains("No connection profile named “zzz”"), "{err}");
    assert!(h.cmdline().is_some(), "an error keeps the command line open");
    h.key(KeyCode::Esc);
    // `:connect <name>` (alias) of a profile without a tab connects it and opens a console
    // (like quick connect); the connected profile stays connected.
    h.command("connect v6");
    assert!(h.cmdline().is_none());
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("v6"));
    h.db(DbEvent::Connected);
    assert_eq!(h.app.conn().map(|c| c.name.as_str()), Some("v6"));
    assert_eq!((h.app.tabs.len(), h.app.focus), (2, Focus::Editor), "a new console, focused");
    assert!(h.app.conns.is_connected(h.app.profiles[0].id), "local-pg is still connected");
    // A profile with a tab: its tab, no new one.
    h.command("conn local-pg");
    assert_eq!(h.app.conn().map(|c| c.name.as_str()), Some("local-pg"));
    assert_eq!(h.app.tabs.len(), 2);
    // Choosing a completion with the arrow keys picks that one.
    h.keys(":conn ");
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("分析-replica"));
    // `:conn` alone is a usage error.
    let mut h = Harness::connected(Lang::En);
    h.keys(":conn ");
    h.app.profiles.clear();
    h.key(KeyCode::Backspace);
    h.type_text(" ");
    h.key(KeyCode::Enter);
    assert!(h.screen(80, 24).contains("Usage: :conn <profile>"));
}

#[test]
fn set_command_changes_and_saves_settings() {
    let path = temp_config("set", "language = \"en\" # keep me\n");
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.command("set language=ko");
    assert!(h.cmdline().is_none());
    assert_eq!(h.app.i18n.lang, Lang::Ko);
    assert_eq!(h.app.lang_setting, LangSetting::Ko);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "language = \"ko\" # keep me\n");
    // Setting and value completion: `set` → `set language=` → the values.
    h.keys(":set");
    h.key(KeyCode::Enter);
    assert_eq!(h.cmdline().unwrap().input.text(), "set ");
    h.type_text("ico");
    h.key(KeyCode::Enter);
    assert_eq!(h.cmdline().unwrap().input.text(), "set icons=");
    h.type_text("of");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.icons, datarig_core::config::IconsSetting::Off);
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"off\""));
    // Invalid values and settings: a localized error, the command line stays, nothing saved.
    let before = std::fs::read_to_string(&path).unwrap();
    h.command("set language=fr");
    let screen = h.screen(80, 24);
    let invalid = datarig_core::i18n::Msg::CommandsErrorSetValue {
        key: "language".into(),
        value: "fr".into(),
        values: "en|ko|auto".into(),
    };
    assert!(screen.contains(&ko_msg(&invalid)), "{screen}");
    assert!(h.cmdline().is_some());
    h.key(KeyCode::Esc);
    h.command("set colour=red");
    let error = h.cmdline().and_then(|c| c.error.as_ref()).map(|e| e.render(&h.app.i18n).to_string());
    let keys = "language|icons|secrets.default_source|commands.position|detail_view|clipboard|copy_header|\
                editor.cursor_shape|editor.clipboard|theme|editor.format_keyword_case|editor.format_indent|editor.auto_pairs";
    let unknown = ko_msg(&datarig_core::i18n::Msg::CommandsErrorSetKey { key: "colour".into(), keys: keys.into() });
    assert_eq!(error.as_deref(), Some(unknown.as_str()));
    // The error wraps in the popup instead of being cut.
    let head = unknown.split(" (").next().unwrap();
    assert!(h.screen(80, 24).contains(head), "{head:?}");
    h.key(KeyCode::Esc);
    h.command("set =ko");
    assert!(h.screen(80, 24).contains(ko(datarig_core::i18n::Label::CommandsErrorSetUsage)));
    h.key(KeyCode::Esc);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(h.app.i18n.lang, Lang::Ko);
    cleanup(&path);
}

/// `commands.position`: the command line is a popup near the top by default; `:set
/// commands.position=bottom` draws it on the last line (Neovim style), saves it, and a new
/// launch reads it back.
#[test]
fn command_line_popup_or_bottom() {
    use datarig_core::config::CommandsPosition;
    let path = temp_config("position", "language = \"en\" # keep me\n");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = Harness::with_config(&cfg, Lang::En);
    assert_eq!(h.app.prefs.commands_position, CommandsPosition::Popup);
    h.ctrl('k');
    let screen = h.screen(80, 24);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[5].contains("│ : type a command"), "{screen}");
    assert!(!lines[23].starts_with(':'), "the status bar stays: {screen}");
    h.key(KeyCode::Esc);
    h.command("set commands.position=bottom");
    assert_eq!(h.app.prefs.commands_position, CommandsPosition::Bottom);
    assert!(h.status(80, 24).contains("Command line position: The bottom line (Neovim style)"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("language = \"en\" # keep me\n"), "comments kept: {text}");
    assert!(text.contains("[commands]\nposition = \"bottom\"\n"), "{text}");
    h.ctrl('k');
    // The COMMAND badge where the status bar starts, then the input.
    assert!(h.status(80, 24).starts_with(" COMMAND  : type a command"), "{}", h.status(80, 24));
    h.key(KeyCode::Esc);
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    assert_eq!(cfg.prefs.commands_position, CommandsPosition::Bottom);
    cleanup(&path);
}

#[test]
fn unknown_command_shows_an_error_and_keeps_the_list() {
    let mut h = Harness::connected(Lang::En);
    h.keys(":frobz");
    h.key(KeyCode::Enter);
    let c = h.cmdline().expect("still open");
    assert!(c.error.is_some());
    assert!(h.screen(80, 24).contains("Not a command: frobz"));
    // Typing clears the error; Esc closes.
    h.key(KeyCode::Backspace);
    assert!(h.cmdline().unwrap().error.is_none());
    h.key(KeyCode::Esc);
    assert!(h.cmdline().is_none());
    // Korean UI: the error is Korean.
    let mut h = Harness::connected(Lang::Ko);
    h.command("frobz");
    assert!(
        h.screen(80, 24).contains(&ko_msg(&datarig_core::i18n::Msg::CommandsErrorUnknown { name: "frobz".into() }))
    );
}

#[test]
fn help_run_cancel_and_quit_commands() {
    let mut h = Harness::connected(Lang::En);
    // `:help` opens the help of the context below the command line.
    h.command("help");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help));
    assert_eq!(h.app.overlays.help().unwrap().origin, Ctx::VimNormal);
    h.key(KeyCode::Esc);
    h.command("run");
    assert_eq!(executes(&mut h), 1);
    h.command("cancel");
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst));
    // A command that takes no argument, given one, is an action search ("run statement");
    // with nothing matching either it is an error.
    h.db(DbEvent::Failed { id: 1, error: "x".into(), cancelled: false });
    h.command("quit zzz");
    assert!(h.cmdline().is_some() && !h.app.quit);
    assert!(h.screen(80, 24).contains(":quit takes no argument"));
    h.key(KeyCode::Esc);
    h.command("q");
    assert!(h.app.quit);
    // Without a profile there is no editor: `:run` is not offered and says so.
    let mut h = launch(&Config::default(), Startup::Normal);
    h.key(KeyCode::Esc); // the new-profile form
    h.command("run");
    assert!(h.screen(80, 24).contains(":run is not available here"));
    h.key(KeyCode::Esc);
    h.command("quit");
    assert!(h.app.quit);
}

#[test]
fn language_change_via_commands_is_persisted() {
    let path = temp_config("lang", "language = \"en\" # keep me\n");
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.command("korean");
    assert_eq!(h.app.i18n.lang, Lang::Ko);
    assert_eq!(h.app.lang_setting, LangSetting::Ko);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "language = \"ko\" # keep me\n");
    assert!(!text.contains("connections"), "built-in test DB is not written to the file");
    cleanup(&path);
}

// ── completion triggers ──────────────────────────────────────────────────────

#[test]
fn completion_trigger_rules() {
    let mut h = Harness::connected(Lang::En);
    h.keys("Go");
    h.keys("SELECT * FROM s");
    h.settle();
    assert!(h.popup().is_none(), "one identifier char does not pop up");
    h.key_mod(KeyCode::Char(' '), CTRL);
    assert!(h.popup().is_none(), "Ctrl+Space is no longer bound (macOS input source switch)");
    h.keys("h");
    assert!(h.popup().is_none(), "debounced: nothing before the timer");
    h.settle();
    assert!(h.popup().is_some(), "two chars + debounce");
    h.key(KeyCode::Esc);
    assert!(h.popup().is_none());
    assert_eq!(h.app.tab().editor.mode, Mode::Insert, "Esc only dismisses the popup");
    h.settle();
    assert!(h.popup().is_none(), "dismissed popup does not come back by itself");
    // Manual trigger: Ctrl+N (then Ctrl+N/Ctrl+P move the selection) and F4.
    h.ctrl('n');
    let p = h.popup().expect("Ctrl+N opens the popup");
    assert_eq!(p.selected, 0);
    h.ctrl('n');
    assert_eq!(h.popup().unwrap().selected, 1);
    h.ctrl('p');
    assert_eq!(h.popup().unwrap().selected, 0);
    h.key(KeyCode::Esc);
    h.key(KeyCode::F(4));
    assert!(h.popup().is_some(), "F4 opens the popup");
    h.key(KeyCode::Esc);
    // Typing a space cancels a pending popup.
    h.keys("op");
    h.keys(" ");
    h.settle();
    assert!(h.popup().is_none());
    // Open popup refines immediately while typing.
    h.keys("JOIN shop.");
    h.settle();
    let n = h.popup().unwrap().items.len();
    h.keys("u");
    let items = &h.popup().unwrap().items;
    assert!(items.len() < n && items[0].label == "users", "{items:?}");
}

/// An accepted name is SQL: a mixed-case name goes in quoted, also after an opening `"` the
/// user typed (which it replaces with the closing one), and a `WITH` query is a table.
#[test]
fn completion_inserts_names_as_sql() {
    use datarig_core::sql::complete::{ColumnInfo, Relation};
    let mut h = Harness::connected(Lang::En);
    let mut cat = catalog();
    let col = |n: &str| ColumnInfo { name: n.into(), type_name: "text".into() };
    cat.relations.push(Relation {
        schema: "shop".into(),
        name: "Order Lines".into(),
        is_view: false,
        columns: vec![col("Mixed Col"), col("MixedCase")],
    });
    h.db(DbEvent::Catalog(Ok(cat)));
    h.keys("ggdGi");
    h.keys("SELECT  FROM shop.ord");
    h.settle();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.tab().editor.text(), "SELECT  FROM shop.\"Order Lines\"");
    h.key(KeyCode::Esc);
    h.keys("06la");
    h.keys("mixedc");
    h.settle();
    assert_eq!(h.popup().expect("popup").items[0].label, "\"MixedCase\"");
    h.key(KeyCode::Tab);
    assert_eq!(h.app.tab().editor.text(), "SELECT \"MixedCase\" FROM shop.\"Order Lines\"");
    h.key(KeyCode::Esc);
    h.keys("A");
    // An opening quote, then the closing one already there: both are replaced.
    h.keys(" WHERE \"\"");
    h.key(KeyCode::Left);
    h.keys("mi");
    h.ctrl('n');
    assert_eq!(h.popup().expect("popup").items[0].label, "\"Mixed Col\"");
    h.key(KeyCode::Enter);
    let text = h.app.tab().editor.text();
    assert_eq!(text, "SELECT \"MixedCase\" FROM shop.\"Order Lines\" WHERE \"Mixed Col\"");
    assert_eq!(h.app.tab().editor.offset(), text.len(), "the cursor after the name");
    // A WITH query.
    h.key(KeyCode::Esc);
    h.keys("ggdGi");
    h.keys("WITH recent AS (SELECT id FROM shop.orders) SELECT * FROM rec");
    h.settle();
    let p = h.popup().expect("popup");
    assert_eq!(p.items[0].label, "recent");
}

// ── connection profiles ──────────────────────────────────────────────────────

fn temp_config(tag: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-flow-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.toml");
    std::fs::write(&p, body).unwrap();
    p
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn add_edit_duplicate_delete_profiles_with_secret_store() {
    let path = temp_config(
        "crud",
        "# profiles\n[[connections]]\nname = \"local-pg\"\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\ndatabase = \"datarig\"\n",
    );
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.command("new connection");
    assert!(h.form_open());
    h.type_text("prod");
    h.key(KeyCode::Tab);
    h.ctrl('u');
    h.type_text("db.example.com");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("app");
    h.key(KeyCode::Tab); // password storage (the keychain, preselected)
    h.key(KeyCode::Tab);
    h.type_text("s3cr3t");
    h.key(KeyCode::Tab);
    h.type_text("sales");
    assert_eq!(h.form().dsn.text(), "postgres://app@db.example.com:5432/sales?sslmode=prefer");
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(h.app.profiles.len(), 2);
    let prod = h.account("prod");
    assert_eq!(h.store.get(&prod).unwrap().as_deref(), Some("s3cr3t"), "stored under profile:<uuid>");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# profiles\n") && text.contains("name = \"prod\"") && !text.contains("s3cr3t"), "{text}");
    assert!(h.status(160, 45).contains("Saved prod"));
    assert_eq!(h.app.selected_profile().map(|id| id.account()), Some(prod.clone()), "the explorer shows it");

    // Edit + rename (explorer `e`): the id stays, so the keychain entry stays.
    h.key(KeyCode::BackTab);
    h.keys("e");
    assert_eq!(h.form().password.text(), "s3cr3t", "stored password loaded into the form");
    h.ctrl('u');
    h.type_text("prod-eu");
    h.ctrl('s');
    assert_eq!(h.app.profiles[1].name, "prod-eu");
    assert_eq!(h.account("prod-eu"), prod, "same id after the rename");
    assert_eq!(h.store.get(&prod).unwrap().as_deref(), Some("s3cr3t"));

    // Duplicate (`c`): new unique name, same settings, password copied.
    h.keys("c");
    assert_eq!(h.form().name.text(), "prod-eu-copy");
    h.ctrl('s');
    assert_eq!(h.app.profiles.len(), 3);
    let copy = h.account("prod-eu-copy");
    assert_ne!(copy, prod, "a copy has its own id");
    assert_eq!(h.store.get(&copy).unwrap().as_deref(), Some("s3cr3t"));

    // Name clash is a field error, nothing saved.
    h.keys("c");
    h.ctrl('u');
    h.type_text("prod-eu");
    h.ctrl('s');
    assert!(h.form_open(), "still in the form");
    h.key(KeyCode::Esc);

    // Delete (`d`) asks first; 'n' keeps, 'y' deletes (and removes the secret).
    h.explore("prod-eu-copy");
    h.keys("d");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(160, 45).contains("Delete the connection profile “prod-eu-copy”?"));
    h.keys("n");
    assert_eq!(h.app.profiles.len(), 3);
    h.keys("dy");
    assert_eq!(h.app.profiles.len(), 2);
    assert_eq!(h.store.get(&copy).unwrap(), None);

    // The profile of the open tab can be deleted too, after saying that its tab closes;
    // here the answer is no.
    h.explore("local-pg");
    h.keys("d");
    let details = h.app.overlays.confirm().map(|c| c.details.clone()).unwrap_or_default();
    assert!(details.contains(&datarig_core::i18n::Msg::ExplorerDeleteTabs { count: 1 }), "{details:?}");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Its open tab will close."), "{screen}");
    h.keys("n");
    let (again, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    assert_eq!(again.connections.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["local-pg", "prod-eu"]);
    cleanup(&path);
}

#[test]
fn disconnect_keeps_the_tabs_and_resets_the_connection() {
    let mut h = with_edge_results(Lang::En);
    h.app.profiles.push(ConnectionConfig { name: "other".into(), port: 6543, ..ConnectionConfig::test_db() });
    // A slow query is running inside an open transaction.
    h.keys("jjjjjjjjj");
    h.ctrl('e');
    assert!(h.app.tab().exec.running.is_some());
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    // Disconnecting asks first when a query runs or a transaction is open.
    h.keys(" cx");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let screen = h.screen(160, 45);
    assert!(
        screen.contains("Disconnect?") && screen.contains("A query is running and a transaction is open"),
        "{screen}"
    );
    h.keys("n");
    assert!(h.app.current_conn().is_some_and(|c| c.connected));
    assert!(!h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "nothing cancelled on no");
    h.keys(" cx");
    h.keys("y");
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "running query cancelled");
    assert!(
        h.app.tab().exec.session.is_none()
            && !h.app.current_conn().is_some_and(|c| c.connected)
            && !h.app.tab().exec.tx_open
            && h.app.tab().exec.running.is_none()
    );
    // The tab stays, with its text and results, marked as not connected.
    assert_eq!(h.app.tabs.len(), 1);
    assert!(matches!(h.app.tab().results, datarig_tui::app::Results::Rows(_)), "the results stay");
    assert_eq!(h.app.tab().editor.text(), SAMPLE_SQL, "the editor text survives");
    assert!(h.app.conns.catalog(h.app.tab().profile).relations.is_empty());
    assert!(h.status(160, 45).contains("Disconnected from local-pg; the open transaction was rolled back"));
    assert!(h.screen(160, 45).lines().next().unwrap().contains("console 1 ×"));
    // Connect another profile from the explorer; its node shows the attempt.
    h.explore("other");
    h.key(KeyCode::Enter);
    let other = h.app.profiles[1].id;
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("other"));
    assert_eq!(h.app.conns.state(other), NodeState::Connecting);
    // Events of a replaced attempt are dropped.
    let stale = h.app.conns.get(other).unwrap().generation - 1;
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Meta(other), generation: stale, ev: DbEvent::Connected });
    assert!(!h.app.conns.is_connected(other));
    h.db(DbEvent::Connected);
    assert!(h.app.conns.is_connected(other));
    assert_eq!(h.last_used_name().as_deref(), Some("other"));
    assert_eq!(h.app.tabs.len(), 2, "its first connect opened a console");
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
    // Ctrl+O on a profile that has a tab goes to that tab.
    h.ctrl('o');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    h.type_text("local");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.conn().map(|c| c.name.as_str()), Some("local-pg"));
    assert_eq!((h.app.tabs.len(), h.app.focus), (2, Focus::Editor), "no new tab; the editor is focused");
}

// ── startup flow ──────────────────────────────────────────────────

fn launch(cfg: &Config, startup: Startup) -> Harness {
    Harness::launched(cfg, Lang::En, Arc::new(MemoryStore::new()), startup)
}

#[test]
fn startup_focuses_the_explorer_on_the_last_used_profile() {
    let mut h = launch(&sample_config(Some("v6")), Startup::Normal);
    let id = |h: &Harness, n: &str| h.app.profiles.iter().find(|p| p.name == n).unwrap().id;
    assert_eq!(h.app.focus, Focus::Tree);
    assert!(h.app.conns.attempt().is_none() && h.app.tab().exec.session.is_none(), "nothing connects by itself");
    assert_eq!(h.app.selected_profile(), Some(id(&h, "v6")), "cursor on last_used");
    assert_eq!(h.rows(), ["+", "local-pg", "v6", "分析-replica"], "every profile, by name");
    let screen = h.screen(80, 24);
    assert!(screen.contains("＋ New connection") && screen.contains("分析-replica"), "{screen}");
    // Navigation and the `/` filter (a part of the name).
    h.keys("k");
    assert_eq!(h.app.selected_profile(), Some(id(&h, "local-pg")));
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::ExplorerFilter);
    h.type_text("分析");
    assert_eq!(h.rows(), ["+", "分析-replica"]);
    assert_eq!(h.app.selected_profile(), Some(id(&h, "分析-replica")), "cursor follows the filter");
    h.key(KeyCode::Esc);
    assert!(h.app.explorer.filter.text().is_empty(), "Esc clears the filter");
    h.keys("/");
    h.type_text("V");
    h.key(KeyCode::Enter);
    assert_eq!((h.app.key_context(), h.rows()), (Ctx::Explorer, vec!["+".to_string(), "v6".to_string()]));
    h.key(KeyCode::Esc);
    assert_eq!(h.rows().len(), 4, "Esc clears a kept filter too");
    // Enter connects; Esc cancels the attempt; a late event of it is dropped.
    h.explore("分析-replica");
    h.key(KeyCode::Enter);
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("分析-replica"));
    let replica = id(&h, "分析-replica");
    let generation = h.app.conns.get(replica).unwrap().generation;
    h.key(KeyCode::Esc);
    assert!(h.app.conns.attempt().is_none());
    assert_eq!(h.app.conns.state(replica), NodeState::Disconnected);
    assert!(h.status(80, 24).contains("Connection to 分析-replica cancelled"));
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Meta(replica), generation, ev: DbEvent::Connected });
    assert!(!h.app.conns.is_connected(replica));
    // `q` quits from the explorer.
    h.keys("q");
    assert!(h.app.quit);
}

#[test]
fn cli_profile_connects_directly_and_remembers_it() {
    let path = temp_config(
        "direct",
        "[[connections]]\nname = \"local-pg\"\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\n\n\
         [[connections]]\nname = \"prod\"\nhost = \"db.example.com\"\nuser = \"app\"\n",
    );
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launch(&cfg, Startup::Profile("prod".into()));
    let prod = h.app.profiles[1].id;
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("prod"));
    assert_eq!(h.app.selected_profile(), Some(prod));
    h.db(DbEvent::Connected);
    assert_eq!(h.app.tab().profile, Some(prod), "a console on it");
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
    assert!(h.status(160, 45).contains("Connected to prod"));
    assert_eq!(h.rows()[2], "prod", "its node is open");
    // Stored by id (the v1 file was upgraded at launch).
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!("last_used = \"{prod}\"\n")), "{text}");
    let (again, _) = config::load(Some(path.clone()));
    assert_eq!(again.last_used, Some(prod));
    cleanup(&path);
}

#[test]
fn unknown_cli_profile_opens_the_workspace_with_an_error() {
    let mut h = launch(&sample_config(None), Startup::Profile("nope".into()));
    assert_eq!(h.app.focus, Focus::Tree);
    assert!(h.app.conns.attempt().is_none());
    assert!(h.screen(80, 24).contains("No connection profile named “nope”"));
}

#[test]
fn connect_failure_and_timeout_show_on_the_node() {
    let mut h = launch(&sample_config(None), Startup::Normal);
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().is_some());
    h.db(DbEvent::ConnectFailed {
        error: "error connecting to server: Connection refused (os error 61)".into(),
        auth: false,
    });
    assert!(h.prompt().is_none(), "not an auth problem: no password prompt");
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some("Connection failed: error connecting to server: Connection refused (os error 61)")
    );
    assert_eq!(h.rows(), ["+", "local-pg", "  !", "v6", "分析-replica"], "an error line under the node");
    assert!(h.screen(80, 24).contains("✕   local-pg"));
    assert!(h.app.tab().profile.is_none(), "no console for a failed attempt");
    // A server that never answers: the attempt gives up after CONNECT_TIMEOUT.
    h.key(KeyCode::Enter);
    h.app.on_tick(std::time::Instant::now() + datarig_tui::app::CONNECT_TIMEOUT + Duration::from_millis(1));
    assert!(h.app.conns.attempt().is_none());
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("no answer within 10.0s")));
}

/// A connect that failed in the transport it goes through (a tunnel) says so in
/// words, naming where it could not go; reaching the far end failing reads as any connection
/// failure does.
#[test]
fn transport_failures_are_worded() {
    use datarig_core::driver::DbError;
    use datarig_core::transport::{DialError, Refusal};
    let target = |reason| DialError::Refused {
        host: "orders-db.internal".into(),
        port: 5432,
        reason,
        detail: "open failed".into(),
    };
    let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
    for (error, want) in [
        (DialError::NotOpen, "Connection failed: the tunnel to the server is closed; connect the profile again"),
        (target(Refusal::Prohibited), "Connection failed: the tunnel may not forward to orders-db.internal:5432"),
        (target(Refusal::Unreachable), "Connection failed: the tunnel could not reach orders-db.internal:5432"),
        (target(Refusal::Other), "Connection failed: the tunnel refused a connection to orders-db.internal:5432"),
        (
            DialError::Timeout(Duration::from_secs(10)),
            "Connection failed: no connection through the tunnel within 10.0s",
        ),
        (
            DialError::Failed(datarig_core::fault::Fault::io(&refused)),
            "Connection failed: the connection was refused (is the server running?)",
        ),
    ] {
        let mut h = launch(&sample_config(None), Startup::Normal);
        h.key(KeyCode::Enter);
        h.db(DbEvent::ConnectFailed { error: DbError::Transport(error), auth: false });
        assert!(h.prompt().is_none());
        assert_eq!(h.node_error("local-pg").as_deref(), Some(want));
    }
}

#[test]
fn missing_password_prompts_and_saves_to_keychain_after_success() {
    let mut h = launch(&sample_config(None), Startup::Normal);
    h.key(KeyCode::Enter);
    assert!(!h.app.conns.attempt().unwrap().had_password);
    h.db(DbEvent::ConnectFailed { error: "password missing".into(), auth: true });
    let p = h.prompt().expect("password prompt");
    assert!(p.save, "save to keychain is on by default");
    let screen = h.screen(80, 24);
    assert!(
        screen.contains("No saved password for this profile") && screen.contains("[x] Save to OS keychain"),
        "{screen}"
    );
    h.type_text("秘密 pw");
    assert!(!h.screen(80, 24).contains("秘密"), "masked");
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    let local = h.account("local-pg");
    assert_eq!(h.store.get(&local).unwrap(), None, "not stored before it worked");
    // Wrong password: the prompt shows the server's error.
    h.db(DbEvent::ConnectFailed { error: "password authentication failed for user \"datarig\"".into(), auth: true });
    assert!(h.screen(80, 24).contains("password authentication failed"));
    h.type_text("datarig");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    assert!(h.app.conns.is_connected(h.app.profiles[0].id));
    assert_eq!(h.store.get(&local).unwrap().as_deref(), Some("datarig"));
    assert!(h.status(160, 45).contains("Saved the password for local-pg to the OS keychain"));

    // Unchecked (Tab, Space): the password is used for this session only.
    let mut h = launch(&sample_config(None), Startup::Normal);
    h.key(KeyCode::Enter);
    h.db(DbEvent::ConnectFailed { error: "password missing".into(), auth: true });
    h.type_text("once");
    h.key(KeyCode::Tab);
    h.keys(" ");
    assert!(!h.prompt().unwrap().save);
    assert_eq!(h.prompt().unwrap().input.text(), "once", "Space toggles the box, it does not type");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    let local = h.account("local-pg");
    assert_eq!(h.store.get(&local).unwrap(), None);
    assert_eq!(h.app.secrets.session(&local), Some("once"));
}

#[test]
fn no_profiles_open_the_form_over_the_welcome_panel() {
    let cfg = Config { connections: Vec::new(), ..Config::default() };
    let mut h = launch(&cfg, Startup::Normal);
    assert!(h.form_open(), "form opens directly");
    h.key(KeyCode::Esc);
    let screen = h.screen(80, 24);
    assert!(screen.contains("No connection profiles yet") && screen.contains("＋ New connection"), "{screen}");
    assert!(
        screen.contains("n   New connection") && screen.contains("F1  Keyboard help"),
        "the welcome keys: {screen}"
    );
    // The welcome panel has its own keys.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Welcome);
    h.keys("n");
    assert!(h.form_open());
    h.key(KeyCode::Esc);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Explorer);
    h.keys("n");
    h.type_text("first");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("me");
    h.ctrl('s');
    assert_eq!(h.app.profiles.len(), 1);
    assert!(!h.form_open());
    h.key(KeyCode::Enter);
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("first"));
}

#[test]
fn deleting_the_last_profile_shows_the_welcome_panel() {
    // Deleting the last profile leaves the workspace as it is with none configured: the welcome
    // panel and the explorer's "New connection" row.
    let cfg = Config { connections: vec![ConnectionConfig::test_db()], ..Config::default() };
    let mut h = launch(&cfg, Startup::Normal);
    assert_eq!(h.app.profiles.len(), 1);
    h.keys("d");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.keys("y");
    assert!(h.app.profiles.is_empty());
    assert!(!h.form_open());
    let screen = h.screen(80, 24);
    assert!(screen.contains("No connection profiles yet") && screen.contains("Welcome"), "{screen}");
    assert_eq!(h.rows(), ["+"]);
}

#[test]
fn explorer_mouse_and_delete() {
    let mut h = launch(&sample_config(None), Startup::Normal);
    h.draw(80, 24);
    let area = h.app.explorer.area;
    let click = |h: &mut Harness, row: u16| {
        h.app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 5,
            row: area.y + row,
            modifiers: NONE,
        }))
    };
    let name = |h: &Harness| h.app.selected_profile().and_then(|id| h.app.profile(id)).map(|p| p.name.clone());
    click(&mut h, 2);
    assert_eq!(name(&h).as_deref(), Some("v6"));
    assert!(h.app.conns.attempt().is_none(), "single click only selects");
    click(&mut h, 3);
    click(&mut h, 3);
    assert_eq!(h.connecting().map(|c| c.0).as_deref(), Some("分析-replica"), "double click connects");
    h.key(KeyCode::Esc);
    // The wheel scrolls the list, not the cursor.
    let before = h.selected();
    h.app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: area.x + 5,
        row: area.y + 1,
        modifiers: NONE,
    }));
    assert_eq!(h.selected(), before);
    // `d` asks, `y` deletes; the cursor goes to the row above.
    h.explore("分析-replica");
    h.keys("d");
    h.keys("y");
    assert_eq!(h.app.profiles.len(), 2);
    assert_eq!(name(&h).as_deref(), Some("v6"));
    h.keys("d");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.profiles.len(), 2);
}

#[test]
fn plaintext_password_is_migrated_at_launch() {
    let original = "language = \"auto\"      # auto | en | ko\npage_size = 500\n\n[[connections]]\nname = \"local-pg\"\ndriver = \"postgres\"\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\npassword = \"datarig\" # plaintext\ndatabase = \"datarig\"\nsslmode = \"disable\"\n";
    let path = temp_config("migrate", original);
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launch(&cfg, Startup::Normal);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("password"), "{text}");
    assert!(text.contains("language = \"auto\"      # auto | en | ko"), "{text}");
    assert!(text.contains("port = 55432"), "{text}");
    assert_eq!(h.store.get(&h.account("local-pg")).unwrap().as_deref(), Some("datarig"));
    assert!(h.screen(160, 45).contains("Moved 1 password from the config file to the OS keychain"));
    // The connection now takes its password from the keychain.
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    let (again, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    assert!(again.connections[0].password.is_empty());
    cleanup(&path);
}

#[test]
fn keychain_unavailable_at_launch_keeps_file_and_prompts() {
    let original = "[[connections]]\nname = \"local-pg\"\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\npassword = \"datarig\"\ndatabase = \"datarig\"\n";
    let path = temp_config("nokeychain", original);
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h =
        Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::unavailable("no secret service")), Startup::Normal);
    // No keychain: the keychain steps do not apply, so the file becomes version 2 with ids, and
    // the plaintext password stays in it.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("version = 2\n[[connections]]\nid = "), "{text}");
    assert!(text.contains("password = \"datarig\""), "{text}");
    assert!(
        h.screen(160, 45)
            .contains("Keychain unavailable (no keychain on this system); password left in the config file")
    );
    // The plaintext password still works for connecting.
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    h.key(KeyCode::Esc);
    // Other saves keep it too (not migrated, not lost).
    h.command("korean");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("password = \"datarig\""), "{text}");
    // A new profile starts on `prompt` (`secrets.default_source = auto`), and the form says
    // why the keychain cannot be picked.
    h.command(ko(datarig_core::i18n::Label::FormTitleNew));
    assert_eq!(h.form().source, datarig_core::secret::SourceKind::Prompt);
    let screen = h.screen(160, 45);
    assert!(screen.contains(ko(datarig_core::i18n::Label::FormPromptHint)), "{screen}");
    let error = ko(datarig_core::i18n::Label::FaultKeychainNoStore).into();
    let unavailable = ko_msg(&datarig_core::i18n::Msg::FormSourceKeychainUnavailable { error });
    let head = unavailable.split(". ").next().unwrap();
    assert!(screen.contains(head), "{head:?} in {screen}");
    h.key(KeyCode::Esc);
    // No keychain: the prompt says so and offers no checkbox.
    h.app.profiles.push(ConnectionConfig {
        name: "nopw".into(),
        password: String::new(),
        ..ConnectionConfig::test_db()
    });
    h.keys("j");
    h.key(KeyCode::Enter);
    h.db(DbEvent::ConnectFailed { error: "password missing".into(), auth: true });
    let screen = h.screen(80, 24);
    let no_keychain = ko(datarig_core::i18n::Label::PromptPasswordNoKeychain);
    let head = no_keychain.split(" — ").next().unwrap();
    assert!(screen.contains(head) && !screen.contains("[x]"), "{screen}");
    assert!(screen.contains(ko(datarig_core::i18n::Label::PromptPasswordKeysSession)), "{screen}");
    cleanup(&path);
}

const PLAINTEXT: &str = "language = \"auto\" # mine\n\n[[connections]]\nname = \"local-pg\"\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\npassword = \"datarig\"\ndatabase = \"datarig\"\n";

/// Next background event other than the launch-time keychain probe, or fail after a few
/// seconds.
async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> AppEvent {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a background event")
            .expect("channel open");
        if !matches!(ev, AppEvent::KeychainProbed(_)) {
            return ev;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_moves_passwords_in_the_background_behind_a_notice() {
    let path = temp_config("slowkeychain", PLAINTEXT);
    let (cfg, _) = config::load(Some(path.clone()));
    let store = Arc::new(GatedStore::default());
    let _open_at_end = store.open_on_drop();
    let (mut h, mut rx) =
        Harness::started(&cfg, Lang::En, store.clone() as Arc<dyn SecretStore>, Startup::Profile("local-pg".into()));
    // The keychain is waiting for the user: the app shows why and stays responsive.
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Busy));
    let screen = h.screen(80, 24);
    assert!(screen.contains("Moving saved passwords to the system keychain"), "{screen}");
    h.app.first_frame_drawn();
    assert!(h.app.conns.attempt().is_none(), "`datarig <profile>` waits for the password move");
    let before = h.selected();
    h.keys("jd");
    assert_eq!((h.selected(), h.overlay_kind()), (before, Some(OverlayKind::Busy)), "no explorer keys");
    h.ctrl('k');
    assert!(h.cmdline().is_some(), "root keys still work");
    h.key(KeyCode::Esc);
    // A setting changed meanwhile is saved after the move, not over it. The move saves the
    // profile ids first, then waits for the keychain with the password still in the file.
    h.command("set language=ko");
    store.wait_until_held();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("id = ") && text.contains("password = \"datarig\""), "{text}");
    assert!(text.contains("language = \"auto\" # mine"), "the setting waits for the move: {text}");
    store.open();
    let ev = next_event(&mut rx).await;
    assert!(matches!(ev, AppEvent::Migrated(_)), "{ev:?}");
    h.app.on_app_event(ev);
    assert_ne!(h.overlay_kind(), Some(OverlayKind::Busy));
    assert_eq!(store.inner.get(&h.account("local-pg")).unwrap().as_deref(), Some("datarig"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("password") && text.contains("language = \"ko\" # mine"), "{text}");
    assert!(h.status(80, 24).contains(&ko_msg(&datarig_core::i18n::Msg::SecretMigrated { count: 1 })));
    // Then the deferred connection starts, with the password from the keychain.
    assert_eq!(h.app.conns.attempt().map(|c| (c.name.as_str(), c.had_password)), Some(("local-pg", true)));
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_move_keeps_the_password_when_the_keychain_disagrees() {
    let path = temp_config("forgetful", PLAINTEXT);
    let (cfg, _) = config::load(Some(path.clone()));
    let store = Arc::new(GatedStore::forgetful());
    store.open();
    let (mut h, mut rx) = Harness::started(&cfg, Lang::En, store.clone() as Arc<dyn SecretStore>, Startup::Normal);
    let ev = next_event(&mut rx).await;
    h.app.on_app_event(ev);
    // The ids are saved first; the password stays, and so does version 1 (retry next launch).
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("password = \"datarig\"") && text.contains("id = ") && !text.contains("version"), "{text}");
    assert_eq!(h.app.profiles[0].password, "datarig", "still usable for connecting");
    let screen = h.screen(160, 45);
    assert!(screen.contains("local-pg: the keychain gave back a different password"), "{screen}");
    // A keychain that fails outright: the file stays too, and passwords are asked for later.
    let down = Arc::new(MemoryStore::unavailable("no secret service"));
    let (mut h, mut rx) = Harness::started(&cfg, Lang::En, down as Arc<dyn SecretStore>, Startup::Normal);
    let ev = next_event(&mut rx).await;
    h.app.on_app_event(ev);
    assert!(std::fs::read_to_string(&path).unwrap().contains("password = \"datarig\""));
    assert_eq!(h.app.secrets.unavailable.as_ref().map(|f| f.detail.as_str()), Some("no secret service"));
    assert!(
        h.screen(160, 45)
            .contains("Keychain unavailable (no keychain on this system); password left in the config file")
    );
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_without_plaintext_shows_no_notice_and_connects_after_the_first_frame() {
    let path = temp_config(
        "nomove",
        "version = 2\n[[connections]]\nid = \"3f0b8f5e-6a57-4f7e-9a53-0c1c2b8f9d11\"\nname = \"local-pg\"\n",
    );
    let (cfg, _) = config::load(Some(path.clone()));
    let (mut h, _rx) = Harness::started(
        &cfg,
        Lang::En,
        Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>,
        Startup::Profile("local-pg".into()),
    );
    assert_ne!(h.overlay_kind(), Some(OverlayKind::Busy));
    assert!(h.app.conns.attempt().is_none(), "not before the first frame");
    h.app.first_frame_drawn();
    assert_eq!(h.app.conns.attempt().map(|c| c.name.as_str()), Some("local-pg"));
    cleanup(&path);
}

// ── profile ids, config v2, icons ────────────────────────

const V1_NAMED: &str = "# mine\nlast_used = \"local-pg\"\n\n[[connections]]\nname = \"local-pg\" # main\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\n";

#[test]
fn launch_upgrades_a_v1_config_and_rekeys_its_keychain_entry() {
    let path = temp_config("v1", V1_NAMED);
    let (cfg, _) = config::load(Some(path.clone()));
    let store = Arc::new(MemoryStore::new());
    store.set("local-pg", "from-v06").unwrap(); // the v0.6 entry, keyed by name
    let mut h = Harness::launched(&cfg, Lang::En, store.clone(), Startup::Normal);
    let id = h.app.profiles[0].id;
    assert_eq!(h.app.config_version, 2);
    assert_eq!(store.get(&id.account()).unwrap().as_deref(), Some("from-v06"));
    assert_eq!(store.get("local-pg").unwrap(), None, "old entry removed after the check");
    let bak = path.with_file_name("config.toml.v1.bak");
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), V1_NAMED, "the original is kept");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# mine\n"), "{text}");
    assert!(text.contains(&format!("last_used = \"{id}\"")) && text.contains("version = 2"), "{text}");
    assert!(text.contains(&format!("[[connections]]\nid = \"{id}\"\nname = \"local-pg\" # main")), "{text}");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Config file upgraded to format 2; the old file is kept as"), "{screen}");
    assert_eq!(h.app.selected_profile(), Some(id), "cursor on last_used");
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password, "the re-keyed password is used");
    cleanup(&path);
}

#[test]
fn before_the_upgrade_the_old_name_entry_is_still_found() {
    // A v1 config the migration has not finished (not launched here): `profile:<uuid>` is
    // empty, so the entry named after the profile is used.
    let path = temp_config("v1fallback", V1_NAMED);
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.store.set("local-pg", "from-v06").unwrap();
    h.key(KeyCode::BackTab);
    h.keys("x");
    h.key(KeyCode::Enter);
    assert!(h.app.conns.attempt().unwrap().had_password);
    // After the upgrade, a profile never looks at a name entry (it may be another profile's).
    h.app.config_version = 2;
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    assert!(!h.app.conns.attempt().unwrap().had_password);
    cleanup(&path);
}

#[test]
fn renaming_a_profile_keeps_its_password() {
    let mut h = launch(&sample_config(None), Startup::Normal);
    let id = h.app.profiles[0].id;
    h.store.set(&id.account(), "secret").unwrap();
    h.keys("e");
    h.ctrl('u');
    h.type_text("renamed");
    h.ctrl('s');
    assert_eq!((h.app.profiles[0].name.as_str(), h.app.profiles[0].id), ("renamed", id));
    h.key(KeyCode::Enter);
    let c = h.app.conns.attempt().unwrap();
    assert!(c.name == "renamed" && c.had_password, "no password prompt needed");
    h.db(DbEvent::Connected);
    assert!(h.prompt().is_none());
    assert_eq!(h.app.last_used, Some(id));
}

#[test]
fn a_config_without_profiles_starts_with_zero_profiles() {
    // No built-in test DB any more: an empty file means no profiles at all.
    let path = temp_config("zero", "");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launch(&cfg, Startup::Normal);
    assert!(h.app.profiles.is_empty());
    assert!(h.form_open(), "the empty-state flow: the new-profile form");
    h.key(KeyCode::Esc);
    assert!(h.screen(80, 24).contains("No connection profiles yet"));
    cleanup(&path);
}

#[test]
fn keychain_unavailable_warning_is_shown_once_per_machine() {
    let state = std::env::temp_dir().join(format!("datarig-flow-{}-state", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let body = "version = 2\n[[connections]]\nid = \"3f0b8f5e-6a57-4f7e-9a53-0c1c2b8f9d11\"\nname = \"a\"\npassword = \"pw\"\n";
    for run in 0..2 {
        let path = temp_config("once", body);
        let (cfg, _) = config::load(Some(path.clone()));
        let mut app = new_app(&cfg, Lang::En);
        app.set_secret_store(Arc::new(MemoryStore::unavailable("no secret service")) as Arc<dyn SecretStore>);
        app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.clone()) });
        app.launch(Startup::Normal);
        let warned = app.notices.iter().any(|n| matches!(n.msg, datarig_core::i18n::Msg::SecretMigrateFailed { .. }));
        assert_eq!(warned, run == 0, "run {run}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("password = \"pw\""), "kept in the file");
        cleanup(&path);
    }
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn icons_setting_is_saved_and_toggled() {
    let path = temp_config("icons", "language = \"en\"\n");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = Harness::with_config(&cfg, Lang::En);
    assert!(!h.app.icons_on(), "auto is not decided yet: text marks");
    // `:set icons=` shows the description with a glyph preview.
    h.ctrl('k');
    h.type_text("set icons=");
    let screen = h.screen(120, 30);
    assert!(
        screen.contains("Needs a Nerd Font as the terminal font") && screen.contains(&datarig_tui::icons::preview())
    );
    h.type_text("on");
    h.key(KeyCode::Enter);
    assert!(h.app.icons_on());
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"on\""));
    // The toggle action flips what is shown now (`Space ,` opens the settings screen;
    // the action stays on the command line).
    h.command("ui.icons.toggle");
    assert!(!h.app.icons_on());
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"off\""));
    let st = h.status(120, 30);
    assert!(st.contains("Nerd Font icons off"), "{st}");
    // `auto` asks the question again; the answer is saved.
    h.command("set icons=auto");
    assert_eq!(h.overlay_kind(), Some(datarig_tui::app::overlay::OverlayKind::IconsAsk));
    h.key(KeyCode::Char('y'));
    assert!(h.app.icons_on() && h.overlay_kind().is_none());
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"on\""));
    cleanup(&path);
}

#[test]
fn auth_failure_prompts_for_password() {
    let mut h = Harness::new(Lang::En);
    h.db(DbEvent::ConnectFailed { error: "password authentication failed for user \"x\"".into(), auth: true });
    let p = h.prompt().expect("password prompt");
    assert_eq!(p.profile, "local-pg");
    let t = h.draw(80, 24);
    let screen: String = (0..24).map(|y| row_text(t.backend().buffer(), y)).collect::<Vec<_>>().join("\n");
    assert!(screen.contains("Password for local-pg") && screen.contains("password authentication failed"), "{screen}");
    h.type_text("hunter2");
    let t = h.draw(80, 24);
    let screen: String = (0..24).map(|y| row_text(t.backend().buffer(), y)).collect::<Vec<_>>().join("\n");
    assert!(!screen.contains("hunter2") && screen.contains("•••••••"), "masked: {screen}");
    h.key(KeyCode::Enter);
    assert!(h.prompt().is_none());
    let local = h.account("local-pg");
    assert_eq!(h.app.secrets.session(&local), Some("hunter2"));
    assert_eq!(h.store.get(&local).unwrap(), None, "prompted password is not stored");
    // A non-auth failure does not prompt.
    h.db(DbEvent::ConnectFailed { error: "Connection refused".into(), auth: false });
    assert!(h.prompt().is_none());
}

#[test]
fn test_connection_cancel_and_stale_results() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab); // the explorer, on the profile
    h.keys("t");
    assert_eq!(h.app.conn_test.as_ref().unwrap().state, TestState::Running);
    let seq = h.app.conn_test.as_ref().unwrap().seq;
    // Esc in the explorer cancels the running test first.
    h.key(KeyCode::Esc);
    assert_eq!(h.app.conn_test.as_ref().unwrap().state, TestState::Cancelled);
    assert!(h.status(160, 45).contains("Test cancelled"));
    // A late result of the cancelled test is ignored.
    h.app.on_app_event(AppEvent::Ping {
        seq,
        result: Ok(PingInfo { server_version: "17".into(), latency: Duration::ZERO }),
    });
    assert_eq!(h.app.conn_test.as_ref().unwrap().state, TestState::Cancelled);
    h.keys("t");
    h.ping(Err(PingError::Timeout(Duration::from_secs(5))));
    assert!(h.status(160, 45).contains("no answer within 5.0s"), "{}", h.status(160, 45));
    // The command line runs the same action; the result goes to the status bar.
    h.command("test connection");
    h.ping(Ok(PingInfo { server_version: "17.2".into(), latency: Duration::from_millis(3) }));
    assert!(h.status(160, 45).contains("Connection OK · server 17.2 · 3ms"));
}

// ── context keymap ─────────────────────────────────────────────────

use datarig_tui::app::Focus;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::keymap::Ctx;

fn keymap_config(body: &str) -> Config {
    let mut cfg = config::parse(body).unwrap_or_else(|e| panic!("{e:?}"));
    cfg.connections = test_db_config().connections;
    cfg
}

#[test]
fn remapped_keys_run_their_actions_and_bad_entries_are_reported() {
    let cfg = keymap_config(
        "[keymap.explorer]\n\"x\" = \"explorer.bottom\"\n\"q\" = \"none\"\n\"ctrl+e\" = \"explorer.top\"\n\
         [keymap.nav]\n\"space c m\" = \"conn.quick_connect\"\n[keymap.nowhere]\n\"a\" = \"app.quit\"\n",
    );
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.key(KeyCode::BackTab); // editor -> explorer
    h.keys("x");
    assert_eq!(h.selected(), h.rows().len() - 1, "x = explorer.bottom");
    h.keys("q");
    assert!(!h.app.quit, "q unbound in the explorer");
    h.ctrl('e');
    assert_eq!(executes(&mut h), 1, "a protected key cannot be taken from the workspace");
    h.keys(" cm");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect), "Space c m opens quick connect");
    h.key(KeyCode::Esc);
    // The palette shows the remapped key.
    h.key(KeyCode::Esc);
    h.ctrl('k');
    h.type_text("explorer last");
    let t = h.draw(160, 45);
    let rows: String = (0..45).map(|y| row_text(t.backend().buffer(), y)).collect::<Vec<_>>().join("\n");
    assert!(rows.contains("Explorer: last item") && rows.contains("G / x"), "{rows}");
    h.key(KeyCode::Esc);
    // Launched: the skipped entries are reported, the app still starts.
    let mut h = launch(&cfg, Startup::Normal);
    let screen = h.screen(160, 45);
    assert!(screen.contains("Key map: unknown context [keymap.nowhere], ignored"), "{screen}");
    assert!(screen.contains("Key map [explorer] “ctrl+e”: this app key cannot be hidden here; ignored"), "{screen}");
}

#[test]
fn messages_name_the_remapped_keys() {
    let cfg = keymap_config(
        "[keymap.root]\n\"ctrl+c\" = \"none\"\n\"f8\" = \"query.cancel\"\n\
         [keymap.nav]\n\"space t u\" = \"none\"\n\"space c r\" = \"none\"\n\"space x r\" = \"conn.reconnect_current\"\n",
    );
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.ctrl('e');
    h.ctrl('e');
    let status = h.status(160, 45);
    assert!(status.contains("A query is already running (F8 to cancel)"), "{status}");
    h.key(KeyCode::F(8));
    h.db(DbEvent::Failed { id: 1, error: "canceling statement due to user request".into(), cancelled: true });
    // The busy notice names the cancel key (it quits there).
    h.app.overlays.push(datarig_tui::app::overlay::Overlay::Busy(datarig_tui::app::overlay::Busy {
        title: datarig_core::i18n::Label::MigrateTitle,
        text: datarig_core::i18n::Label::MigrateRunning,
    }));
    let screen = h.screen(160, 45);
    assert!(screen.contains("q/F8 quit"), "{screen}");
    // An action without a key: the command that runs it, else its name.
    assert_eq!(h.app.key_for(datarig_tui::app::action::Action::ReconnectCurrent, Ctx::Nav), "Space x r");
    assert_eq!(h.app.key_for(datarig_tui::app::action::Action::ReopenTab, Ctx::Nav), "Reopen closed tab");
    assert_eq!(h.app.key_for(datarig_tui::app::action::Action::ScriptSave, Ctx::Busy), ":w");
    assert_eq!(h.app.key_for(datarig_tui::app::action::Action::ScriptSave, Ctx::Nav), "Ctrl+S");
}

#[test]
fn a_bad_key_in_the_key_map_is_reported_in_the_ui_language() {
    let cfg = keymap_config("[keymap.explorer]\n\"hyper+x\" = \"explorer.bottom\"\n\"shift+1\" = \"explorer.top\"\n");
    let mut h = Harness::launched(&cfg, Lang::Ko, Arc::new(MemoryStore::new()), Startup::Normal);
    let screen = h.screen(160, 45);
    let (context, part) = (|| "explorer".to_string(), |k: &str| k.to_string());
    let unknown = datarig_core::i18n::Msg::KeymapBadKeyUnknown {
        context: context(),
        key: part("hyper+x"),
        part: part("hyper+x"),
    };
    assert!(screen.contains(&ko_msg(&unknown)), "{screen}");
    let shifted = datarig_core::i18n::Msg::KeymapBadKeyShifted {
        context: context(),
        key: part("shift+1"),
        part: part("shift+1"),
    };
    assert!(screen.contains(&ko_msg(&shifted)), "{screen}");
    assert!(!screen.contains("unknown key") && !screen.contains("as the character"), "no English inside: {screen}");
}

#[test]
fn hangul_input_source_navigates_outside_text_input() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab); // explorer
    h.type_text("\u{3153}\u{3153}");
    assert_eq!(h.selected(), 3, "\u{3153} = j");
    h.type_text("\u{314F}");
    assert_eq!(h.selected(), 2, "\u{314F} = k");
    h.type_text("\u{314E}\u{314E}"); // g g
    assert_eq!(h.selected(), 0);
    assert!(h.status(160, 45).contains("\u{D55C}/A"), "the status bar says the input source is Korean");
    h.app.on_tick(std::time::Instant::now() + Duration::from_secs(4));
    assert!(!h.status(160, 45).contains("\u{D55C}/A"), "only for a few seconds");
    // Editor Normal: the jamo on `j` and `l` move, the one on `g` twice goes to the top;
    // Insert types Hangul as is. (Jamo and syllables are written as \u escapes.)
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::VimNormal);
    assert_eq!((h.app.tab().editor.row, h.app.tab().editor.col), (1, 0));
    h.type_text("\u{3153}\u{3153}\u{3163}");
    assert_eq!((h.app.tab().editor.row, h.app.tab().editor.col), (3, 1));
    // An IME-composed syllable counts as its keys: U+D788 = g l (vim `g` then `l`: nothing).
    h.type_text("\u{D788}");
    assert_eq!((h.app.tab().editor.row, h.app.tab().editor.col), (3, 1));
    h.type_text("\u{314E}\u{314E}");
    assert_eq!(h.app.tab().editor.row, 0);
    h.type_text("\u{3151}"); // i
    assert_eq!(h.app.tab().editor.mode, Mode::Insert);
    h.type_text("\u{3153}\u{D55C}\u{AE00}");
    assert!(
        h.app.tab().editor.lines[0].starts_with("\u{3153}\u{D55C}\u{AE00}-- datarig"),
        "{}",
        h.app.tab().editor.lines[0]
    );
    h.key(KeyCode::Esc);
    // The explorer at launch: the jamo on `j` moves; its `/` filter takes CJK text as text.
    let mut h = launch(&sample_config(None), Startup::Normal);
    h.type_text("\u{3153}");
    assert_eq!(h.selected(), 2);
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::ExplorerFilter);
    h.type_text("分析");
    assert_eq!(h.app.explorer.filter.text(), "分析");
    assert_eq!(h.rows(), ["+", "分析-replica"]);
    // A confirmation reads the jamo on `y` (U+315B) as y.
    h.key(KeyCode::Esc);
    h.keys("d");
    h.type_text("\u{315B}");
    assert_eq!(h.app.profiles.len(), 2, "\u{315B} = y confirms");
}

#[test]
fn held_keys_repeat_movement_but_not_commands() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab); // explorer
    h.key(KeyCode::Char('j'));
    for _ in 0..2 {
        h.key_kind(KeyCode::Char('j'), NONE, KeyEventKind::Repeat);
    }
    assert_eq!(h.selected(), 4, "held j keeps moving");
    h.key_kind(KeyCode::Char('g'), NONE, KeyEventKind::Repeat);
    h.key_kind(KeyCode::Char('g'), NONE, KeyEventKind::Repeat);
    assert_eq!(h.selected(), 4, "a held key does not start a sequence");
    h.key_kind(KeyCode::Char('e'), CTRL, KeyEventKind::Press);
    h.key_kind(KeyCode::Char('e'), CTRL, KeyEventKind::Repeat);
    h.key_kind(KeyCode::Char('e'), CTRL, KeyEventKind::Repeat);
    assert_eq!(executes(&mut h), 1, "held Ctrl+E runs once");
}

#[test]
fn explorer_grid_and_editor_keys_follow_the_plan() {
    let mut h = with_edge_results(Lang::En);
    // Editor Normal: `q` no longer quits; `?` is vim's (backward search), not help.
    h.keys("q?");
    assert!(!h.app.quit && h.overlay_kind().is_none());
    assert_eq!(h.app.key_context(), Ctx::VimSearch, "the search prompt");
    h.key(KeyCode::Esc);
    // Space ? and F1 are help there.
    h.keys(" ?");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help));
    h.key(KeyCode::Esc);
    h.key(KeyCode::F(1));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help));
    h.key(KeyCode::Esc);
    // vim Insert: Ctrl+W deletes the word before the cursor and never closes anything.
    h.keys("A");
    h.ctrl('w');
    assert!(h.overlay_kind().is_none() && !h.app.quit && h.app.focus == Focus::Editor);
    assert_eq!(h.app.tab().editor.text(), SAMPLE_SQL.replacen("id <= 8;", "id <= 8", 1));
    h.key(KeyCode::Esc);
    h.keys("u");
    assert_eq!(h.app.tab().editor.text(), SAMPLE_SQL);
    // Grid: `g g` / `G`, and `q` / `Esc` go back to the editor instead of quitting.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.keys("G");
    assert_eq!(h.app.tab().grid.row, 7);
    h.keys("g");
    assert_eq!(h.app.tab().grid.row, 7, "a single g waits for the next key");
    h.keys("g");
    assert_eq!(h.app.tab().grid.row, 0);
    h.keys("q");
    assert!(!h.app.quit);
    assert_eq!(h.app.focus, Focus::Editor);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Esc);
    assert_eq!(h.app.focus, Focus::Editor);
    // Explorer: `?` alone does nothing, Space ? and F1 are help as everywhere, `G` / `g g`
    // jump, `q` quits (the outermost pane).
    h.key(KeyCode::BackTab);
    h.keys("?");
    assert!(h.overlay_kind().is_none());
    h.keys(" ?");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help));
    h.key(KeyCode::Esc);
    h.key(KeyCode::F(1));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help));
    h.key(KeyCode::Esc);
    h.keys("G");
    // The last row (the databases level counts too).
    assert_eq!(h.selected(), h.rows().len() - 1);
    h.keys("gg");
    assert_eq!(h.selected(), 0);
    // The leader works in every non-text pane.
    h.keys(" ,");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Settings), "Space ,: the settings screen");
    h.key(KeyCode::Esc);
    h.keys(" cn");
    assert!(h.form_open(), "Space c n: new profile");
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    h.keys("q");
    assert!(h.app.quit);
}

/// A config file from before the editor had vim keys only may say `[editor] mode`: it loads
/// without a word, the editor has vim keys, and the next save drops the key (and the table it
/// leaves empty).
#[test]
fn a_config_with_the_retired_editor_mode_loads_silently_and_loses_it_on_save() {
    let path = temp_config("mode", "language = \"en\"\n\n[editor]\nmode = \"standard\"\n");
    let (mut cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none(), "{err:?}");
    cfg.connections = test_db_config().connections;
    let mut h = launch(&cfg, Startup::Normal);
    let screen = h.screen(160, 45);
    assert!(!screen.to_lowercase().contains("standard"), "no notice: {screen}");
    h.command("set language=ko");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("language = \"ko\"\n") && !text.contains("editor") && !text.contains("\nmode"), "{text}");
    cleanup(&path);
}

/// Open the cell viewer on the first cell of the edge-case results.
fn with_viewer() -> Harness {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    assert_eq!(h.app.key_context(), Ctx::CellViewer);
    h
}

#[test]
fn workspace_keys_pass_through_the_cell_viewer() {
    // The viewer is not modal (as before the context keymap): run, cancel, the palette and
    // "switch connection" work while it is open; its own keys win over everything else.
    let mut h = with_viewer();
    h.ctrl('e');
    assert_eq!(executes(&mut h), 1, "Ctrl+E runs with the viewer open");
    assert!(h.viewer().is_some(), "and the viewer stays");
    h.ctrl('c');
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "Ctrl+C cancels the running query");
    h.key_mod(KeyCode::Enter, CTRL);
    assert!(h.viewer().is_some());
    // Its own keys: scrolling, and Tab / Shift+Tab / `:` do not reach the panes below.
    h.keys("j");
    assert_eq!(h.viewer().unwrap().scroll, 1);
    h.key(KeyCode::Tab);
    h.key(KeyCode::BackTab);
    h.keys(":");
    assert_eq!(h.app.focus, Focus::Results);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    // The palette opens on top and closes back to the viewer.
    h.ctrl('k');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands));
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    h.key(KeyCode::Esc);
    assert!(h.viewer().is_none(), "Esc closes the viewer");
    // Ctrl+O (quick connect) and Ctrl+Q also pass through; quick connect opens over it.
    let mut h = with_viewer();
    h.ctrl('o');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    assert!(h.viewer().is_some());
    let mut h = with_viewer();
    h.ctrl('q');
    assert!(h.app.quit);
}

// ── quit safety ────────────────────────────────────────────────────

#[test]
fn quitting_with_a_running_query_asks_then_cancels_and_closes() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    assert!(h.app.tab().exec.running.is_some());
    // Every quit path asks first and says what would be lost.
    h.ctrl('q');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(!h.app.quit);
    let screen = h.screen(80, 24);
    assert!(screen.contains("Quit datarig?") && screen.contains("A query is still running"), "{screen}");
    assert!(screen.contains("y quit · n/Enter stay"), "{screen}");
    // Enter stays (one key must not lose a running query), as n does.
    h.key(KeyCode::Enter);
    assert!(
        h.overlay_kind().is_none() && !h.app.quit && h.app.tab().exec.running.is_some(),
        "Enter stays, the query runs on"
    );
    h.ctrl('q');
    h.keys("n");
    assert!(
        h.overlay_kind().is_none() && !h.app.quit && h.app.tab().exec.running.is_some(),
        "n stays, the query runs on"
    );
    h.command("q");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), ":q asks too");
    h.key(KeyCode::Esc);
    h.key(KeyCode::BackTab);
    h.keys("q");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "explorer q asks too");
    // Other keys do nothing while it asks; the answer is y, or n/Enter/Esc.
    h.keys("jx");
    h.app.handle_event(Event::Paste("pasted".into()));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(!h.app.tab().editor.text().contains("pasted"));
    // Yes: cancel the query, wait for it to stop, then close the session and quit.
    h.keys("y");
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "running query cancelled");
    assert!(!h.app.quit, "waits for the cancel");
    h.db(DbEvent::Failed { id: 1, error: "canceling statement due to user request".into(), cancelled: true });
    assert!(h.app.quit);
    assert!(h.app.tab().exec.session.is_none(), "the connection is closed (the server rolls back)");
}

#[test]
fn quitting_with_an_open_transaction_asks_and_rolls_back() {
    let mut h = Harness::connected(Lang::Ko);
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    h.ctrl('q');
    let screen = h.screen(80, 24);
    let words: Vec<&str> = ko(datarig_core::i18n::Label::QuitTx).split_whitespace().collect();
    assert!(words.iter().take(2).all(|w| screen.contains(w)), "{screen}");
    h.key(KeyCode::Enter);
    assert!(!h.app.quit && h.app.tab().exec.tx_open, "Enter keeps the transaction");
    h.ctrl('q');
    h.keys("y");
    assert!(h.app.quit && h.app.tab().exec.session.is_none() && !h.app.tab().exec.tx_open);
    assert!(!h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "nothing was running");
    // Both at once: one message names both.
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    h.command("quit");
    assert!(h.screen(160, 45).contains("A query is running and a transaction is open"));
    // A query that does not stop: the app quits after the grace time anyway.
    h.keys("y");
    assert!(!h.app.quit);
    h.app.on_tick(std::time::Instant::now() + datarig_tui::app::QUIT_GRACE);
    assert!(h.app.quit && h.app.tab().exec.session.is_none());
}

#[test]
fn quitting_with_nothing_pending_is_immediate() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.db(DbEvent::Done { id: 1, outcome: datarig_core::driver::Outcome::Affected(0), elapsed: Duration::ZERO });
    h.ctrl('q');
    assert!(h.app.quit && h.overlay_kind().is_none());
}

#[test]
fn modal_dialogs_pass_only_root_keys() {
    // Palette, profile manager and password prompt block the workspace keys, as before.
    let mut h = Harness::connected(Lang::En);
    h.ctrl('k');
    h.ctrl('e');
    h.ctrl('o');
    assert_eq!(executes(&mut h), 0);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands), "no quick connect from the command line");
    h.ctrl('k');
    assert!(h.overlay_kind().is_none(), "Ctrl+K closes the palette again");
    h.command("new connection profile");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm));
    h.ctrl('e');
    assert_eq!(executes(&mut h), 0, "no run under the profile form");
    h.ctrl('k');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands), "the palette opens over it");
    h.key(KeyCode::Esc);
    h.ctrl('q');
    assert!(h.app.quit);
}

#[test]
fn workspace_keys_work_while_the_terminal_is_too_small() {
    let mut h = Harness::connected(Lang::En);
    h.draw(20, 5);
    assert!(h.app.layout.too_small);
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Editor, "no pane to move to");
    h.ctrl('t');
    assert_eq!(h.app.tabs.len(), 2, "Ctrl+T still opens a tab");
}

// ── key guide ──────────────────────────────────────────────────────

use datarig_tui::app::guide::{HelpRow, WHICH_KEY_DELAY};
use std::time::Instant;

fn which_prefix(h: &Harness) -> Option<String> {
    h.app.overlays.which_key().map(|w| datarig_tui::keymap::keys::label(&w.prefix))
}

#[test]
fn which_key_appears_after_the_delay_and_steps_back() {
    let mut h = Harness::connected(Lang::En);
    h.keys(" ");
    h.app.on_tick(Instant::now() + Duration::from_millis(100));
    assert!(h.overlay_kind().is_none(), "no popup within the delay");
    h.app.on_tick(Instant::now() + WHICH_KEY_DELAY + Duration::from_millis(50));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::WhichKey));
    assert_eq!(which_prefix(&h).as_deref(), Some("Space"));
    // Once shown it stays, however long it takes.
    h.app.on_tick(Instant::now() + Duration::from_secs(60));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::WhichKey));
    h.keys("t");
    assert_eq!(which_prefix(&h).as_deref(), Some("Space t"));
    h.key(KeyCode::Backspace);
    assert_eq!(which_prefix(&h).as_deref(), Some("Space"));
    h.keys("tn");
    assert!(h.overlay_kind().is_none(), "an action closes the popup");
    assert_eq!(h.app.tabs.len(), 2, "Space t n: a new console");
    // Backspace at the top and Esc close it; nothing else happens.
    for close in [KeyCode::Backspace, KeyCode::Esc] {
        h.keys(" ");
        h.settle();
        assert_eq!(h.overlay_kind(), Some(OverlayKind::WhichKey));
        h.key(close);
        assert!(h.overlay_kind().is_none());
        assert!(h.app.key_state.pending().is_empty());
    }
    // Typed within the delay: no popup at all.
    h.keys(" tn");
    h.settle();
    assert!(h.overlay_kind().is_none());
    assert_eq!(h.app.tabs.len(), 3);
    // Hangul keys work in the popup too (the jamo U+314A sits on the `c` key).
    h.keys(" ");
    h.settle();
    h.type_text("\u{314A}");
    assert_eq!(which_prefix(&h).as_deref(), Some("Space c"));
    h.key(KeyCode::Esc);
}

#[test]
fn unknown_leader_keys_are_reported() {
    let mut h = Harness::connected(Lang::En);
    h.keys(" x");
    assert!(h.status(160, 45).contains("Space x: no binding"), "{}", h.status(160, 45));
    // From the popup: it closes with the same notice.
    h.keys(" ");
    h.settle();
    h.keys("cz");
    assert!(h.overlay_kind().is_none());
    assert!(h.status(160, 45).contains("Space c z: no binding"));
    // Esc just drops a sequence.
    let mut h = Harness::connected(Lang::En);
    h.keys(" ");
    h.key(KeyCode::Esc);
    assert!(!h.status(160, 45).contains("no binding"));
    assert!(h.app.key_state.pending().is_empty());
}

#[test]
fn which_key_items_that_cannot_run_do_nothing() {
    // `explorer.top` only runs in the explorer; from the editor it is listed dimmed.
    let cfg = keymap_config("[keymap.nav]\n\"space c m\" = \"explorer.top\"\n");
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.key(KeyCode::BackTab);
    h.keys("G");
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::VimNormal);
    h.keys(" c");
    h.settle();
    let (_, items) = h.app.which_key_items().unwrap();
    let m = items.iter().find(|i| i.key == "m").unwrap();
    assert!(!m.enabled && m.label == "Explorer: first item", "{m:?}");
    h.keys("m");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::WhichKey), "still open");
    assert_eq!(h.selected(), h.rows().len() - 1, "nothing ran");
    h.key(KeyCode::Esc);
    // From the explorer the same key runs.
    h.key(KeyCode::BackTab);
    h.keys(" cm");
    assert_eq!(h.selected(), 0);
}

#[test]
fn space_types_a_space_while_typing() {
    let mut h = Harness::connected(Lang::En);
    h.keys("i ");
    h.settle();
    assert!(h.overlay_kind().is_none());
    assert!(h.app.tab().editor.lines[1].starts_with(" SELECT"), "{}", h.app.tab().editor.lines[1]);
    h.key(KeyCode::Esc);
    // The help filter takes Space as text as well.
    h.key(KeyCode::BackTab);
    h.keys(" ?/");
    assert_eq!(h.app.key_context(), Ctx::HelpFilter);
    h.keys("last item");
    assert_eq!(h.app.overlays.help().unwrap().filter.text(), "last item");
}

fn help_entries(h: &Harness) -> Vec<String> {
    h.app
        .help_rows()
        .into_iter()
        .filter_map(|r| if let HelpRow::Entry(e) = r { Some(format!("{} = {}", e.keys, e.label)) } else { None })
        .collect()
}

/// Section headers of the keyboard help: (context, open).
fn help_sections(h: &Harness) -> Vec<(Ctx, bool)> {
    h.app
        .help_rows()
        .into_iter()
        .filter_map(|r| if let HelpRow::Section { ctx, open, .. } = r { Some((ctx, open)) } else { None })
        .collect()
}

fn help_selected(h: &Harness) -> HelpRow {
    let i = h.app.overlays.help().unwrap().selected;
    h.app.help_rows().swap_remove(i)
}

#[test]
fn keyboard_help_lists_filters_and_runs_actions() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.key(KeyCode::BackTab);
    h.keys(" ?");
    assert_eq!(h.app.key_context(), Ctx::Help);
    // One list: the context and its ancestors first and open, every other context closed.
    let sections = help_sections(&h);
    assert_eq!(
        sections[..4],
        [(Ctx::Explorer, true), (Ctx::Nav, true), (Ctx::Workspace, true), (Ctx::Root, true)],
        "the context and its ancestors"
    );
    let rest: Vec<Ctx> = sections[4..].iter().map(|s| s.0).collect();
    assert!(sections[4..].iter().all(|s| !s.1), "{sections:?}");
    assert!(rest.contains(&Ctx::Grid) && rest.contains(&Ctx::VimInsert) && rest.contains(&Ctx::Welcome));
    assert!(rest.contains(&Ctx::VimNormal), "the editor's own section: where typing starts and stops");
    let entries = help_entries(&h);
    assert!(entries.contains(&"j / Down = Explorer: next item".to_string()), "{entries:?}");
    // Without the kitty keyboard protocol the terminal cannot send Ctrl+Enter: the help shows
    // the key that works.
    assert!(entries.contains(&"Ctrl+E = Run statement under cursor".to_string()), "{entries:?}");
    h.app.set_keyboard_enhanced(true);
    let entries = help_entries(&h);
    assert!(entries.contains(&"Ctrl+Enter / Ctrl+E = Run statement under cursor".to_string()), "{entries:?}");
    h.app.set_keyboard_enhanced(false);
    assert!(!entries.iter().any(|e| e.contains("Results:")), "closed sections list nothing");
    assert!(matches!(help_selected(&h), HelpRow::Entry(ref e) if e.label == "Explorer: next item"));
    // The search covers every context and opens each section with a match.
    h.keys("/next row");
    assert_eq!(help_entries(&h), ["j / Down = Results: next row"]);
    assert_eq!(help_sections(&h), [(Ctx::Grid, true)]);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Help, "Enter leaves the filter");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help), "grid.down cannot run in the explorer");
    // Esc in the filter clears it; then Enter runs the selected action and closes the help.
    h.keys("/x");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.overlays.help().unwrap().filter.text(), "");
    h.keys("/last item");
    h.key(KeyCode::Enter);
    assert_eq!(help_entries(&h), ["G = Explorer: last item"]);
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert_eq!(h.selected(), h.rows().len() - 1);
    // Esc and q close.
    h.key(KeyCode::F(1));
    h.keys("q");
    assert!(h.overlay_kind().is_none());
    // vim Normal: Space ? opens the same list; from the palette every section is open.
    h.key(KeyCode::Tab);
    h.keys(" ?");
    let entries = help_entries(&h);
    assert!(entries.contains(&"Space ? = Keyboard help".to_string()), "{entries:?}");
    // vim Normal binds no action of its own (its keys are vim's): its section says where
    // typing starts and stops, then its ancestors follow.
    assert_eq!(help_sections(&h)[..2], [(Ctx::VimNormal, true), (Ctx::Nav, true)]);
    h.key(KeyCode::Esc);
    h.command("keyboard help expand all");
    assert!(h.app.overlays.help().is_some_and(|x| x.origin == Ctx::VimNormal));
    assert!(help_sections(&h).iter().all(|s| s.1), "{:?}", help_sections(&h));
}

#[test]
fn keyboard_help_sections_open_and_close_by_key_and_mouse() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab);
    h.key(KeyCode::F(1));
    // `h` on an entry closes its section and selects the header; `l` opens it again.
    h.keys("h");
    assert_eq!(help_sections(&h)[0], (Ctx::Explorer, false));
    assert!(matches!(help_selected(&h), HelpRow::Section { ctx: Ctx::Explorer, .. }));
    h.keys("l");
    assert_eq!(help_sections(&h)[0], (Ctx::Explorer, true));
    // Enter on a header toggles it.
    h.key(KeyCode::Enter);
    assert_eq!(help_sections(&h)[0], (Ctx::Explorer, false));
    // Walk down to the closed Results section and open it.
    while !matches!(help_selected(&h), HelpRow::Section { ctx: Ctx::Grid, .. }) {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Right);
    assert!(help_entries(&h).contains(&"j / Down = Results: next row".to_string()));
    // A click on a header opens or closes that section; a click on an entry only selects it.
    h.key(KeyCode::Esc);
    h.key(KeyCode::F(1));
    h.draw(80, 24);
    let click = |h: &mut Harness, row: u16| {
        let list = h.app.overlays.help().unwrap().list;
        h.app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x + 2,
            row: list.y + row,
            modifiers: KeyModifiers::NONE,
        }));
    };
    let scroll = h.app.overlays.help().unwrap().scroll;
    assert_eq!(scroll, 0);
    click(&mut h, 0);
    assert_eq!(help_sections(&h)[0], (Ctx::Explorer, false));
    click(&mut h, 0);
    assert_eq!(help_sections(&h)[0], (Ctx::Explorer, true));
    click(&mut h, 2);
    assert!(matches!(help_selected(&h), HelpRow::Entry(ref e) if e.label == "Explorer: previous item"));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help), "a click does not run the action");
    assert_eq!(h.selected(), 1, "the cursor stays on the profile");
}

#[test]
fn hint_line_follows_the_context_and_the_run_key() {
    let mut h = Harness::connected(Lang::En);
    let status = |h: &mut Harness| h.status(160, 45);
    assert!(status(&mut h).contains("i type · Ctrl+E run · Tab next pane · : commands · F1 help · Space more"));
    h.app.set_keyboard_enhanced(true);
    assert!(status(&mut h).contains("i type · Ctrl+Enter run ·"));
    h.keys("i");
    let insert = "Esc stop typing · Ctrl+Enter run · Ctrl+N complete · Ctrl+K commands · F1 help";
    assert!(status(&mut h).contains(insert), "{}", status(&mut h));
    h.key(KeyCode::Esc);
    h.key(KeyCode::BackTab);
    assert!(status(&mut h).contains("Enter open · Tab next pane · : commands · F1 help · q quit · Space more"));
    // Remapped keys show up in the hint line.
    let cfg = keymap_config("[keymap.explorer]\n\"o\" = \"explorer.activate\"\n\"enter\" = \"none\"\n");
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.key(KeyCode::BackTab);
    assert!(h.status(160, 45).contains("o open ·"), "{}", h.status(160, 45));
    // A running query puts Ctrl+C first.
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    assert!(status(&mut h).contains("Ctrl+C cancel · i type · Ctrl+E run"), "{}", status(&mut h));
}

/// The cursor's shape follows the mode: a block in Normal and Visual and in
/// the panes, a bar in Insert and in every text input; `editor.cursor_shape = off` leaves the
/// terminal's own shape.
#[test]
fn cursor_shape_follows_the_mode() {
    use datarig_core::config::CursorShape as Setting;
    use datarig_tui::terminal::{CursorShape, cursor_shape};
    let mut h = Harness::connected(Lang::En);
    let shape = |h: &Harness| cursor_shape(&h.app);
    assert_eq!(shape(&h), Some(CursorShape::Block), "Normal");
    h.keys("i");
    assert_eq!(shape(&h), Some(CursorShape::Bar), "Insert");
    h.key(KeyCode::Esc);
    h.keys("v");
    assert_eq!(shape(&h), Some(CursorShape::Block), "Visual");
    h.key(KeyCode::Esc);
    h.keys(":");
    assert_eq!(shape(&h), Some(CursorShape::Bar), "the command line");
    h.key(KeyCode::Esc);
    h.key(KeyCode::BackTab);
    assert_eq!(shape(&h), Some(CursorShape::Block), "the explorer");
    h.keys("/");
    assert_eq!(shape(&h), Some(CursorShape::Bar), "the explorer's filter");
    h.key(KeyCode::Esc);
    h.command("set editor.cursor_shape=off");
    assert_eq!(h.app.prefs.cursor_shape, Setting::Off);
    assert_eq!(shape(&h), None, "off: the terminal's own shape");
    h.command("set editor.cursor_shape=on");
    assert_eq!(shape(&h), Some(CursorShape::Block));
}

/// Connecting reads the keychain off the UI thread. Over SSH the app froze on
/// connect: macOS held the keychain call while it asked on a screen the remote user never saw,
/// and the UI thread waited for it. A slow keychain here: the key returns at once, and the
/// answer connects with the stored password when it comes.
#[tokio::test(flavor = "multi_thread")]
async fn connecting_never_waits_for_the_keychain_on_the_ui_thread() {
    struct Slow(MemoryStore);
    impl SecretStore for Slow {
        fn get(&self, account: &str) -> Result<Option<String>, datarig_core::secret::Unavailable> {
            std::thread::sleep(Duration::from_millis(1500));
            self.0.get(account)
        }
        fn set(&self, account: &str, secret: &str) -> Result<(), datarig_core::secret::Unavailable> {
            self.0.set(account, secret)
        }
        fn delete(&self, account: &str) -> Result<bool, datarig_core::secret::Unavailable> {
            self.0.delete(account)
        }
    }
    let cfg = sample_config(None);
    let store = Arc::new(Slow(MemoryStore::new()));
    store.0.set(&cfg.connections[0].id.account(), "stored-pw").unwrap();
    let (h, mut rx) = Harness::started(&cfg, Lang::En, store as Arc<dyn SecretStore>, Startup::Normal);
    let mut h = h.with_fake_driver();
    let t = Instant::now();
    h.key(KeyCode::Enter);
    assert!(t.elapsed() < Duration::from_millis(500), "the UI thread waited {:?}", t.elapsed());
    let passwords = h.driver.passwords.clone();
    let until = Instant::now() + Duration::from_secs(5);
    while passwords.lock().unwrap().is_empty() {
        let left = until.saturating_duration_since(Instant::now());
        let ev = tokio::time::timeout(left, rx.recv()).await.expect("the keychain answered").expect("open");
        h.app.on_app_event(ev);
    }
    assert_eq!(*passwords.lock().unwrap(), ["stored-pw"]);
}

/// A terminal that did not grant the kitty keyboard protocol (iTerm2 over SSH may
/// not) sends plain Enter for Ctrl+Enter. Ctrl+E runs the statement there, and the hints, the
/// keyboard help and the command line's list show the key that works.
#[test]
fn without_the_kitty_protocol_the_run_key_shown_is_ctrl_e() {
    let mut h = Harness::connected(Lang::En);
    h.app.set_keyboard_enhanced(false);
    h.sent();
    h.ctrl('e');
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::Execute { .. })), "Ctrl+E runs");
    assert_eq!(h.app.run_key(), "Ctrl+E");
    let status = h.status(160, 45);
    assert!(status.contains("Ctrl+E run") && !status.contains("Ctrl+Enter"), "{status}");
    h.ctrl('k');
    h.type_text("run");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Ctrl+E") && !screen.contains("Ctrl+Enter"), "{screen}");
    h.key(KeyCode::Esc);
    h.key(KeyCode::F(1));
    let entries = help_entries(&h);
    assert!(entries.contains(&"Ctrl+E = Run statement under cursor".to_string()), "{entries:?}");
    assert!(!entries.iter().any(|e| e.contains("Ctrl+Enter")), "{entries:?}");
}
