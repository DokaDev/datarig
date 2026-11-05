//! `DbCommand::ApplyEdits` against a real MySQL (`DATARIG_TEST_MYSQL_URL`, as
//! `integration_mysql`), with the statements `edit` writes for a result read by the session: row
//! checks lock without waiting, a write counts the rows it matched (a value set to what it was
//! too), text compares exactly (case, trailing spaces), one of identical rows changes just one,
//! a failed step leaves nothing behind, and manual commit keeps the transaction open. A range
//! pasted into a result goes through the same pipeline. Tables are `shop.zz_it_apply_*` of this
//! process, dropped when each test ends.

use datarig_core::driver::{
    AppliedStep, ApplyError, ApplyFailure, ApplyTx, Cell, ColumnMeta, ConnectOptions, DbCommand, DbError, DbEvent,
    Driver, EditFormat, KeyCatalog, MySqlEdit, PagingMode, Session, SessionRole, TxRule, TxStep,
};
use datarig_core::edit::{self, Choice, EditBatch, EditTarget, Expect, Facts, Plan, PlanError, Value};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_MYSQL_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_MYSQL_URL must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set DATARIG_TEST_MYSQL_URL");
            None
        }
    }
}

fn profile(url: &str) -> ConnectionConfig {
    let sslmode = std::env::var("DATARIG_TEST_MYSQL_SSLMODE").unwrap_or_else(|_| "DISABLED".into());
    ConnectionConfig {
        name: "it".into(),
        driver: "mysql".into(),
        dsn: Some(url.to_string()),
        sslmode,
        ..Default::default()
    }
}

async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("the side connection opens")
}

/// `shop.zz_it_apply_<name>_<pid>`, made on a side connection, dropped with the guard.
struct Table {
    url: String,
    name: String,
}

impl Table {
    async fn make(url: &str, name: &str, ddl: &[&str]) -> Table {
        let name = format!("zz_it_apply_{name}_{}", std::process::id());
        let mut c = side(url).await;
        c.query_drop(format!("DROP TABLE IF EXISTS shop.{name}")).await.unwrap();
        let t = Table { url: url.to_string(), name };
        for sql in ddl {
            c.query_drop(sql.replace("{t}", &t.q())).await.unwrap();
        }
        c.disconnect().await.unwrap();
        t
    }

    fn q(&self) -> String {
        format!("shop.{}", self.name)
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        let (url, name) = (self.url.clone(), self.name.clone());
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&url).await;
                let _ = c.query_drop(format!("DROP TABLE IF EXISTS shop.{name}")).await;
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    id: u64,
    dialect: Dialect,
    edit: Option<EditFormat>,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, read_only: bool) -> Conn {
        Conn::open_paged(url, role, read_only, 500).await
    }

    /// [`Conn::open`] with pages of `page` rows.
    async fn open_paged(url: &str, role: SessionRole, read_only: bool, page: usize) -> Conn {
        let (tx, rx) = unbounded_channel();
        let tag = format!("it{}-apply", std::process::id());
        let opts = ConnectOptions::new(page, role, &tag).read_only(read_only);
        let session = MyDriver.connect(&profile(url), role, opts, tx);
        let mut c = Conn { session, rx, id: 0, dialect: Dialect::MySql(Default::default()), edit: None };
        for ev in c.collect(|e| matches!(e, DbEvent::Edit(_)), 15).await {
            match ev {
                DbEvent::Language(Language::Sql(d)) => c.dialect = d,
                DbEvent::Edit(f) => c.edit = f,
                _ => {}
            }
        }
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

    async fn read(&mut self, sql: &str) -> (Vec<ColumnMeta>, Vec<Vec<Cell>>) {
        match self.run_with(sql, TxRule::Auto).await {
            DbEvent::Page { columns: Some(c), rows, more: false, .. } => (c, rows),
            other => panic!("{sql}: {other:?}"),
        }
    }

    async fn rows(&mut self, sql: &str) -> Vec<Vec<Cell>> {
        self.read(sql).await.1
    }

    async fn apply(&mut self, steps: Vec<edit::Step>, tx: ApplyTx) -> Result<Vec<AppliedStep>, ApplyFailure> {
        self.apply_within(steps, tx, 30).await
    }

    /// [`Conn::apply`], its answer awaited up to `secs` seconds.
    async fn apply_within(
        &mut self,
        steps: Vec<edit::Step>,
        tx: ApplyTx,
        secs: u64,
    ) -> Result<Vec<AppliedStep>, ApplyFailure> {
        self.id += 1;
        let id = self.id;
        self.session.send(DbCommand::ApplyEdits { id, steps, tx });
        match self.wait(|e| matches!(e, DbEvent::Applied { id: i, .. } if *i == id), secs).await {
            DbEvent::Applied { result, .. } => result,
            _ => unreachable!(),
        }
    }

    async fn settle(&mut self, ms: u64) -> Vec<DbEvent> {
        let mut seen = Vec::new();
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(ms), self.rx.recv()).await {
            seen.push(ev);
        }
        seen
    }
}

