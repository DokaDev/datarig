//! A table tab's filter history: a filter and order are kept once the server answered them with
//! rows (never a run refused, failed, cancelled or answered too late), in a file of the state
//! directory that only the user can read; `Ctrl+R` in the filter line (`Space r H` from the
//! grid) lists them, and one picked fills the line without running (`Enter` then runs it,
//! checked as any text). A file that cannot be read is never taken for an empty history nor
//! written over; the setting turns it off; `:filter history clear [all]` asks first.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, ServerMessage};
use datarig_core::filter_history::{self as fh, TableKey};
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_i18n::Lang;
use datarig_tui::app::Startup;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::table_query::Field;
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// A state directory of the test's own, removed when it ends.
struct State(PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-filter-history-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join(fh::FILE)
    }

    /// What the file holds for table `k`: (where, order) pairs.
    fn entries(&self, k: &TableKey) -> Vec<(String, String)> {
        let h = fh::load(&self.file()).expect("the file reads");
        h.entries(k).iter().map(|e| (e.filter.clone(), e.order.clone())).collect()
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `shop.users` of the profile of `cfg` as the history names it (the profile's own database,
/// by its name).
fn key(cfg: &Config) -> TableKey {
    let p = &cfg.connections[0];
    TableKey { profile: p.id, database: p.database.clone(), schema: "shop".into(), table: "users".into() }
}

/// A table of another profile.
fn other_key() -> TableKey {
    TableKey {
        profile: datarig_core::profile::ProfileId::new(),
        database: "postgres".into(),
        schema: "shop".into(),
        table: "users".into(),
    }
}

fn launch(cfg: &Config, state: Option<&State>) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: state.map(|s| s.0.clone()) });
    let h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    let mut h = h.with_fake_driver();
    h.app.launch(Startup::Normal);
    h
}

/// Connect the profile, open `shop.users` (a table tab, which runs) and answer it.
fn open_users(h: &mut Harness) {
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.explore("local-pg");
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".to_string()], Vec::new()).into()) });
    h.keys("jj"); // Tables, users
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
    answer(h);
    h.sent();
    // The grid has the keys.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Results);
}

/// `shop.users` open in a fresh app on `state`.
fn users(cfg: &Config, state: Option<&State>) -> Harness {
    let mut h = launch(cfg, state);
    open_users(&mut h);
    h
}

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's run `id` with a first page of `shop.users` rows.
fn page_of(h: &mut Harness, id: u64) {
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    page_of(h, id);
}

/// Apply `text` with `:where` and answer it.
fn ran(h: &mut Harness, text: &str) {
    h.command(&format!("where {text}"));
    answer(h);
}

/// Write what is held back now (the debounce) and wait for the writer.
fn flush(h: &mut Harness) {
    h.app.flush_filter_history();
}

/// The status line's text.
fn said(h: &mut Harness) -> String {
    h.app.status_line().map(|(m, _, _)| m.to_string()).unwrap_or_default()
}

