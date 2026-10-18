//! Exports to a file through the real `App` event path, with the fake driver and real files in
//! a scratch folder: the writer runs on its own thread, and its events are handed in with
//! `App::pump_exports` (headless). Every row of a result the server has more of runs its
//! statement again (`DbCommand::Export`, the test plays the session: it sends the batches into
//! the export's channel and answers `Exported`); the rows fetched and a selection are read from
//! the result. Each export ends with one message; a cancel or a failure keeps the rows written
//! so far in `<file>.partial`.

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ExportBatch, ExportGuard, ExportRead, ExportSink};
use datarig_core::i18n::Lang;
use datarig_core::policy::Policy;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::tabs::TableRef;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// A new empty folder for a test's files.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-export-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rows(from: usize, n: usize) -> Vec<Vec<Option<String>>> {
    (from..from + n)
        .map(|i| vec![Some(i.to_string()), if i % 7 == 0 { None } else { Some(format!("n\u{d55c}{i}, \"q\"")) }])
        .collect()
}

fn columns() -> Vec<ColumnMeta> {
    vec![meta("id", "int8", true, false), meta("name", "text", false, false)]
}

/// Run the statement under the cursor and answer with a first page of `n` rows (`more`: the
/// server has more).
fn first_page(h: &mut Harness, n: usize, more: bool) -> u64 {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns()), rows: rows(0, n), more, elapsed: Duration::ZERO });
    id
}

/// Hand the writer's events in until `done` holds (or fail after [`WAIT`]).
fn pump_until(h: &mut Harness, what: &str, done: impl Fn(&Harness) -> bool) {
    let start = std::time::Instant::now();
    while !done(h) {
        assert!(
            start.elapsed() < WAIT,
            "waited for {what}: {}",
            h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
        );
        h.app.pump_exports(Duration::from_millis(50));
    }
}

/// The tab's status, as text.
fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The export command sent to the tab's session.
fn export_sent(h: &mut Harness) -> Option<(u64, String, ExportGuard, ExportSink)> {
    h.sent().into_iter().find_map(|c| match c {
        DbCommand::Export { id, sql, guard, sink, .. } => Some((id, sql, guard, sink)),
        _ => None,
    })
}

/// Wait for the export's statement (its file opened first).
fn wait_export(h: &mut Harness) -> (u64, String, ExportGuard, ExportSink) {
    let start = std::time::Instant::now();
    loop {
        if let Some(e) = export_sent(h) {
            return e;
        }
        assert!(start.elapsed() < WAIT, "no export was sent: {}", status(h));
        h.app.pump_exports(Duration::from_millis(50));
    }
}

/// Play the session: the columns and `rows` into `sink`, from a thread (the channel holds two
/// batches and the writer drains it).
fn stream(sink: ExportSink, batches: Vec<Vec<Vec<Option<String>>>>) -> std::thread::JoinHandle<bool> {
    std::thread::spawn(move || {
        if sink.blocking_send(ExportBatch::Columns(columns())).is_err() {
            return false;
        }
        for b in batches {
            if sink.blocking_send(ExportBatch::Rows(b)).is_err() {
                return false;
            }
        }
        true
    })
}

fn done(h: &Harness) -> bool {
    !h.app.exporting(h.app.tab().id)
}

#[test]
fn the_rows_fetched_go_to_a_csv_file_without_the_server() {
    let dir = scratch("fetched");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 300, true);
    h.sent();
    let path = dir.join("users.csv");
    h.command(&format!("export fetched {}", path.display()));
    pump_until(&mut h, "the end", done);
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::Export { .. })), "nothing runs on the server");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut want = String::from("id,name\n");
    for r in rows(0, 300) {
        let name = r[1].as_ref().map(|n| format!("\"{}\"", n.replace('"', "\"\""))).unwrap_or_default();
        want.push_str(&format!("{},{name}\n", r[0].as_ref().unwrap()));
    }
    assert_eq!(text, want);
    assert!(!dir.join("users.csv.partial").exists());
    let s = status(&h);
    assert!(s.starts_with("Exported 300 rows to ") && s.contains("users.csv"), "{s}");
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event, nothing after it");
}

