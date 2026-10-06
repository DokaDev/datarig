//! What the editor says about runs: the statement that runs now is drawn apart (its own tint,
//! a spinner in the gutter) wherever the cursor goes and whatever is edited around it, and once
//! a run ended each statement it ran gets a dim hint after its last line saying what it did.
//! The hints are not text, go when their statement is edited, never attach to a statement
//! edited while it ran or to an answer of another run, and the `[editor] run_hints` setting
//! turns them off.

mod common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Outcome};
use datarig_core::i18n::Lang;
use datarig_tui::theme::DARK;
use datarig_tui::widgets::SPINNER;
use datarig_tui::widgets::editor::Editor;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::time::Duration;

const MS: Duration = Duration::from_millis(42);

/// A connected harness whose editor holds `text` (the cursor on its first line); the time of
/// day is 14:03.
fn harness(text: &str) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new(text);
    h.sent();
    h
}

/// Run what `Ctrl+E` takes now; the run's id and statements.
fn run(h: &mut Harness) -> (u64, Vec<String>) {
    h.ctrl('e');
    let sent = h.sent();
    let [DbCommand::Execute { id, statements, .. }] = &sent[..] else { panic!("{sent:?}") };
    (*id, statements.clone())
}

fn rows(id: u64, n: usize, more: bool) -> DbEvent {
    let cols = vec![meta("a", "int4", true, false)];
    DbEvent::Page { id, columns: Some(cols), rows: vec![vec![Some("1".into())]; n], more, elapsed: MS }
}

fn done(id: u64, outcome: Outcome) -> DbEvent {
    DbEvent::Done { id, outcome, elapsed: MS }
}

/// Where `text` starts on the screen (its first cell).
fn find(buf: &Buffer, text: &str) -> Option<(u16, u16)> {
    let want: Vec<String> = text.chars().map(String::from).collect();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(want.len() as u16 - 1) {
            if want.iter().enumerate().all(|(i, c)| buf[(x + i as u16, y)].symbol() == c) {
                return Some((x, y));
            }
        }
    }
    None
}

/// The editor line that shows `text`: its row and the column its text starts at (the gutter's
/// mark is the column before).
fn line(h: &mut Harness, text: &str) -> (Buffer, u16, u16) {
    let t = h.draw(160, 45);
    let buf = t.backend().buffer().clone();
    let (x, y) = find(&buf, text).unwrap_or_else(|| panic!("{text:?} not on screen:\n{}", buffer_text(&buf)));
    (buf, x, y)
}

/// Whether the line showing `text` is marked as the running statement: its tint behind the
/// text (unless the cursor line's) and the spinner (or the bar) in the gutter in its color.
fn running(h: &mut Harness, text: &str) -> bool {
    let (buf, x, y) = line(h, text);
    let gutter = &buf[(x - 1, y)];
    let marked = gutter.fg == DARK.running_stmt_bar && (SPINNER.contains(&gutter.symbol()) || gutter.symbol() == "▎");
    let bg = buf[(x, y)].bg;
    marked && (bg == DARK.running_stmt.bg.unwrap() || bg == DARK.cursor_line.bg.unwrap())
}