/// The entries the history list shows, as labels.
fn listed(h: &Harness) -> Vec<String> {
    match h.app.overlays.picker() {
        Some(datarig_tui::app::picker::Picker::FilterHistory { list, .. }) => {
            list.rows().iter().map(|r| list.label(*r).to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// The filter line's condition and order.
fn line(h: &Harness) -> (String, String) {
    let tq = h.app.tab().doc.table_query.as_ref().expect("a table query");
    (tq.line.where_.text().to_string(), tq.line.order.text().to_string())
}

/// Only a run the server answered with rows is kept, after the debounce; written 0600.
#[test]
fn only_a_run_answered_with_rows_is_kept() {
    let state = State::new("success");
    let cfg = test_db_config();
    let mut h = users(&cfg, Some(&state));
    // The plain run of the table (no filter, no order) keeps nothing, and reads nothing.
    flush(&mut h);
    assert!(!state.file().exists());
    ran(&mut h, "id > 1");
    // Held back for a second: not written yet.
    assert!(!state.file().exists());
    h.advance(Duration::from_millis(1100));
    h.app.pump_filter_history(Duration::from_secs(5));
    assert_eq!(state.entries(&key(&cfg)), [("id > 1".to_string(), String::new())]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(state.file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "only the user reads it");
    }
    // An order alone is kept too (with the filter applied).
    h.command("orderby name desc");
    answer(&mut h);
    flush(&mut h);
    let kept = [("id > 1".to_string(), "name DESC".to_string()), ("id > 1".to_string(), String::new())];
    assert_eq!(state.entries(&key(&cfg)), kept);
    // The server refuses it: not kept.
    h.command("where nme = 'x'");
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    let error =
        DbError::Server(ServerMessage { text: "column \"nme\" does not exist".into(), position: None, hint: None });
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
    // Cancelled before its rows: not kept.
    h.command("where id > 5");
    let id = h.app.tab().exec.query_id;
    h.tab_db(index, DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
    // Refused by the app: nothing is sent, nothing kept.
    h.sent();
    h.command("where id > 1; drop table users");
    assert!(runs(&mut h).is_empty());
    flush(&mut h);
    assert_eq!(state.entries(&key(&cfg)), kept);
    let text = std::fs::read_to_string(state.file()).unwrap();
    assert!(!text.contains("nme") && !text.contains("id > 5") && !text.contains("drop"), "{text}");
}

/// A late page of an earlier run (its query id) is not that filter's success.
#[test]
fn a_late_answer_of_an_earlier_filter_is_not_kept() {
    let state = State::new("stale");
    let cfg = test_db_config();
    let mut h = users(&cfg, Some(&state));
    h.command("where id > 1");
    let first = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Failed { id: first, error: DbError::Cancelled, cancelled: true });
    h.command("where id > 2");
    assert_eq!(runs(&mut h).last(), Some(&vec![format!("{ALL} WHERE (id > 2)")]));
    // The first one's page comes late: dropped.
    page_of(&mut h, first);
    flush(&mut h);
    assert!(!state.file().exists(), "nothing ran: {:?}", std::fs::read_to_string(state.file()));
    answer(&mut h);
    flush(&mut h);
    assert_eq!(state.entries(&key(&cfg)), [("id > 2".to_string(), String::new())]);
}

/// `Ctrl+R` in the condition lists the table's filters, the most recent first, typing narrows
/// them; a pick fills the condition and the order and sends nothing; `Enter` then runs the
/// compiled statement, checked as any text.
#[test]
fn a_picked_filter_fills_the_line_and_runs_only_with_enter() {
    let cfg = test_db_config();
    let mut h = users(&cfg, None);
    ran(&mut h, "id > 1");
    h.command("orderby name desc");
    answer(&mut h);
    ran(&mut h, "name = 'n2'");
    h.command("where");
    answer(&mut h);
    h.command("orderby");
    answer(&mut h);
    h.sent();
    h.keys("f");
    h.ctrl('r');
    assert!(h.app.overlays.is_open(OverlayKind::Picker));
    assert_eq!(listed(&h), ["(no condition)", "name = 'n2'", "id > 1", "id > 1"]);
    h.type_text("n2");
    assert_eq!(listed(&h), ["name = 'n2'"]);
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Backspace);
    h.keys("id");
    // The second `id > 1`: without an order.
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert!(!h.app.overlays.is_open(OverlayKind::Picker));
    assert!(runs(&mut h).is_empty(), "nothing runs on a pick");
    assert_eq!(line(&h), ("id > 1".into(), String::new()));
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().line.focus, Some(Field::Where));
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
    answer(&mut h);
    // From the grid, `Space r H`: the most recent first, both texts.
    h.keys(" rH");
    assert_eq!(listed(&h), ["id > 1", "(no condition)", "name = 'n2'", "id > 1"]);
    for _ in 0..3 {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Enter);
    assert_eq!(line(&h), ("id > 1".into(), "name DESC".into()));
    assert!(runs(&mut h).is_empty());
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC"#)]]);
}

/// `Esc` in the list changes nothing; another tab closes the list.
#[test]
fn esc_leaves_the_line_and_another_tab_closes_the_list() {
    let cfg = test_db_config();
    let mut h = users(&cfg, None);
    ran(&mut h, "id > 1");
    ran(&mut h, "id > 2");
    h.keys("f");
    h.type_text(" AND id < 9");
    h.ctrl('r');
    h.key(KeyCode::Esc);
    assert!(!h.app.overlays.is_open(OverlayKind::Picker));
    assert_eq!(line(&h).0, "id > 2 AND id < 9");
    h.key(KeyCode::Esc);
    h.keys(" rH");
    assert!(h.app.overlays.is_open(OverlayKind::Picker));
    let other = (0..h.app.tabs.len()).find(|i| *i != h.app.tabs.active_index()).expect("another tab");
    h.app.tabs.activate(other);
    h.key(KeyCode::Down);
    assert!(!h.app.overlays.is_open(OverlayKind::Picker), "its tab is not the active one");
}

/// Pending edits of the rows: a picked filter asks about them when it runs (`Enter` keeps them).
#[test]
fn a_picked_filter_asks_about_pending_edits_when_it_runs() {
    let cfg = test_db_config();
    let mut h = users(&cfg, None);
    ran(&mut h, "id > 1");
    h.command("where");
    answer(&mut h);
    if let datarig_tui::app::Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    h.sent();
    h.keys(" rH");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Enter);
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "Enter keeps the edits: nothing runs");
}