#[test]
fn every_row_streams_from_the_session_into_the_file() {
    let dir = scratch("all");
    let mut h = Harness::connected(Lang::En);
    let qid = first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("all.json");
    h.command(&format!("export {}", path.display()));
    let (id, sql, guard, sink) = wait_export(&mut h);
    assert_eq!((id, guard), (qid, ExportGuard::Repeatable), "on the allowlist: not asked");
    assert_eq!(sql, "SELECT * FROM shop.users WHERE id <= 8");
    assert!(h.app.tab_busy(h.app.tab().id), "the tab is busy while its session exports");
    // A run is refused meanwhile.
    h.ctrl('e');
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::Execute { .. })));
    let feeder = stream(sink, vec![rows(0, 2_000), rows(2_000, 2_000), rows(4_000, 1_234)]);
    assert!(feeder.join().unwrap());
    h.tab_db(
        0,
        DbEvent::Exported { id, result: Ok(ExportRead { rows: 5_234, last: None, complete: true }), cancelled: false },
    );
    assert!(h.app.tab().exec.running.is_none());
    pump_until(&mut h, "the end", done);
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let items = v.as_array().unwrap();
    assert_eq!(items.len(), 5_234);
    assert_eq!(items[5_233]["id"], serde_json::json!(5233));
    assert_eq!(items[1]["name"], serde_json::json!("n\u{d55c}1, \"q\""));
    assert!(items[7]["name"].is_null());
    assert!(status(&h).starts_with("Exported 5,234 rows to "), "{}", status(&h));
}

#[test]
fn a_cancel_keeps_the_rows_so_far_in_a_well_formed_partial_file() {
    let dir = scratch("cancel");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("part.json");
    h.command(&format!("export all {}", path.display()));
    let (id, _, _, sink) = wait_export(&mut h);
    let feeder = stream(sink, vec![rows(0, 50)]);
    assert!(feeder.join().unwrap());
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the session's statement is cancelled");
    h.tab_db(0, DbEvent::Exported { id, result: Err(DbError::Cancelled), cancelled: true });
    pump_until(&mut h, "the end", done);
    assert!(!path.exists(), "nothing at the path");
    let partial = dir.join("part.json.partial");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&partial).unwrap()).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 50, "a closed JSON array of the rows written");
    let s = status(&h);
    assert!(s.starts_with("Export cancelled after 50 rows; they are in ") && s.contains("part.json.partial"), "{s}");
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event");
    // A partial file in the way stops the next export before anything runs.
    h.command(&format!("export all {}", path.display()));
    pump_until(&mut h, "the end", done);
    assert!(export_sent(&mut h).is_none());
    assert!(status(&h).contains("in the way"), "{}", status(&h));
}

#[test]
fn a_file_at_the_path_asks_and_enter_keeps_it() {
    let dir = scratch("exists");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 10, false);
    h.sent();
    let path = dir.join("keep.csv");
    std::fs::write(&path, "mine\n").unwrap();
    h.command(&format!("export {}", path.display()));
    pump_until(&mut h, "the question", |h| h.overlay_kind() == Some(OverlayKind::Confirm));
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::ExportReplace));
    // It opened by itself: keys typed as it came up do nothing.
    h.key(KeyCode::Char('y'));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "not armed yet");
    h.draw(160, 45);
    h.advance(Duration::from_secs(1));
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n", "Enter keeps the file");
    assert!(status(&h).contains("the file is kept"), "{}", status(&h));
    assert!(!h.app.exporting(h.app.tab().id));
    // `y` replaces it.
    h.command(&format!("export {}", path.display()));
    pump_until(&mut h, "the question", |h| h.overlay_kind() == Some(OverlayKind::Confirm));
    h.draw(160, 45);
    h.advance(Duration::from_secs(1));
    h.key(KeyCode::Char('y'));
    pump_until(&mut h, "the end", done);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("id,name\n0,") && text.lines().count() == 11, "{text}");
}

