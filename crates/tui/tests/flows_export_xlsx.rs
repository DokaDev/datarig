//! Exports to an Excel workbook (`.xlsx`) through the real `App` event path, with the fake
//! driver and real files in a scratch folder, read back with a workbook reader: the cells'
//! types, NULL, a cancel (the workbook of the rows so far in `<file>.partial`), a value too long
//! for a cell (the export fails naming its column, the rows before it kept) and a result wider
//! than a sheet (refused before anything is written). The sheets past 1,048,576 rows are the core
//! writer's tests (`export::xlsx`).

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ExportBatch, ExportGuard, ExportSink};
use datarig_core::export::xlsx::read::{self, Cell as Data};
use datarig_core::i18n::Lang;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// A new empty folder for a test's files.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-export-xlsx-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn columns() -> Vec<ColumnMeta> {
    vec![
        meta("id", "int8", true, false),
        meta("name", "text", false, false),
        meta("price", "numeric", true, false),
        meta("ok", "bool", false, false),
        meta("doc", "jsonb", false, true),
        meta("born", "date", false, false),
    ]
}

/// Row `i`: every seventh's name and price are NULL; the price of every fifth has more digits than
/// Excel keeps.
fn row(i: usize) -> Vec<Option<String>> {
    let name = (!i.is_multiple_of(7)).then(|| format!("n\u{d55c}\u{ae00}{i}"));
    let price = match i {
        i if i.is_multiple_of(7) => None,
        i if i.is_multiple_of(5) => Some(format!("{i}.0000000000000001")),
        i => Some(format!("{i}.50")),
    };
    vec![
        Some(i.to_string()),
        name,
        price,
        Some(if i.is_multiple_of(2) { "true" } else { "false" }.to_string()),
        Some(format!("{{\"n\": {i}}}")),
        Some("2024-01-02".to_string()),
    ]
}

fn rows(from: usize, n: usize) -> Vec<Vec<Option<String>>> {
    (from..from + n).map(row).collect()
}

/// The cells row `i` reads back as.
fn cells(i: usize) -> Vec<Data> {
    let r = row(i);
    let price = match &r[2] {
        None => Data::Empty,
        Some(p) if i.is_multiple_of(5) => Data::Text(p.clone()),
        Some(_) => Data::Number(i as f64 + 0.5),
    };
    vec![
        Data::Number(i as f64),
        r[1].clone().map_or(Data::Empty, Data::Text),
        price,
        Data::Bool(i.is_multiple_of(2)),
        Data::Text(format!("{{\"n\": {i}}}")),
        Data::Text("2024-01-02".into()),
    ]
}

fn header() -> Vec<Data> {
    columns().into_iter().map(|c| Data::Text(c.name)).collect()
}

/// Run the statement under the cursor and answer with a first page of `page` (`more`: the server
/// has more).
fn first_page(h: &mut Harness, columns: Vec<ColumnMeta>, page: Vec<Vec<Option<String>>>, more: bool) {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows: page, more, elapsed: Duration::ZERO });
}

/// Hand the writer's events in until the export ended (or fail after [`WAIT`]).
fn pump_until_done(h: &mut Harness) {
    let start = std::time::Instant::now();
    while h.app.exporting(h.app.tab().id) {
        assert!(start.elapsed() < WAIT, "waited for the end: {}", status(h));
        h.app.pump_exports(Duration::from_millis(50));
    }
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// Wait for the export's statement (its file opened first).
fn wait_export(h: &mut Harness) -> (u64, ExportGuard, ExportSink) {
    let start = std::time::Instant::now();
    loop {
        let sent = h.sent().into_iter().find_map(|c| match c {
            DbCommand::Export { id, guard, sink, .. } => Some((id, guard, sink)),
            _ => None,
        });
        if let Some(e) = sent {
            return e;
        }
        assert!(start.elapsed() < WAIT, "no export was sent: {}", status(h));
        h.app.pump_exports(Duration::from_millis(50));
    }
}

/// Every sheet of the workbook at `path`: its name and its cells, row by row.
fn read(path: &Path) -> Vec<(String, Vec<Vec<Data>>)> {
    read::sheets(&std::fs::read(path).unwrap()).expect("a workbook")
}

#[test]
fn the_rows_fetched_go_to_a_workbook_with_their_types() {
    let dir = scratch("fetched");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 300), true);
    h.sent();
    let path = dir.join("users.xlsx");
    h.command(&format!("export fetched {}", path.display()));
    pump_until_done(&mut h);
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::Export { .. })), "nothing runs on the server");
    assert!(!dir.join("users.xlsx.partial").exists());
    let book = read(&path);
    assert_eq!(book.len(), 1);
    assert_eq!(book[0].0, "Sheet1");
    let got = &book[0].1;
    assert_eq!(got.len(), 301);
    assert_eq!(got[0], header());
    for i in 0..300 {
        assert_eq!(got[i + 1], cells(i), "row {i}");
    }
    let s = status(&h);
    assert!(s.starts_with("Exported 300 rows to ") && s.contains("users.xlsx"), "{s}");
    assert!(!s.contains("sheets"), "{s}");
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event, nothing after it");
}

