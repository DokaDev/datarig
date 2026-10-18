//! The export dialog through the real `App` event path, with the fake driver: opening it
//! (`Space r e`, the results menu, `:export` alone), its fields and the notes that follow them,
//! what Export refuses (it stays open and says why at the field), and an export it starts,
//! written to a scratch folder (the home folder is a scratch folder too).

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_tui::app::export_form::ExportField;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::tabs::TableRef;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// A new empty folder for a test's files.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-export-dialog-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn columns() -> Vec<ColumnMeta> {
    vec![meta("id", "int8", true, false), meta("name", "text", false, false)]
}

/// Run the statement under the cursor and answer with a first page of `n` rows (`more`: the
/// server has more).
fn first_page(h: &mut Harness, n: usize, more: bool) {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let rows = (0..n).map(|i| vec![Some(i.to_string()), if i == 2 { None } else { Some(format!("n{i}")) }]).collect();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns()), rows, more, elapsed: Duration::ZERO });
    h.sent();
}

/// A harness whose home folder is a new scratch folder.
fn with_home(tag: &str) -> (Harness, PathBuf) {
    let home = scratch(tag);
    let mut h = Harness::connected(Lang::En);
    let env_home = home.display().to_string();
    h.app.set_env_lookup(std::sync::Arc::new(move |k: &str| (k == "HOME").then(|| env_home.clone())));
    (h, home)
}

/// `Space r e` from the grid.
fn open(h: &mut Harness) {
    h.key(KeyCode::Tab);
    h.keys(" re");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export), "the dialog opened");
}