/// The text after `stmt` on its line, trimmed (its hint, if any).
fn after(h: &mut Harness, stmt: &str) -> String {
    let (buf, x, y) = line(h, stmt);
    let row: String = (x..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
    let rest = &row[stmt.len()..];
    rest.split('│').next().unwrap_or("").trim().to_string()
}

#[test]
fn the_running_statement_is_drawn_apart_and_follows_its_text() {
    let mut h = harness("SELECT 1;\nSELECT 2;\nSELECT 3;");
    h.key(KeyCode::Char('j'));
    let (id, stmts) = run(&mut h);
    assert_eq!(stmts, ["SELECT 2"]);
    assert!(running(&mut h, "SELECT 2;"), "the statement under the cursor runs");
    let (buf, x, y) = line(&mut h, "SELECT 2;");
    assert!(SPINNER.contains(&buf[(x - 1, y)].symbol()), "a spinner on its first line");
    assert!(!running(&mut h, "SELECT 1;") && !running(&mut h, "SELECT 3;"));

    // The cursor goes elsewhere: the running statement keeps its tint; the run target's bar
    // follows the cursor.
    h.keys("gg");
    assert!(running(&mut h, "SELECT 2;"));
    let (buf, x, y) = line(&mut h, "SELECT 2;");
    assert_eq!(buf[(x, y)].bg, DARK.running_stmt.bg.unwrap(), "its own tint, off the cursor line");
    let (buf, x, y) = line(&mut h, "SELECT 1;");
    assert_eq!(buf[(x - 1, y)].fg, DARK.current_stmt_bar, "the run target is marked as before");

    // Lines added above move it.
    h.keys("Oselect 0;");
    h.key(KeyCode::Esc);
    assert!(running(&mut h, "SELECT 2;"), "it moved down with its text");
    assert!(!running(&mut h, "SELECT 1;"));

    // The run ends: its tint and spinner go, the run target is marked as before.
    h.db(rows(id, 1, false));
    assert!(!running(&mut h, "SELECT 2;"));
    let (buf, x, y) = line(&mut h, "SELECT 2;");
    assert_eq!(buf[(x, y)].bg, DARK.bg, "back to the plain background");
    assert_eq!(buf[(x - 1, y)].symbol(), " ");
    h.key(KeyCode::Char('j'));
    let (buf, x, y) = line(&mut h, "SELECT 1;");
    assert_eq!(buf[(x - 1, y)].fg, DARK.current_stmt_bar);
}

#[test]
fn a_run_of_several_statements_marks_the_one_executing() {
    let mut h = harness("SELECT 1;\nSELECT 2;\nSELECT 3;");
    h.keys("ggVG");
    let (id, stmts) = run(&mut h);
    assert_eq!(stmts, ["SELECT 1", "SELECT 2", "SELECT 3"]);
    // Before the driver says which: the first.
    assert!(running(&mut h, "SELECT 1;") && !running(&mut h, "SELECT 2;"));
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT 1".into()), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    assert!(running(&mut h, "SELECT 2;"), "the one executing");
    assert!(!running(&mut h, "SELECT 1;") && !running(&mut h, "SELECT 3;"));
    h.db(DbEvent::Finished { id, index: 1, outcome: Outcome::Affected(3), elapsed: MS });
    h.db(DbEvent::Started { id, index: 2 });
    assert!(running(&mut h, "SELECT 3;") && !running(&mut h, "SELECT 2;"));
    h.db(rows(id, 2, false));
    assert!(!running(&mut h, "SELECT 3;"));
    // Each statement says what it did.
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} SELECT 1 · 42ms · 14:03");
    assert_eq!(after(&mut h, "SELECT 2;"), "\u{2713} 3 rows affected · 42ms · 14:03");
    assert_eq!(after(&mut h, "SELECT 3;"), "\u{2713} 2 rows · 42ms · 14:03");
}

#[test]
fn hints_say_what_a_run_did() {
    let mut h = harness("SELECT 1;");
    let (id, _) = run(&mut h);
    h.db(rows(id, 1, false));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 1 row · 42ms · 14:03");
    let (id, _) = run(&mut h);
    h.db(rows(id, 1200, true));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 1,200+ rows · 42ms · 14:03", "the latest run only");
    let (id, _) = run(&mut h);
    h.db(done(id, Outcome::Affected(3)));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 3 rows affected · 42ms · 14:03");
    let (id, _) = run(&mut h);
    h.db(done(id, Outcome::Command("CREATE TABLE".into())));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} CREATE TABLE · 42ms · 14:03");
    let (id, _) = run(&mut h);
    h.db(DbEvent::Failed { id, error: DbError::from("ERROR: division by zero\nDETAIL: more"), cancelled: false });
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2717} ERROR: division by zero", "the error's first line");
    let (id, _) = run(&mut h);
    h.ctrl('c');
    h.db(DbEvent::Failed { id, error: DbError::from("ERROR: canceling statement"), cancelled: true });
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2298} cancelled");
    // Marks in their colors, the text in the hint's style.
    let (buf, x, y) = line(&mut h, "SELECT 1;");
    let mark = &buf[(x + 11, y)];
    assert_eq!((mark.symbol(), mark.fg), ("\u{2298}", DARK.warning));
    let text = &buf[(x + 13, y)];
    assert_eq!(text.fg, DARK.run_hint.fg.unwrap());
    assert!(text.modifier.contains(ratatui::style::Modifier::ITALIC));

    // Nerd Font glyphs with icons on.
    h.app.icons = IconsSetting::On;
    let (id, _) = run(&mut h);
    h.db(rows(id, 1, false));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{f00c} 1 row · 42ms · 14:03");
}