async fn meta(url: &str, t: &Table) -> (KeyCatalog, datarig_core::driver::structure::TableStructure) {
    let mut m = Conn::open(url, SessionRole::Meta, false).await;
    m.session.send(DbCommand::LoadKeys);
    let keys = match m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await {
        DbEvent::Keys(k) => k.expect("keys"),
        _ => unreachable!(),
    };
    m.session.send(DbCommand::LoadStructure { schema: "shop".into(), table: t.name.clone() });
    let structure = match m.wait(|e| matches!(e, DbEvent::Structure { .. }), 30).await {
        DbEvent::Structure { result, .. } => *result.expect("structure"),
        _ => unreachable!(),
    };
    (keys, structure)
}

struct Editing {
    target: EditTarget,
    rows: Vec<Vec<Cell>>,
    format: Box<dyn edit::EditSql>,
}

impl Editing {
    async fn new(url: &str, c: &mut Conn, t: &Table, select: &str) -> Editing {
        let (keys, structure) = meta(url, t).await;
        let (columns, rows) = c.read(select).await;
        let format = edit::format(c.edit.expect("the server takes edits"));
        let source = edit::source(c.dialect, Some(&keys), &columns, select).expect("an editable result");
        let target = source.target(&structure, &*format).expect("an editable target");
        Editing { target, rows, format }
    }