/// The screen, with the time in a default file name written `YYYYMMDD-HHMMSS`.
fn screen(h: &mut Harness) -> String {
    let s = h.screen(160, 45);
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find('-') {
        let (head, tail) = rest.split_at(i);
        let stamp = |t: &str| {
            let b = t.as_bytes();
            head.len() >= 8
                && head.as_bytes()[head.len() - 8..].iter().all(u8::is_ascii_digit)
                && b.len() >= 7
                && b[1..7].iter().all(u8::is_ascii_digit)
        };
        if stamp(tail) {
            out.push_str(&head[..head.len() - 8]);
            out.push_str("YYYYMMDD-HHMMSS");
            rest = &tail[7..];
        } else {
            out.push_str(head);
            out.push('-');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

fn focus(h: &Harness) -> ExportField {
    h.app.overlays.export_form().expect("the dialog").focus
}

fn path_text(h: &Harness) -> String {
    h.app.overlays.export_form().expect("the dialog").path.text().to_string()
}

/// Tab to field `f`.
fn tab_to(h: &mut Harness, f: ExportField) {
    for _ in 0..12 {
        if focus(h) == f {
            return;
        }
        h.key(KeyCode::Tab);
    }
    panic!("no field {f:?}");
}

/// Hand the writer's events in until the tab's export ended (or fail after [`WAIT`]).
fn pump_until_done(h: &mut Harness) {
    let start = std::time::Instant::now();
    while h.app.exporting(h.app.tab().id) {
        assert!(start.elapsed() < WAIT, "the export did not end");
        h.app.pump_exports(Duration::from_millis(50));
    }
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

#[test]
fn the_key_the_menu_and_the_command_open_it_and_esc_closes_it() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 5, true);
    open(&mut h);
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None);
    assert!(!h.app.exporting(h.app.tab().id), "Esc exports nothing");
    // The results menu.
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    h.menu_pick("Results → export to a file…");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    h.key(KeyCode::Esc);
    // `:export` alone; with words it still exports at once.
    h.command("export");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    // Cancel closes it too.
    tab_to(&mut h, ExportField::Cancel);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert!(h.sent().is_empty(), "nothing was sent");
}

#[test]
fn the_fields_follow_the_format_and_the_file_name_its_extension() {
    let (mut h, _home) = with_home("fields");
    first_page(&mut h, 5, true);
    open(&mut h);
    let fields = |h: &Harness| h.app.overlays.export_form().unwrap().fields();
    use ExportField::*;
    assert_eq!(fields(&h), vec![Scope, Format, Delimiter, Quote, Header, Null, Path, Export, Cancel]);
    let made = path_text(&h);
    assert!(made.starts_with("~/Downloads/console 1-") && made.ends_with(".csv"), "{made}");
    // Scope: no selection, no Selection.
    assert_eq!(h.app.overlays.export_form().unwrap().scopes.len(), 2);
    // `→` on the format: TSV (its header only), then JSON past the formats still to come.
    tab_to(&mut h, Format);
    h.key(KeyCode::Right);
    assert_eq!(fields(&h), vec![Scope, Format, Header, Path, Export, Cancel]);
    assert!(path_text(&h).ends_with(".tsv"));
    h.key(KeyCode::Right);
    assert_eq!(fields(&h), vec![Scope, Format, Path, Export, Cancel]);
    assert_eq!(path_text(&h), made.replace(".csv", ".json"));
    // The folder changed: the name still follows the format.
    tab_to(&mut h, Path);
    h.key(KeyCode::Home);
    h.type_text("x");
    tab_to(&mut h, Format);
    h.key(KeyCode::Right);
    assert_eq!(path_text(&h), format!("x{}", made.replace(".csv", ".json")), "JSON Pretty: `.json` too");
    h.key(KeyCode::Right);
    assert!(path_text(&h).ends_with(".jsonl"), "{}", path_text(&h));
    // A name the user wrote stays as it is.
    tab_to(&mut h, Path);
    h.ctrl('u');
    h.type_text("/tmp/mine.txt");
    tab_to(&mut h, Format);
    h.key(KeyCode::Left);
    assert_eq!(path_text(&h), "/tmp/mine.txt");
    // The list: Enter opens it, a pick sets the format.
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
    h.type_text("markdown");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    assert_eq!(h.app.overlays.export_form().unwrap().format, datarig_tui::app::export::ExportFormat::Markdown);
    // With a selection, the selection is offered.
    h.key(KeyCode::Esc);
    h.keys("vj");
    h.keys(" re");
    assert_eq!(h.app.overlays.export_form().unwrap().scopes.len(), 3);
}

#[test]
fn what_export_refuses_stays_in_the_dialog_at_its_field() {
    let (mut h, _home) = with_home("refused");
    first_page(&mut h, 5, true);
    open(&mut h);
    tab_to(&mut h, ExportField::Delimiter);
    h.ctrl('u');
    h.type_text("ab");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    assert_eq!(focus(&h), ExportField::Delimiter);
    assert!(screen(&mut h).contains("The delimiter is one character (\\t for a tab)"));
    h.ctrl('u');
    h.type_text("\"");
    assert!(!screen(&mut h).contains("The delimiter is one character"), "typing clears it");
    h.key(KeyCode::Enter);
    assert!(screen(&mut h).contains("a double quote or a line break cannot separate CSV values"));
    // NULL written as text with the delimiter in it.
    h.ctrl('u');
    h.type_text("\\t");
    tab_to(&mut h, ExportField::Null);
    h.type_text("a\tb");
    h.key(KeyCode::Enter);
    assert_eq!(focus(&h), ExportField::Null);
    assert!(screen(&mut h).contains("the text for NULL holds the delimiter"));
    h.ctrl('u');
    tab_to(&mut h, ExportField::Path);
    h.ctrl('u');
    h.key(KeyCode::Enter);
    assert_eq!(focus(&h), ExportField::Path);
    assert!(screen(&mut h).contains("Name the file to write"));
    assert!(!h.app.exporting(h.app.tab().id));
    assert!(h.sent().is_empty());
}

#[test]
fn export_writes_the_rows_fetched_as_the_dialog_says() {
    let (mut h, home) = with_home("write");
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    first_page(&mut h, 5, true);
    open(&mut h);
    // Fetched rows, CSV with `;`, every value quoted, NULL as `NULL`, no header line.
    h.key(KeyCode::Right);
    tab_to(&mut h, ExportField::Delimiter);
    h.ctrl('u');
    h.type_text(";");
    tab_to(&mut h, ExportField::Quote);
    h.key(KeyCode::Right);
    tab_to(&mut h, ExportField::Header);
    h.key(KeyCode::Char(' '));
    tab_to(&mut h, ExportField::Null);
    h.type_text("NULL");
    let made = path_text(&h);
    tab_to(&mut h, ExportField::Export);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None, "the dialog closed; the export runs behind");
    pump_until_done(&mut h);
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::Export { .. })), "nothing runs on the server");
    let file = home.join("Downloads").join(made.trim_start_matches("~/Downloads/"));
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    assert_eq!(text, "\"0\";\"n0\"\n\"1\";\"n1\"\n\"2\";NULL\n\"3\";\"n3\"\n\"4\";\"n4\"\n");
    assert!(status(&h).starts_with("Exported 5 rows to "), "{}", status(&h));
    // JSON to a path of its own, the selection of two rows of the first column.
    let dir = scratch("write-json");
    h.keys("vj");
    h.keys(" re");
    h.key(KeyCode::Right);
    h.key(KeyCode::Right);
    tab_to(&mut h, ExportField::Format);
    h.key(KeyCode::Right);
    h.key(KeyCode::Right);
    tab_to(&mut h, ExportField::Path);
    h.ctrl('u');
    let json = dir.join("sel.json");
    h.type_text(&json.display().to_string());
    h.key(KeyCode::Enter);
    pump_until_done(&mut h);
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!([{ "id": 0 }, { "id": 1 }]));
}