#[test]
fn a_statement_off_the_allowlist_asks_and_says_it_runs_again() {
    let dir = scratch("rerun");
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT random() AS id, 'x' AS name FROM generate_series(1, 5000)");
    first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("r.csv");
    h.command(&format!("export {}", path.display()));
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::ExportStart));
    let screen = h.screen(200, 50);
    assert!(screen.contains("Export every row?"), "{screen}");
    assert!(screen.contains("runs the statement again") && screen.contains("random"), "{screen}");
    // Enter cancels.
    h.key(KeyCode::Enter);
    assert!(export_sent(&mut h).is_none());
    assert_eq!(status(&h), "Export not started");
    assert!(!dir.join("r.csv.partial").exists() && !path.exists());
    // `y` runs it, guarded as confirmed.
    h.command(&format!("export {}", path.display()));
    h.key(KeyCode::Char('y'));
    let (id, _, guard, sink) = wait_export(&mut h);
    assert_eq!(guard, ExportGuard::Confirmed);
    stream(sink, vec![rows(0, 3)]).join().unwrap();
    h.tab_db(
        0,
        DbEvent::Exported { id, result: Ok(ExportRead { rows: 3, last: None, complete: true }), cancelled: false },
    );
    pump_until(&mut h, "the end", done);
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 4);
}

#[test]
fn a_read_only_environment_asks_with_the_lock_and_then_exports() {
    let dir = scratch("read-only");
    let mut h = Harness::connected(Lang::En);
    h.app.policies.insert("default", Policy { read_only: true, ..Policy::default() });
    first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("ro.csv");
    h.command(&format!("export {}", path.display()));
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::ExportStart));
    let screen = h.screen(220, 50);
    assert!(screen.contains("read lock") && screen.contains("Environment: default"), "{screen}");
    h.key(KeyCode::Char('y'));
    let (id, _, guard, sink) = wait_export(&mut h);
    assert_eq!(guard, ExportGuard::Repeatable);
    stream(sink, vec![rows(0, 5)]).join().unwrap();
    h.tab_db(
        0,
        DbEvent::Exported { id, result: Ok(ExportRead { rows: 5, last: None, complete: true }), cancelled: false },
    );
    pump_until(&mut h, "the end", done);
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 6);
}

#[test]
fn inside_the_users_transaction_it_reads_there_and_says_so() {
    let dir = scratch("in-tx");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 100, true);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.sent();
    let path = dir.join("tx.csv");
    h.command(&format!("export {}", path.display()));
    let (id, _, _, sink) = wait_export(&mut h);
    let s = status(&h);
    assert!(s.contains("inside your open transaction"), "{s}");
    stream(sink, vec![rows(0, 1)]).join().unwrap();
    h.tab_db(
        0,
        DbEvent::Exported { id, result: Ok(ExportRead { rows: 1, last: None, complete: true }), cancelled: false },
    );
    pump_until(&mut h, "the end", done);
    assert!(path.exists());
    // An aborted transaction refuses it before anything runs.
    h.tab_db(0, DbEvent::TxAborted(true));
    h.command(&format!("export {}", dir.join("no.csv").display()));
    assert!(status(&h).contains("aborted"), "{}", status(&h));
    assert!(!h.app.exporting(h.app.tab().id));
}

#[test]
fn a_lost_session_ends_the_export_with_the_rows_so_far() {
    let dir = scratch("lost");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("lost.md");
    h.command(&format!("export {}", path.display()));
    let (_, _, _, sink) = wait_export(&mut h);
    stream(sink, vec![rows(0, 20)]).join().unwrap();
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    pump_until(&mut h, "the end", done);
    let s = status(&h);
    assert!(s.starts_with("Export failed after 20 rows: the tab's session ended"), "{s}");
    let text = std::fs::read_to_string(dir.join("lost.md.partial")).unwrap();
    assert_eq!(text.lines().count(), 22, "the Markdown header and the rows so far");
}