#[test]
fn the_null_text_option_writes_null_as_that_text() {
    let dir = scratch("null");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 8), false);
    let path = dir.join("n.xlsx");
    h.command(&format!("export fetched null=NULL {}", path.display()));
    pump_until_done(&mut h);
    assert!(path.exists(), "{}", status(&h));
    let got = &read(&path)[0].1;
    assert_eq!(got[1][1], Data::Text("NULL".into()));
    assert_eq!(got[1][2], Data::Text("NULL".into()));
    assert_eq!(got[2][1], Data::Text("n\u{d55c}\u{ae00}1".into()));
}

#[test]
fn a_cancel_keeps_the_workbook_of_the_rows_so_far_in_the_partial_file() {
    let dir = scratch("cancel");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 100), true);
    h.sent();
    let path = dir.join("part.xlsx");
    h.command(&format!("export all {}", path.display()));
    let (id, _, sink) = wait_export(&mut h);
    let feeder = std::thread::spawn(move || {
        sink.blocking_send(ExportBatch::Columns(columns())).is_ok()
            && sink.blocking_send(ExportBatch::Rows(rows(0, 50))).is_ok()
    });
    assert!(feeder.join().unwrap());
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the session's statement is cancelled");
    h.tab_db(0, DbEvent::Exported { id, result: Err(DbError::Cancelled), cancelled: true });
    pump_until_done(&mut h);
    assert!(!path.exists(), "nothing at the path");
    let partial = dir.join("part.xlsx.partial");
    let got = &read(&partial)[0].1;
    assert_eq!(got.len(), 51, "a workbook of the rows written, with its header");
    assert_eq!(got[50], cells(49));
    let s = status(&h);
    assert!(s.starts_with("Export cancelled after 50 rows; they are in ") && s.contains("part.xlsx.partial"), "{s}");
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event");
}

#[test]
fn a_value_too_long_for_a_cell_fails_naming_its_column_and_keeps_the_rows_before() {
    let dir = scratch("long");
    let mut h = Harness::connected(Lang::En);
    let mut page = rows(0, 10);
    page[4][1] = Some("x".repeat(32_768));
    first_page(&mut h, columns(), page, false);
    let path = dir.join("long.xlsx");
    h.command(&format!("export fetched {}", path.display()));
    pump_until_done(&mut h);
    assert!(!path.exists());
    let s = status(&h);
    assert!(
        s.starts_with("Export failed after 4 rows: a value of column name (row 5) has 32768 characters, more than an Excel cell holds (32,767)"),
        "{s}"
    );
    assert!(s.contains("long.xlsx.partial"), "{s}");
    let got = &read(&dir.join("long.xlsx.partial"))[0].1;
    assert_eq!(got.len(), 5);
    assert_eq!(got[4], cells(3));
}

#[test]
fn a_result_wider_than_a_sheet_is_refused_before_anything_is_written() {
    let dir = scratch("wide");
    let mut h = Harness::connected(Lang::En);
    let wide: Vec<ColumnMeta> = (0..16_385).map(|i| meta(&format!("c{i}"), "text", false, false)).collect();
    first_page(&mut h, wide, vec![vec![Some("x".to_string()); 16_385]], false);
    let path = dir.join("wide.xlsx");
    h.command(&format!("export fetched {}", path.display()));
    pump_until_done(&mut h);
    assert!(!path.exists() && !dir.join("wide.xlsx.partial").exists(), "nothing written");
    let s = h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert_eq!(
        s,
        "Not exported: an Excel sheet holds 16,384 columns, and the result has 16,385; export fewer columns, or another format"
    );
}

#[test]
fn a_workbook_takes_the_null_text_and_no_other_csv_option() {
    let dir = scratch("options");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 3), false);
    let path = dir.join("o.xlsx");
    h.command(&format!("export fetched delim=; {}", path.display()));
    let err = h.cmdline().and_then(|c| c.error.clone()).map(|n| n.render(&h.app.i18n).to_string());
    assert!(err.as_deref().is_some_and(|e| e.contains("Excel null= only")), "{err:?}");
    assert!(!path.exists());
}