#[test]
fn every_row_of_a_console_result_says_it_runs_the_statement_again() {
    let (mut h, _home) = with_home("console");
    first_page(&mut h, 5, true);
    open(&mut h);
    let s = screen(&mut h);
    assert!(s.contains("Runs the statement again, once"), "{s}");
    insta::assert_snapshot!("export_dialog_console_en_160x45", s);
    // The rows fetched: nothing is sent.
    h.key(KeyCode::Right);
    assert!(screen(&mut h).contains("The 5 rows fetched so far (the server has more); nothing is sent to the server."));
}

#[test]
fn csv_without_quotes_warns_that_the_file_breaks() {
    let (mut h, _home) = with_home("noquotes");
    first_page(&mut h, 5, false);
    open(&mut h);
    tab_to(&mut h, ExportField::Quote);
    h.key(KeyCode::Enter);
    h.type_text("none");
    h.key(KeyCode::Enter);
    let s = screen(&mut h);
    assert!(s.contains("None (suppress quotes)") && s.contains("breaks the file's columns"), "{s}");
    assert!(s.contains("All 5 rows of the result are fetched"), "{s}");
    insta::assert_snapshot!("export_dialog_csv_no_quotes_en_160x45", s);
}

#[test]
fn a_table_tab_says_it_is_read_in_chunks_by_its_key() {
    let (mut h, _home) = with_home("keyset");
    h.app.tab_mut().editor = Editor::new("SELECT * FROM \"shop\".\"users\"");
    h.app.tab_mut().doc.table = Some(TableRef { schema: "shop".into(), name: "users".into() });
    h.ctrl('e');
    let qid = h.app.tab().exec.query_id;
    let cols: Vec<ColumnMeta> = ["id", "email", "name", "nickname", "profile"]
        .iter()
        .enumerate()
        .map(|(i, n)| meta_from(n, if i == 0 { "int8" } else { "text" }, i == 0, USERS, i as i16 + 1))
        .collect();
    let row = |i: usize| vec![Some(i.to_string()), Some(format!("u{i}@x")), Some("n".into()), None, None];
    h.tab_db(
        0,
        DbEvent::Page {
            id: qid,
            columns: Some(cols),
            rows: (0..10).map(row).collect(),
            more: true,
            elapsed: Duration::ZERO,
        },
    );
    h.sent();
    open(&mut h);
    // The key is read for the note.
    assert!(
        h.sent()
            .iter()
            .any(|c| matches!(c, DbCommand::LoadStructure { schema, table } if schema == "shop" && table == "users"))
    );
    assert!(screen(&mut h).contains("Reading the table's key"));
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    let s = screen(&mut h);
    assert!(s.contains("Reads shop.users in chunks by its key"), "{s}");
    assert!(s.contains("about 11,000 rows"), "{s}");
    insta::assert_snapshot!("export_dialog_keyset_en_160x45", s);
    // Opened again: the structure is known, nothing is asked.
    h.key(KeyCode::Esc);
    h.keys(" re");
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })));
}

#[test]
fn insert_and_update_are_struck_with_the_copys_reason() {
    let (mut h, _home) = with_home("insert");
    first_page(&mut h, 5, false);
    open(&mut h);
    tab_to(&mut h, ExportField::Format);
    h.key(KeyCode::Enter);
    let s = screen(&mut h);
    assert!(s.contains("SQL INSERT") && s.contains("coming in a later version"), "{s}");
    let reason = "a column is computed, not a column of the table";
    assert!(s.lines().any(|l| l.contains("SQL INSERT") && l.contains(reason)), "{s}");
    assert!(s.lines().any(|l| l.contains("SQL UPDATE") && l.contains(reason)), "{s}");
    insta::assert_snapshot!("export_dialog_insert_disabled_en_160x45", s);
    // `←`/`→` pass over them: from XML, `→` stays.
    h.key(KeyCode::Esc);
    for _ in 0..12 {
        h.key(KeyCode::Right);
    }
    assert_eq!(h.app.overlays.export_form().unwrap().format, datarig_tui::app::export::ExportFormat::Xml);
}