/// An entry whose condition or order holds a line break (the one-line field cannot hold it
/// as it is) is not filled in: said why, the line left as it was, nothing run.
#[test]
fn an_entry_the_line_cannot_hold_is_refused_never_altered() {
    let state = State::new("lines");
    let cfg = test_db_config();
    let mut seed = fh::History::default();
    seed.record(key(&cfg), "id > 1", "name\r\nDESC", 1);
    seed.record(key(&cfg), "note = 'a\nb'", "", 2);
    fh::save(&state.file(), &seed).unwrap();
    let mut h = users(&cfg, Some(&state));
    h.sent();
    for down in [0, 1] {
        h.keys(" rH");
        assert_eq!(listed(&h).len(), 2);
        for _ in 0..down {
            h.key(KeyCode::Down);
        }
        h.key(KeyCode::Enter);
        assert!(!h.app.overlays.is_open(OverlayKind::Picker));
        assert_eq!(line(&h), (String::new(), String::new()), "never filled in altered");
        assert!(said(&mut h).contains("line break"), "{}", said(&mut h));
        assert!(runs(&mut h).is_empty());
    }
}

/// A file that cannot be read turns the history off: said once, `Ctrl+R` says it is not
/// available (never "no history"), and the file is never written, not by a clear either.
#[test]
fn an_unreadable_file_is_never_an_empty_history_nor_written() {
    let state = State::new("unreadable");
    let cfg = test_db_config();
    let broken = "version = 1\n[[tables]\nwhere = 'secret'\n";
    std::fs::write(state.file(), broken).unwrap();
    let mut h = users(&cfg, Some(&state));
    ran(&mut h, "id > 1");
    assert!(said(&mut h).contains("Filter history unavailable"), "{}", said(&mut h));
    // Read once: the next run neither reads it again nor says it again.
    ran(&mut h, "id > 2");
    h.keys(" rH");
    assert!(!h.app.overlays.is_open(OverlayKind::Picker));
    let s = said(&mut h);
    assert!(s.contains("Filter history unavailable") && !s.contains("No filter history"), "{s}");
    h.command("filter history clear");
    h.keys("y");
    let s = said(&mut h);
    assert!(s.contains("not cleared") && s.contains(fh::FILE), "{s}");
    h.command("filter history clear all");
    h.keys("y");
    flush(&mut h);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
    assert_eq!(std::fs::read_to_string(state.file()).unwrap(), broken, "left as it was");
    // The error log is written by a thread of its own: wait for it.
    assert!(h.app.drain_filter_history_log(Duration::from_secs(5)), "the error log is written");
    let log = std::fs::read_to_string(state.0.join("errors.log")).unwrap();
    assert_eq!(log.matches("filter_history.unreadable").count(), 1, "{log}");
    assert!(!log.contains("secret"), "never its text: {log}");
}