/// A `ROLLBACK` that ended the user's transaction, and an `EXPLAIN ANALYZE` of a change (run
/// in a transaction the driver rolls back), say they rolled back.
#[test]
fn rolled_back_runs_say_so() {
    let mut h = harness("BEGIN;\nUPDATE t SET a = 1 WHERE a = 2;\nROLLBACK;");
    // As the driver reports it: the statement's answer, then the user's block.
    let (id, _) = run(&mut h);
    h.db(done(id, Outcome::Command("BEGIN".into())));
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    h.keys("G");
    let (id, _) = run(&mut h);
    h.db(done(id, Outcome::Command("ROLLBACK".into())));
    assert_eq!(after(&mut h, "ROLLBACK;"), "\u{2713} ROLLBACK · 42ms · 14:03", "not known yet");
    h.db(DbEvent::Block(false));
    h.db(DbEvent::TxOpen(false));
    assert_eq!(after(&mut h, "ROLLBACK;"), "\u{21ba} rolled back · 42ms · 14:03");
    assert_eq!(after(&mut h, "BEGIN;"), "\u{2713} BEGIN · 42ms · 14:03");
    let (buf, x, y) = line(&mut h, "ROLLBACK;");
    assert_eq!(buf[(x + 11, y)].fg, DARK.warning);

    let mut h = harness("EXPLAIN ANALYZE UPDATE t SET a = 1 WHERE a = 2;");
    let (id, stmts) = run(&mut h);
    assert_eq!(stmts, ["EXPLAIN ANALYZE UPDATE t SET a = 1 WHERE a = 2"]);
    h.db(rows(id, 4, false));
    assert_eq!(after(&mut h, "WHERE a = 2;"), "\u{21ba} rolled back · 42ms · 14:03");
}

#[test]
fn editing_a_statement_drops_its_hint_only() {
    let mut h = harness("SELECT 1;\nSELECT 2;");
    h.keys("ggVG");
    let (id, _) = run(&mut h);
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT 1".into()), elapsed: MS });
    h.db(rows(id, 1, false));
    // Lines added above and below, and text typed after a `;`, leave both.
    h.keys("ggOselect 0;");
    h.key(KeyCode::Esc);
    h.keys("Go");
    h.key(KeyCode::Esc);
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} SELECT 1 · 42ms · 14:03");
    assert_eq!(after(&mut h, "SELECT 2;"), "\u{2713} 1 row · 42ms · 14:03");
    // An edit of the first statement drops its hint, not the other's; undo does not bring it
    // back.
    h.keys("2gg0");
    h.keys("x");
    assert_eq!(after(&mut h, "ELECT 1;"), "");
    assert_eq!(after(&mut h, "SELECT 2;"), "\u{2713} 1 row · 42ms · 14:03");
    h.keys("u");
    assert_eq!(after(&mut h, "SELECT 1;"), "");
    assert_eq!(h.app.tab_mut().editor.run_hints().count(), 1);
}

/// Statements edited while their run goes on: one that runs stays marked (it still runs, as
/// sent), and the answer of an edited one never attaches to the edited text. Text typed after
/// a statement's `;` is not its own.
#[test]
fn a_statement_edited_while_it_runs_gets_no_hint() {
    let mut h = harness("SELECT 1;\nSELECT 2;\nSELECT 3;");
    h.keys("ggVG");
    let (id, _) = run(&mut h);
    h.db(DbEvent::Started { id, index: 0 });
    h.keys("ggA -- x");
    h.key(KeyCode::Esc);
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT 1".into()), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    h.keys("2gg0x");
    assert!(running(&mut h, "ELECT 2;"), "the running statement stays marked, edited");
    h.keys("G0x");
    h.db(DbEvent::Finished { id, index: 1, outcome: Outcome::Command("SELECT 2".into()), elapsed: MS });
    h.db(DbEvent::Started { id, index: 2 });
    h.db(rows(id, 1, false));
    assert_eq!(after(&mut h, "SELECT 1; -- x"), "\u{2713} SELECT 1 · 42ms · 14:03", "text after its `;`");
    assert_eq!(after(&mut h, "ELECT 2;"), "", "edited while it ran: no hint");
    assert_eq!(after(&mut h, "ELECT 3;"), "", "edited before it ran: no hint");
    assert_eq!(h.app.tab_mut().editor.run_hints().count(), 1);
}