#[test]
fn the_mouse_opens_a_list_and_presses_the_buttons() {
    let (mut h, _home) = with_home("mouse");
    first_page(&mut h, 5, false);
    open(&mut h);
    h.draw(160, 45);
    let hit = |h: &Harness, f: ExportField| {
        let form = h.app.overlays.export_form().unwrap();
        form.hits
            .iter()
            .find(|(_, x)| *x == datarig_tui::app::export_form::ExportHit::Value(f))
            .map(|(r, _)| (r.x, r.y))
            .unwrap()
    };
    let (x, y) = hit(&h, ExportField::Format);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Picker));
    h.key(KeyCode::Esc);
    let (x, y) = hit(&h, ExportField::Header);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    assert!(!h.app.overlays.export_form().unwrap().header, "the checkbox cleared");
    h.draw(160, 45);
    h.advance(Duration::from_millis(500));
    let cancel = h.app.overlays.export_form().unwrap().buttons.rects[1];
    h.mouse(MouseEventKind::Down(MouseButton::Left), cancel.x + 1, cancel.y);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export), "a button acts on its release");
    h.mouse(MouseEventKind::Up(MouseButton::Left), cancel.x + 1, cancel.y);
    assert_eq!(h.overlay_kind(), None);
}

/// The status bar's passing word, as text.
fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The dialog opened on a result that a statement already running replaces while it is open
/// (`selection`: on a selected range): nothing is exported, and the dialog says why as it goes.
fn a_new_result_under_the_dialog(selection: bool) {
    let (mut h, home) = with_home(if selection { "stale-selection" } else { "stale-fetched" });
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    first_page(&mut h, 5, false);
    h.key(KeyCode::Tab);
    if selection {
        h.keys("vj");
    }
    // Another run: the rows shown stay until it answers.
    h.ctrl('e');
    let qid = h.app.tab().exec.query_id;
    h.sent();
    h.keys(" re");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    h.key(KeyCode::Right);
    if selection {
        h.key(KeyCode::Right);
        assert_eq!(h.app.overlays.export_form().unwrap().scope, datarig_tui::app::export::ExportScope::Selection);
    }
    let index = h.app.tabs.active_index();
    let rows = (100..103).map(|i| vec![Some(i.to_string()), Some(format!("new{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id: qid, columns: Some(columns()), rows, more: false, elapsed: Duration::ZERO });
    if h.overlay_kind() == Some(OverlayKind::Export) {
        tab_to(&mut h, ExportField::Export);
        h.key(KeyCode::Enter);
        pump_until_done(&mut h);
    }
    assert_eq!(h.overlay_kind(), None, "the dialog closed with the result it showed");
    assert!(flashed(&h).starts_with("Not exported: the result is not from this tab's session"), "{}", flashed(&h));
    assert!(!h.app.exporting(h.app.tab().id));
    assert_eq!(std::fs::read_dir(home.join("Downloads")).unwrap().count(), 0, "no file");
}

#[test]
fn a_new_result_under_the_dialog_exports_nothing() {
    a_new_result_under_the_dialog(false);
}

#[test]
fn a_new_result_under_the_dialog_on_a_selection_exports_nothing() {
    a_new_result_under_the_dialog(true);
}

#[test]
fn the_dialog_closes_when_another_tab_becomes_the_active_one() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 5, true);
    open(&mut h);
    h.command("tabnew");
    assert_ne!(h.overlay_kind(), Some(OverlayKind::Export), "the dialog does not show another tab");
    assert!(h.app.overlays.export_form().is_none());
    assert!(flashed(&h).starts_with("Not exported"), "{}", flashed(&h));
}

/// A console tab inside the user's open transaction, the server with more rows.
fn in_transaction(h: &mut Harness) {
    first_page(h, 5, true);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.sent();
}

#[test]
fn inside_the_transaction_the_note_says_when_it_asks_first() {
    let (mut h, _home) = with_home("tx-ro");
    h.app.policies.insert("default", datarig_core::policy::Policy { read_only: true, ..Default::default() });
    in_transaction(&mut h);
    open(&mut h);
    let s = screen(&mut h);
    assert!(s.contains("inside your open transaction"), "{s}");
    assert!(s.contains("Asks first: default is a read-only or prod-like environment."), "{s}");
    let (mut h, _home) = with_home("tx-rerun");
    h.app.tab_mut().editor = Editor::new("SELECT random() AS id, 'x' AS name FROM generate_series(1, 5000)");
    in_transaction(&mut h);
    open(&mut h);
    let s = screen(&mut h);
    assert!(
        s.contains("inside your open transaction") && s.contains("Asks first: the statement is not a plain read"),
        "{s}"
    );
}

#[test]
fn every_row_in_a_read_only_environment_asks_and_enter_sends_nothing() {
    let (mut h, _home) = with_home("ro-submit");
    h.app.policies.insert("default", datarig_core::policy::Policy { read_only: true, ..Default::default() });
    first_page(&mut h, 5, true);
    open(&mut h);
    tab_to(&mut h, ExportField::Export);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(datarig_tui::app::overlay::ConfirmAction::ExportStart));
    assert!(h.screen(220, 50).contains("Environment: default"));
    h.key(KeyCode::Enter);
    assert!(!h.app.exporting(h.app.tab().id));
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::Export { .. })), "Enter keeps it from running");
}