    fn orig(&self) -> impl Fn(usize) -> Vec<Cell> + '_ {
        |r| self.rows[r].clone()
    }

    /// What each changed row found by its values matches, counted on `c`.
    async fn counts(&self, c: &mut Conn, b: &EditBatch) -> BTreeMap<usize, u64> {
        let whole = |_: usize, _: usize| true;
        let mut matches = BTreeMap::new();
        for (row, sql) in edit::count_statements(&self.target, b, &*self.format, &whole).expect("counts") {
            let n: u64 = c.rows(&sql).await[0][0].as_deref().unwrap().parse().unwrap();
            matches.insert(row, n);
        }
        matches
    }

    async fn try_plan(&self, c: &mut Conn, b: &EditBatch, choices: &[(usize, Choice)]) -> Result<Plan, PlanError> {
        let whole = |_: usize, _: usize| true;
        let matches = self.counts(c, b).await;
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
    }

    async fn plan(&self, c: &mut Conn, b: &EditBatch, choices: &[(usize, Choice)]) -> Plan {
        self.try_plan(c, b, choices).await.expect("a plan")
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn counts(done: &[AppliedStep]) -> Vec<u64> {
    done.iter().map(|d| d.count).collect()
}

fn cells(rows: &[Vec<Cell>]) -> Vec<Vec<Option<&str>>> {
    rows.iter().map(|r| r.iter().map(|c| c.as_deref()).collect()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_takes_edits_says_so_and_a_keyed_apply_commits_with_the_insert_id() {
    let Some(url) = url("a_server_that_takes_edits_says_so_and_a_keyed_apply_commits_with_the_insert_id") else {
        return;
    };
    let t = Table::make(
        &url,
        "keyed",
        &[
            "CREATE TABLE {t} (id INT AUTO_INCREMENT PRIMARY KEY, v VARCHAR(20), n INT NOT NULL DEFAULT 7)",
            "INSERT INTO {t} (v) VALUES ('a'), ('b'), ('c')",
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    assert_eq!(c.edit, Some(EditFormat::MySql(MySqlEdit { nowait: true })));
    let select = format!("SELECT id, v, n FROM {} ORDER BY id", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
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
    let done = c.apply(plan.steps, ApplyTx::Commit).await.expect("applied");
    assert_eq!(counts(&done), [1, 1, 1, 1, 1]);
    assert_eq!(done.iter().map(|d| d.insert_id).collect::<Vec<_>>(), [None, None, None, None, Some(4)]);
    assert!(done.iter().all(|d| d.returned.is_none()));
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let rows = other.rows(&select).await;
    assert_eq!(
        cells(&rows),
        [
            vec![Some("1"), Some("it's new"), Some("7")],
            vec![Some("3"), Some("c"), Some("7")],
            vec![Some("4"), Some("added"), Some("7")]
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_set_to_what_the_table_holds_counts_as_the_row_it_matched() {
    let Some(url) = url("a_value_set_to_what_the_table_holds_counts_as_the_row_it_matched") else { return };
    let t = Table::make(
        &url,
        "found",
        &["CREATE TABLE {t} (id INT PRIMARY KEY, d DECIMAL(5,1))", "INSERT INTO {t} VALUES (1, 1.0)"],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT id, d FROM {}", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    let mut b = EditBatch::new();
    // `1` is not the text read (`1.0`), and the same number: the row is matched, not changed.
    b.set(&[(0, 1)], &text("1"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    let done = c.apply(plan.steps, ApplyTx::Commit).await.expect("a matched row counts");
    assert_eq!(counts(&done), [1, 1]);
    // The user's own statements count matched rows too.
    let ev = c.run_with(&format!("UPDATE {} SET d = 1 WHERE id = 1", t.q()), TxRule::Auto).await;
    assert!(matches!(&ev, DbEvent::Done { outcome: datarig_core::driver::Outcome::Affected(1), .. }), "{ev:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_changed_elsewhere_or_locked_fails_and_nothing_is_written() {
    let Some(url) = url("a_row_changed_elsewhere_or_locked_fails_and_nothing_is_written") else { return };
    let t = Table::make(
        &url,
        "changed",
        &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(20))", "INSERT INTO {t} VALUES (1, 'a'), (2, 'b')"],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT id, v FROM {} ORDER BY id", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    b.set(&[(1, 1)], &text("mine too"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    let mut s = side(&url).await;
    s.query_drop(format!("UPDATE {} SET v = 'theirs' WHERE id = 2", t.q())).await.unwrap();
    let failure = c.apply(plan.steps.clone(), ApplyTx::Commit).await.unwrap_err();
    assert_eq!(failure, ApplyFailure { step: Some(1), why: ApplyError::Count { got: 0 } });
    // Locked by another session: fails at once.
    s.query_drop(format!("UPDATE {} SET v = 'b' WHERE id = 2", t.q())).await.unwrap();
    s.query_drop("START TRANSACTION").await.unwrap();
    s.query_drop(format!("SELECT * FROM {} WHERE id = 1 FOR UPDATE", t.q())).await.unwrap();
    let started = Instant::now();
    let failure = c.apply(plan.steps, ApplyTx::Commit).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(failure.step, Some(0));
    assert!(matches!(&failure.why, ApplyError::Db(DbError::LockBusy(m)) if m.contains("3572")), "{failure:?}");
    s.query_drop("ROLLBACK").await.unwrap();
    let rows: Vec<(i32, String)> = s.query(format!("SELECT id, v FROM {} ORDER BY id", t.q())).await.unwrap();
    assert_eq!(rows, [(1, "a".to_string()), (2, "b".to_string())]);
    s.disconnect().await.unwrap();
    // Nothing is left open on the session.
    c.ok(&format!("ALTER TABLE {} ADD COLUMN w INT", t.q())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn identical_rows_of_a_keyless_table_change_one_or_all_and_text_compares_exactly() {
    let Some(url) = url("identical_rows_of_a_keyless_table_change_one_or_all_and_text_compares_exactly") else {
        return;
    };
    let t = Table::make(
        &url,
        "keyless",
        &[
            "CREATE TABLE {t} (k INT, v VARCHAR(10) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci)",
            "INSERT INTO {t} VALUES (1, 'same'), (1, 'same'), (1, 'same'), (2, 'abc'), (2, 'ABC'), (3, 'a'), (3, 'a ')",
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT k, v FROM {} ORDER BY k, v COLLATE utf8mb4_bin", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    // `abc` and `ABC` are equal in the column's collation, `a` and `a ` too (PAD SPACE): each
    // matches only itself.
    let find = |v: &str| e.rows.iter().position(|r| r[1].as_deref() == Some(v)).unwrap();
    let mut b = EditBatch::new();
    b.set(&[(find("abc"), 1)], &text("x"), e.orig());
    b.set(&[(find("a "), 1)], &text("y"), e.orig());
    let matches = e.counts(&mut c, &b).await;
    assert_eq!(matches.values().copied().collect::<Vec<_>>(), [1, 1]);
    let plan = e.plan(&mut c, &b, &[]).await;
    c.apply(plan.steps, ApplyTx::Commit).await.expect("applied");
    // One of three identical rows.
    let same = find("same");
    let mut b = EditBatch::new();
    b.set(&[(same, 1)], &text("one"), e.orig());
    let plan = e.plan(&mut c, &b, &[(same, Choice::One)]).await;
    assert_eq!(counts(&c.apply(plan.steps, ApplyTx::Commit).await.expect("applied")), [3, 1]);
    // Grouped in a collation that tells every one of them apart (`utf8mb4_bin` pads).
    let grouped =
        format!("SELECT k, v COLLATE utf8mb4_0900_bin AS b, COUNT(*) FROM {} GROUP BY k, b ORDER BY k, b", t.q());
    let rows = c.rows(&grouped).await;
    assert_eq!(
        cells(&rows),
        [
            vec![Some("1"), Some("one"), Some("1")],
            vec![Some("1"), Some("same"), Some("2")],
            vec![Some("2"), Some("ABC"), Some("1")],
            vec![Some("2"), Some("x"), Some("1")],
            vec![Some("3"), Some("a"), Some("1")],
            vec![Some("3"), Some("y"), Some("1")]
        ]
    );
    // All of the two left.
    let (_, rows) = c.read(&select).await;
    let e = Editing { rows, ..e };
    let same = e.rows.iter().position(|r| r[1].as_deref() == Some("same")).unwrap();
    let mut b = EditBatch::new();
    b.set(&[(same, 1)], &text("all"), e.orig());
    let plan = e.plan(&mut c, &b, &[(same, Choice::All)]).await;
    assert_eq!(counts(&c.apply(plan.steps, ApplyTx::Commit).await.expect("applied")), [2, 2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_that_differ_only_where_values_do_not_compare_are_refused() {
    let Some(url) = url("rows_that_differ_only_where_values_do_not_compare_are_refused") else { return };
    let t = Table::make(
        &url,
        "float",
        &["CREATE TABLE {t} (k INT, f DOUBLE)", "INSERT INTO {t} VALUES (1, 0.5), (1, 0.25)"],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT k, f FROM {} ORDER BY f", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 0)], &text("9"), e.orig());
    assert_eq!(e.try_plan(&mut c, &b, &[]).await.unwrap_err(), PlanError::Ambiguous { row: 0, matches: 2 });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_table_whose_invisible_key_the_result_hides_is_edited_by_its_values() {
    let Some(url) = url("a_table_whose_invisible_key_the_result_hides_is_edited_by_its_values") else { return };
    let t = Table::make(
        &url,
        "gipk",
        &[
            "CREATE TABLE {t} (my_row_id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT INVISIBLE PRIMARY KEY, v VARCHAR(10))",
            "INSERT INTO {t} (v) VALUES ('a'), ('b')",
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT * FROM {} ORDER BY v", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    assert!(matches!(e.target.identity, edit::Identity::Values { .. }), "{:?}", e.target.identity);
    let mut b = EditBatch::new();
    b.set(&[(1, 0)], &text("c"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    assert_eq!(counts(&c.apply(plan.steps, ApplyTx::Commit).await.expect("applied")), [1, 1]);
    let rows = c.rows(&format!("SELECT my_row_id, v FROM {} ORDER BY my_row_id", t.q())).await;
    assert_eq!(cells(&rows), [vec![Some("1"), Some("a")], vec![Some("2"), Some("c")]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn in_an_open_transaction_a_failed_apply_goes_back_to_its_savepoint_and_manual_commit_keeps_it_open() {
    let Some(url) =
        url("in_an_open_transaction_a_failed_apply_goes_back_to_its_savepoint_and_manual_commit_keeps_it_open")
    else {
        return;
    };
    let t = Table::make(
        &url,
        "savepoint",
        &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(20))", "INSERT INTO {t} VALUES (1, 'a'), (2, 'b')"],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT id, v FROM {} ORDER BY id", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    // Manual commit, no transaction open: the Apply opens one and leaves it open.
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("kept open"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    c.apply(plan.steps, ApplyTx::Keep { idle_timeout: None }).await.expect("applied");
    let after = c.collect(|e| matches!(e, DbEvent::TxOpen(true)), 5).await;
    assert!(matches!(after[0], DbEvent::ManualBegun { index: 0, .. }), "{after:?}");
    let mut s = side(&url).await;
    let seen: Option<String> = s.query_first(format!("SELECT v FROM {} WHERE id = 1", t.q())).await.unwrap();
    assert_eq!(seen.as_deref(), Some("a"), "not committed");
    // A later statement of the user's in it, then an Apply that fails on its insert.
    let manual = TxRule::Manual { steps: vec![TxStep::Open], idle_timeout: None };
    let ev = c.run_with(&format!("INSERT INTO {} VALUES (3, 'earlier')", t.q()), manual).await;
    assert!(matches!(ev, DbEvent::Done { .. }), "{ev:?}");
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("mine"), e.orig());
    b.add(1, None, vec![text("3"), text("dup")]);
    let plan = e.plan(&mut c, &b, &[]).await;
    let last = plan.steps.len() - 1;
    let failure = c.apply(plan.steps, ApplyTx::Keep { idle_timeout: None }).await.unwrap_err();
    assert_eq!(failure.step, Some(last));
    assert!(matches!(&failure.why, ApplyError::Db(DbError::Server(m)) if m.contains("1062")), "{failure:?}");
    let after = c.settle(300).await;
    assert!(!after.iter().any(|e| matches!(e, DbEvent::Block(false) | DbEvent::TxClosedBy { .. })), "{after:?}");
    let rows = c.rows(&select).await;
    assert_eq!(
        cells(&rows),
        [vec![Some("1"), Some("kept open")], vec![Some("2"), Some("b")], vec![Some("3"), Some("earlier")]]
    );
    c.id += 1;
    let id = c.id;
    c.session.send(DbCommand::EndTx { id, commit: false });
    c.wait(|e| matches!(e, DbEvent::TxEnded { id: i, .. } if *i == id), 10).await;
    let rows = c.rows(&select).await;
    assert_eq!(cells(&rows), [vec![Some("1"), Some("a")], vec![Some("2"), Some("b")]]);
    s.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_refuses_an_apply_unsent() {
    let Some(url) = url("a_read_only_session_refuses_an_apply_unsent") else { return };
    let t = Table::make(
        &url,
        "readonly",
        &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(20))", "INSERT INTO {t} VALUES (1, 'a')"],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT id, v FROM {}", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("mine"), e.orig());
    let plan = e.plan(&mut c, &b, &[]).await;
    let mut ro = Conn::open(&url, SessionRole::Query, true).await;
    let failure = ro.apply(plan.steps, ApplyTx::Commit).await.unwrap_err();
    assert_eq!(failure, ApplyFailure { step: None, why: ApplyError::Db(DbError::ReadWriteRefused) });
    assert_eq!(cells(&c.rows(&select).await), [vec![Some("1"), Some("a")]]);
}

/// A 3×3 block pasted from the second column of the first row: two rows overwritten, one new row
/// (its key left to `AUTO_INCREMENT`), read, laid over the rows as one step, planned and applied.
#[tokio::test(flavor = "multi_thread")]
async fn a_pasted_block_overwrites_two_rows_and_adds_one() {
    let Some(url) = url("a_pasted_block_overwrites_two_rows_and_adds_one") else {
        return;
    };
    let t = Table::make(
        &url,
        "paste",
        &[
            "CREATE TABLE {t} (id INT AUTO_INCREMENT PRIMARY KEY, v VARCHAR(20), n INT NOT NULL DEFAULT 7, \
             w VARCHAR(20))",
            "INSERT INTO {t} (v, n, w) VALUES ('a', 1, 'x'), ('b', 2, NULL)",
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT id, v, n, w FROM {} ORDER BY id", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    let pasted =
        edit::paste::parse_as("a1,10,x1\nb1,20,\\N\nc1,30,z1\n", edit::paste::Format::Csv, &Default::default())
            .expect("CSV");
    assert_eq!(pasted.format, edit::paste::Format::Csv);
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
    assert_eq!((done.changed, done.added.len()), (5, 1));
    let plan = e.plan(&mut c, &b, &[]).await;
    c.apply(plan.steps, ApplyTx::Commit).await.expect("applied");
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let rows = other.rows(&select).await;
    assert_eq!(
        cells(&rows),
        [
            vec![Some("1"), Some("a1"), Some("10"), Some("x1")],
            vec![Some("2"), Some("b1"), Some("20"), None],
            vec![Some("3"), Some("c1"), Some("30"), Some("z1")]
        ]
    );
}

/// Cells as the grid's `y` keeps them (the text as read, NULL as NULL: no text read again) pasted
/// below themselves: a tab, a line break, the NULL token as text and an empty text come back as
/// they were, NULL as NULL.
#[tokio::test(flavor = "multi_thread")]
async fn cells_copied_in_the_grid_paste_back_exactly() {
    let Some(url) = url("cells_copied_in_the_grid_paste_back_exactly") else {
        return;
    };
    let t = Table::make(
        &url,
        "gridcopy",
        &[
            "CREATE TABLE {t} (id INT AUTO_INCREMENT PRIMARY KEY, v VARCHAR(20), w VARCHAR(20))",
            "INSERT INTO {t} (v, w) VALUES ('a\\tb\\nc', NULL), ('\\\\N', ''), ('x', 'y')",
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let select = format!("SELECT id, v, w FROM {} ORDER BY id", t.q());
    let read = c.rows(&select).await;
    let e = Editing::new(&url, &mut c, &t, &select).await;
    // Rows 1–2, `v` and `w`, onto row 3 and a new row.
    let copy: Vec<Vec<Value>> =
        read[..2].iter().map(|r| r[1..].iter().map(|v| v.clone().map_or(Value::Null, Value::Text)).collect()).collect();
    let mut ops = edit::PasteOps { width: 3, ..Default::default() };
    ops.set.extend(copy[0].iter().enumerate().map(|(k, v)| ((2, k + 1), v.clone())));
    ops.add.push(vec![Value::Default, copy[1][0].clone(), copy[1][1].clone()]);
    let mut b = EditBatch::new();
    let done = b.paste(ops, e.orig()).expect("pasted");
    assert_eq!((done.changed, done.added.len()), (2, 1));
    let plan = e.plan(&mut c, &b, &[]).await;
    c.apply(plan.steps, ApplyTx::Commit).await.expect("applied");
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let rows = other.rows(&select).await;
    assert_eq!(cells(&rows)[0], [Some("1"), Some("a\tb\nc"), None]);
    assert_eq!(cells(&rows)[2], [Some("3"), Some("a\tb\nc"), None]);
    assert_eq!(cells(&rows)[3], [Some("4"), Some("\\N"), Some("")]);
}

/// How long the Apply of 2,000 statements, sent one by one, may take: a slow runner needs
/// far more than the other tests' wait.
const PASTE_1K_SECS: u64 = 180;

/// 1,000 keyed updates from one paste: a check and a write each, one statement per step (the
/// executor sends them one by one, between the transaction's start and its commit), so at most
/// 2 + 2 × 1,000 statements.
#[tokio::test(flavor = "multi_thread")]
async fn a_paste_of_a_thousand_keyed_updates_stays_within_its_statements() {
    let Some(url) = url("a_paste_of_a_thousand_keyed_updates_stays_within_its_statements") else {
        return;
    };
    let values: Vec<String> = (1..=1_000).map(|i| format!("({i}, 'old')")).collect();
    let insert = format!("INSERT INTO {{t}} VALUES {}", values.join(", "));
    let t = Table::make(&url, "paste_1k", &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(20))", &insert]).await;
    let mut c = Conn::open_paged(&url, SessionRole::Query, false, 2_000).await;
    let select = format!("SELECT id, v FROM {} ORDER BY id", t.q());
    let e = Editing::new(&url, &mut c, &t, &select).await;
    assert_eq!(e.rows.len(), 1_000);
    let text: String = (1..=1_000).map(|i| format!("new {i}\n")).collect();
    let pasted = edit::paste::parse(&text, &Default::default()).expect("one column");
    let mut ops = edit::PasteOps { width: 2, ..Default::default() };
    ops.set = pasted.rows.iter().enumerate().map(|(r, row)| ((r, 1), row[0].clone())).collect();
    let mut b = EditBatch::new();
    assert_eq!(b.paste(ops, e.orig()).expect("pasted").changed, 1_000);
    let plan = e.plan(&mut c, &b, &[]).await;
    assert!(plan.steps.len() <= 2 * 1_000, "{} statements besides the transaction's", plan.steps.len());
    let done = c.apply_within(plan.steps, ApplyTx::Commit, PASTE_1K_SECS).await.expect("applied");
    assert_eq!(done.len(), 2_000);
    let rows = c.rows(&format!("SELECT count(*) FROM {} WHERE v LIKE 'new %'", t.q())).await;
    assert_eq!(rows[0][0].as_deref(), Some("1000"));
}
