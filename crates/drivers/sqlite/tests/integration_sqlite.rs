//! The SQLite driver against real database files in a scratch directory. No server is needed,
//! so these tests always run.

mod common;

use common::*;
use datarig_core::driver::ddl::{DdlObject, DdlSource};
use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, PingError, SessionRole, TxRule,
};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_sqlite::SqliteDriver;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

#[test]
fn a_read_write_profile_writes_and_reads_back() {
    let scratch = Scratch::new();
    let file = scratch.file("rw.db");
    create(&file, "CREATE TABLE t (a INTEGER, b TEXT);");
    let (session, mut rx) = connect(&file, SessionRole::Query, false);

    let written = run(&session, &mut rx, 1, "INSERT INTO t VALUES (7, 'seven')");
    assert!(matches!(written.last(), Some(DbEvent::Done { .. })));
    let read = run(&session, &mut rx, 2, "SELECT a, b FROM t");
    assert_eq!(page_rows(&read), vec![vec![Some("7".to_string()), Some("seven".to_string())]]);
}

#[test]
fn a_read_only_profile_is_refused_a_write_by_the_engine() {
    let scratch = Scratch::new();
    let file = scratch.file("ro.db");
    create(&file, "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1);");
    let (session, mut rx) = connect(&file, SessionRole::Query, true);

    let refused = run(&session, &mut rx, 1, "DELETE FROM t");
    let (message, cancelled) = failure(&refused);
    assert!(message.contains("readonly"), "{message}");
    assert!(!cancelled);
    // The row is still there: nothing was written.
    let read = run(&session, &mut rx, 2, "SELECT count(*) FROM t");
    assert_eq!(page_rows(&read), vec![vec![Some("1".to_string())]]);
}

#[test]
fn a_missing_file_is_not_created() {
    let scratch = Scratch::new();
    let file = scratch.file("missing.db");
    let (tx, mut rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, SessionRole::Query, "test");
    let _session = SqliteDriver.connect(&profile(&file.display().to_string()), SessionRole::Query, opts, tx);
    assert!(matches!(rx.blocking_recv(), Some(DbEvent::ConnectFailed { .. })));
    assert!(!file.exists(), "the driver created the file");
}

#[test]
fn a_relative_path_is_refused() {
    let (tx, mut rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, SessionRole::Query, "test");
    let _session = SqliteDriver.connect(&profile("data/relative.db"), SessionRole::Query, opts, tx);
    match rx.blocking_recv() {
        Some(DbEvent::ConnectFailed { error: DbError::Server(msg), .. }) => assert!(msg.contains("absolute"), "{msg}"),
        other => panic!("expected a refused path, got {other:?}"),
    }
}

#[test]
fn a_profile_without_a_path_is_refused() {
    let (tx, mut rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, SessionRole::Query, "test");
    let cfg = ConnectionConfig { driver: "sqlite".into(), path: None, ..ConnectionConfig::default() };
    let _session = SqliteDriver.connect(&cfg, SessionRole::Query, opts, tx);
    assert!(matches!(rx.blocking_recv(), Some(DbEvent::ConnectFailed { error: DbError::NoDatabase, .. })));
}

#[test]
fn attach_is_refused_and_creates_nothing() {
    let scratch = Scratch::new();
    let file = scratch.file("main.db");
    let other = scratch.file("other.db");
    create(&file, "CREATE TABLE t (a INTEGER);");
    let (session, mut rx) = connect(&file, SessionRole::Query, false);

    let attempt = run(&session, &mut rx, 1, &format!("ATTACH DATABASE '{}' AS other", other.display()));
    let (message, _) = failure(&attempt);
    assert!(message.contains("not authorized"), "{message}");
    assert!(!other.exists(), "ATTACH created a file");
}

