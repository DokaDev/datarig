use super::{TxAfter, command_tag, forgets_prepared, plain_read, returns_no_rows, split_page, tx_after};

#[test]
fn tags() {
    assert_eq!(command_tag("create table x (a int)"), ("CREATE TABLE".into(), false));
    assert_eq!(command_tag("CREATE OR REPLACE FUNCTION f()"), ("CREATE FUNCTION".into(), false));
    assert_eq!(command_tag("update t set a = 1"), ("UPDATE".into(), true));
    assert_eq!(command_tag("WITH x AS (SELECT 1) DELETE FROM t"), ("DELETE".into(), true));
    assert_eq!(command_tag("SET search_path = shop"), ("SET".into(), false));
}

#[test]
fn pages_carry_the_row_beyond_a_full_page() {
    let mut carry = None;
    // Fewer rows than asked for: complete, nothing carried (the portal ends at once).
    assert_eq!(split_page(&mut carry, vec![1, 2], 3), (vec![1, 2], false));
    assert_eq!(carry, None);
    // Exactly one page: complete as well (one more row was asked for and did not come).
    assert_eq!(split_page(&mut carry, vec![1, 2, 3], 3), (vec![1, 2, 3], false));
    // One row more than a page: a full page, more to come, the extra row starts the next page.
    assert_eq!(split_page(&mut carry, vec![1, 2, 3, 4], 3), (vec![1, 2, 3], true));
    assert_eq!(carry, Some(4));
    assert_eq!(split_page(&mut carry, vec![5, 6, 7], 3), (vec![4, 5, 6], true));
    assert_eq!(carry, Some(7));
    assert_eq!(split_page(&mut carry, vec![], 3), (vec![7], false), "the carried row ends the result");
    assert_eq!(carry, None);
}

#[test]
fn transaction_control_sets_the_state() {
    let open = [
        "BEGIN",
        "begin;",
        "Begin Transaction Isolation Level Serializable",
        "BEGIN WORK",
        "start transaction isolation level repeatable read, read only",
        "ROLLBACK TO SAVEPOINT sp",
        "rollback to sp",
        "ROLLBACK WORK TO SAVEPOINT sp",
        "rollback transaction to sp",
        "SAVEPOINT sp",
        "RELEASE SAVEPOINT sp",
        "release sp",
    ];
    for sql in open {
        assert_eq!(tx_after(sql), TxAfter::Block(true), "{sql:?}");
    }
    // A chain ends the block and opens a new one.
    for sql in [
        "COMMIT AND CHAIN",
        "commit work and chain",
        "END AND CHAIN",
        "rollback and chain",
        "ABORT TRANSACTION AND CHAIN",
    ] {
        assert_eq!(tx_after(sql), TxAfter::Chain, "{sql:?}");
    }
    let idle = [
        "COMMIT",
        "commit;",
        "commit work and no chain",
        "END",
        "end transaction",
        "ROLLBACK",
        "ABORT",
        "rollback and no chain",
        "PREPARE TRANSACTION 'gid'",
        "COMMIT PREPARED 'gid'",
        "ROLLBACK PREPARED 'gid'",
        "rollback prepared 'to'",
    ];
    for sql in idle {
        assert_eq!(tx_after(sql), TxAfter::Block(false), "{sql:?}");
    }
}

#[test]
fn procedures_ask() {
    for sql in [
        "CALL p()",
        "call s.p(1, 'x')",
        "DO $$ BEGIN COMMIT; END $$",
        "do language plpgsql $body$ begin null; end $body$",
    ] {
        assert_eq!(tx_after(sql), TxAfter::Ask, "{sql:?}");
    }
}

#[test]
fn comments_case_and_whitespace_do_not_fool_it() {
    for sql in [
        "/* x */ commit",
        "-- c\nBEGIN",
        "  \n\t RollBack  ;  ",
        "/* a /* nested */ comment */ START TRANSACTION",
        "-- one\n-- two\n/* three */\ncommit and chain;",
        "begin;;",
    ] {
        assert!(matches!(tx_after(sql), TxAfter::Block(_) | TxAfter::Chain), "{sql:?}");
    }
    for sql in ["/* commit */ SELECT 1", "-- BEGIN\nINSERT INTO t VALUES (1)", "UPDATE t SET a = 1 -- rollback"] {
        assert_eq!(tx_after(sql), TxAfter::Same, "{sql:?}");
    }
}

