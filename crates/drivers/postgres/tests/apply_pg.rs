//! `DbCommand::ApplyEdits` against a real PostgreSQL (`DATARIG_TEST_PG_URL`, as `integration_pg`),
//! with the statements `edit` writes for a result read by the session: row checks lock without
//! waiting, every count is compared, a failed step leaves nothing behind (outside a block,
//! inside one under the driver's savepoint), and manual commit keeps the transaction open.
//! A range pasted into a result goes through the same pipeline (read, laid over the rows as one
//! step, planned, applied), and a paste of 1,000 keyed updates stays within its count of
//! statements sent. Tables are `public.zz_apply_*` of this process, dropped when each test ends.

#[allow(dead_code)]
mod pg_clean;
#[allow(dead_code)]
mod pg_proxy;

use datarig_core::driver::{
    AppliedStep, ApplyError, ApplyFailure, ApplyTx, Cell, ColumnMeta, ConnectOptions, DbCommand, DbError, DbEvent,
    Driver, EditFormat, KeyCatalog, PagingMode, Session, SessionRole, TxRule, TxStep,
};
use datarig_core::edit::{self, Choice, EditBatch, EditTarget, Expect, Facts, Plan, Value};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::Dialect;
use datarig_driver_postgres::PgDriver;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn pg_url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => {
            pg_clean::sweep_stale(&u);
            pg_clean::hold_slot();
            Some(u)
        }
        _ => {
            if std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_PG_URL must be set when DATARIG_REQUIRE_PG=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set DATARIG_TEST_PG_URL");
            None
        }
    }
}

/// Drops a table on a fresh connection when it goes out of scope.
struct Guard(String, String);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = pg_clean::run_fresh(&self.0, &self.1);
    }
}

fn table(test: &str) -> String {
    format!("zz_apply_{test}_{}", std::process::id())
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    id: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, read_only: bool) -> Conn {
        Conn::open_paged(url, role, read_only, 500).await
    }

    /// [`Conn::open`] with pages of `page` rows.
    async fn open_paged(url: &str, role: SessionRole, read_only: bool, page: usize) -> Conn {
        let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
        let (tx, rx) = unbounded_channel();
        let opts = ConnectOptions::new(page, role, &format!("it{}", std::process::id())).read_only(read_only);
        let session = PgDriver.connect(&cfg, role, opts, tx);
        let mut c = Conn { session, rx, id: 0 };
        c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
        c
    }

    async fn wait(&mut self, pred: impl Fn(&DbEvent) -> bool, secs: u64) -> DbEvent {
        self.collect(pred, secs).await.pop().unwrap()
    }

    /// Every event up to the first `pred` takes, that one last.
    async fn collect(&mut self, pred: impl Fn(&DbEvent) -> bool, secs: u64) -> Vec<DbEvent> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("connect failed: {error:?}"),
                Ok(Some(ev)) => {
                    let done = pred(&ev);
                    seen.push(ev);
                    if done {
                        return seen;
                    }
                }
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out after {secs}s waiting for event; saw {seen:?}"),
            }
        }
    }

    /// Run `sql` (one page, the whole result), its answer.
    async fn run_with(&mut self, sql: &str, tx: TxRule) -> DbEvent {
        self.id += 1;
        let id = self.id;
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: PagingMode::NoHold, tx });
        self.wait(
            |e| matches!(e, DbEvent::Page { id: i, columns: Some(_), .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
            60,
        )
        .await
    }

    async fn ok(&mut self, sql: &str) {
        let ev = self.run_with(sql, TxRule::Auto).await;
        assert!(!matches!(ev, DbEvent::Failed { .. }), "{sql}: {ev:?}");
    }

    /// The columns and rows of `sql`.
    async fn read(&mut self, sql: &str) -> (Vec<ColumnMeta>, Vec<Vec<Cell>>) {
        match self.run_with(sql, TxRule::Auto).await {
            DbEvent::Page { columns: Some(c), rows, more: false, .. } => (c, rows),
            other => panic!("{sql}: {other:?}"),
        }
    }

    async fn rows(&mut self, sql: &str) -> Vec<Vec<Cell>> {
        self.read(sql).await.1
    }

    /// Send `plan`'s steps; the events up to its answer, and the answer.
    async fn apply(
        &mut self,
        steps: Vec<edit::Step>,
        tx: ApplyTx,
    ) -> (Vec<DbEvent>, Result<Vec<AppliedStep>, ApplyFailure>) {
        self.apply_within(steps, tx, 30).await
    }

    /// [`Conn::apply`], its answer awaited up to `secs` seconds.
    async fn apply_within(
        &mut self,
        steps: Vec<edit::Step>,
        tx: ApplyTx,
        secs: u64,
    ) -> (Vec<DbEvent>, Result<Vec<AppliedStep>, ApplyFailure>) {
        self.id += 1;
        let id = self.id;
        self.session.send(DbCommand::ApplyEdits { id, steps, tx });
        let mut seen = self.collect(|e| matches!(e, DbEvent::Applied { id: i, .. } if *i == id), secs).await;
        match seen.pop() {
            Some(DbEvent::Applied { result, .. }) => (seen, result),
            _ => unreachable!(),
        }
    }

    /// The events that come within `ms` (the state an answer's changes leave).
    async fn settle(&mut self, ms: u64) -> Vec<DbEvent> {
        let mut seen = Vec::new();
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(ms), self.rx.recv()).await {
            seen.push(ev);
        }
        seen
    }
}