#[test]
fn cancel_interrupts_a_long_statement() {
    let scratch = Scratch::new();
    let file = scratch.file("long.db");
    create(&file, "CREATE TABLE t (a INTEGER);");
    let (session, mut rx) = connect(&file, SessionRole::Query, false);

    session.send(DbCommand::Execute {
        id: 1,
        statements: vec![
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c".to_string(),
        ],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    std::thread::sleep(Duration::from_millis(200));
    session.cancel();
    let events = until_answer(&mut rx, 1);
    let (_, cancelled) = failure(&events);
    assert!(cancelled, "the statement was not cancelled");
    // The session keeps working after the cancel.
    let after = run(&session, &mut rx, 2, "SELECT 1");
    assert_eq!(page_rows(&after), vec![vec![Some("1".to_string())]]);
}

#[test]
fn a_run_of_several_statements_reports_each_and_answers_with_the_last() {
    let scratch = Scratch::new();
    let file = scratch.file("multi.db");
    create(&file, "");
    let (session, mut rx) = connect(&file, SessionRole::Query, false);

    session.send(DbCommand::Execute {
        id: 1,
        statements: vec![
            "CREATE TABLE t (a INTEGER)".to_string(),
            "INSERT INTO t VALUES (1)".to_string(),
            "SELECT a FROM t".to_string(),
        ],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    let events = until_answer(&mut rx, 1);
    let started = events.iter().filter(|e| matches!(e, DbEvent::Started { .. })).count();
    let finished = events.iter().filter(|e| matches!(e, DbEvent::Finished { .. })).count();
    assert_eq!((started, finished), (3, 2), "{events:?}");
    assert_eq!(page_rows(&events), vec![vec![Some("1".to_string())]]);
}

#[test]
fn values_keep_their_storage_class_as_text() {
    let scratch = Scratch::new();
    let file = scratch.file("values.db");
    create(&file, "CREATE TABLE v (a INTEGER, b REAL, c TEXT, d BLOB, e);");
    let (session, mut rx) = connect(&file, SessionRole::Query, false);
    run(&session, &mut rx, 1, "INSERT INTO v VALUES (1, 1.0, 'x', x'0a0b', NULL)");

    let events = run(&session, &mut rx, 2, "SELECT * FROM v");
    let row = page_rows(&events).remove(0);
    assert_eq!(row, vec![Some("1".into()), Some("1.0".into()), Some("x".into()), Some("\\x0a0b".into()), None]);
    match events.last() {
        Some(DbEvent::Page { columns: Some(columns), .. }) => {
            let kinds: Vec<_> = columns.iter().map(|c| c.kind).collect();
            assert_eq!(
                kinds,
                vec![
                    datarig_core::driver::ValueKind::Integer,
                    datarig_core::driver::ValueKind::Float,
                    datarig_core::driver::ValueKind::Text,
                    datarig_core::driver::ValueKind::Bytes,
                    datarig_core::driver::ValueKind::Other,
                ]
            );
        }
        other => panic!("expected the columns, got {other:?}"),
    }
}

#[test]
fn the_metadata_session_lists_objects_and_their_create_text() {
    let scratch = Scratch::new();
    let file = scratch.file("meta.db");
    create(
        &file,
        "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT); CREATE VIEW titles AS SELECT title FROM books;",
    );
    let (session, mut rx) = connect(&file, SessionRole::Meta, true);

    session.send(DbCommand::LoadSchemas);
    assert!(matches!(rx.blocking_recv(), Some(DbEvent::Schemas(Ok(s))) if s == vec!["main".to_string()]));

    session.send(DbCommand::LoadObjects { schema: "main".into() });
    match rx.blocking_recv() {
        Some(DbEvent::Objects { result: Ok(objects), .. }) => {
            assert_eq!(objects.tables, vec!["books".to_string()]);
            assert_eq!(objects.views, vec!["titles".to_string()]);
        }
        other => panic!("expected the objects, got {other:?}"),
    }

    session.send(DbCommand::LoadCatalog);
    match rx.blocking_recv() {
        Some(DbEvent::Catalog(Ok(catalog))) => {
            let books = catalog.relations.iter().find(|r| r.name == "books").expect("books");
            assert!(!books.is_view);
            let columns: Vec<_> = books.columns.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(columns, vec!["id", "title"]);
        }
        other => panic!("expected the catalog, got {other:?}"),
    }

    session.send(DbCommand::LoadDdl {
        id: 9,
        object: DdlObject::Relation { schema: "main".into(), name: "books".into() },
    });
    match rx.blocking_recv() {
        Some(DbEvent::Ddl { id: 9, result: Ok(DdlSource::Verbatim { text, .. }) }) => {
            assert!(text.starts_with("CREATE TABLE books"), "{text}");
        }
        other => panic!("expected the CREATE text, got {other:?}"),
    }
}

#[test]
fn a_command_the_driver_does_not_offer_is_answered_not_supported() {
    let scratch = Scratch::new();
    let file = scratch.file("unsupported.db");
    create(&file, "");
    let (session, mut rx) = connect(&file, SessionRole::Meta, true);
    session.send(DbCommand::LoadKeys);
    assert!(matches!(rx.blocking_recv(), Some(DbEvent::Keys(Err(DbError::NotSupported)))));
}

#[test]
fn ping_reports_the_library_version_and_never_creates_a_missing_file() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let scratch = Scratch::new();
    let file = scratch.file("ping.db");
    create(&file, "CREATE TABLE t (a INTEGER);");

    let info = runtime
        .block_on(SqliteDriver.ping(&profile(&file.display().to_string()), Duration::from_secs(5), None))
        .expect("the file pings");
    assert!(info.server_version.starts_with("SQLite 3."), "{}", info.server_version);

    let missing = scratch.file("nope.db");
    let refused =
        runtime.block_on(SqliteDriver.ping(&profile(&missing.display().to_string()), Duration::from_secs(5), None));
    assert!(matches!(refused, Err(PingError::Failed(_))));
    assert!(!missing.exists(), "ping created the file");
}

#[test]
fn a_statement_waits_for_another_connections_lock_only_as_long_as_the_lock_wait() {
    let scratch = Scratch::new();
    let file = scratch.file("locked.db");
    create(&file, "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1);");
    // The session connects first (its own probe needs no lock held elsewhere).
    let (session, mut rx) = connect(&file, SessionRole::Query, false);
    // Another connection takes the file exclusively: a read of the session must wait for it.
    let holder = rusqlite::Connection::open(&file).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let started = std::time::Instant::now();
    let events = run(&session, &mut rx, 1, "SELECT count(*) FROM t");
    let waited = started.elapsed();
    holder.execute_batch("ROLLBACK").unwrap();

    let (message, cancelled) = failure(&events);
    assert!(message.contains("locked"), "{message}");
    assert!(!cancelled);
    // With no lock wait set, the driver's default (5 s) applies: it neither gives up at once nor
    // waits forever. A caller's own wait is tested in the next test.
    assert!(waited >= Duration::from_millis(900), "gave up at once: {waited:?}");
}

#[test]
fn a_lock_wait_set_by_the_caller_bounds_the_wait() {
    let scratch = Scratch::new();
    let file = scratch.file("bounded.db");
    create(&file, "CREATE TABLE t (a INTEGER);");
    let (tx, mut rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, SessionRole::Query, "test").lock_wait(Some(Duration::from_millis(150)));
    let session = SqliteDriver.connect(&profile(&file.display().to_string()), SessionRole::Query, opts, tx);
    assert!(matches!(rx.blocking_recv(), Some(DbEvent::Connected)));

    let holder = rusqlite::Connection::open(&file).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let started = std::time::Instant::now();
    let events = run(&session, &mut rx, 1, "SELECT count(*) FROM t");
    let waited = started.elapsed();
    holder.execute_batch("ROLLBACK").unwrap();

    let (message, _) = failure(&events);
    assert!(message.contains("locked"), "{message}");
    assert!(waited < Duration::from_secs(4), "the caller's 150 ms wait was not honoured: {waited:?}");
}