/// A symbolic link at the file's path (to a history elsewhere, or to nothing) is never
/// followed: the history is off for the session, the path named, and nothing is written there
/// nor where the link points.
#[cfg(unix)]
#[test]
fn a_link_at_the_file_is_never_followed() {
    let state = State::new("link");
    let cfg = test_db_config();
    let elsewhere = state.0.join("elsewhere.toml");
    let mut seed = fh::History::default();
    seed.record(key(&cfg), "elsewhere = 1", "", 1);
    fh::save(&elsewhere, &seed).unwrap();
    let before = std::fs::read_to_string(&elsewhere).unwrap();
    for target in [elsewhere.clone(), state.0.join("nowhere.toml")] {
        let _ = std::fs::remove_file(state.file());
        std::os::unix::fs::symlink(&target, state.file()).unwrap();
        let mut h = users(&cfg, Some(&state));
        ran(&mut h, "id > 1");
        let s = said(&mut h);
        assert!(s.contains("Filter history unavailable") && s.contains("not a plain file"), "{s}");
        h.keys(" rH");
        assert!(!h.app.overlays.is_open(OverlayKind::Picker), "never listed");
        h.command("filter history clear all");
        h.keys("y");
        flush(&mut h);
        h.app.dispatch(datarig_tui::app::action::Action::Quit);
        assert!(std::fs::symlink_metadata(state.file()).unwrap().file_type().is_symlink(), "the link stays");
    }
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), before, "never written through");
    assert!(!state.0.join("nowhere.toml").exists(), "nothing created where it points");
}