/// The key catalog and the structure of `public.<name>`, as the metadata session reads them.
async fn meta(url: &str, name: &str) -> (KeyCatalog, datarig_core::driver::structure::TableStructure) {
    let mut m = Conn::open(url, SessionRole::Meta, false).await;
    m.session.send(DbCommand::LoadKeys);
    let keys = match m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await {
        DbEvent::Keys(k) => k.expect("keys"),
        _ => unreachable!(),
    };
    m.session.send(DbCommand::LoadStructure { schema: "public".into(), table: name.into() });
    let structure = match m.wait(|e| matches!(e, DbEvent::Structure { .. }), 30).await {
        DbEvent::Structure { result, .. } => *result.expect("structure"),
        _ => unreachable!(),
    };
    (keys, structure)
}

/// What a result read by `select` is, as an editable target of format PostgreSQL.
struct Editing {
    target: EditTarget,
    rows: Vec<Vec<Cell>>,
    format: Box<dyn edit::EditSql>,
}

impl Editing {
    async fn new(url: &str, c: &mut Conn, name: &str, select: &str) -> Editing {
        let (keys, structure) = meta(url, name).await;
        let (columns, rows) = c.read(select).await;
        let format = edit::format(EditFormat::Postgres);
        let source = edit::source(Dialect::Postgres, Some(&keys), &columns, select).expect("an editable result");
        let target = source.target(&structure, &*format).expect("an editable target");
        Editing { target, rows, format }
    }