#[test]
fn a_failed_statement_says_why_and_a_late_answer_changes_nothing() {
    let dir = scratch("failed");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("f.xml");
    h.command(&format!("export {}", path.display()));
    let (id, _, _, sink) = wait_export(&mut h);
    drop(sink);
    // Another id is not this export's answer.
    h.tab_db(
        0,
        DbEvent::Exported {
            id: id + 5,
            result: Ok(ExportRead { rows: 0, last: None, complete: true }),
            cancelled: false,
        },
    );
    assert!(h.app.tab().exec.running.is_some());
    h.tab_db(
        0,
        DbEvent::Exported { id, result: Err(DbError::Server("ERROR: permission denied".into())), cancelled: false },
    );
    pump_until(&mut h, "the end", done);
    assert_eq!(status(&h), "Export failed: ERROR: permission denied");
    assert!(!dir.join("f.xml.partial").exists(), "no row: nothing kept");
}

#[test]
fn a_table_tab_is_read_in_chunks_by_its_key() {
    let dir = scratch("keyset");
    let mut h = Harness::connected(Lang::En);
    let table = TableRef { schema: "shop".into(), name: "users".into() };
    h.app.tab_mut().editor = Editor::new("SELECT * FROM \"shop\".\"users\"");
    h.app.tab_mut().doc.table = Some(table);
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
            columns: Some(cols.clone()),
            rows: (0..10).map(row).collect(),
            more: true,
            elapsed: Duration::ZERO,
        },
    );
    h.sent();
    let path = dir.join("users.jsonl");
    h.command(&format!("export {}", path.display()));
    // The table's key comes from its structure, asked first.
    let asked = h.sent();
    assert!(
        asked
            .iter()
            .any(|c| matches!(c, DbCommand::LoadStructure { schema, table } if schema == "shop" && table == "users")),
        "{asked:?}"
    );
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    assert_eq!(h.overlay_kind(), None, "a keyset export is not asked");
    let (id, sql, guard, sink) = wait_export(&mut h);
    assert_eq!(guard, ExportGuard::Keyset);
    assert_eq!(sql, "SELECT * FROM \"shop\".\"users\" ORDER BY \"id\" LIMIT 10000");
    let s = status(&h);
    assert!(s.contains("shop.users is read in chunks by its key"), "{s}");
    // A full chunk: the next one starts after its last key.
    let send = |sink: ExportSink, from: usize, n: usize| {
        let batch: Vec<Vec<Option<String>>> = (from..from + n).map(row).collect();
        std::thread::spawn(move || {
            sink.blocking_send(ExportBatch::Columns(
                ["id", "email", "name", "nickname", "profile"].iter().map(|n| meta(n, "text", false, false)).collect(),
            ))
            .unwrap();
            for c in batch.chunks(2_000) {
                sink.blocking_send(ExportBatch::Rows(c.to_vec())).unwrap();
            }
        })
        .join()
        .unwrap();
    };
    send(sink, 0, 10_000);
    let last = Some(row(9_999));
    h.tab_db(
        0,
        DbEvent::Exported { id, result: Ok(ExportRead { rows: 10_000, last, complete: true }), cancelled: false },
    );
    let (id2, sql2, guard2, sink2) = export_sent(&mut h).expect("the next chunk");
    assert_eq!((id2, guard2), (id, ExportGuard::Keyset));
    assert_eq!(sql2, "SELECT * FROM \"shop\".\"users\" WHERE \"id\" > 9999 ORDER BY \"id\" LIMIT 10000");
    assert!(h.app.tab_busy(h.app.tab().id));
    send(sink2, 10_000, 1_500);
    h.tab_db(
        0,
        DbEvent::Exported {
            id,
            result: Ok(ExportRead { rows: 1_500, last: Some(row(11_499)), complete: true }),
            cancelled: false,
        },
    );
    assert!(export_sent(&mut h).is_none(), "a short chunk is the last");
    pump_until(&mut h, "the end", done);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 11_500);
    let ids: Vec<u64> =
        text.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["id"].as_u64().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[1] == w[0] + 1), "every row once, in key order");
}