#[test]
fn ordinary_statements_leave_the_state_alone() {
    for sql in [
        "SELECT 1",
        "insert into t values (1)",
        "UPDATE t SET a = 1",
        "DELETE FROM t",
        "MERGE INTO t USING s ON true WHEN MATCHED THEN DELETE",
        "WITH x AS (SELECT 1) DELETE FROM t",
        "COPY t FROM STDIN",
        "CREATE TABLE t (a int)",
        "ALTER TABLE t ADD COLUMN b int",
        "DROP TABLE t",
        "CREATE PROCEDURE p() LANGUAGE sql AS $$ COMMIT $$",
        "CREATE FUNCTION f() RETURNS int LANGUAGE sql AS 'SELECT 1'",
        "SET lock_timeout = 0",
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "SET LOCAL search_path = shop",
        "RESET ALL",
        "SHOW search_path",
        "PREPARE q AS SELECT 1",
        "EXECUTE q",
        "DEALLOCATE q",
        "LOCK TABLE t",
        "TRUNCATE t",
        "VACUUM t",
        "EXPLAIN ANALYZE SELECT 1",
        "select 'commit; begin'",
        "SELECT $$;COMMIT$$",
        "SELECT \"begin\" FROM t",
        "INSERT INTO t VALUES (1);",
    ] {
        assert_eq!(tx_after(sql), TxAfter::Same, "{sql:?}");
    }
}

#[test]
fn when_in_doubt_it_asks() {
    for sql in [
        "",
        "   ",
        "-- only a comment",
        ";",
        "(SELECT 1)",
        "\"commit\"",
        "SELECT 1; COMMIT",
        "INSERT INTO t VALUES (1); SELECT 2",
        "select 1; -- trailing\nbegin",
    ] {
        assert_eq!(tx_after(sql), TxAfter::Ask, "{sql:?}");
    }
}

#[test]
fn statements_known_to_return_no_rows_skip_the_prepare_round_trip() {
    for sql in [
        "INSERT INTO t VALUES (1)",
        "insert into t select * from s;",
        "UPDATE t SET a = 1 WHERE b = 'returning'",
        "DELETE FROM t",
        "MERGE INTO t USING s ON true WHEN MATCHED THEN DELETE",
        "CREATE TABLE t (a int)",
        "create table t2 as select * from t",
        "ALTER TABLE t ADD COLUMN b int",
        "DROP TABLE t",
        "TRUNCATE t",
        "SET lock_timeout = 0",
        "RESET ALL",
        "BEGIN",
        "commit",
        "ROLLBACK TO SAVEPOINT a",
        "PREPARE TRANSACTION 'x'",
        "DO $$ BEGIN NULL; END $$",
        "VACUUM t",
        "ANALYZE t",
        "LOCK TABLE t",
        "GRANT SELECT ON t TO PUBLIC",
        "COMMENT ON TABLE t IS 'x'",
        "REFRESH MATERIALIZED VIEW v",
        "-- a comment\n/* and another */ insert into t values (1);",
    ] {
        assert!(returns_no_rows(sql), "{sql:?}");
    }
    for sql in [
        "SELECT 1",
        "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
        "INSERT INTO t VALUES (1) RETURNING id",
        "update t set a = 1 returning *",
        "DELETE FROM t RETURNING a",
        "MERGE INTO t USING s ON true WHEN MATCHED THEN DELETE RETURNING *",
        "VALUES (1)",
        "TABLE t",
        "SHOW search_path",
        "EXPLAIN SELECT 1",
        "CALL p()",
        "COPY t TO STDOUT",
        "FETCH ALL FROM c",
        "EXECUTE q",
        "PREPARE q AS SELECT 1",
        "INSERT INTO t VALUES (1); SELECT 1",
        "",
        "-- only a comment",
        "(SELECT 1)",
    ] {
        assert!(!returns_no_rows(sql), "{sql:?}");
    }
}

#[test]
fn only_plain_reads_are_shown_before_their_commit() {
    for sql in ["SELECT 1", "/* c */ select * from t", "VALUES (1)", "TABLE t", "SHOW search_path"] {
        assert!(plain_read(sql), "{sql:?}");
    }
    for sql in [
        "INSERT INTO t VALUES (1) RETURNING *",
        "WITH x AS (SELECT 1) SELECT * FROM x",
        "EXPLAIN ANALYZE DELETE FROM t",
        "CALL p()",
        "(SELECT 1)",
        "",
    ] {
        assert!(!plain_read(sql), "{sql:?}");
    }
}