/// Answers bound to another run, another session generation or another editor never attach.
#[test]
fn stale_answers_never_attach() {
    let mut h = harness("SELECT 1;\nSELECT 2;");
    let (first, _) = run(&mut h);
    h.db(rows(first, 1, false));
    h.key(KeyCode::Char('j'));
    let (second, _) = run(&mut h);
    // The first run's answer again: ignored.
    h.db(done(first, Outcome::Affected(9)));
    assert!(running(&mut h, "SELECT 2;"));
    assert_eq!(after(&mut h, "SELECT 2;"), "");
    // An answer of an older session generation: dropped before the tab sees it.
    let t = h.app.tab();
    let (target, generation) = (datarig_tui::app::EventTarget::Tab(t.id), t.exec.generation);
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target,
        generation: generation + 100,
        ev: rows(second, 7, false),
    });
    assert!(running(&mut h, "SELECT 2;"));
    h.db(rows(second, 2, false));
    assert_eq!(after(&mut h, "SELECT 2;"), "\u{2713} 2 rows · 42ms · 14:03");
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 1 row · 42ms · 14:03", "the other statement keeps its own");

    // The tab's text replaced while it ran (a saved query opened in it): nothing is marked on
    // the new text and the answer attaches nowhere.
    let (third, _) = run(&mut h);
    h.app.tab_mut().editor = Editor::new("SELECT 1;\nSELECT 2;");
    assert!(!running(&mut h, "SELECT 2;"));
    h.db(rows(third, 3, false));
    assert_eq!(h.app.tab_mut().editor.run_hints().count(), 0);
}

/// At 80 columns the editor's text is 50 wide: a hint that fits is whole, one with room for a
/// few characters is cut with `…`, one without is left out; it never covers text, and a line
/// scrolled sideways shows its hint only where the line's end is on screen.
#[test]
fn a_narrow_editor_cuts_the_hint_or_leaves_it_out() {
    let short = "SELECT 1;";
    let mid = "SELECT 'aaaaaaaaaa';";
    let long = "SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';";
    let mut h = harness(&format!("{short}\n{mid}\n{long}"));
    h.keys("ggVG");
    let (id, _) = run(&mut h);
    for i in 0..2 {
        h.db(DbEvent::Started { id, index: i });
        h.db(DbEvent::Finished { id, index: i, outcome: Outcome::Affected(128), elapsed: MS });
    }
    h.db(DbEvent::Started { id, index: 2 });
    h.db(done(id, Outcome::Affected(128)));
    let t = h.draw(80, 24);
    let buf = t.backend().buffer();
    let text = |s: &str| {
        let (x, y) = find(buf, s).unwrap();
        let row: String = (x..80).map(|x| buf[(x, y)].symbol().to_string()).collect();
        row.split('\u{2502}').next().unwrap().to_string()
    };
    assert_eq!(text(short).trim_end(), "SELECT 1;  \u{2713} 128 rows affected · 42ms · 14:03");
    let cut = text(mid);
    assert!(cut.starts_with("SELECT 'aaaaaaaaaa';  \u{2713} 128 rows affected · 42"), "{cut}");
    assert!(cut.ends_with('\u{2026}'), "cut at the editor's edge: {cut:?}");
    assert_eq!(width(&cut), 50, "to the last column, not over the border");
    assert_eq!(text(long).trim_end(), long, "no room: left out");

    // A line longer than the editor, its end off screen: no hint anywhere on it; scrolled to
    // its end, the hint would start past the edge: left out, never drawn over the text.
    let very = format!("SELECT '{}';", "x".repeat(70));
    let mut h = harness(&very);
    let (id, _) = run(&mut h);
    h.db(rows(id, 1, false));
    for keys in ["0", "$"] {
        h.keys(keys);
        let s = buffer_text(h.draw(80, 24).backend().buffer());
        assert!(!s.contains('\u{2713}'), "{keys}: {s}");
    }
    // Shorter again (an edit drops the hint; a new run brings it back after the line's end).
    h.keys("0f'lD");
    h.keys("a';");
    h.key(KeyCode::Esc);
    h.keys("0");
    let (id, _) = run(&mut h);
    h.db(rows(id, 1, false));
    assert_eq!(after(&mut h, "SELECT '';"), "\u{2713} 1 row · 42ms · 14:03");
}

fn width(s: &str) -> usize {
    datarig_tui::text::width(s)
}