#[test]
fn the_default_path_is_in_downloads_under_home_or_home_itself() {
    let home = scratch("home");
    let mut h = Harness::connected(Lang::En);
    let env_home = home.display().to_string();
    h.app.set_env_lookup(std::sync::Arc::new(move |k: &str| (k == "HOME").then(|| env_home.clone())));
    first_page(&mut h, 3, false);
    h.sent();
    // No Downloads folder: the home folder.
    h.command("export fetched");
    pump_until(&mut h, "the end", done);
    let files = |dir: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".csv"))
            .collect();
        v.sort();
        v
    };
    let at_home = files(&home);
    assert_eq!(at_home.len(), 1, "{at_home:?}");
    let name = &at_home[0];
    // `console 1-YYYYMMDD-HHMMSS.csv`
    assert!(name.starts_with("console 1-") && name.len() == "console 1-".len() + 15 + 4, "{name}");
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    h.command("export json_pretty fetched");
    pump_until(&mut h, "the end", done);
    // `json_pretty` names the format; the path is the default (a `.json` file).
    let downloads: Vec<String> = std::fs::read_dir(home.join("Downloads"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(downloads.len() == 1 && downloads[0].ends_with(".json"), "{downloads:?}");
}

#[test]
fn the_command_says_what_it_does_not_understand() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 3, false);
    h.command("export parquet");
    let err = h.cmdline().and_then(|c| c.error.clone()).map(|n| n.render(&h.app.i18n).to_string());
    assert!(err.as_deref().is_some_and(|e| e.starts_with("Unknown export format: parquet")), "{err:?}");
    h.key(KeyCode::Esc);
    h.command("export all out.txt");
    let err = h.cmdline().and_then(|c| c.error.clone()).map(|n| n.render(&h.app.i18n).to_string());
    assert!(err.as_deref().is_some_and(|e| e.contains("Which format is")), "{err:?}");
}

#[test]
fn a_selection_exports_its_range_only() {
    let dir = scratch("selection");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 50, true);
    h.key(KeyCode::Tab);
    // Rows 1–3 of the first column.
    h.keys("jvjj");
    let path = dir.join("sel.tsv");
    h.command(&format!("export selection {}", path.display()));
    pump_until(&mut h, "the end", done);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "id\n1\n2\n3\n");
}

#[test]
fn sql_statements_follow_the_rules_of_a_copy() {
    let dir = scratch("sql");
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let cols = vec![meta_from("id", "int8", true, USERS, 1), meta_from("name", "text", false, USERS, 3)];
    let rows = vec![vec![Some("1".into()), Some("O'Neil".into())], vec![Some("2".into()), None]];
    h.tab_db(0, DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::ZERO });
    let insert = dir.join("u.sql");
    h.command(&format!("export {}", insert.display()));
    pump_until(&mut h, "the end", done);
    assert_eq!(
        std::fs::read_to_string(&insert).unwrap(),
        "INSERT INTO \"shop\".\"users\" (\"id\", \"name\") VALUES (1, 'O''Neil');\n\
         INSERT INTO \"shop\".\"users\" (\"id\", \"name\") VALUES (2, NULL);\n"
    );
    let update = dir.join("u2.sql");
    h.command(&format!("export update {}", update.display()));
    pump_until(&mut h, "the end", done);
    assert_eq!(
        std::fs::read_to_string(&update).unwrap(),
        "UPDATE \"shop\".\"users\" SET \"name\" = 'O''Neil' WHERE \"id\" = 1;\n\
         UPDATE \"shop\".\"users\" SET \"name\" = NULL WHERE \"id\" = 2;\n"
    );
    // Without the key in the result, UPDATE is refused with the copy's reason.
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let cols = vec![meta_from("name", "text", false, USERS, 3)];
    h.tab_db(
        0,
        DbEvent::Page {
            id,
            columns: Some(cols),
            rows: vec![vec![Some("x".into())]],
            more: false,
            elapsed: Duration::ZERO,
        },
    );
    let refused = dir.join("u3.sql");
    h.command(&format!("export update {}", refused.display()));
    assert!(!h.app.exporting(h.app.tab().id));
    let flash = h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(flash.contains("primary key"), "{flash}");
    assert!(!refused.exists());
}

