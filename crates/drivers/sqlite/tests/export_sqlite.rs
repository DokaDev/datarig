//! The export command and the runtime: rows stream to the sink in batches, a closed sink stops
//! the statement, and a cancelled statement does not block the async runtime.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbError, DbEvent, ExportBatch, ExportGuard, PagingMode, SessionRole, TxRule};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A file with a table `seq` of `n` rows, numbered from 1.
fn seq_file(scratch: &Scratch, n: u64) -> std::path::PathBuf {
    let file = scratch.file("seq.db");
    create(
        &file,
        &format!(
            "CREATE TABLE seq (x INTEGER); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < {n}) INSERT INTO seq SELECT x FROM c;"
        ),
    );
    file
}

fn export(
    session: &datarig_core::driver::Session,
    sql: &str,
    sink: tokio::sync::mpsc::Sender<ExportBatch>,
    batch: usize,
) {
    session.send(DbCommand::Export { id: 7, sql: sql.to_string(), guard: ExportGuard::Repeatable, sink, batch });
}

#[test]
fn export_streams_every_row_in_batches_and_answers_once() {
    let scratch = Scratch::new();
    let file = seq_file(&scratch, 1000);
    let (session, mut rx) = connect(&file, SessionRole::Query, true);
    let (sink, mut batches) = tokio::sync::mpsc::channel(4);
    export(&session, "SELECT x FROM seq ORDER BY x", sink, 100);

    let (mut columns, mut batch_sizes) = (0, Vec::new());
    while let Some(b) = batches.blocking_recv() {
        match b {
            ExportBatch::Columns(c) => {
                assert_eq!(c.len(), 1);
                columns += 1;
            }
            ExportBatch::Rows(rows) => batch_sizes.push(rows.len()),
        }
    }
    assert_eq!(columns, 1);
    assert_eq!(batch_sizes.iter().sum::<usize>(), 1000);
    assert!(batch_sizes.iter().all(|n| *n <= 100) && batch_sizes.len() >= 10, "{batch_sizes:?}");
    match rx.blocking_recv() {
        Some(DbEvent::Exported { id: 7, result: Ok(read), cancelled: false }) => {
            assert_eq!((read.rows, read.complete), (1000, true));
            assert_eq!(read.last, Some(vec![Some("1000".to_string())]));
        }
        other => panic!("expected the export's answer, got {other:?}"),
    }
}

#[test]
fn a_closed_sink_stops_the_export_and_says_it_did_not_read_to_the_end() {
    let scratch = Scratch::new();
    let file = seq_file(&scratch, 500);
    let (session, mut rx) = connect(&file, SessionRole::Query, true);
    let (sink, mut batches) = tokio::sync::mpsc::channel(1);
    export(&session, "SELECT x FROM seq ORDER BY x", sink, 10);
    assert!(matches!(batches.blocking_recv(), Some(ExportBatch::Columns(_))));
    drop(batches);
    match rx.blocking_recv() {
        Some(DbEvent::Exported { id: 7, result: Ok(read), .. }) => assert!(!read.complete, "{read:?}"),
        other => panic!("expected an unfinished export, got {other:?}"),
    }
}

#[test]
fn an_export_of_a_statement_without_rows_is_not_supported() {
    let scratch = Scratch::new();
    let file = scratch.file("norows.db");
    create(&file, "");
    let (session, mut rx) = connect(&file, SessionRole::Query, false);
    let (sink, _batches) = tokio::sync::mpsc::channel(1);
    export(&session, "CREATE TABLE q (x)", sink, 10);
    assert!(matches!(rx.blocking_recv(), Some(DbEvent::Exported { id: 7, result: Err(DbError::NotSupported), .. })));
}

#[test]
fn a_cancelled_statement_does_not_block_the_async_runtime() {
    let scratch = Scratch::new();
    let file = scratch.file("runtime.db");
    create(&file, "");
    let (session, rx) = connect(&file, SessionRole::Query, false);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async move {
        let ticks = Arc::new(AtomicU64::new(0));
        let counter = ticks.clone();
        let ticker = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });
        session.send(DbCommand::Execute {
            id: 1,
            statements: vec![
                "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c".to_string(),
            ],
            paging: PagingMode::NoHold,
            tx: TxRule::Auto,
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let ticks_while_running = ticks.load(Ordering::SeqCst);
        session.cancel();
        let mut rx = rx;
        let events = tokio::task::spawn_blocking(move || until_answer(&mut rx, 1)).await.unwrap();
        ticker.abort();
        let (_, cancelled) = failure(&events);
        assert!(cancelled, "the statement was not cancelled");
        assert!(ticks_while_running >= 20, "the runtime was blocked: {ticks_while_running} ticks in 300 ms");
    });
}