/// What the history records in the error log never holds up the app: with an `errors.log`
/// that blocks whoever writes it (a FIFO nobody reads), a file that cannot be read and a write
/// that fails are still said at once, and the lines (what failed and the path, never a text)
/// come once something reads it.
#[cfg(unix)]
#[test]
fn the_error_log_is_written_off_the_ui_thread() {
    use std::io::Read;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let state = State::new("log-fifo");
    let cfg = test_db_config();
    let log = state.0.join("errors.log");
    let made = std::process::Command::new("mkfifo").arg(&log).status();
    if !made.is_ok_and(|s| s.success()) {
        return; // No mkfifo here: nothing to check.
    }
    std::fs::write(state.file(), "version = 1\n[[tables]\nwhere = 'secret'\n").unwrap();
    let mut h = users(&cfg, Some(&state));
    ran(&mut h, "id > 1");
    assert!(said(&mut h).contains("Filter history unavailable"), "{}", said(&mut h));
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);

    std::fs::remove_file(state.file()).unwrap();
    let mut h = users(&cfg, Some(&state));
    let lock = |mode| std::fs::set_permissions(&state.0, std::fs::Permissions::from_mode(mode)).unwrap();
    lock(0o500);
    let root = std::fs::write(state.0.join("probe"), "").is_ok();
    ran(&mut h, "id > 2");
    flush(&mut h);
    lock(0o700);
    assert!(root || said(&mut h).contains("not saved"), "{}", said(&mut h));

    // Read what the writers waited to write.
    let mut reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(&log)
        .unwrap();
    let (mut text, mut buf) = (String::new(), [0u8; 4096]);
    let start = std::time::Instant::now();
    let want = if root { 1 } else { 2 };
    while text.matches("filter_history.").count() < want && start.elapsed() < Duration::from_secs(5) {
        match reader.read(&mut buf) {
            Ok(n) if n > 0 => text.push_str(&String::from_utf8_lossy(&buf[..n])),
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert!(text.contains("filter_history.unreadable"), "{text}");
    assert!(root || text.contains("filter_history.save_failed"), "{text}");
    assert!(text.contains(fh::FILE) && !text.contains("secret") && !text.contains("id >"), "{text}");
}

/// An error log that blocks for good (a FIFO nobody ever reads) never holds up the history's
/// writes: once the folder can be written again, a clear after a failed one is written and
/// said, and quitting stays quick.
#[cfg(unix)]
#[test]
fn a_stuck_error_log_never_holds_up_a_write() {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let state = State::new("log-stuck");
    let cfg = test_db_config();
    let log = state.0.join("errors.log");
    let made = std::process::Command::new("mkfifo").arg(&log).status();
    if !made.is_ok_and(|s| s.success()) {
        return; // No mkfifo here: nothing to check.
    }
    let mut seed = fh::History::default();
    seed.record(key(&cfg), "secret = 1", "", 1);
    fh::save(&state.file(), &seed).unwrap();
    let mut h = users(&cfg, Some(&state));
    let lock = |mode| std::fs::set_permissions(&state.0, std::fs::Permissions::from_mode(mode)).unwrap();
    lock(0o500);
    if std::fs::write(state.0.join("probe"), "").is_ok() {
        return lock(0o700); // Root writes anywhere: nothing to check.
    }
    // Each fails, and its line for the error log waits for a reader that never comes (more
    // lines than the log's thread keeps waiting).
    for _ in 0..20 {
        h.command("filter history clear");
        h.keys("y");
        flush(&mut h);
        assert!(said(&mut h).contains("not saved"), "{}", said(&mut h));
    }
    lock(0o700);
    h.command("filter history clear");
    h.keys("y");
    flush(&mut h);
    assert!(said(&mut h).contains("cleared"), "{}", said(&mut h));
    assert!(state.entries(&key(&cfg)).is_empty(), "written");
    ran(&mut h, "id > 1");
    let start = std::time::Instant::now();
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(start.elapsed() < Duration::from_secs(3), "quitting took {:?}", start.elapsed());
    assert_eq!(state.entries(&key(&cfg)), [("id > 1".to_string(), String::new())], "written on quitting");
    // Waiting for the error log's thread to catch up is bounded too.
    let start = std::time::Instant::now();
    assert!(!h.app.drain_filter_history_log(Duration::from_secs(1)), "never written");
    assert!(start.elapsed() < Duration::from_secs(3), "waited {:?}", start.elapsed());
    // Let the error log's thread go.
    let _reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(&log)
        .unwrap();
}

/// `history.filters = off`: nothing is kept and `Ctrl+R` says the history is off.
#[test]
fn the_setting_off_keeps_nothing() {
    let state = State::new("off");
    let cfg = test_db_config();
    let mut h = users(&cfg, Some(&state));
    h.command("set history.filters=off");
    ran(&mut h, "id > 1");
    flush(&mut h);
    assert!(!state.file().exists());
    h.keys(" rH");
    assert!(said(&mut h).contains("Filter history is off"), "{}", said(&mut h));
    h.command("set history.filters=on");
    ran(&mut h, "id > 2");
    flush(&mut h);
    assert_eq!(state.entries(&key(&cfg)), [("id > 2".to_string(), String::new())]);
}

/// `:filter history clear` asks (`Enter` cancels), `y` forgets this table only; `clear all`
/// from any tab forgets every table. Said once written.
#[test]
fn clearing_asks_first_and_forgets_this_table_or_all() {
    let state = State::new("clear");
    let cfg = test_db_config();
    let other = other_key();
    let mut seed = fh::History::default();
    seed.record(other.clone(), "kept = 1", "", 1);
    fh::save(&state.file(), &seed).unwrap();
    let mut h = users(&cfg, Some(&state));
    ran(&mut h, "id > 1");
    h.command("filter history clear");
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::FilterHistoryClear { all: false }));
    h.key(KeyCode::Enter);
    flush(&mut h);
    assert_eq!(state.entries(&key(&cfg)).len(), 1, "Enter cancels");
    h.command("filter history clear");
    h.keys("y");
    flush(&mut h);
    assert!(said(&mut h).contains("cleared (1 entry)"), "{}", said(&mut h));
    assert!(state.entries(&key(&cfg)).is_empty());
    assert_eq!(fh::load(&state.file()).unwrap().entries(&other).len(), 1, "another table stays");
    // Every table, from another tab.
    let console = (0..h.app.tabs.len()).find(|i| *i != h.app.tabs.active_index()).expect("another tab");
    h.app.tabs.activate(console);
    assert!(!h.app.tab().is_table());
    h.command("filter history clear all");
    h.keys("y");
    flush(&mut h);
    assert_eq!(fh::load(&state.file()), Ok(fh::History::default()));
    // A wrong argument says how.
    h.command("filter history");
    assert!(h.app.overlays.is_open(OverlayKind::Commands));
}