#[test]
fn the_setting_turns_hints_off() {
    let mut h = harness("SELECT 1;");
    let (id, _) = run(&mut h);
    h.db(rows(id, 1, false));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 1 row · 42ms · 14:03");
    h.command("set editor.run_hints=off");
    assert_eq!(h.app.prefs.run_hints, datarig_core::config::RunHints::Off);
    assert_eq!(after(&mut h, "SELECT 1;"), "");
    h.command("set editor.run_hints=on");
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 1 row · 42ms · 14:03");
}

/// A hint is not text: a yank of the line, the text the tab saves, a search and the statement
/// a run takes never have it.
#[test]
fn hints_are_not_text() {
    let mut h = harness("SELECT 1;");
    let (id, _) = run(&mut h);
    h.db(rows(id, 1, false));
    h.keys("yy");
    assert_eq!(h.app.tab().editor.register('"').map(|r| r.text.clone()), Some("SELECT 1;".to_string()));
    assert_eq!(h.app.tab().editor.text(), "SELECT 1;");
    h.keys("/row");
    h.key(KeyCode::Enter);
    assert!(h.status(100, 30).contains("not found") || h.status(100, 30).contains("Pattern"), "{}", h.status(100, 30));
    let (_, stmts) = run(&mut h);
    assert_eq!(stmts, ["SELECT 1"]);

    // `:w` writes the text alone.
    let dir = std::env::temp_dir().join(format!("datarig-run-hints-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut app, clock) = new_app_with_clock(&test_db_config(), Lang::En);
    app.set_paths(datarig_core::paths::Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) });
    let driver = FakeDriver::default();
    let fake = driver.clone();
    app.set_drivers(Arc::new(move |name: &str| {
        (name == "postgres").then(|| Arc::new(fake.clone()) as Arc<dyn datarig_core::driver::Driver>)
    }));
    app.launch(datarig_tui::app::Startup::Normal);
    let store = Arc::new(datarig_core::secret::MemoryStore::new());
    let mut h = Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock };
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.meta_db("local-pg", DbEvent::Connected);
    h.key(KeyCode::Tab);
    h.keys("i");
    h.type_text("SELECT 1;");
    h.key(KeyCode::Esc);
    h.sent();
    let (id, _) = run(&mut h);
    h.tab_db(h.app.tabs.active_index(), rows(id, 1, false));
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2713} 1 row · 42ms · 14:03");
    h.command("w hinted");
    let file = datarig_core::scripts::ScriptStore::open(&dir.join("data")).file("hinted.sql");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "SELECT 1;");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The editor of a run in progress and of finished ones, in English: the text, and the styles
/// of the running statement and of the hints asserted cell by cell.
#[test]
fn snapshot_of_a_run_and_its_hints() {
    let mut h = harness("SELECT 1;\nSELECT 2 FROM t\nWHERE a > 1;\nSELECT 3;");
    h.keys("ggVG");
    let (id, _) = run(&mut h);
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Affected(1200), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    let t = h.draw(80, 24);
    let buf = t.backend().buffer();
    // The editor's lines (the status bar counts the time it runs).
    let editor: Vec<String> = (0..10).map(|y| row_text(buf, y)).collect();
    insta::assert_snapshot!("run_feedback_running_en_80x24", editor.join("\n"));
    let (x, y) = find(buf, "SELECT 2 FROM t").unwrap();
    for line in 0..2 {
        assert_eq!(buf[(x, y + line)].bg, DARK.running_stmt.bg.unwrap(), "both lines of the running statement");
        assert_eq!(buf[(x - 1, y + line)].fg, DARK.running_stmt_bar);
    }
    assert!(SPINNER.contains(&buf[(x - 1, y)].symbol()) && buf[(x - 1, y + 1)].symbol() == "▎");
    assert!(find(buf, "\u{2713}").is_none(), "hints come once the run ended");

    h.db(DbEvent::Failed { id, error: DbError::from("ERROR: relation \"t\" does not exist"), cancelled: false });
    let t = h.draw(80, 24);
    assert_screen!("run_feedback_hints_en_80x24", Lang::En, &t);
    let buf = t.backend().buffer();
    let (hx, hy) = find(buf, "\u{2713} 1,200 rows affected").unwrap();
    assert_eq!(buf[(hx, hy)].fg, DARK.success);
    assert_eq!(buf[(hx + 2, hy)].fg, DARK.run_hint.fg.unwrap());
    assert!(buf[(hx + 2, hy)].modifier.contains(ratatui::style::Modifier::ITALIC));
    let (hx, hy) = find(buf, "\u{2717} ERROR: relation").unwrap();
    assert_eq!(buf[(hx, hy)].fg, DARK.error);
    assert_eq!(find(buf, "WHERE a > 1;").unwrap().1, hy, "after the statement's last line");
    assert_eq!(after(&mut h, "SELECT 3;"), "", "the statement that did not run has none");
}