#[test]
fn a_cancel_while_the_file_opens_asks_nothing_more() {
    let dir = scratch("cancel-early");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 10, false);
    h.sent();
    let path = dir.join("there.csv");
    std::fs::write(&path, "mine\n").unwrap();
    h.command(&format!("export {}", path.display()));
    // Cancelled before the writer said anything: the file in the way asks nothing.
    h.ctrl('c');
    pump_until(&mut h, "the end", done);
    assert_eq!(h.overlay_kind(), None);
    assert!(status(&h).starts_with("Export cancelled before any row"), "{}", status(&h));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
}

#[test]
fn a_session_that_never_closes_its_channel_does_not_keep_the_writer() {
    let dir = scratch("wedged");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 100, true);
    h.sent();
    let path = dir.join("w.csv");
    h.command(&format!("export {}", path.display()));
    let (_, _, _, sink) = wait_export(&mut h);
    sink.blocking_send(ExportBatch::Columns(columns())).unwrap();
    sink.blocking_send(ExportBatch::Rows(rows(0, 3))).unwrap();
    // The session is lost; its end of the channel stays open (a wedged driver).
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    pump_until(&mut h, "the end", done);
    assert!(status(&h).starts_with("Export failed after 3 rows: the tab's session ended"), "{}", status(&h));
    assert_eq!(std::fs::read_to_string(dir.join("w.csv.partial")).unwrap().lines().count(), 4);
    drop(sink);
}

#[test]
fn a_question_that_comes_later_is_not_asked_over_another() {
    use datarig_core::policy::Confirm;
    let dir = scratch("two-questions");
    let mut h = Harness::connected(Lang::En);
    // Every write asks: a streaming export asks too.
    h.app.policies.insert("default", Policy { confirm: Confirm::Writes, ..Policy::default() });
    // Tab 1, a table tab: its export waits for the table's structure.
    h.app.tab_mut().editor = Editor::new("SELECT * FROM \"shop\".\"users\"");
    h.app.tab_mut().doc.table = Some(TableRef { schema: "shop".into(), name: "users".into() });
    first_page(&mut h, 10, true);
    h.command(&format!("export {}", dir.join("a.csv").display()));
    let first = h.app.tab().id;
    // Tab 2: a file in the way asks to replace it.
    h.ctrl('t');
    h.app.tab_mut().editor = Editor::new("SELECT 1");
    first_page(&mut h, 3, false);
    let there = dir.join("b.csv");
    std::fs::write(&there, "mine\n").unwrap();
    h.command(&format!("export {}", there.display()));
    pump_until(&mut h, "the question", |h| h.overlay_kind() == Some(OverlayKind::Confirm));
    // The structure comes: tab 1's question would open over tab 2's.
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Err(DbError::Locked) },
    );
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::ExportReplace), "still tab 2's");
    assert!(!h.app.exporting(first));
    let t1 = h.app.tabs.get(first).and_then(|t| t.status.as_ref()).map(|n| n.render(&h.app.i18n).to_string());
    assert!(t1.as_deref().is_some_and(|s| s.contains("another question is open")), "{t1:?}");
}