/// A clear whose write failed is not taken as done: said as failed, and the next clear (with
/// nothing left to forget in memory) writes it again, said cleared only once written; so does
/// quitting.
#[cfg(unix)]
#[test]
fn a_clear_that_could_not_be_written_is_written_again() {
    use std::os::unix::fs::PermissionsExt;
    let state = State::new("clear-retry");
    let cfg = test_db_config();
    let mut seed = fh::History::default();
    seed.record(key(&cfg), "secret = 1", "", 1);
    fh::save(&state.file(), &seed).unwrap();
    let mut h = users(&cfg, Some(&state));
    let lock = |mode| std::fs::set_permissions(&state.0, std::fs::Permissions::from_mode(mode)).unwrap();
    lock(0o500);
    if std::fs::write(state.0.join("probe"), "").is_ok() {
        return lock(0o700); // Root writes anywhere: nothing to check.
    }
    h.command("filter history clear");
    h.keys("y");
    flush(&mut h);
    assert!(said(&mut h).contains("not saved"), "{}", said(&mut h));
    assert_eq!(state.entries(&key(&cfg)).len(), 1, "still in the file");
    lock(0o700);
    h.command("filter history clear");
    h.keys("y");
    flush(&mut h);
    assert!(said(&mut h).contains("cleared"), "{}", said(&mut h));
    assert!(state.entries(&key(&cfg)).is_empty(), "written this time");

    // A clear that failed, then quitting: written on the way out.
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    fh::save(&state.file(), &seed).unwrap();
    let mut h = users(&cfg, Some(&state));
    lock(0o500);
    h.command("filter history clear all");
    h.keys("y");
    flush(&mut h);
    assert_eq!(state.entries(&key(&cfg)).len(), 1);
    lock(0o700);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(state.entries(&key(&cfg)).is_empty(), "written on quitting");
}

/// A clear asked again while the first one's write is still on its way is said cleared only
/// once that write is done, and as failed when it fails (never cleared before the file is).
#[cfg(unix)]
#[test]
fn a_clear_asked_again_waits_for_the_write_on_its_way() {
    use std::os::unix::fs::PermissionsExt;
    let state = State::new("clear-again");
    let cfg = test_db_config();
    let mut seed = fh::History::default();
    seed.record(key(&cfg), "secret = 1", "", 1);
    let lock = |mode| std::fs::set_permissions(&state.0, std::fs::Permissions::from_mode(mode)).unwrap();
    for fails in [false, true] {
        fh::save(&state.file(), &seed).unwrap();
        let mut h = users(&cfg, Some(&state));
        if fails {
            lock(0o500);
            if std::fs::write(state.0.join("probe"), "").is_ok() {
                return lock(0o700); // Root writes anywhere: nothing to check.
            }
        }
        h.command("filter history clear");
        h.keys("y");
        // The first write is sent, its answer not handed to the app yet.
        h.command("filter history clear");
        h.keys("y");
        assert!(!said(&mut h).contains("cleared"), "said before the write is done: {}", said(&mut h));
        flush(&mut h);
        lock(0o700);
        if fails {
            assert!(said(&mut h).contains("not saved"), "{}", said(&mut h));
            assert_eq!(state.entries(&key(&cfg)).len(), 1, "still in the file");
        } else {
            assert!(said(&mut h).contains("cleared"), "{}", said(&mut h));
            assert!(state.entries(&key(&cfg)).is_empty());
        }
        drop(h);
    }
}