/// A rolled-back hint is amended once, when the driver says the block ended: later events of
/// the session never stamp it with a new time.
#[test]
fn a_rolled_back_hint_keeps_its_time() {
    let mut h = harness("BEGIN;\nROLLBACK;");
    let (id, _) = run(&mut h);
    h.db(done(id, Outcome::Command("BEGIN".into())));
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    h.keys("G");
    let (id, _) = run(&mut h);
    h.db(done(id, Outcome::Command("ROLLBACK".into())));
    h.db(DbEvent::Block(false));
    h.db(DbEvent::TxOpen(false));
    assert_eq!(after(&mut h, "ROLLBACK;"), "\u{21ba} rolled back · 42ms · 14:03");
    h.app.set_time_of_day(Arc::new(|| (15, 27)));
    h.db(DbEvent::TxAborted(false));
    h.db(DbEvent::TxOpen(false));
    assert_eq!(after(&mut h, "ROLLBACK;"), "\u{21ba} rolled back · 42ms · 14:03");
}

/// A run stopped by a change of the tab's connection or context, or by a disconnect, says it
/// was cancelled at once (no event of the session comes for it).
#[test]
fn a_run_stopped_by_a_rebind_or_a_disconnect_says_cancelled_at_once() {
    let mut h = harness("SELECT 1;");
    run(&mut h);
    h.command("use .shop");
    h.keys("y");
    assert!(h.app.tab().exec.running.is_none());
    assert_eq!(h.app.tab().editor.active_run(), None, "the mark ends with the run");
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2298} cancelled");

    let mut h = harness("SELECT 1;");
    run(&mut h);
    h.keys(" cx");
    h.keys("y");
    assert!(h.app.tab().exec.running.is_none());
    assert_eq!(h.app.tab().editor.active_run(), None);
    assert_eq!(after(&mut h, "SELECT 1;"), "\u{2298} cancelled");
    ended_by_closed_session(&h);
}

/// The tab's run ended cancelled because its session was closed under it: once, said in its
/// Messages with the reason, and nothing of that session (paging, transaction) is left.
fn ended_by_closed_session(h: &Harness) {
    let t = h.app.tab();
    assert!(t.exec.running.is_none());
    let outcomes: Vec<_> = t.exec.run.statements.iter().map(|s| s.outcome.clone()).collect();
    assert_eq!(outcomes, [datarig_tui::app::runlog::StatementOutcome::Cancelled]);
    assert!(matches!(t.results, datarig_tui::app::Results::Cancelled));
    assert_eq!(t.exec.view, datarig_tui::app::tabs::ResultView::Messages);
    let reason = datarig_core::i18n::Msg::Label(datarig_core::i18n::Label::QueryCancelledSessionClosed);
    assert_eq!(t.exec.run.notes.iter().filter(|n| n.msg == reason).count(), 1, "{:?}", t.exec.run.notes);
    assert!(!t.exec.tx_open && t.exec.paging == datarig_tui::app::Paging::None);
}

/// Another tab connects the profile again while this tab's statement runs (its connection was
/// lost meanwhile): that tab's session is closed under the run, which ends there, cancelled,
/// with exactly one terminal outcome; a late answer of the closed session changes nothing.
/// The same when that attempt then fails.
#[test]
fn a_reconnect_ends_a_running_tabs_run_once() {
    for fails in [false, true] {
        let mut h = harness("SELECT 1;");
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        let (id, _) = run(&mut h);
        let (first, old) = (h.app.tab().id, h.app.tab().exec.generation);
        let pid = h.app.tab().profile.unwrap();
        // The tunnel was lost: the profile is no longer resolved, the tab's session not told yet.
        let c = h.app.conns.entry(pid);
        c.resolved = None;
        c.connected = false;
        h.ctrl('t');
        h.app.tab_mut().editor = Editor::new("SELECT 2;");
        h.ctrl('e');
        assert!(h.app.is_queued(h.app.tab().id), "the other tab waits for the new attempt");
        if fails {
            h.db(DbEvent::ConnectFailed { error: DbError::from("no route to host"), auth: false });
        } else {
            h.db(DbEvent::Connected);
        }
        assert!(h.app.tabs.activate(0) || h.app.tab().id == first);
        assert_eq!(h.app.tab().id, first);
        ended_by_closed_session(&h);
        assert_eq!(after(&mut h, "SELECT 1;"), "\u{2298} cancelled");
        let before = h.app.tab().exec.run.clone();
        let target = datarig_tui::app::EventTarget::Tab(first);
        h.app.on_app_event(datarig_tui::app::AppEvent::Db { target, generation: old, ev: rows(id, 1, false) });
        assert_eq!(h.app.tab().exec.run, before, "a late answer of the closed session");
    }
}

