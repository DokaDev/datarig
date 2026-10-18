//! Exports to a Parquet file (`.parquet`) through the real `App` event path, with the fake driver
//! and real files in a scratch folder, read back with the parquet crate's reader: the columns'
//! types, NULL, a cancel (the row groups so far and a footer in `<file>.partial`, which reads) and
//! a value its column's type cannot hold (the export fails naming its column and row, the rows
//! before it kept). Row groups, dates and the other parsing are the core writer's tests
//! (`export::parquet`).

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbError, DbEvent, ExportBatch, ExportGuard, ExportSink, ValueKind};
use datarig_core::export::parquet::read::{self, Field};
use datarig_core::i18n::Lang;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// A new empty folder for a test's files.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-export-parquet-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn columns() -> Vec<ColumnMeta> {
    let c = |name: &str, ty: &str, kind| ColumnMeta::new(name.into(), ty.into(), kind, None);
    vec![
        c("id", "int8", ValueKind::Integer),
        c("\u{c774}\u{b984}", "text", ValueKind::Text),
        c("price", "numeric", ValueKind::Decimal),
        c("ok", "bool", ValueKind::Bool),
        c("born", "date", ValueKind::Date),
        c("at", "timestamptz", ValueKind::TimestampTz),
    ]
}

/// Row `i`: every seventh's name and price are NULL.
fn row(i: usize) -> Vec<Option<String>> {
    let name = (!i.is_multiple_of(7)).then(|| format!("n\u{d55c}\u{ae00}{i}"));
    let price = (!i.is_multiple_of(7)).then(|| format!("{i}.0000000000000001"));
    vec![
        Some(i.to_string()),
        name,
        price,
        Some(if i.is_multiple_of(2) { "t" } else { "f" }.to_string()),
        Some("1970-01-02".to_string()),
        Some(format!("1970-01-01 00:00:{:02}+00", i % 60)),
    ]
}

fn rows(from: usize, n: usize) -> Vec<Vec<Option<String>>> {
    (from..from + n).map(row).collect()
}

/// The values row `i` reads back as.
fn values(i: usize) -> Vec<Field> {
    let r = row(i);
    vec![
        Field::Long(i as i64),
        r[1].clone().map_or(Field::Null, Field::Str),
        r[2].clone().map_or(Field::Null, Field::Str),
        Field::Bool(i.is_multiple_of(2)),
        Field::Date(1),
        Field::TimestampMicros((i % 60) as i64 * 1_000_000),
    ]
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

/// The Parquet file at `path`, read back.
fn read(path: &Path) -> read::File {
    read::file(&std::fs::read(path).unwrap()).expect("a Parquet file")
}

#[test]
fn the_rows_fetched_go_to_a_parquet_file_with_their_types() {
    let dir = scratch("fetched");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 300), true);
    h.sent();
    let path = dir.join("users.parquet");
    h.command(&format!("export fetched {}", path.display()));
    pump_until_done(&mut h);
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::Export { .. })), "nothing runs on the server");
    assert!(!dir.join("users.parquet.partial").exists());
    let f = read(&path);
    let names: Vec<&str> = f.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "\u{c774}\u{b984}", "price", "ok", "born", "at"]);
    assert!(f.columns.iter().all(|c| c.optional));
    assert_eq!(f.groups, vec![300]);
    for i in 0..300 {
        assert_eq!(f.rows[i], values(i), "row {i}");
    }
    let s = status(&h);
    assert!(s.starts_with("Exported 300 rows to ") && s.contains("users.parquet"), "{s}");
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event, nothing after it");
}

#[test]
fn a_cancel_keeps_a_parquet_file_of_the_rows_so_far_in_the_partial_file() {
    let dir = scratch("cancel");
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, columns(), rows(0, 100), true);
    h.sent();
    let path = dir.join("part.parquet");
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
    let f = read(&dir.join("part.parquet.partial"));
    assert_eq!(f.rows.len(), 50, "a file of the rows written, with its footer");
    assert_eq!(f.rows[49], values(49));
    let s = status(&h);
    assert!(s.starts_with("Export cancelled after 50 rows; they are in ") && s.contains("part.parquet.partial"), "{s}");
    assert_eq!(h.app.pump_exports(Duration::from_millis(100)), 0, "one terminal event");
}

#[test]
fn a_value_its_type_cannot_hold_fails_naming_its_column_and_keeps_the_rows_before() {
    let dir = scratch("bad");
    let mut h = Harness::connected(Lang::En);
    let mut page = rows(0, 10);
    page[4][5] = Some("infinity".into());
    first_page(&mut h, columns(), page, false);
    let path = dir.join("bad.parquet");
    h.command(&format!("export fetched {}", path.display()));
    pump_until_done(&mut h);
    assert!(!path.exists());
    let s = status(&h);
    assert!(
        s.starts_with(
            "Export failed after 4 rows: a value of column at (row 5), infinity, does not fit the column's Parquet type TIMESTAMP (UTC)"
        ),
        "{s}"
    );
    assert!(s.contains("bad.parquet.partial"), "{s}");
    let f = read(&dir.join("bad.parquet.partial"));
    assert_eq!(f.rows.len(), 4);
    assert_eq!(f.rows[3], values(3));
}