#[test]
fn statements_that_deallocate_prepared_statements() {
    for sql in [
        "DEALLOCATE ALL",
        "deallocate prepare all;",
        "DEALLOCATE s3",
        "/* reset */ DISCARD ALL",
        "discard plans",
        "-- c\nDiscard All;",
    ] {
        assert!(forgets_prepared(sql), "{sql:?}");
    }
    for sql in [
        "DISCARD TEMP",
        "DISCARD SEQUENCES",
        "DISCARD",
        "SELECT 'DEALLOCATE ALL'",
        "-- DEALLOCATE ALL\nSELECT 1",
        "PREPARE q AS SELECT 1",
        "\"deallocate\"",
        "",
    ] {
        assert!(!forgets_prepared(sql), "{sql:?}");
    }
}

// ── every run ends with exactly one terminal event ───────────────────────────

mod terminal {
    use super::super::{BeforeBind, Env, Prepared, Reply, State, Tx, execute};
    use crate::link::Link;
    use datarig_core::driver::{DbCommand, DbError, DbEvent, PagingMode};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
    use tokio::sync::oneshot;
    use tokio_postgres::{Client, NoTls};

    /// The test database, as the integration tests find it (`DATARIG_TEST_PG_URL`).
    fn pg_url(test: &str) -> Option<String> {
        match std::env::var("DATARIG_TEST_PG_URL") {
            Ok(u) if !u.is_empty() => Some(u),
            _ => {
                if std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1") {
                    panic!("DATARIG_TEST_PG_URL must be set when DATARIG_REQUIRE_PG=1");
                }
                eprintln!("SKIPPED {test}: set DATARIG_TEST_PG_URL");
                None
            }
        }
    }

    async fn connect(url: &str) -> (Client, oneshot::Receiver<DbError>) {
        let (client, conn) = tokio_postgres::connect(url, NoTls).await.expect("connect");
        let (done, ended) = oneshot::channel();
        tokio::spawn(async move {
            let _ = conn.await;
            let _ = done.send(DbError::Closed);
        });
        (client, ended)
    }

    /// Drops the test's table on a fresh connection, also when an assertion fails.
    struct TableGuard(String, String);

    impl Drop for TableGuard {
        fn drop(&mut self) {
            let (url, table) = (self.0.clone(), self.1.clone());
            let _ = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(async {
                    let (c, _) = connect(&url).await;
                    let _ = c.batch_execute(&format!("DROP TABLE IF EXISTS {table}")).await;
                });
            })
            .join();
        }
    }

    /// A query session driven by hand: runs go through [`execute`] with a hook before each
    /// bind.
    struct Session {
        client: Client,
        link: Link,
        token: crate::route::Cancel,
        events: UnboundedSender<DbEvent>,
        rx: UnboundedReceiver<DbEvent>,
        _cmds: UnboundedSender<DbCommand>,
        tx: Tx,
        prepared: Prepared,
        id: u64,
        cancel: Arc<AtomicBool>,
        /// What a result with more rows keeps (`Hold` unless a test says otherwise).
        paging: PagingMode,
    }

    impl Session {
        async fn open(url: &str) -> Self {
            let (client, ended) = connect(url).await;
            let (cmds, cmd_rx) = unbounded_channel();
            let (events, rx) = unbounded_channel();
            let token = crate::route::Cancel { token: client.cancel_token(), route: None };
            let link = Link::new(cmd_rx, ended);
            Self {
                client,
                link,
                token,
                events,
                rx,
                _cmds: cmds,
                tx: Tx::default(),
                prepared: Prepared::default(),
                id: 0,
                cancel: Arc::new(AtomicBool::new(false)),
                paging: PagingMode::Hold,
            }
        }

        /// Run `statements` with `hook` before each bind; every event of the run.
        async fn run_all(&mut self, statements: &[&str], hook: Option<&BeforeBind>) -> Vec<DbEvent> {
            self.id += 1;
            let env = Env {
                events: &self.events,
                token: &self.token,
                cancel: &self.cancel,
                page_size: 10,
                read_only: false,
                path: None,
                before_bind: hook,
            };
            let mut s = State { tx: &mut self.tx, prepared: &mut self.prepared };
            let statements = statements.iter().map(|s| s.to_string()).collect();
            let run = execute(&mut self.client, &mut self.link, &env, &mut s, self.id, statements, None, self.paging);
            tokio::time::timeout(std::time::Duration::from_secs(20), run).await.expect("the run returns").ok();
            let mut events = Vec::new();
            while let Ok(ev) = self.rx.try_recv() {
                events.push(ev);
            }
            events
        }

        /// Run `sql` with `hook` before each bind; the run's terminal events (it must have
        /// exactly one).
        async fn run(&mut self, sql: &str, hook: Option<&BeforeBind>) -> DbEvent {
            self.id += 1;
            let env = Env {
                events: &self.events,
                token: &self.token,
                cancel: &self.cancel,
                page_size: 10,
                read_only: false,
                path: None,
                before_bind: hook,
            };
            let mut s = State { tx: &mut self.tx, prepared: &mut self.prepared };
            let run = execute(
                &mut self.client,
                &mut self.link,
                &env,
                &mut s,
                self.id,
                vec![sql.to_string()],
                None,
                self.paging,
            );
            tokio::time::timeout(std::time::Duration::from_secs(20), run).await.expect("the run returns").ok();
            let mut terminal = Vec::new();
            while let Ok(ev) = self.rx.try_recv() {
                if matches!(ev, DbEvent::Page { columns: Some(_), .. } | DbEvent::Done { .. } | DbEvent::Failed { .. })
                {
                    terminal.push(ev);
                }
            }
            assert_eq!(terminal.len(), 1, "{sql:?}: exactly one terminal event: {terminal:?}");
            terminal.pop().unwrap()
        }
    }

    /// A hook that changes the column's type on another connection before each of the next
    /// `times` binds, so the statement prepared for it is stale every time.
    fn alter(other: &Arc<Client>, table: &str, times: usize) -> Box<BeforeBind> {
        let (other, table, count) = (other.clone(), table.to_string(), Arc::new(AtomicUsize::new(0)));
        Box::new(move || {
            let (other, table, n) = (other.clone(), table.clone(), count.fetch_add(1, Ordering::SeqCst));
            Box::pin(async move {
                if n < times {
                    // Always another type than the table has now (tests running in parallel each
                    // flip their own table).
                    let flip = format!(
                        "DO $$ BEGIN IF (SELECT atttypid FROM pg_attribute WHERE attrelid = '{table}'::regclass \
                         AND attname = 'a') = 'int4'::regtype THEN ALTER TABLE {table} ALTER a TYPE int8; \
                         ELSE ALTER TABLE {table} ALTER a TYPE int4; END IF; END $$"
                    );
                    other.batch_execute(&flip).await.expect("alter");
                }
            })
        })
    }

    /// A statement whose table changes between its Parse and its Bind is prepared again once;
    /// when it changes again, the run fails with `SchemaChanged` instead of ending without an
    /// event (the tab would stay "running" for good). Fresh and reused statements alike.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_statement_stale_twice_in_a_row_fails_once() {
        let Some(url) = pg_url("a_statement_stale_twice_in_a_row_fails_once") else { return };
        let table = format!("public.it_stale_{}", std::process::id());
        let _drop = TableGuard(url.clone(), table.clone());
        let (other, _) = connect(&url).await;
        let other = Arc::new(other);
        other.batch_execute(&format!("CREATE TABLE {table} (a int4); INSERT INTO {table} VALUES (1)")).await.unwrap();
        let mut s = Session::open(&url).await;
        let sql = format!("SELECT a FROM {table}");
        for reused in [false, true] {
            if reused {
                assert!(matches!(s.run(&sql, None).await, DbEvent::Page { .. }), "prepared and kept");
            } else {
                s.prepared = Prepared::default();
            }
            // Stale at the first bind and again after it was prepared again.
            let hook = alter(&other, &table, 2);
            match s.run(&sql, Some(hook.as_ref())).await {
                DbEvent::Failed { error: DbError::SchemaChanged, cancelled: false, .. } => {}
                ev => panic!("reused {reused}: {ev:?}"),
            }
            assert!(!s.tx.block && !s.tx.reported, "reused {reused}: nothing stays open");
            // Stale once: prepared again and run.
            let hook = alter(&other, &table, 1);
            match s.run(&sql, Some(hook.as_ref())).await {
                DbEvent::Page { rows, .. } => assert_eq!(rows[0][0].as_deref(), Some("1")),
                ev => panic!("reused {reused}: {ev:?}"),
            }
            // The session goes on.
            assert!(matches!(s.run("SELECT 1", None).await, DbEvent::Page { .. }));
        }
    }

    /// The same with nothing held (`PagingMode::NoHold`, whose first page ends its transaction
    /// in the same request): stale twice fails once, stale once is prepared again and runs, and
    /// no transaction is left open either way, for a result with more rows than a page too.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_statement_stale_without_hold_fails_once_and_leaves_nothing_open() {
        let Some(url) = pg_url("a_statement_stale_without_hold_fails_once_and_leaves_nothing_open") else { return };
        let table = format!("public.zz_stale_nohold_{}", std::process::id());
        let _drop = TableGuard(url.clone(), table.clone());
        let (other, _) = connect(&url).await;
        let other = Arc::new(other);
        other
            .batch_execute(&format!("CREATE TABLE {table} (a int4); INSERT INTO {table} SELECT generate_series(1, 25)"))
            .await
            .unwrap();
        let mut s = Session::open(&url).await;
        s.paging = PagingMode::NoHold;
        let sql = format!("SELECT a FROM {table} ORDER BY a");
        let hook = alter(&other, &table, 2);
        match s.run(&sql, Some(hook.as_ref())).await {
            DbEvent::Failed { error: DbError::SchemaChanged, cancelled: false, .. } => {}
            ev => panic!("{ev:?}"),
        }
        assert!(!s.tx.block && !s.tx.reported, "nothing stays open");
        let hook = alter(&other, &table, 1);
        let events = s.run_all(&[&sql], Some(hook.as_ref())).await;
        let released = events.iter().position(|e| matches!(e, DbEvent::Released { .. }));
        let page = events.iter().position(|e| matches!(e, DbEvent::Page { columns: Some(_), more: true, .. }));
        assert!(released.is_some() && released < page, "released before its page: {events:?}");
        assert!(!events.iter().any(|e| matches!(e, DbEvent::TxOpen(true))), "{events:?}");
        assert!(!s.tx.block && !s.tx.reported);
        // The connection is idle, not in a transaction: a statement that needs no transaction runs.
        let idle = s.client.simple_query("SELECT pg_catalog.now() = pg_catalog.statement_timestamp()").await.unwrap();
        assert!(idle.iter().any(|m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(r) if r.get(0) == Some("t"))));
    }

    /// Statements deallocated behind the session's back (`26000`) are prepared again once,
    /// and the session forgets all of them: the first run after `DEALLOCATE ALL` works. The
    /// second time on the same connection the session stops keeping them (as
    /// behind a pooler that loses them), and the run still works.
    #[tokio::test(flavor = "multi_thread")]
    async fn statements_deallocated_behind_its_back_are_prepared_again() {
        let Some(url) = pg_url("statements_deallocated_behind_its_back_are_prepared_again") else { return };
        let mut s = Session::open(&url).await;
        let (a, b) = ("SELECT 1 AS a", "SELECT 2 AS b");
        for sql in [a, b] {
            assert!(matches!(s.run(sql, None).await, DbEvent::Page { .. }));
        }
        assert_eq!(s.prepared.order.len(), 2);
        for (i, reset) in ["DEALLOCATE ALL", "DISCARD ALL"].into_iter().enumerate() {
            s.client.batch_execute(reset).await.unwrap();
            match s.run(a, None).await {
                DbEvent::Page { rows, .. } => assert_eq!(rows[0][0].as_deref(), Some("1"), "{reset}"),
                ev => panic!("{reset}: {ev:?}"),
            }
            if i == 0 {
                assert_eq!(s.prepared.order, [a], "{reset}: the others are forgotten too");
                assert!(!s.prepared.off, "{reset}: once can be the user's own doing");
            } else {
                assert!(s.prepared.off && s.prepared.order.is_empty(), "{reset}: a second loss turns the cache off");
            }
            assert!(matches!(s.run(b, None).await, DbEvent::Page { .. }), "{reset}");
        }
    }

    /// The events of a run that say what happened, without the transaction state.
    fn outline(events: &[DbEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|ev| match ev {
                DbEvent::Started { index, .. } => Some(format!("started {index}")),
                DbEvent::Finished { index, outcome, .. } => Some(format!("finished {index}: {outcome:?}")),
                DbEvent::Page { columns: Some(_), rows, .. } => Some(format!("page {}", rows.len())),
                DbEvent::Done { outcome, .. } => Some(format!("done: {outcome:?}")),
                DbEvent::Failed { error, cancelled, .. } => Some(format!("failed (cancelled {cancelled}): {error:?}")),
                _ => None,
            })
            .collect()
    }

    /// A run of several statements says when each one starts and what each
    /// but the last did, and stops at the first that fails: the ones after it never run.
    #[tokio::test(flavor = "multi_thread")]
    async fn several_statements_report_each_one_and_stop_at_the_first_failure() {
        let Some(url) = pg_url("several_statements_report_each_one_and_stop_at_the_first_failure") else { return };
        let table = format!("public.zz_steps_{}", std::process::id());
        let _drop = TableGuard(url.clone(), table.clone());
        let (other, _) = connect(&url).await;
        other.batch_execute(&format!("CREATE TABLE {table} (a int4)")).await.unwrap();
        let mut s = Session::open(&url).await;
        let insert = |v: i32| format!("INSERT INTO {table} VALUES ({v})");
        let (one, two, three) = (insert(1), insert(2), insert(3));
        let events = s.run_all(&[&one, "SELECT 1", "SELECT 1/0", &three], None).await;
        assert_eq!(
            outline(&events),
            [
                "started 0",
                "finished 0: Affected(1)",
                "started 1",
                "finished 1: Command(\"SELECT\")",
                "started 2",
                "failed (cancelled false): Server(\"ERROR: division by zero\")",
            ]
        );
        let rows = other.query(&format!("SELECT a FROM {table} ORDER BY a"), &[]).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.get::<_, i32>(0)).collect::<Vec<_>>(), [1], "the rest never ran");
        // All well: the last statement's result answers the run.
        let events = s.run_all(&[&two, &format!("SELECT a FROM {table} ORDER BY a")], None).await;
        assert_eq!(outline(&events), ["started 0", "finished 0: Affected(1)", "started 1", "page 2"]);
        // One statement: no progress events, as before.
        assert_eq!(outline(&s.run_all(&["SELECT 1"], None).await), ["page 1"]);
    }

    /// A cancel that comes between two statements (the server has nothing running to cancel)
    /// stops the run there: the next statements do not run, the run fails as cancelled. A
    /// cancel asked for before a run does not reach it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancel_between_statements_stops_the_rest() {
        let Some(url) = pg_url("a_cancel_between_statements_stops_the_rest") else { return };
        let table = format!("public.zz_cancel_steps_{}", std::process::id());
        let _drop = TableGuard(url.clone(), table.clone());
        let (other, _) = connect(&url).await;
        other.batch_execute(&format!("CREATE TABLE {table} (a int4)")).await.unwrap();
        let mut s = Session::open(&url).await;
        // Flagged while the first statement is bound, as if the user cancelled just as it ended.
        let flag = s.cancel.clone();
        let hook: Box<BeforeBind> = Box::new(move || {
            flag.store(true, Ordering::SeqCst);
            Box::pin(async {})
        });
        let insert = format!("INSERT INTO {table} VALUES (2)");
        let events = s.run_all(&["SELECT 1", &insert, "SELECT 3"], Some(hook.as_ref())).await;
        assert_eq!(
            outline(&events),
            ["started 0", "finished 0: Command(\"SELECT\")", "failed (cancelled true): Cancelled"]
        );
        let count: i64 = other.query_one(&format!("SELECT count(*) FROM {table}"), &[]).await.unwrap().get(0);
        assert_eq!(count, 0, "the statements after the cancel never ran");
        assert!(!s.tx.block && !s.tx.reported, "nothing stays open");
        // The flag is still set from that run: the next run starts afresh.
        let events = s.run_all(&["SELECT 1", &insert], None).await;
        assert_eq!(
            outline(&events),
            ["started 0", "finished 0: Command(\"SELECT\")", "started 1", "done: Affected(1)"]
        );
    }

    /// A statement before the last runs to its end, as psql runs it (a repro:
    /// only its first row used to be fetched, so `nextval` ran once): every row is fetched,
    /// page by page, and comes as the statement's own rows.
    #[tokio::test(flavor = "multi_thread")]
    async fn statements_before_the_last_run_to_their_end() {
        let Some(url) = pg_url("statements_before_the_last_run_to_their_end") else { return };
        let mut s = Session::open(&url).await;
        let events = s
            .run_all(
                &["CREATE TEMP SEQUENCE s", "SELECT nextval('s') FROM generate_series(1, 25)", "SELECT currval('s')"],
                None,
            )
            .await;
        let last = events.iter().find_map(|e| match e {
            DbEvent::Page { columns: Some(_), rows, .. } => Some(rows.clone()),
            _ => None,
        });
        assert_eq!(last, Some(vec![vec![Some("25".to_string())]]), "psql says 25");
        // The second statement's rows: three pages of 10, 10 and 5, columns with the first.
        let pages: Vec<(bool, usize, bool)> = events
            .iter()
            .filter_map(|e| match e {
                DbEvent::StepRows { index: 1, columns, rows, more, .. } => Some((columns.is_some(), rows.len(), *more)),
                _ => None,
            })
            .collect();
        assert_eq!(pages, [(true, 10, true), (false, 10, true), (false, 5, false)]);
        let values: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                DbEvent::StepRows { rows, .. } => Some(rows.iter().map(|r| r[0].clone().unwrap_or_default())),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(values, (1..=25).map(|n| n.to_string()).collect::<Vec<_>>());
        assert!(!s.tx.block && !s.tx.reported, "nothing stays open");
    }

    /// Row locks of a statement before the last cover every row, not only the first.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_locking_read_before_the_last_statement_locks_every_row() {
        let Some(url) = pg_url("a_locking_read_before_the_last_statement_locks_every_row") else { return };
        let table = format!("public.zz_locks_{}", std::process::id());
        let _drop = TableGuard(url.clone(), table.clone());
        let (other, _) = connect(&url).await;
        other.batch_execute(&format!("CREATE TABLE {table} AS SELECT generate_series(1, 23) AS a")).await.unwrap();
        let mut s = Session::open(&url).await;
        s.run_all(&["BEGIN"], None).await;
        s.run_all(&[&format!("SELECT a FROM {table} FOR UPDATE"), "SELECT 1"], None).await;
        let free: i64 = other
            .query_one(&format!("SELECT count(*) FROM (SELECT a FROM {table} FOR UPDATE SKIP LOCKED) x"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(free, 0, "all 23 rows are locked by the open transaction");
        s.run_all(&["ROLLBACK"], None).await;
        let free: i64 = other
            .query_one(&format!("SELECT count(*) FROM (SELECT a FROM {table} FOR UPDATE SKIP LOCKED) x"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(free, 23);
    }

    /// `EXPLAIN ANALYZE` runs its statement to measure it and keeps nothing:
    /// outside the user's block its transaction is rolled back, inside it a savepoint keeps
    /// the block as it was (its earlier uncommitted changes too), even when it fails.
    #[tokio::test(flavor = "multi_thread")]
    async fn explain_analyze_keeps_no_change() {
        let Some(url) = pg_url("explain_analyze_keeps_no_change") else { return };
        let table = format!("public.zz_explain_{}", std::process::id());
        let _drop = TableGuard(url.clone(), table.clone());
        let (other, _) = connect(&url).await;
        other.batch_execute(&format!("CREATE TABLE {table} AS SELECT generate_series(1, 3) AS a")).await.unwrap();
        let count = |events: &[DbEvent]| {
            events.iter().rev().find_map(|e| match e {
                DbEvent::Page { columns: Some(_), rows, .. } => rows[0][0].clone(),
                _ => None,
            })
        };
        let committed =
            || async { other.query_one(&format!("SELECT count(*) FROM {table}"), &[]).await.unwrap().get::<_, i64>(0) };
        let mut s = Session::open(&url).await;
        let delete = format!("EXPLAIN (ANALYZE, COSTS off) DELETE FROM {table}");
        let select = format!("SELECT count(*) FROM {table}");
        // Outside a block: alone, and before another statement of the same run.
        let events = s.run_all(&[&delete], None).await;
        assert!(events.iter().any(|e| matches!(e, DbEvent::Page { columns: Some(_), .. })), "the plan: {events:?}");
        assert_eq!(committed().await, 3);
        assert_eq!(count(&s.run_all(&[&delete, &select], None).await).as_deref(), Some("3"));
        assert!(!s.tx.block && !s.tx.reported, "nothing stays open");
        // Inside the user's block: its own change stays, the explained one does not.
        s.run_all(&["BEGIN"], None).await;
        s.run_all(&[&format!("INSERT INTO {table} VALUES (100)")], None).await;
        s.run_all(&[&delete], None).await;
        assert_eq!(
            count(&s.run_all(&[&select], None).await).as_deref(),
            Some("4"),
            "the insert stays, the delete does not"
        );
        assert!(s.tx.block, "the block is still open");
        // A failing one does not abort the block either.
        let events = s.run_all(&[&format!("EXPLAIN ANALYZE INSERT INTO {table} VALUES (1/0)")], None).await;
        assert!(events.iter().any(|e| matches!(e, DbEvent::Failed { .. })), "{events:?}");
        assert_eq!(count(&s.run_all(&[&select], None).await).as_deref(), Some("4"), "the block still runs statements");
        assert_eq!(committed().await, 3, "nothing is committed yet");
        s.run_all(&["COMMIT"], None).await;
        assert_eq!(committed().await, 4, "the user's own insert is committed");
        assert!(!s.tx.block);
        // An aborted block refuses the savepoint as it refuses the statement.
        s.run_all(&["BEGIN"], None).await;
        s.run_all(&["SELECT 1/0"], None).await;
        let events = s.run_all(&[&delete], None).await;
        assert!(events.iter().any(|e| matches!(e, DbEvent::Failed { .. })), "{events:?}");
        s.run_all(&["ROLLBACK"], None).await;
        assert_eq!(committed().await, 4);
    }

    /// A failure inside the user's block aborts it: `TxAborted(true)` until `ROLLBACK` ends
    /// the block or `ROLLBACK TO SAVEPOINT` repairs it.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_aborted_block_is_reported_until_it_is_repaired_or_ends() {
        let Some(url) = pg_url("an_aborted_block_is_reported_until_it_is_repaired_or_ends") else { return };
        let aborted = |events: &[DbEvent]| {
            events
                .iter()
                .filter_map(|e| match e {
                    DbEvent::TxOpen(o) => Some(format!("open {o}")),
                    DbEvent::TxAborted(a) => Some(format!("aborted {a}")),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mut s = Session::open(&url).await;
        assert_eq!(aborted(&s.run_all(&["BEGIN"], None).await), ["open true"]);
        assert_eq!(aborted(&s.run_all(&["SELECT 1/0"], None).await), ["aborted true"]);
        assert!(aborted(&s.run_all(&["SELECT 1"], None).await).is_empty(), "still aborted");
        assert_eq!(aborted(&s.run_all(&["ROLLBACK"], None).await), ["open false", "aborted false"]);
        s.run_all(&["BEGIN", "SAVEPOINT a"], None).await;
        assert_eq!(aborted(&s.run_all(&["SELECT 1/0"], None).await), ["aborted true"]);
        assert_eq!(aborted(&s.run_all(&["ROLLBACK TO SAVEPOINT a"], None).await), ["aborted false"]);
        assert!(s.tx.block && !s.tx.aborted);
        s.run_all(&["ROLLBACK"], None).await;
    }

    /// The implicit transaction of a portal that ends within the run is not reported: the
    /// indicator no longer flickers during a run of several statements.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_of_reads_does_not_flicker_the_transaction_indicator() {
        let Some(url) = pg_url("a_run_of_reads_does_not_flicker_the_transaction_indicator") else { return };
        let mut s = Session::open(&url).await;
        let events = s.run_all(&["SELECT 1", "SELECT generate_series(1, 25)", "VALUES (1)", "SELECT 2"], None).await;
        let tx: Vec<&DbEvent> = events.iter().filter(|e| matches!(e, DbEvent::TxOpen(_))).collect();
        assert!(tx.is_empty(), "{tx:?}");
        assert!(events.iter().any(|e| matches!(e, DbEvent::Page { columns: Some(_), .. })));
    }

    /// A run that ends without its terminal event (a bug) still answers: the dropped `Reply`
    /// sends `NoResult`, and a debug build says so loudly.
    #[test]
    fn an_unanswered_run_answers_when_dropped() {
        let (events, mut rx) = unbounded_channel();
        let dropped = std::panic::catch_unwind(move || drop(Reply::new(&events, 7)));
        assert_eq!(dropped.is_err(), cfg!(debug_assertions), "the debug assertion");
        assert!(matches!(rx.try_recv(), Ok(DbEvent::Failed { id: 7, error: DbError::NoResult, cancelled: false })));
        assert!(rx.try_recv().is_err(), "once");
        // Answered, or closed by the UI: nothing more. (Each answer consumes the reply, so a
        // second one does not compile.)
        let (events, mut rx) = unbounded_channel();
        Reply::new(&events, 8).fail(DbError::SchemaChanged, false);
        Reply::new(&events, 9).closed();
        assert!(matches!(rx.try_recv(), Ok(DbEvent::Failed { id: 8, .. })));
        assert!(rx.try_recv().is_err());
        // A first page may be followed by one failure, and dropping it sends nothing more.
        let paged = Reply::new(&events, 10).page(Vec::new(), Vec::new(), true, std::time::Duration::ZERO);
        paged.fail(DbError::Closed, false);
        let _paged = Reply::new(&events, 11).page(Vec::new(), Vec::new(), false, std::time::Duration::ZERO);
        assert!(matches!(rx.try_recv(), Ok(DbEvent::Page { id: 10, columns: Some(_), .. })));
        assert!(matches!(rx.try_recv(), Ok(DbEvent::Failed { id: 10, error: DbError::Closed, .. })));
        assert!(matches!(rx.try_recv(), Ok(DbEvent::Page { id: 11, .. })));
        assert!(rx.try_recv().is_err());
    }
}