/// What `Ctrl+E` took is kept for its run only: a run refused (busy, read-only, unsupported)
/// or dropped at the confirmation leaves nothing that a later run from elsewhere could take.
#[test]
fn a_refused_or_dropped_run_leaves_nothing_staged() {
    let mut h = harness("SELECT 1;\nDELETE FROM t;");
    let (id, _) = run(&mut h);
    h.ctrl('e');
    assert!(h.status(100, 30).contains("already running"));
    assert!(!h.app.tab().editor.has_staged_run(), "busy: refused");
    h.db(rows(id, 1, false));
    h.keys("j");
    h.ctrl('e');
    assert_eq!(h.overlay_kind(), Some(datarig_tui::app::overlay::OverlayKind::RunConfirm));
    h.key(KeyCode::Esc);
    assert!(h.sent().is_empty());
    assert!(!h.app.tab().editor.has_staged_run(), "dropped at the confirmation");
}

/// The error a hint shows is cleaned as grid cells are: a tab is a mark, other control
/// characters are replacement characters, never dropped silently.
#[test]
fn an_error_hint_is_cleaned_like_a_cell() {
    let mut h = harness("SELECT 1;");
    let (id, _) = run(&mut h);
    let error = DbError::from("ERROR: a\x1b[2Jb\tc\u{9b}d\nDETAIL: more");
    h.db(DbEvent::Failed { id, error, cancelled: false });
    let hint = h.app.tab_mut().editor.run_hints().next().map(|(_, h)| h.text.clone()).unwrap();
    assert_eq!(hint, datarig_tui::text::sanitize_cell("ERROR: a\x1b[2Jb\tc\u{9b}d"));
    assert!(!hint.chars().any(char::is_control), "{hint:?}");
}

/// A disconnected tab: `Ctrl+E` connects again and the statement waits for it.
fn waiting() -> Harness {
    let mut h = harness("SELECT 1;");
    h.keys(" cx");
    assert!(h.app.current_conn().is_none_or(|c| !c.connected));
    h.ctrl('e');
    assert!(h.app.is_queued(h.app.tab().id), "waits for the connection");
    h
}

/// A run that waits for its connection keeps what `Ctrl+E` took: a second `Ctrl+E` refused
/// meanwhile does not replace or drop it, and the run is marked once it is sent.
#[test]
fn a_waiting_run_keeps_its_marks() {
    let mut h = waiting();
    h.ctrl('e');
    assert!(h.app.tab().editor.has_staged_run(), "a refused Ctrl+E keeps the waiting run's statements");
    h.sent();
    h.db(DbEvent::Connected);
    let sent = h.sent();
    let Some(DbCommand::Execute { id, .. }) = sent.iter().find(|c| matches!(c, DbCommand::Execute { .. })) else {
        panic!("{sent:?}")
    };
    assert_eq!(h.app.tab().editor.active_run(), Some(*id), "marked once sent");
}

/// A run that never starts leaves nothing staged: disconnected while it waited, or quick
/// connect closed for a tab without a connection.
#[test]
fn a_run_that_never_starts_leaves_nothing_staged() {
    let mut h = waiting();
    h.keys(" cx");
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert!(!h.app.is_queued(h.app.tab().id));
    assert!(!h.app.tab().editor.has_staged_run(), "disconnected while it waited");

    let mut h = harness("SELECT 1;");
    h.app.tab_mut().profile = None;
    h.ctrl('e');
    assert!(h.overlay_kind().is_some(), "quick connect");
    h.key(KeyCode::Esc);
    assert!(!h.app.tab().editor.has_staged_run(), "quick connect closed");
}
