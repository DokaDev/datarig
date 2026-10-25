//! Exports to an Excel workbook (`.xlsx`) through the real `App` event path, with the fake
//! driver and real files in a scratch folder, read back with a workbook reader: the cells'
//! types, NULL, a cancel (the workbook of the rows so far in `<file>.partial`), a value too long
//! for a cell (the export fails naming its column, the rows before it kept) and a result wider
//! than a sheet (refused before anything is written), and a cancel while the workbook is made
//! (dropped: nothing kept). The sheets past 1,048,576 rows are the core writer's tests
//! (`export::xlsx`); their message, and those of a workbook that could not be made, are said
//! from the writer's terminal event handed in.

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ExportBatch, ExportGuard, ExportSink};
use datarig_core::export::xlsx::TooLong;
use datarig_core::export::xlsx::read::{self, Cell as Data};
use datarig_core::fault::Fault;
use datarig_core::i18n::Lang;
use datarig_tui::app::AppEvent;
use datarig_tui::app::export::{ExportEvent, ExportNews, Failure, Outcome};
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

#[test]
fn a_value_too_long_in_the_first_row_says_no_row_was_written() {
    let dir = scratch("long-first");
    let mut h = Harness::connected(Lang::En);
    let mut page = rows(0, 3);
    page[0][1] = Some("x".repeat(32_768));
    first_page(&mut h, columns(), page, false);
    let path = dir.join("first.xlsx");
    h.command(&format!("export fetched {}", path.display()));
    pump_until_done(&mut h);
    assert!(!path.exists() && !dir.join("first.xlsx.partial").exists(), "nothing kept");
    assert_eq!(
        status(&h),
        "Export failed: a value of column name in the first row has 32768 characters, more than an Excel cell holds (32,767); no row was written"
    );
}

/// Rows whose text column holds `width` letters that hardly compress: enough for the workbook to
/// take a while to be made (20,000 rows of 2,000: more than ten seconds in a debug build, far
/// longer than the app takes to see the status and hand the second cancel over).
fn wide_rows(from: usize, n: usize, width: usize) -> Vec<Vec<Option<String>>> {
    let mut seed = from as u64 + 1;
    (from..from + n)
        .map(|i| {
            let mut r = row(i);
            let text = (0..width)
                .map(|_| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                    char::from(b'a' + (seed >> 59) as u8 % 26)
                })
                .collect();
            r[1] = Some(text);
            r
        })
        .collect()
}

#[test]
fn a_cancel_while_the_workbook_is_made_drops_it_and_keeps_nothing() {
    let dir = scratch("drop");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 100), true);
    h.sent();
    let path = dir.join("big.xlsx");
    h.command(&format!("export all {}", path.display()));
    let (id, _, sink) = wait_export(&mut h);
    let feeder = std::thread::spawn(move || {
        sink.blocking_send(ExportBatch::Columns(columns())).is_ok()
            && (0..20).all(|b| sink.blocking_send(ExportBatch::Rows(wide_rows(b * 1_000, 1_000, 2_000))).is_ok())
    });
    assert!(feeder.join().unwrap());
    // The first cancel stops the rows: the workbook of those is made, and the app says so.
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the session's statement is cancelled");
    h.tab_db(0, DbEvent::Exported { id, result: Err(DbError::Cancelled), cancelled: true });
    let start = std::time::Instant::now();
    let making = "Making the workbook big.xlsx of ";
    while !status(&h).starts_with(making) {
        assert!(h.app.exporting(h.app.tab().id), "ended before the workbook was made: {}", status(&h));
        assert!(start.elapsed() < WAIT, "{}", status(&h));
        h.app.pump_exports(Duration::from_millis(5));
    }
    // The rows the writer had taken when the cancel came.
    let s = status(&h);
    let count = s[making.len()..].split(' ').next().unwrap().to_string();
    assert_eq!(s, format!("{making}{count} rows\u{2026} (a cancel now keeps nothing)"));
    // The second drops it.
    h.ctrl('c');
    pump_until_done(&mut h);
    assert!(!path.exists() && !dir.join("big.xlsx.partial").exists(), "nothing kept");
    assert_eq!(
        status(&h),
        format!("Export cancelled while the workbook of {count} rows was being made; nothing was kept")
    );
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event");
}

/// The message of the writer's terminal event `outcome` (in tab 1, the job not its own any more:
/// what a late word says).
fn said(h: &mut Harness, outcome: Outcome) -> String {
    let tab = h.app.tab().id;
    h.app.on_app_event(AppEvent::Export(ExportEvent { job: 99, tab, news: ExportNews::Ended(outcome) }));
    status(h)
}

#[test]
fn the_end_of_a_workbook_is_said_as_it_was() {
    let mut h = Harness::connected(Lang::En);
    let path = PathBuf::from("/d/big.xlsx");
    let done = Outcome::Done { rows: 2_500_000, bytes: 3 << 20, path: path.clone(), damaged: 0, sheets: 3 };
    assert_eq!(
        said(&mut h, done),
        "Exported 2,500,000 rows to /d/big.xlsx (3 MB) on 3 sheets (a sheet holds 1,048,575 rows under its header)"
    );
    let full = Fault::io_at(&std::io::Error::from(std::io::ErrorKind::StorageFull), Path::new("/d/big.xlsx.partial"));
    let long = TooLong { column: "body".into(), row: 7, chars: 40_000 };
    let not_saved = Failure::NotSaved { fault: full, first: Some(Box::new(Failure::TooLong(long))) };
    let s = said(&mut h, Outcome::Failed { why: not_saved, rows: 6, partial: None });
    assert!(
        s.starts_with(
            "Export failed: a value of column body (row 7) has 40000 characters, more than an Excel cell holds (32,767); then the workbook could not be made: "
        ) && s.ends_with("; nothing was kept (a workbook reads only once it is complete)"),
        "{s}"
    );
    assert!(!s.contains("rows before it"), "{s}");
    // The rows' temporary file, not the partial file, failed: its folder is named.
    let tmp = std::env::temp_dir();
    let full = Fault::io_at(&std::io::Error::from(std::io::ErrorKind::StorageFull), &tmp);
    let s = said(&mut h, Outcome::Failed { why: Failure::TempFile(full, tmp.clone()), rows: 0, partial: None });
    assert!(
        s.starts_with(&format!(
            "Export failed: the workbook's temporary file in {} could not be written: ",
            tmp.display()
        )),
        "{s}"
    );
    assert!(!s.contains("middle of a row"), "{s}");
    let dropped = Outcome::Dropped { rows: 6, first: Some(Box::new(Failure::Key)) };
    let s = said(&mut h, dropped);
    assert!(
        s.starts_with("Export cancelled while the workbook of 6 rows was being made, after the export failed: ")
            && s.ends_with("; nothing was kept"),
        "{s}"
    );
}