/// Without a state directory the history works in memory; a second instance (another holds
/// the state directory) reads the file and never writes it.
#[test]
fn without_a_writable_state_directory_nothing_is_written() {
    let cfg = test_db_config();
    let mut h = users(&cfg, None);
    ran(&mut h, "id > 1");
    flush(&mut h);
    h.keys(" rH");
    assert_eq!(listed(&h), ["id > 1"]);
    h.key(KeyCode::Esc);

    let state = State::new("second");
    let mut seed = fh::History::default();
    seed.record(key(&cfg), "first = 1", "", 1);
    fh::save(&state.file(), &seed).unwrap();
    let before = std::fs::read_to_string(state.file()).unwrap();
    let mut h = launch(&cfg, Some(&state));
    h.app.read_only = true;
    open_users(&mut h);
    ran(&mut h, "id > 1");
    flush(&mut h);
    h.keys(" rH");
    assert_eq!(listed(&h), ["id > 1", "first = 1"], "read, and kept in memory");
    h.key(KeyCode::Esc);
    h.command("filter history clear all");
    h.keys("y");
    assert!(said(&mut h).contains("in this window only"), "{}", said(&mut h));
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert_eq!(std::fs::read_to_string(state.file()).unwrap(), before, "never written");
}

/// The empty line's hint as drawn on a `w` by `h` screen: from `WHERE` to the refresh mark.
fn hint_drawn(h: &mut Harness, w: u16, hh: u16) -> String {
    let screen = h.screen(w, hh);
    let row = screen.lines().find(|l| l.contains("WHERE")).expect("the filter line").to_string();
    let from = row.find("WHERE").unwrap();
    let to = row.find('\u{27f3}').expect("the refresh mark");
    row[from..to].trim_end().to_string()
}

/// With the explorer and the cell pane open the empty line's hint is narrow: it shows its key
/// parts whole (as many as fit, the first always), never one cut in its middle, in English and
/// Korean; with room every part shows.
#[test]
fn the_line_hint_drops_whole_parts_when_narrow() {
    let cfg = test_db_config();
    let mut h = users(&cfg, None);
    let msg = datarig_i18n::Msg::TableFilterHint { filter: "f".into(), sort: "s".into(), history: "Ctrl+R".into() };
    for lang in [Lang::En, Lang::Ko] {
        h.app.i18n.set_lang(lang);
        let full = h.app.i18n.msg(&msg).to_string();
        let parts: Vec<&str> = full.split(" \u{b7} ").collect();
        assert_eq!(parts.len(), 3, "{full}");
        let narrow = hint_drawn(&mut h, 80, 24);
        let whole: Vec<String> = (1..=parts.len()).map(|k| parts[..k].join(" \u{b7} ")).collect();
        assert!(whole.contains(&narrow), "{lang:?}: {narrow:?} is not whole parts of {full:?}");
        assert!(!narrow.contains('\u{2026}'), "{lang:?}: {narrow:?}");
        assert_eq!(hint_drawn(&mut h, 120, 30), full, "{lang:?}");
    }
    h.app.i18n.set_lang(Lang::Ko);
    assert_screen!("filter_history_hint_ko_80x24", Lang::Ko, h.draw(80, 24));
    assert_screen!("filter_history_hint_ko_120x30", Lang::Ko, h.draw(120, 30));
}

/// The list with the selected entry's texts under it, the line's hint naming `Ctrl+R`, and
/// the clear question.
#[test]
fn the_history_list_hint_and_clear_question_look_right() {
    let cfg = test_db_config();
    let mut h = users(&cfg, None);
    assert_screen!("filter_history_hint_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_history_hint_en_120x30", Lang::En, h.draw(120, 30));
    ran(&mut h, "status = 'done' AND amount > 1500 AND note LIKE '%a very long note that wraps%'");
    h.command("orderby name desc, id");
    answer(&mut h);
    ran(&mut h, "id > 1");
    h.keys("f");
    h.ctrl('r');
    h.key(KeyCode::Down);
    assert_screen!("filter_history_list_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_history_list_en_120x30", Lang::En, h.draw(120, 30));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    h.command("filter history clear");
    assert_screen!("filter_history_clear_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_history_clear_en_120x30", Lang::En, h.draw(120, 30));
}