    fn orig(&self) -> impl Fn(usize) -> Vec<Cell> + '_ {
        |r| self.rows[r].clone()
    }

    /// The plan of `b`, its rows found by their values counted on `c` first.
    async fn plan(&self, c: &mut Conn, b: &EditBatch, choices: &[(usize, Choice)]) -> Plan {
        let whole = |_: usize, _: usize| true;
        let mut matches = BTreeMap::new();
        for (row, sql) in edit::count_statements(&self.target, b, &*self.format, &whole).expect("counts") {
            let n: u64 = c.rows(&sql).await[0][0].as_deref().unwrap().parse().unwrap();
            matches.insert(row, n);
        }
        let choices: BTreeMap<usize, Choice> = choices.iter().copied().collect();
        edit::plan(
            &self.target,
            b,
            &*self.format,
            &Facts {
                whole: &whole,
                matches: &matches,
                choices: &choices,
                invalid: &BTreeSet::new(),
                invalid_for: b.revision(),
            },
        )
        .expect("a plan")
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn count_failure(step: usize, got: u64) -> ApplyFailure {
    ApplyFailure { step: Some(step), why: ApplyError::Count { got } }
}

/// The rows an update and an insert return are in the form the result shows its rows (a
/// boolean as `true`), or said to be in another (a time stamp, which a run shows).
#[tokio::test(flavor = "multi_thread")]
async fn returned_rows_are_in_the_results_form_or_said_not_to_be() {
    let Some(url) = pg_url("returned_rows_are_in_the_results_form_or_said_not_to_be") else {
        return;
    };
    let name = table("forms");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!(
        "CREATE TABLE public.{name} (id int PRIMARY KEY, b bool NOT NULL DEFAULT true, \
         at timestamptz NOT NULL DEFAULT '2024-01-02 03:04:05+00')"
    ))
    .await;
    c.ok(&format!("INSERT INTO public.{name} (id, b) VALUES (1, false)")).await;
    let select = format!("SELECT id, b, at FROM public.{name} ORDER BY id");
    let (_, read) = c.read(&select).await;
    assert_eq!(read[0][1].as_deref(), Some("false"), "the result's form");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 0)], &text("5"), e.orig());
    b.add(1, None, vec![text("2"), Value::Default, Value::Default]);
    let plan = e.plan(&mut c, &b, &[]).await;
    let (_, result) = c.apply(plan.steps.clone(), ApplyTx::Commit).await;
    let done = result.expect("applied");
    for (s, d) in plan.steps.iter().zip(&done) {
        let Some((cols, rows)) = &d.returned else { continue };
        assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "b", "at"], "{}", s.sql);
        let b = if s.sql.starts_with("UPDATE") { "false" } else { "true" };
        assert_eq!(rows[0][1].as_deref(), Some(b), "{}", s.sql);
        assert_eq!(d.unshown, [2], "{}", s.sql);
    }
    assert_eq!(done.iter().filter(|d| d.returned.is_some()).count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keyed_update_delete_and_insert_commit_together_and_return_the_stored_rows() {
    let Some(url) = pg_url("a_keyed_update_delete_and_insert_commit_together_and_return_the_stored_rows") else {
        return;
    };
    let name = table("keyed");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!(
        "CREATE TABLE public.{name} (id int GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, v text, \
         n int NOT NULL DEFAULT 7)"
    ))
    .await;
    c.ok(&format!("INSERT INTO public.{name} (v) VALUES ('a'), ('b'), ('c')")).await;
    let select = format!("SELECT id, v, n FROM public.{name} ORDER BY id");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("it's new"), e.orig());
    b.delete(&[1], e.orig());
    b.add(2, None, vec![Value::Default, text("added"), Value::Default]);
    let plan = e.plan(&mut c, &b, &[]).await;
    let expects: Vec<Expect> = plan.steps.iter().map(|s| s.expect).collect();
    assert_eq!(
        expects,
        [Expect::Rows(1), Expect::Rows(1), Expect::Affected(1), Expect::Affected(1), Expect::Affected(1)]
    );
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    let done = result.expect("applied");
    assert_eq!(done.iter().map(|d| d.count).collect::<Vec<_>>(), [1, 1, 1, 1, 1]);
    // The update and the insert return the rows as stored: the default and the identity.
    let (cols, rows) = done[3].returned.clone().expect("the updated row");
    assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "v", "n"]);
    assert_eq!(rows, [vec![Some("1".into()), Some("it's new".into()), Some("7".into())]]);
    let (_, rows) = done[4].returned.clone().expect("the inserted row");
    assert_eq!(rows, [vec![Some("4".into()), Some("added".into()), Some("7".into())]]);
    assert!(done[0].returned.is_none() && done[2].returned.is_none(), "checks and the delete return nothing");
    // Committed: another session sees it.
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    let rows = side.rows(&select).await;
    let got: Vec<(String, String)> = rows.iter().map(|r| (r[0].clone().unwrap(), r[1].clone().unwrap())).collect();
    assert_eq!(got, [("1".into(), "it's new".into()), ("3".into(), "c".into()), ("4".into(), "added".into())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_changed_elsewhere_fails_its_check_and_nothing_is_written() {
    let Some(url) = pg_url("a_row_changed_elsewhere_fails_its_check_and_nothing_is_written") else { return };
    let name = table("changed");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE public.{name} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO public.{name} VALUES (1, 'a'), (2, 'b')")).await;
    let select = format!("SELECT id, v FROM public.{name} ORDER BY id");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    b.set(&[(1, 1)], &text("mine too"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    // Another session changes the second row's value after it was read.
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    side.ok(&format!("UPDATE public.{name} SET v = 'theirs' WHERE id = 2")).await;
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    assert_eq!(result.unwrap_err(), count_failure(1, 0));
    // Nothing of it was kept, the first row's change neither.
    let rows = side.rows(&select).await;
    assert_eq!(rows, [vec![Some("1".into()), Some("a".into())], vec![Some("2".into()), Some("theirs".into())]]);
    // The session is ready for more: no transaction was left open.
    c.ok(&format!("ALTER TABLE public.{name} ADD COLUMN w int")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_another_session_locks_fails_at_once_without_waiting() {
    let Some(url) = pg_url("a_row_another_session_locks_fails_at_once_without_waiting") else { return };
    let name = table("locked");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE public.{name} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO public.{name} VALUES (1, 'a')")).await;
    let select = format!("SELECT id, v FROM public.{name}");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    side.ok("BEGIN").await;
    side.ok(&format!("SELECT * FROM public.{name} WHERE id = 1 FOR UPDATE")).await;
    let started = Instant::now();
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    let failure = result.unwrap_err();
    assert_eq!(failure.step, Some(0));
    assert!(matches!(failure.why, ApplyError::Db(DbError::LockBusy(_))), "{failure:?}");
    side.ok("ROLLBACK").await;
    assert_eq!(side.rows(&select).await, [vec![Some("1".into()), Some("a".into())]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_of_identical_rows_of_a_keyless_partitioned_table_changes_just_one() {
    let Some(url) = pg_url("one_of_identical_rows_of_a_keyless_partitioned_table_changes_just_one") else { return };
    let name = table("parts");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE public.{name} (k int NOT NULL, v text) PARTITION BY LIST (k)")).await;
    c.ok(&format!("CREATE TABLE public.{name}_1 PARTITION OF public.{name} FOR VALUES IN (1)")).await;
    c.ok(&format!("CREATE TABLE public.{name}_2 PARTITION OF public.{name} FOR VALUES IN (2)")).await;
    c.ok(&format!("INSERT INTO public.{name} VALUES (1, 'same'), (1, 'same'), (1, 'same'), (2, 'same')")).await;
    let select = format!("SELECT k, v FROM public.{name} ORDER BY k");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("one"), e.orig());
    let plan = e.plan(&mut c, &b, &[(0, Choice::One)]).await;
    assert_eq!(plan.steps[0].expect, Expect::Rows(3), "the check locks all three");
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    assert_eq!(result.expect("applied").iter().map(|d| d.count).collect::<Vec<_>>(), [3, 1]);
    let rows = c.rows(&format!("SELECT k, v, count(*) FROM public.{name} GROUP BY k, v ORDER BY k, v")).await;
    let got: Vec<Vec<Option<&str>>> = rows.iter().map(|r| r.iter().map(|c| c.as_deref()).collect()).collect();
    assert_eq!(
        got,
        [
            vec![Some("1"), Some("one"), Some("1")],
            vec![Some("1"), Some("same"), Some("2")],
            vec![Some("2"), Some("same"), Some("1")]
        ]
    );
    // All of them.
    let (_, rows) = c.read(&select).await;
    let e = Editing { rows, ..e };
    let same = e.rows.iter().position(|r| r[1].as_deref() == Some("same")).unwrap();
    let mut b = EditBatch::new();
    b.set(&[(same, 1)], &text("all"), e.orig());
    let plan = e.plan(&mut c, &b, &[(same, Choice::All)]).await;
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    assert_eq!(result.expect("applied").iter().map(|d| d.count).collect::<Vec<_>>(), [2, 2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn in_an_open_block_a_failed_apply_goes_back_to_its_savepoint_and_keeps_what_came_before() {
    let Some(url) = pg_url("in_an_open_block_a_failed_apply_goes_back_to_its_savepoint_and_keeps_what_came_before")
    else {
        return;
    };
    let name = table("savepoint");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE public.{name} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO public.{name} VALUES (1, 'a'), (2, 'b')")).await;
    let select = format!("SELECT id, v FROM public.{name} ORDER BY id");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    // An earlier write of the user's opens a transaction for manual commit.
    let manual = TxRule::Manual { steps: vec![TxStep::Open], idle_timeout: None };
    let ev = c.run_with(&format!("INSERT INTO public.{name} VALUES (3, 'earlier')"), manual).await;
    assert!(matches!(ev, DbEvent::Done { .. }), "{ev:?}");
    c.settle(200).await;
    // The second insert collides with the key the earlier write took: the Apply fails.
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    b.add(1, None, vec![text("3"), text("dup")]);
    let plan = e.plan(&mut c, &b, &[]).await;
    let last = plan.steps.len() - 1;
    let (_, result) = c.apply(plan.steps, ApplyTx::Keep { idle_timeout: None }).await;
    let failure = result.unwrap_err();
    assert_eq!(failure.step, Some(last));
    assert!(matches!(&failure.why, ApplyError::Db(DbError::Server(s)) if s.contains("duplicate key")), "{failure:?}");
    let after = c.settle(300).await;
    assert!(!after.iter().any(|e| matches!(e, DbEvent::Block(false) | DbEvent::TxAborted(true))), "{after:?}");
    // The block is open and not aborted, with the earlier write and without the Apply's.
    let rows = c.rows(&select).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][1].as_deref(), Some("a"));
    assert_eq!(rows[2][1].as_deref(), Some("earlier"));
    // A working Apply in the block is not committed: another session does not see it.
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    assert!(result.is_ok(), "{result:?}");
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    assert_eq!(side.rows(&select).await[0][1].as_deref(), Some("a"));
    c.ok("ROLLBACK").await;
    assert_eq!(c.rows(&select).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_commit_opens_a_transaction_the_apply_leaves_open_and_a_rollback_undoes_it() {
    let Some(url) = pg_url("manual_commit_opens_a_transaction_the_apply_leaves_open_and_a_rollback_undoes_it") else {
        return;
    };
    let name = table("keep");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE public.{name} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO public.{name} VALUES (1, 'a')")).await;
    let select = format!("SELECT id, v FROM public.{name}");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("kept open"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    let (_, result) = c.apply(plan.steps, ApplyTx::Keep { idle_timeout: Some(Duration::from_secs(600)) }).await;
    assert!(result.is_ok(), "{result:?}");
    let after = c.collect(|e| matches!(e, DbEvent::TxOpen(true)), 5).await;
    assert!(matches!(after[0], DbEvent::ManualBegun { index: 0, .. }), "{after:?}");
    assert!(after.iter().any(|e| matches!(e, DbEvent::Block(true))), "{after:?}");
    // The idle limit is the transaction's.
    let limit = c.rows("SHOW idle_in_transaction_session_timeout").await;
    assert_eq!(limit[0][0].as_deref(), Some("10min"));
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    assert_eq!(side.rows(&select).await[0][1].as_deref(), Some("a"), "not committed");
    c.id += 1;
    let id = c.id;
    c.session.send(DbCommand::EndTx { id, commit: false });
    c.wait(|e| matches!(e, DbEvent::TxEnded { id: i, .. } if *i == id), 10).await;
    assert_eq!(c.rows(&select).await[0][1].as_deref(), Some("a"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_block_and_a_read_only_session_refuse_an_apply_unsent() {
    let Some(url) = pg_url("an_aborted_block_and_a_read_only_session_refuse_an_apply_unsent") else { return };
    let name = table("refused");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE public.{name} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO public.{name} VALUES (1, 'a')")).await;
    let select = format!("SELECT id, v FROM public.{name}");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    c.ok("BEGIN").await;
    let ev = c.run_with("SELECT 1/0", TxRule::Auto).await;
    assert!(matches!(ev, DbEvent::Failed { .. }), "{ev:?}");
    let (_, result) = c.apply(plan.steps.clone(), ApplyTx::Commit).await;
    assert_eq!(result.unwrap_err(), ApplyFailure { step: None, why: ApplyError::Db(DbError::NotSupported) });
    c.ok("ROLLBACK").await;
    let mut ro = Conn::open(&url, SessionRole::Query, true).await;
    let (_, result) = ro.apply(plan.steps, ApplyTx::Commit).await;
    assert_eq!(result.unwrap_err(), ApplyFailure { step: None, why: ApplyError::Db(DbError::ReadWriteRefused) });
    assert_eq!(c.rows(&select).await[0][1].as_deref(), Some("a"));
}

/// A 3×3 block pasted from the second column of the first row: two rows overwritten, one new row
/// (its key left to the identity), read, laid over the rows as one step, planned and applied.
#[tokio::test(flavor = "multi_thread")]
async fn a_pasted_block_overwrites_two_rows_and_adds_one() {
    let Some(url) = pg_url("a_pasted_block_overwrites_two_rows_and_adds_one") else {
        return;
    };
    let name = table("paste");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!(
        "CREATE TABLE public.{name} (id int GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, v text, \
         n int NOT NULL DEFAULT 7, w text)"
    ))
    .await;
    c.ok(&format!("INSERT INTO public.{name} (v, n, w) VALUES ('a', 1, 'x'), ('b', 2, NULL)")).await;
    let select = format!("SELECT id, v, n, w FROM public.{name} ORDER BY id");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    let pasted = edit::paste::parse("a1\t10\tx1\r\nb1\t20\t\\N\r\nc1\t30\tz1\r\n", &Default::default()).expect("TSV");
    assert_eq!(pasted.format, edit::paste::Format::Tsv);
    let mut ops = edit::PasteOps { width: 4, ..Default::default() };
    for (r, row) in pasted.rows.iter().enumerate() {
        let cells = row.iter().enumerate().map(|(k, v)| (k + 1, v.clone()));
        if r < 2 {
            ops.set.extend(cells.map(|(col, v)| ((r, col), v)));
        } else {
            let mut new = vec![Value::Default; 4];
            for (col, v) in cells {
                new[col] = v;
            }
            ops.add.push(new);
        }
    }
    let mut b = EditBatch::new();
    let done = b.paste(ops, e.orig()).expect("pasted");
    // Row 2's `w` was NULL already.
    assert_eq!((done.changed, done.added.len()), (5, 1));
    let plan = e.plan(&mut c, &b, &[]).await;
    let (_, result) = c.apply(plan.steps, ApplyTx::Commit).await;
    result.expect("applied");
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    let rows = side.rows(&select).await;
    let s = |v: &str| Some(v.to_string());
    assert_eq!(
        rows,
        [
            vec![s("1"), s("a1"), s("10"), s("x1")],
            vec![s("2"), s("b1"), s("20"), None],
            vec![s("3"), s("c1"), s("30"), s("z1")]
        ]
    );
    // Undone, the paste is gone as one step.
    assert!(b.undo());
    assert!(b.is_empty());
}

/// How long the Apply of 2,000 statements, sent one by one, may take: a slow runner needs
/// far more than the other tests' wait.
const PASTE_1K_SECS: u64 = 180;

/// 1,000 keyed updates from one paste: a check and a write each, between `BEGIN` and `COMMIT`,
/// so at most 2 + 2 × 1,000 statements go to the server (counted at the wire, as `Parse` and
/// simple `Query` messages: each one is waited for).
#[tokio::test(flavor = "multi_thread")]
async fn a_paste_of_a_thousand_keyed_updates_stays_within_its_statements() {
    let Some(url) = pg_url("a_paste_of_a_thousand_keyed_updates_stays_within_its_statements") else {
        return;
    };
    let name = table("paste_1k");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS public.{name}"));
    let proxy = pg_proxy::Proxy::start(&pg_proxy::upstream(&url)).await;
    let mut c = Conn::open_paged(&proxy.url(&url), SessionRole::Query, false, 2_000).await;
    c.ok(&format!("CREATE TABLE public.{name} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO public.{name} SELECT g, 'old' FROM generate_series(1, 1000) g")).await;
    let select = format!("SELECT id, v FROM public.{name} ORDER BY id");
    let e = Editing::new(&url, &mut c, &name, &select).await;
    assert_eq!(e.rows.len(), 1_000);
    let text: String = (1..=1_000).map(|i| format!("new {i}\n")).collect();
    let pasted = edit::paste::parse(&text, &Default::default()).expect("one column");
    let mut ops = edit::PasteOps { width: 2, ..Default::default() };
    ops.set = pasted.rows.iter().enumerate().map(|(r, row)| ((r, 1), row[0].clone())).collect();
    let mut b = EditBatch::new();
    assert_eq!(b.paste(ops, e.orig()).expect("pasted").changed, 1_000);
    let plan = e.plan(&mut c, &b, &[]).await;
    assert_eq!(plan.steps.len(), 2_000, "a check and a write per row");
    let before = proxy.sent().len();
    let (_, result) = c.apply_within(plan.steps, ApplyTx::Commit, PASTE_1K_SECS).await;
    assert_eq!(result.expect("applied").len(), 2_000);
    let sent = proxy.sent().len() - before;
    assert!(sent <= 2 + 2 * 1_000, "{sent} statements sent");
    let rows = c.rows(&format!("SELECT count(*) FROM public.{name} WHERE v LIKE 'new %'")).await;
    assert_eq!(rows[0][0].as_deref(), Some("1000"));
}