#[test]
fn a_statement_off_the_allowlist_asks_before_it_runs_again() {
    let (mut h, _home) = with_home("rerun-submit");
    h.app.tab_mut().editor = Editor::new("SELECT random() AS id, 'x' AS name FROM generate_series(1, 5000)");
    first_page(&mut h, 5, true);
    open(&mut h);
    tab_to(&mut h, ExportField::Path);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(datarig_tui::app::overlay::ConfirmAction::ExportStart));
    let s = h.screen(220, 50);
    assert!(s.contains("runs the statement again") && s.contains("random"), "{s}");
    h.key(KeyCode::Enter);
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::Export { .. })));
}

#[test]
fn a_file_in_the_way_asks_and_enter_keeps_it() {
    let (mut h, home) = with_home("exists");
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    first_page(&mut h, 5, true);
    open(&mut h);
    h.key(KeyCode::Right);
    let made = path_text(&h);
    let file = home.join("Downloads").join(made.trim_start_matches("~/Downloads/"));
    std::fs::write(&file, "keep").unwrap();
    tab_to(&mut h, ExportField::Export);
    h.key(KeyCode::Enter);
    let start = std::time::Instant::now();
    while h.app.overlays.confirm().is_none() {
        assert!(start.elapsed() < WAIT, "no question");
        h.app.pump_exports(Duration::from_millis(20));
    }
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(datarig_tui::app::overlay::ConfirmAction::ExportReplace)
    );
    // Drawn first: the question is armed only once it was on screen.
    h.draw(160, 45);
    h.advance(Duration::from_millis(600));
    h.key(KeyCode::Enter);
    pump_until_done(&mut h);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
    assert!(status(&h).contains("the file is kept"), "{}", status(&h));
}

#[test]
fn a_release_on_export_starts_the_export() {
    let (mut h, home) = with_home("mouse-export");
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    first_page(&mut h, 5, false);
    open(&mut h);
    let made = path_text(&h);
    h.draw(160, 45);
    h.advance(Duration::from_millis(500));
    let export = h.app.overlays.export_form().unwrap().buttons.rects[0];
    h.mouse(MouseEventKind::Down(MouseButton::Left), export.x + 1, export.y);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    h.mouse(MouseEventKind::Up(MouseButton::Left), export.x + 1, export.y);
    assert_eq!(h.overlay_kind(), None);
    pump_until_done(&mut h);
    let file = home.join("Downloads").join(made.trim_start_matches("~/Downloads/"));
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 6, "a header and 5 rows");
}

#[test]
fn an_error_of_a_field_the_format_hides_goes_with_it() {
    let (mut h, _home) = with_home("hidden-error");
    first_page(&mut h, 5, false);
    open(&mut h);
    tab_to(&mut h, ExportField::Delimiter);
    h.ctrl('u');
    h.type_text("ab");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.export_form().unwrap().error.is_some());
    tab_to(&mut h, ExportField::Format);
    h.key(KeyCode::Right);
    h.key(KeyCode::Left);
    assert!(h.app.overlays.export_form().unwrap().error.is_none(), "not said again on the way back");
}

#[test]
fn a_file_name_that_only_ends_like_the_made_one_keeps_its_extension() {
    let (mut h, _home) = with_home("suffix");
    first_page(&mut h, 5, false);
    open(&mut h);
    let made = path_text(&h);
    let name = made.trim_start_matches("~/Downloads/").to_string();
    tab_to(&mut h, ExportField::Path);
    h.ctrl('u');
    h.type_text(&format!("~/Downloads/my{name}"));
    tab_to(&mut h, ExportField::Format);
    h.key(KeyCode::Right);
    assert_eq!(path_text(&h), format!("~/Downloads/my{name}"));
}
