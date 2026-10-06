//! Integration tests against a real PostgreSQL loaded with `dev/init/*.sql`.
//!
//! Connection: `DATARIG_TEST_PG_URL` (e.g.
//! `postgres://datarig:datarig@127.0.0.1:55432/datarig`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_PG=1` (CI's `integration` job) -> the test fails, so that job
//!   can never silently skip. Other CI jobs run without a database and skip like a local run.
//!
//! Tables a test creates are `public.it_*` with a drop guard, and stale ones of killed runs are
//! swept before the first test (see `pg_clean`), so a run leaves `public` empty.
//!
//! `DATARIG_TEST_DIAL=tcp` runs the suite with every session, test connection and cancel
//! request going through a dialer (`transport::TcpDialer`), as they do through
//! an SSH tunnel; CI runs the suite both ways. The few tests of the direct path's own errors
//! stay direct.

mod pg_clean;
mod pg_proxy;

use datarig_core::driver::PagingMode;
use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, Outcome, PingError, Session, SessionRole,
};
use datarig_core::fault::FaultKind;
use datarig_core::profile::ConnectionConfig;
use datarig_core::transport::{DialError, Dialer, DialerRef, TcpDialer};
use datarig_driver_postgres::PgDriver;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PAGE: usize = 500;

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
            // Written to the real stderr handle so it is visible even with output capture.
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: set DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig"
            );
            None
        }
    }
}

fn opts(role: SessionRole) -> ConnectOptions {
    ConnectOptions::new(PAGE, role, &format!("it{}", std::process::id())).dialer(dialer())
}

/// The dialer every session of the run goes through (`DATARIG_TEST_DIAL=tcp`), or none.
fn dialer() -> Option<DialerRef> {
    run_dialer().map(|d| DialerRef(d as Arc<dyn Dialer>))
}

fn run_dialer() -> Option<Arc<TcpDialer>> {
    static DIALER: std::sync::OnceLock<Option<Arc<TcpDialer>>> = std::sync::OnceLock::new();
    DIALER
        .get_or_init(|| match std::env::var("DATARIG_TEST_DIAL").as_deref() {
            Ok("tcp") => Some(Arc::new(TcpDialer::default())),
            Ok("") | Err(_) => None,
            Ok(other) => panic!("DATARIG_TEST_DIAL={other}: only `tcp` is known"),
        })
        .clone()
}

/// With `DATARIG_TEST_DIAL=tcp` the suite's sessions really go through the run's dialer.
#[tokio::test(flavor = "multi_thread")]
async fn the_suite_dials_when_asked_to() {
    let Some(url) = pg_url("the_suite_dials_when_asked_to") else { return };
    let Some(d) = run_dialer() else { return };
    let before = d.dials.load(std::sync::atomic::Ordering::SeqCst);
    let mut c = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(c.run(1, "SELECT 1").await, DbEvent::Page { .. }));
    assert!(d.dials.load(std::sync::atomic::Ordering::SeqCst) > before, "the session dialed");
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Conn {
    async fn open(url: &str, role: SessionRole) -> Conn {
        Conn::open_with(url, role, false).await
    }

    /// A session of a profile whose policy is read-only (`read_only`) or not.
    async fn open_with(url: &str, role: SessionRole, read_only: bool) -> Conn {
        Conn::open_in(url, role, read_only, Default::default()).await
    }

    /// A session in `context` (a database and schema of the server).
    async fn open_in(
        url: &str,
        role: SessionRole,
        read_only: bool,
        context: datarig_core::driver::SessionContext,
    ) -> Conn {
        let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
        let (tx, rx) = unbounded_channel();
        let session = PgDriver.connect(&cfg, role, opts(role).read_only(read_only).context(context), tx);
        let mut c = Conn { session, rx };
        c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
        c
    }

    /// Wait for the first event matching `pred` (other events are dropped).
    async fn wait(&mut self, pred: impl Fn(&DbEvent) -> bool, secs: u64) -> DbEvent {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("connect failed: {error:?}"),
                Ok(Some(ev)) if pred(&ev) => return ev,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out after {secs}s waiting for event"),
            }
        }
    }

    async fn result(&mut self, id: u64) -> DbEvent {
        self.wait(
            |e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
            30,
        )
        .await
    }

    async fn run(&mut self, id: u64, sql: &str) -> DbEvent {
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: PagingMode::Hold });
        self.result(id).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn introspection_lists_schemas_objects_and_catalog() {
    let Some(url) = pg_url("introspection_lists_schemas_objects_and_catalog") else { return };
    let mut c = Conn::open(&url, SessionRole::Meta).await;
    let DbEvent::Schemas(Ok(schemas)) = c.wait(|e| matches!(e, DbEvent::Schemas(_)), 10).await else {
        panic!("schemas failed")
    };
    assert!(schemas.contains(&"analytics".to_string()) && schemas.contains(&"shop".to_string()), "{schemas:?}");
    assert!(schemas.iter().all(|s| s != "pg_catalog" && s != "information_schema" && !s.starts_with("pg_toast")));
    let mut sorted = schemas.clone();
    sorted.sort();
    assert_eq!(schemas, sorted, "schemas must be sorted by name");

    let DbEvent::Catalog(Ok(cat)) = c.wait(|e| matches!(e, DbEvent::Catalog(_)), 10).await else {
        panic!("catalog failed")
    };
    let users = cat.relations.iter().find(|r| r.schema == "shop" && r.name == "users").expect("shop.users in catalog");
    assert_eq!(users.columns[0].name, "id");
    assert_eq!(users.columns[0].type_name, "bigint");
    assert!(cat.relations.iter().any(|r| r.name == "order_summary" && r.is_view));

    c.session.send(DbCommand::LoadObjects { schema: "shop".into() });
    let DbEvent::Objects { result: Ok(objects), .. } = c.wait(|e| matches!(e, DbEvent::Objects { .. }), 10).await
    else {
        panic!("objects failed")
    };
    assert_eq!(objects.tables, ["audit_log", "order_items", "orders", "products", "reviews", "users"]);
    assert_eq!(objects.views, ["order_summary"]);
    assert!(objects.materialized.is_empty(), "order_summary is a plain view");
    // Every table has its estimates, the same as its structure's (one set of rules); the view none.
    assert_eq!(objects.stats.keys().collect::<Vec<_>>(), objects.tables.iter().collect::<Vec<_>>());
    for table in &objects.tables {
        c.session.send(DbCommand::LoadStructure { schema: "shop".into(), table: table.clone() });
        let DbEvent::Structure { result, .. } = c.wait(|e| matches!(e, DbEvent::Structure { .. }), 10).await else {
            unreachable!()
        };
        assert_eq!(result.expect(table).stats().as_ref(), objects.stats.get(table), "{table}");
    }
}

/// A schema's materialized views are listed with its views and named as materialized (step
/// 2.7.5: the explorer draws them with an icon of their own); partitioned and foreign tables
/// are tables.
#[tokio::test(flavor = "multi_thread")]
async fn materialized_views_are_views_marked_materialized() {
    let Some(url) = pg_url("materialized_views_are_views_marked_materialized") else { return };
    let schema = format!("zz_mv_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.t (id int)"),
        format!("CREATE TABLE {schema}.p (id int) PARTITION BY RANGE (id)"),
        format!("CREATE VIEW {schema}.v AS SELECT id FROM {schema}.t"),
        format!("CREATE MATERIALIZED VIEW {schema}.m AS SELECT id FROM {schema}.t"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let mut c = Conn::open(&url, SessionRole::Meta).await;
    c.session.send(DbCommand::LoadObjects { schema: schema.clone() });
    let DbEvent::Objects { result: Ok(objects), .. } = c.wait(|e| matches!(e, DbEvent::Objects { .. }), 10).await
    else {
        panic!("objects failed")
    };
    assert_eq!(objects.tables, ["p", "t"]);
    assert_eq!(objects.views, ["m", "v"]);
    assert_eq!(objects.materialized.iter().collect::<Vec<_>>(), ["m"]);
    assert_eq!(objects.stats.keys().collect::<Vec<_>>(), ["m", "p", "t"], "a view has no estimates");
}

/// Key metadata: the metadata session reports the keys of every table
/// after the catalog (single and composite primary keys, foreign keys, unique constraints), and
/// the columns of a JOIN name the table column they come from; an expression names none.
#[tokio::test(flavor = "multi_thread")]
async fn keys_and_column_origins_mark_a_join() {
    use datarig_core::driver::KeyMarks;
    let Some(url) = pg_url("keys_and_column_origins_mark_a_join") else { return };
    // Tables of its own, so other tests' DDL (in parallel runs) cannot change what it compares.
    let schema = format!("zz_keys_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.a (id int PRIMARY KEY, code text UNIQUE, note text)"),
        format!(
            "CREATE TABLE {schema}.b (a_id int REFERENCES {schema}.a, n int, tag text, \
             PRIMARY KEY (a_id, n), UNIQUE (n, tag))"
        ),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let mut meta = Conn::open(&url, SessionRole::Meta).await;
    let DbEvent::Keys(Ok(keys)) = meta.wait(|e| matches!(e, DbEvent::Keys(_)), 10).await else { panic!("keys failed") };
    let m = |t: &str, c: &str| keys.marks_by_name("shop", t, c);
    let (pk, fk, uq) = (
        KeyMarks { pk: true, ..KeyMarks::default() },
        KeyMarks { fk: true, ..KeyMarks::default() },
        KeyMarks { unique: true, ..KeyMarks::default() },
    );
    assert_eq!(m("users", "id"), pk);
    assert_eq!(m("users", "email"), uq);
    assert_eq!(m("users", "name"), KeyMarks::default());
    assert_eq!(m("orders", "user_id"), fk);
    assert_eq!(m("order_items", "order_id"), KeyMarks { pk: true, fk: true, unique: false }, "composite PK + FK");
    assert_eq!(m("order_items", "line_no"), pk, "composite PK");
    assert_eq!(m("order_items", "product_id"), fk);
    assert_eq!(
        (m("reviews", "product_id"), m("reviews", "user_id")),
        (KeyMarks { fk: true, unique: true, pk: false }, KeyMarks { fk: true, unique: true, pk: false }),
        "composite UNIQUE"
    );
    assert_eq!(keys.marks_by_name("analytics", "daily_stats", "metric"), pk);
    let own = |k: &datarig_core::driver::KeyCatalog| {
        [("a", "id"), ("a", "code"), ("a", "note"), ("b", "a_id"), ("b", "n"), ("b", "tag")]
            .map(|(t, c)| k.marks_by_name(&schema, t, c))
    };
    let both = |pk, fk, unique| KeyMarks { pk, fk, unique };
    let want = [pk, uq, KeyMarks::default(), both(true, true, false), both(true, false, true), uq];
    assert_eq!(own(&keys), want);
    // Asked again (a refresh), the same for its own tables (the rest of the catalog may change
    // under parallel runs).
    meta.session.send(DbCommand::LoadKeys);
    let DbEvent::Keys(Ok(again)) = meta.wait(|e| matches!(e, DbEvent::Keys(_)), 10).await else { panic!() };
    assert_eq!(own(&again), want);

    let mut q = Conn::open(&url, SessionRole::Query).await;
    let sql = "SELECT o.id, o.user_id, u.email, u.name, i.order_id, i.line_no, i.product_id, \
               o.total_amount * 2 AS doubled, now() AS at \
               FROM shop.orders o JOIN shop.users u ON u.id = o.user_id \
               JOIN shop.order_items i ON i.order_id = o.id LIMIT 3";
    let DbEvent::Page { columns: Some(cols), .. } = q.run(1, sql).await else { panic!("no rows") };
    let marks: Vec<KeyMarks> = cols.iter().map(|c| keys.marks(c.origin)).collect();
    assert_eq!(
        marks,
        [
            pk,
            fk,
            uq,
            KeyMarks::default(),
            KeyMarks { pk: true, fk: true, unique: false },
            pk,
            fk,
            KeyMarks::default(),
            KeyMarks::default()
        ]
    );
    assert!(cols[7].origin.is_none() && cols[8].origin.is_none(), "expressions have no origin");
    let users = keys.table(cols[2].origin.unwrap().table).unwrap();
    assert_eq!((users.schema.as_str(), users.name.as_str()), ("shop", "users"));
}

/// A table's structure: exact columns (identity, stored generated, defaults), the
/// primary key, foreign keys (multi-column, `ON DELETE CASCADE`, `ON UPDATE SET NULL`), indexes
/// (partial, expression, gin, `INCLUDE`, the key's), checks and triggers (enabled and disabled);
/// no row estimate before the table is analyzed (never 0), one after; a partitioned table sums
/// its partitions (no size either before they have statistics); a view has its `INSTEAD OF`
/// trigger, a materialized view its index; quoted names work, and a table that does not exist
/// is the server's error. Each read is one unnamed
/// statement (one Parse on the wire), and the dev tables are not touched.
#[tokio::test(flavor = "multi_thread")]
async fn table_structure_reads_the_catalog_in_one_statement() {
    use datarig_core::driver::structure::*;
    let Some(url) = pg_url("table_structure_reads_the_catalog_in_one_statement") else { return };
    let schema = format!("zz_struct_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    let s = &schema;
    // Autovacuum off where the test expects no statistics yet: nothing analyzes them behind the
    // test's back (the partitions' 400 rows are enough for it to).
    let off = "WITH (autovacuum_enabled = false)";
    for sql in [
        format!("CREATE SCHEMA {s}"),
        format!(
            "CREATE TABLE {s}.parent (a int, b int, code text, PRIMARY KEY (a, b), CONSTRAINT parent_code_key UNIQUE (code))"
        ),
        format!(
            "CREATE TABLE {s}.child (\
             id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, pa int NOT NULL, pb int NOT NULL, \
             qty int NOT NULL DEFAULT 1 CONSTRAINT child_qty_check CHECK (qty > 0), doc jsonb, \
             twice int GENERATED ALWAYS AS (qty * 2) STORED, note text, \
             CONSTRAINT child_parent_fkey FOREIGN KEY (pa, pb) REFERENCES {s}.parent (a, b) ON DELETE CASCADE, \
             CONSTRAINT child_note_fkey FOREIGN KEY (note) REFERENCES {s}.parent (code) ON UPDATE SET NULL) {off}"
        ),
        format!(
            "CREATE INDEX child_note_partial ON {s}.child (note COLLATE \"C\" text_pattern_ops, qty NULLS FIRST) \
             WHERE note IS NOT NULL"
        ),
        format!("CREATE INDEX child_lower_note ON {s}.child (lower(note) DESC)"),
        format!("CREATE INDEX child_doc_gin ON {s}.child USING gin (doc)"),
        format!("CREATE UNIQUE INDEX child_pa_key ON {s}.child (pa) INCLUDE (qty)"),
        format!("CREATE FUNCTION {s}.touch() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$"),
        format!(
            "CREATE TRIGGER child_touch BEFORE INSERT OR UPDATE OF qty, note ON {s}.child FOR EACH ROW \
             WHEN (NEW.note <> ') EXECUTE (') EXECUTE FUNCTION {s}.touch()"
        ),
        format!("CREATE TRIGGER child_audit AFTER DELETE ON {s}.child FOR EACH STATEMENT EXECUTE FUNCTION {s}.touch()"),
        format!("ALTER TABLE {s}.child DISABLE TRIGGER child_audit"),
        format!("INSERT INTO {s}.parent SELECT g, g, 'c' || g FROM generate_series(1, 3) g"),
        format!("ANALYZE {s}.parent"),
        format!("CREATE TABLE {s}.part (id int, at date) PARTITION BY RANGE (at)"),
        format!(
            "CREATE TABLE {s}.part_2025 PARTITION OF {s}.part FOR VALUES FROM ('2025-01-01') TO ('2026-01-01') {off}"
        ),
        format!(
            "CREATE TABLE {s}.part_2026 PARTITION OF {s}.part FOR VALUES FROM ('2026-01-01') TO ('2027-01-01') {off}"
        ),
        format!("INSERT INTO {s}.part SELECT g, date '2025-06-01' + g FROM generate_series(1, 400) g"),
        format!("CREATE MATERIALIZED VIEW {s}.mv {off} AS SELECT pa, count(*) AS n FROM {s}.child GROUP BY pa"),
        format!("CREATE INDEX mv_pa ON {s}.mv (pa)"),
        format!("CREATE VIEW {s}.v AS SELECT id, qty FROM {s}.child"),
        format!("CREATE TRIGGER v_ins INSTEAD OF INSERT ON {s}.v FOR EACH ROW EXECUTE FUNCTION {s}.touch()"),
        format!(r#"CREATE TABLE {s}."Odd ""Name""" (x int)"#),
        format!(
            "CREATE TABLE {s}.keyed (k1 int, k2 text, lo int, hi int, note text, \"Mixed Case\" int, \"user\" int, \
             PRIMARY KEY (k2, k1), CONSTRAINT keyed_pair_key UNIQUE (hi, lo), \
             CONSTRAINT keyed_parent_fkey FOREIGN KEY (hi, lo) REFERENCES {s}.parent (a, b), \
             CONSTRAINT keyed_range_check CHECK (lo < hi))"
        ),
        format!(
            "CREATE INDEX keyed_mixed ON {s}.keyed (note COLLATE \"C\" text_pattern_ops, lower(note) DESC, \
             \"Mixed Case\" DESC NULLS LAST) INCLUDE (hi, k1)"
        ),
        format!(
            "CREATE TRIGGER keyed_touch BEFORE UPDATE OF note, \"Mixed Case\", \"user\" ON {s}.keyed \
             FOR EACH ROW EXECUTE FUNCTION {s}.touch()"
        ),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let proxy = pg_proxy::Proxy::start(&pg_proxy::upstream(&url)).await;
    let mut meta = Conn::open(&proxy.url(&url), SessionRole::Meta).await;
    meta.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let mut read = async |table: &str| {
        let parses = proxy.sent().iter().filter(|m| matches!(m, pg_proxy::Sent::Parse(_))).count();
        meta.session.send(DbCommand::LoadStructure { schema: schema.clone(), table: table.to_string() });
        let ev = meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 30).await;
        let DbEvent::Structure { schema: sch, table: t, result } = ev else { unreachable!() };
        assert_eq!((sch.as_str(), t.as_str()), (schema.as_str(), table));
        let sent = proxy.sent();
        let new: Vec<_> = sent.iter().filter(|m| matches!(m, pg_proxy::Sent::Parse(_))).skip(parses).collect();
        assert_eq!(new.len(), 1, "{table}: one statement: {new:?}");
        result.map(|b| *b)
    };
    let child = read("child").await.expect("child");
    let names = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let col = |name: &str, ty: &str, not_null: bool, default: Option<&str>, fill: ColumnFill| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null,
        default: default.map(str::to_string),
        fill,
    };
    assert_eq!(
        child.columns,
        [
            col("id", "bigint", true, None, ColumnFill::IdentityAlways),
            col("pa", "integer", true, None, ColumnFill::Default),
            col("pb", "integer", true, None, ColumnFill::Default),
            col("qty", "integer", true, Some("1"), ColumnFill::Default),
            col("doc", "jsonb", false, None, ColumnFill::Default),
            col("twice", "integer", false, None, ColumnFill::Stored("qty * 2".into())),
            col("note", "text", false, None, ColumnFill::Default),
        ]
    );
    assert_eq!(
        child.primary_key,
        Some(KeyConstraint {
            name: "child_pkey".into(),
            columns: names(&["id"]),
            definition: "PRIMARY KEY (id)".into()
        })
    );
    assert_eq!(
        child.foreign_keys,
        [
            ForeignKey {
                name: "child_note_fkey".into(),
                columns: names(&["note"]),
                ref_schema: schema.clone(),
                ref_table: "parent".into(),
                ref_columns: names(&["code"]),
                on_delete: FkAction::NoAction,
                on_update: FkAction::SetNull,
                definition: format!("FOREIGN KEY (note) REFERENCES {s}.parent(code) ON UPDATE SET NULL"),
            },
            ForeignKey {
                name: "child_parent_fkey".into(),
                columns: names(&["pa", "pb"]),
                ref_schema: schema.clone(),
                ref_table: "parent".into(),
                ref_columns: names(&["a", "b"]),
                on_delete: FkAction::Cascade,
                on_update: FkAction::NoAction,
                definition: format!("FOREIGN KEY (pa, pb) REFERENCES {s}.parent(a, b) ON DELETE CASCADE"),
            },
        ]
    );
    let index =
        |name: &str, cols: &[&str], include: &[&str], unique, method: &str, predicate: Option<&str>, key, def: &str| {
            Index {
                name: name.into(),
                columns: names(cols),
                options: vec![String::new(); cols.len()],
                include: names(include),
                key_columns: cols.iter().map(|c| Some(c.to_string())).collect(),
                include_columns: names(include),
                unique,
                method: method.into(),
                predicate: predicate.map(str::to_string),
                primary: key,
                constraint: key,
                definition: def.to_string(),
            }
        };
    let mut expected = [
        index(
            "child_doc_gin",
            &["doc"],
            &[],
            false,
            "gin",
            None,
            false,
            &format!("CREATE INDEX child_doc_gin ON {s}.child USING gin (doc)"),
        ),
        index(
            "child_lower_note",
            &["lower(note)"],
            &[],
            false,
            "btree",
            None,
            false,
            &format!("CREATE INDEX child_lower_note ON {s}.child USING btree (lower(note) DESC)"),
        ),
        index(
            "child_note_partial",
            &["note", "qty"],
            &[],
            false,
            "btree",
            Some("note IS NOT NULL"),
            false,
            &format!(
                "CREATE INDEX child_note_partial ON {s}.child USING btree \
                     (note COLLATE \"C\" text_pattern_ops, qty NULLS FIRST) WHERE (note IS NOT NULL)"
            ),
        ),
        index(
            "child_pa_key",
            &["pa"],
            &["qty"],
            true,
            "btree",
            None,
            false,
            &format!("CREATE UNIQUE INDEX child_pa_key ON {s}.child USING btree (pa) INCLUDE (qty)"),
        ),
        index(
            "child_pkey",
            &["id"],
            &[],
            true,
            "btree",
            None,
            true,
            &format!("CREATE UNIQUE INDEX child_pkey ON {s}.child USING btree (id)"),
        ),
    ];
    // Each key's collation, operator class and order, when not the defaults (no lock: catalogs).
    expected[1].options = names(&["DESC"]);
    // An expression is no column of the table.
    expected[1].key_columns = vec![None];
    expected[2].options = names(&["COLLATE \"C\" text_pattern_ops", "NULLS FIRST"]);
    assert_eq!(child.indexes, expected);
    for x in &child.indexes {
        let keys = format!("({})", x.keys().join(", "));
        assert!(x.definition.contains(&keys), "{keys} as the server prints it: {}", x.definition);
    }
    assert!(child.unique_constraints.is_empty(), "a unique index is no constraint");
    assert_eq!(
        child.checks,
        [CheckConstraint {
            name: "child_qty_check".into(),
            expression: "qty > 0".into(),
            columns: names(&["qty"]),
            definition: "CHECK (qty > 0)".into()
        }]
    );
    assert_eq!(
        child.triggers,
        [
            Trigger {
                name: "child_audit".into(),
                timing: TriggerTiming::After,
                events: vec![TriggerEvent::Delete],
                for_each_row: false,
                function: format!("{s}.touch"),
                enabled: false,
                update_columns: Vec::new(),
                condition: None,
                definition: format!(
                    "CREATE TRIGGER child_audit AFTER DELETE ON {s}.child FOR EACH STATEMENT EXECUTE FUNCTION {s}.touch()"
                ),
            },
            Trigger {
                name: "child_touch".into(),
                timing: TriggerTiming::Before,
                events: vec![TriggerEvent::Insert, TriggerEvent::Update],
                for_each_row: true,
                function: format!("{s}.touch"),
                enabled: true,
                update_columns: names(&["qty", "note"]),
                condition: Some("new.note <> ') EXECUTE ('::text".into()),
                definition: format!(
                    "CREATE TRIGGER child_touch BEFORE INSERT OR UPDATE OF qty, note ON {s}.child FOR EACH ROW \
                     WHEN (new.note <> ') EXECUTE ('::text) EXECUTE FUNCTION {s}.touch()"
                ),
            },
        ]
    );
    assert_eq!(child.kind, RelationKind::Table);
    assert_eq!(child.estimated_rows, None, "never analyzed: unknown, not 0");
    assert_eq!(child.total_bytes, None, "no statistics yet: its indexes' pages are not its size");
    let m = |c| child.marks(c);
    assert_eq!(
        (m("id").pk, m("pa").fk, m("pa").unique, m("note").fk, m("qty").unique),
        (true, true, true, true, false)
    );

    let parent = read("parent").await.expect("parent");
    assert_eq!(parent.estimated_rows, Some(3), "analyzed");
    assert_eq!(parent.primary_key.as_ref().map(|k| k.columns.clone()), Some(names(&["a", "b"])));
    assert_eq!(
        parent.unique_constraints,
        [KeyConstraint {
            name: "parent_code_key".into(),
            columns: names(&["code"]),
            definition: "UNIQUE (code)".into()
        }]
    );

    // The columns each key, index and check covers, from the catalog's positions (`indkey`,
    // `conkey`, `confkey`): in the key's order, an expression's key none, `INCLUDE` last.
    let keyed = read("keyed").await.expect("keyed");
    let item = |g, k| keyed.item_columns(g, k);
    let plain = |c: &str| ItemColumn {
        column: Some(c.into()),
        text: c.into(),
        options: String::new(),
        include: false,
        references: None,
    };
    assert_eq!(item(StructureGroup::PrimaryKey, 0), [plain("k2"), plain("k1")]);
    assert_eq!(item(StructureGroup::UniqueConstraints, 0), [plain("hi"), plain("lo")]);
    assert_eq!(
        item(StructureGroup::ForeignKeys, 0),
        [
            ItemColumn { references: Some("a".into()), ..plain("hi") },
            ItemColumn { references: Some("b".into()), ..plain("lo") }
        ]
    );
    assert_eq!(keyed.checks[0].columns, names(&["lo", "hi"]));
    assert_eq!(item(StructureGroup::CheckConstraints, 0), [plain("lo"), plain("hi")]);
    let mixed = keyed.indexes.iter().position(|i| i.name == "keyed_mixed").expect("keyed_mixed");
    assert_eq!(keyed.indexes[mixed].key_columns, [Some("note".to_string()), None, Some("Mixed Case".to_string())]);
    assert_eq!(keyed.indexes[mixed].include_columns, names(&["hi", "k1"]));
    assert_eq!(
        item(StructureGroup::Indexes, mixed),
        [
            ItemColumn { options: "COLLATE \"C\" text_pattern_ops".into(), ..plain("note") },
            ItemColumn { column: None, text: "lower(note)".into(), options: "DESC".into(), ..plain("") },
            ItemColumn { options: "DESC NULLS LAST".into(), ..plain("Mixed Case") },
            ItemColumn { include: true, ..plain("hi") },
            ItemColumn { include: true, ..plain("k1") },
        ]
    );
    // `UPDATE OF` names its columns as SQL writes them, as the index's keys are.
    assert_eq!(keyed.triggers[0].update_columns, names(&["note", "\"Mixed Case\"", "\"user\""]));
    assert!(keyed.indexes[mixed].columns[2].starts_with("\"Mixed Case\""), "{:?}", keyed.indexes[mixed].columns);
    let pkey = keyed.indexes.iter().position(|i| i.name == "keyed_pkey").expect("keyed_pkey");
    assert_eq!(item(StructureGroup::Indexes, pkey), [plain("k2"), plain("k1")]);

    let part = read("part").await.expect("part");
    assert_eq!((part.kind, part.estimated_rows), (RelationKind::PartitionedTable, None), "partitions never analyzed");
    assert_eq!(part.total_bytes, None, "no statistics yet: the size is unknown too, not 0");
    pg_clean::run_fresh(&url, &format!("ANALYZE {s}.part")).unwrap();
    let part = read("part").await.expect("part");
    assert_eq!(part.estimated_rows, Some(400), "the partitions' estimates");
    assert!(part.total_bytes.is_some_and(|b| b > 0), "its partitions' size: {:?}", part.total_bytes);

    let mv = read("mv").await.expect("mv");
    assert_eq!(mv.kind, RelationKind::MaterializedView);
    assert_eq!(mv.indexes.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["mv_pa"]);
    assert_eq!(mv.total_bytes, None, "never analyzed");
    pg_clean::run_fresh(&url, &format!("ANALYZE {s}.mv")).unwrap();
    assert!(read("mv").await.expect("mv").total_bytes.is_some_and(|b| b > 0), "analyzed: its index's pages");
    let v = read("v").await.expect("v");
    assert_eq!((v.kind, v.estimated_rows, v.total_bytes), (RelationKind::View, None, None));
    assert_eq!(v.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "qty"]);
    let t = &v.triggers[0];
    assert_eq!(
        (t.timing, t.events.as_slice(), t.for_each_row),
        (TriggerTiming::InsteadOf, [TriggerEvent::Insert].as_slice(), true)
    );

    let odd = read("Odd \"Name\"").await.expect("a quoted name");
    assert_eq!(odd.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["x"]);
    match read("no_such_table").await {
        Err(DbError::Server(e)) => assert!(e.contains("does not exist"), "{e}"),
        other => panic!("{other:?}"),
    }
    // The metadata session still answers (a failed read leaves nothing open).
    assert!(read("parent").await.is_ok());
}

/// A lookup never waits on another session. Another session's `ACCESS EXCLUSIVE` lock on a
/// table, held (an `ALTER TABLE`) or waited for behind a reader (a migration queued behind a
/// long transaction): the table's structure answers at once with `DbError::Locked`, and the
/// metadata session has asked for no lock on the table meanwhile (none in `pg_locks`), so it
/// waits behind no one and no one waits behind it. A partitioned table whose partition is
/// locked still has its structure and size (statistics: no lock on the partition). Without a
/// lock the size is the statistics' estimate, in whole pages, and a read after the lock is
/// gone works. Listing the schema's objects with their estimates takes no lock on any of them
/// either: behind a held or a queued lock it answers at once, the locked table's estimates
/// included.
#[tokio::test(flavor = "multi_thread")]
async fn table_structure_never_waits_for_a_lock() {
    let Some(url) = pg_url("table_structure_never_waits_for_a_lock") else { return };
    let schema = format!("zz_lock_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    let s = &schema;
    for sql in [
        format!("CREATE SCHEMA {s}"),
        format!(
            "CREATE TABLE {s}.t (id int PRIMARY KEY, n int DEFAULT 1 CHECK (n > 0), \
             twice int GENERATED ALWAYS AS (n * 2) STORED, note text)"
        ),
        format!("CREATE INDEX t_lower ON {s}.t (lower(note)) WHERE n > 1"),
        format!("CREATE FUNCTION {s}.touch() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$"),
        format!(
            "CREATE TRIGGER t_touch BEFORE UPDATE ON {s}.t FOR EACH ROW \
             WHEN (OLD.n IS DISTINCT FROM NEW.n) EXECUTE FUNCTION {s}.touch()"
        ),
        format!(
            "INSERT INTO {s}.t (id, n, note) SELECT g, 1 + g % 5, repeat('x', 200) FROM generate_series(1, 2000) g"
        ),
        format!("ANALYZE {s}.t"),
        format!("CREATE TABLE {s}.p (id int) PARTITION BY RANGE (id)"),
        format!("CREATE TABLE {s}.p1 PARTITION OF {s}.p FOR VALUES FROM (0) TO (1000)"),
        format!("INSERT INTO {s}.p SELECT g FROM generate_series(0, 999) g"),
        format!("ANALYZE {s}.p"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let tag = format!("lock{}", std::process::id());
    let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.clone()), ..ConnectionConfig::test_db() };
    let (tx, rx) = unbounded_channel();
    let o = ConnectOptions::new(PAGE, SessionRole::Meta, &tag).dialer(dialer());
    let mut meta = Conn { session: PgDriver.connect(&cfg, SessionRole::Meta, o, tx), rx };
    meta.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let mut observer = Conn::open(&url, SessionRole::Query).await;
    // The structure of `table` (the schema's objects with `None`), how long it took, and the
    // relation locks the metadata session held or asked for on any relation of the schema while
    // it was being read (looked at 300 ms in, or when it came if it came earlier: none either
    // way).
    let mut n = 100;
    let mut ask = async |table: Option<&str>| {
        let t0 = Instant::now();
        meta.session.send(match table {
            Some(table) => DbCommand::LoadStructure { schema: schema.clone(), table: table.to_string() },
            None => DbCommand::LoadObjects { schema: schema.clone() },
        });
        let ev = tokio::time::timeout(Duration::from_millis(300), meta.rx.recv()).await;
        n += 1;
        let sql = format!(
            "SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE a.application_name = 'datarig-meta-{tag}' AND l.locktype = 'relation' \
             AND l.relation IN (SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relnamespace = '{s}'::regnamespace)"
        );
        let DbEvent::Page { rows, .. } = observer.run(n, &sql).await else { panic!("pg_locks") };
        let locks = rows[0][0].clone().unwrap_or_default();
        let ev = match ev {
            Ok(Some(ev)) => ev,
            _ => tokio::time::timeout(Duration::from_secs(10), meta.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("{table:?}: no answer within 10 s ({locks} locks asked for)"))
                .expect("the metadata session ended"),
        };
        (ev, t0.elapsed(), locks)
    };
    let structure = |(ev, took, locks)| match ev {
        DbEvent::Structure { result, .. } => (result.map(|b| *b), took, locks),
        ev => panic!("{ev:?}"),
    };
    let objects = |(ev, took, locks)| match ev {
        DbEvent::Objects { result, .. } => (result.expect("objects"), took, locks),
        ev => panic!("{ev:?}"),
    };

    let (t, _, _) = structure(ask(Some("t")).await);
    let t = t.expect("t, not locked");
    assert_eq!(t.estimated_rows, Some(2000));
    let bytes = t.total_bytes.expect("analyzed: a size");
    assert!(bytes > 2000 * 200 && bytes % 8192 == 0, "whole pages of the heap and its indexes: {bytes}");
    assert_eq!(t.checks.len(), 1);
    assert_eq!(t.indexes.len(), 2);
    let t_stats = t.stats().expect("a table's estimates");
    // The schema's objects with their estimates, `t` locked: at once, no lock asked for.
    let listed_at_once = |(listed, took, locks): (datarig_core::driver::SchemaObjects, Duration, String), why: &str| {
        assert!(took < Duration::from_secs(1), "{why}: at once: {took:?}");
        assert_eq!(locks, "0", "{why}: no lock asked for on any relation of the schema");
        assert_eq!(listed.tables, ["p", "p1", "t"], "{why}");
        assert_eq!(listed.stats["t"], t_stats, "{why}: the locked table's estimates");
        assert_eq!((listed.stats["p"].rows, listed.stats["p1"].rows), (Some(1000), Some(1000)), "{why}");
    };

    // Held: an open transaction's `LOCK TABLE` on the table and on the partition.
    let mut holder = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(holder.run(1, "BEGIN").await, DbEvent::Done { .. }));
    let lock = format!("LOCK TABLE {s}.t, {s}.p1 IN ACCESS EXCLUSIVE MODE");
    assert!(matches!(holder.run(2, &lock).await, DbEvent::Done { .. }));
    let (t, took, locks) = structure(ask(Some("t")).await);
    assert_eq!(t, Err(DbError::Locked));
    assert!(took < Duration::from_secs(1), "at once: {took:?}");
    assert_eq!(locks, "0", "no lock asked for on the locked table");
    let (p, took, locks) = structure(ask(Some("p")).await);
    let p = p.expect("the partitioned table is not locked, only its partition");
    assert!(took < Duration::from_secs(1), "at once: {took:?}");
    assert_eq!(locks, "0", "the parent's lock was taken and let go with the read");
    assert_eq!((p.estimated_rows, p.total_bytes.is_some_and(|b| b > 0)), (Some(1000), true), "{p:?}");
    listed_at_once(objects(ask(None).await), "held");
    assert!(matches!(holder.run(3, "ROLLBACK").await, DbEvent::Done { .. }));

    // Waited for: a reader's transaction holds the table, and a `LOCK TABLE` waits behind it.
    let mut reader = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(reader.run(1, "BEGIN").await, DbEvent::Done { .. }));
    assert!(matches!(reader.run(2, &format!("SELECT count(*) FROM {s}.t")).await, DbEvent::Page { .. }));
    let mut migration = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(migration.run(1, "BEGIN").await, DbEvent::Done { .. }));
    migration.session.send(DbCommand::Execute {
        id: 2,
        statements: vec![format!("LOCK TABLE {s}.t")],
        paging: PagingMode::Hold,
    });
    let waiting = format!(
        "SELECT count(*) FROM pg_locks WHERE relation = '{s}.t'::regclass \
         AND mode = 'AccessExclusiveLock' AND NOT granted"
    );
    let mut watch = Conn::open(&url, SessionRole::Query).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    for id in 1.. {
        let DbEvent::Page { rows, .. } = watch.run(id, &waiting).await else { panic!("pg_locks") };
        if rows[0][0].as_deref() == Some("1") {
            break;
        }
        assert!(Instant::now() < deadline, "the LOCK TABLE never queued");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (t, took, locks) = structure(ask(Some("t")).await);
    assert_eq!(t, Err(DbError::Locked));
    assert!(took < Duration::from_secs(1), "at once: {took:?}");
    assert_eq!(locks, "0", "not queued behind the waiting lock");
    listed_at_once(objects(ask(None).await), "waited for");
    assert!(matches!(reader.run(3, "ROLLBACK").await, DbEvent::Done { .. }));
    assert!(matches!(migration.result(2).await, DbEvent::Done { .. }), "the lock was granted");
    assert!(matches!(migration.run(3, "ROLLBACK").await, DbEvent::Done { .. }));

    // Asked again once the lock is gone.
    assert!(structure(ask(Some("t")).await).0.is_ok());
}

/// The size of a table that was never vacuumed or analyzed is unknown, as its rows are: its
/// heap's `relpages` is still 0 however many rows it has, while its indexes and its TOAST
/// table's index have pages from their creation on (a text column, a primary key, a partitioned
/// table's partitions). Once analyzed it is its heap, TOAST table and indexes, as the server
/// counts them (in whole pages). The schema's list has the same estimates for each of them,
/// read with it (a partitioned table its leaf partitions' sum, each partition its own).
#[tokio::test(flavor = "multi_thread")]
async fn table_size_is_unknown_until_the_table_has_statistics() {
    use datarig_core::driver::structure::RelationStats;
    let Some(url) = pg_url("table_size_is_unknown_until_the_table_has_statistics") else { return };
    let schema = format!("zz_size_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    let s = &schema;
    // Autovacuum off: nothing analyzes them behind the test's back.
    let off = "WITH (autovacuum_enabled = false)";
    for sql in [
        format!("CREATE SCHEMA {s}"),
        format!("CREATE TABLE {s}.text_only (note text) {off}"),
        format!("INSERT INTO {s}.text_only SELECT repeat('x', 30) FROM generate_series(1, 5000)"),
        format!("CREATE TABLE {s}.keyed (id int PRIMARY KEY, note text) {off}"),
        format!("INSERT INTO {s}.keyed SELECT g, repeat('y', 200) FROM generate_series(1, 20000) g"),
        format!("CREATE TABLE {s}.part (id int, note text) PARTITION BY RANGE (id)"),
        format!("CREATE TABLE {s}.part_a PARTITION OF {s}.part FOR VALUES FROM (0) TO (10000) {off}"),
        format!("CREATE TABLE {s}.part_b PARTITION OF {s}.part FOR VALUES FROM (10000) TO (20000) {off}"),
        format!("CREATE INDEX part_id ON {s}.part (id)"),
        format!("INSERT INTO {s}.part SELECT g, repeat('z', 100) FROM generate_series(0, 19999) g"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let mut meta = Conn::open(&url, SessionRole::Meta).await;
    meta.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let mut observer = Conn::open(&url, SessionRole::Query).await;
    let mut n = 0;
    let mut read = async |table: &str| {
        meta.session.send(DbCommand::LoadStructure { schema: schema.clone(), table: table.to_string() });
        let DbEvent::Structure { result, .. } = meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 30).await else {
            unreachable!()
        };
        let st = result.unwrap_or_else(|e| panic!("{table}: {e:?}"));
        // What the server counts (it locks the tables: fine here, nothing else holds them).
        n += 1;
        let sql = format!(
            "SELECT (pg_catalog.pg_total_relation_size('{s}.{table}') \
             + coalesce((SELECT sum(pg_catalog.pg_total_relation_size(r.relid)) \
             FROM pg_catalog.pg_partition_tree('{s}.{table}') r WHERE r.isleaf), 0))::int8"
        );
        let real: u64 = rows_of(observer.run(n, &sql).await)[0][0].as_deref().unwrap().parse().unwrap();
        meta.session.send(DbCommand::LoadObjects { schema: schema.clone() });
        let DbEvent::Objects { result, .. } = meta.wait(|e| matches!(e, DbEvent::Objects { .. }), 30).await else {
            unreachable!()
        };
        let listed = result.expect("objects").stats;
        assert_eq!(listed.get(table), st.stats().as_ref(), "{table}: the list's estimates are its structure's");
        (st.estimated_rows, st.total_bytes, real, listed)
    };

    for table in ["text_only", "keyed", "part"] {
        let (rows, bytes, real, listed) = read(table).await;
        assert!(real > 100_000, "{table} has rows: {real}");
        assert_eq!((rows, bytes), (None, None), "{table}: never analyzed, rows and size unknown");
        for partition in ["part_a", "part_b"] {
            assert_eq!(listed[partition], RelationStats::default(), "{partition}: never analyzed");
        }
    }
    for (table, count) in [("text_only", 5000), ("keyed", 20_000), ("part", 20_000)] {
        pg_clean::run_fresh(&url, &format!("ANALYZE {s}.{table}")).unwrap();
        let (rows, bytes, real, listed) = read(table).await;
        assert_eq!(rows, Some(count), "{table}");
        if table == "part" {
            let [a, b] = ["part_a", "part_b"].map(|p| listed[p]);
            assert_eq!((a.rows, b.rows), (Some(10_000), Some(10_000)), "each partition its own");
            assert_eq!(a.bytes.zip(b.bytes).map(|(a, b)| a + b), bytes, "the parent's is their sum");
        }
        let bytes = bytes.unwrap_or_else(|| panic!("{table}: analyzed, a size"));
        // The server's count adds the free space maps, which the statistics leave out.
        assert!(bytes <= real && bytes >= real * 9 / 10, "{table}: ~{bytes} for {real} bytes");
    }
}

/// Rows added after the last `ANALYZE` count before the next one: a table analyzed at 1000 rows
/// that has 200000 now is estimated at 200000 (the live rows of the cumulative statistics), and
/// its size grows with them, near what the server counts; a partitioned table sums its
/// partitions'. The schema's list has the same estimates. Neither read asks for a lock on any
/// relation, also while another session holds a grown partition (`pg_locks`). Rows deleted
/// since never lower the estimate below the last `ANALYZE`'s, and neither do counters that
/// count again the rows it saw (no more than twice as many: its estimate stays).
#[tokio::test(flavor = "multi_thread")]
async fn estimates_follow_rows_added_since_the_last_analyze() {
    let Some(url) = pg_url("estimates_follow_rows_added_since_the_last_analyze") else { return };
    let schema = format!("zz_grow_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    let s = &schema;
    // Autovacuum off: nothing analyzes them behind the test's back.
    let off = "WITH (autovacuum_enabled = false)";
    for sql in [
        format!("CREATE SCHEMA {s}"),
        format!("CREATE TABLE {s}.t (id int PRIMARY KEY, note text) {off}"),
        format!("CREATE TABLE {s}.p (id int, note text) PARTITION BY RANGE (id)"),
        format!("CREATE TABLE {s}.p_a PARTITION OF {s}.p FOR VALUES FROM (0) TO (1000000) {off}"),
        format!("CREATE TABLE {s}.p_b PARTITION OF {s}.p FOR VALUES FROM (1000000) TO (2000000) {off}"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    // The writer's counters are flushed at once where the server can (PostgreSQL 15 and later;
    // else within a second or so).
    let flush = "DO $$BEGIN IF pg_catalog.current_setting('server_version_num')::int >= 150000 THEN \
                 PERFORM pg_catalog.pg_stat_force_next_flush(); END IF; END$$";
    let mut writer = Conn::open(&url, SessionRole::Query).await;
    let mut w = 0;
    let mut write = async |sql: &str| {
        w += 1;
        match writer.run(w, sql).await {
            DbEvent::Done { .. } => {}
            ev => panic!("{sql}: {ev:?}"),
        }
    };
    // Analyzed before its counters are flushed: they count its rows again.
    write(&format!("INSERT INTO {s}.t SELECT g, repeat('x', 100) FROM generate_series(1, 1000) g")).await;
    write(&format!("INSERT INTO {s}.p SELECT g * 1000, 'n' FROM generate_series(0, 1999) g")).await;
    write(&format!("ANALYZE {s}.t, {s}.p")).await;
    write(flush).await;
    let tag = format!("grow{}", std::process::id());
    let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.clone()), ..ConnectionConfig::test_db() };
    let (tx, rx) = unbounded_channel();
    let o = ConnectOptions::new(PAGE, SessionRole::Meta, &tag).dialer(dialer());
    let mut meta = Conn { session: PgDriver.connect(&cfg, SessionRole::Meta, o, tx), rx };
    meta.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let mut observer = Conn::open(&url, SessionRole::Query).await;
    let mut n = 0;
    // The estimates of `table` (its structure's, the same in the schema's list), the relation
    // locks the metadata session held or asked for meanwhile (looked at 300 ms in, or when the
    // structure came), and how long the structure took.
    let mut read = async |table: &str| {
        let t0 = Instant::now();
        meta.session.send(DbCommand::LoadStructure { schema: schema.clone(), table: table.to_string() });
        let ev = tokio::time::timeout(Duration::from_millis(300), meta.rx.recv()).await;
        n += 1;
        let sql = format!(
            "SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE a.application_name = 'datarig-meta-{tag}' AND l.locktype = 'relation' \
             AND l.relation IN (SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relnamespace = '{s}'::regnamespace)"
        );
        let locks = rows_of(observer.run(n, &sql).await)[0][0].clone().unwrap_or_default();
        let ev = match ev {
            Ok(Some(ev)) => ev,
            _ => meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 10).await,
        };
        let took = t0.elapsed();
        let DbEvent::Structure { result, .. } = ev else { panic!("{ev:?}") };
        let st = result.unwrap_or_else(|e| panic!("{table}: {e:?}"));
        meta.session.send(DbCommand::LoadObjects { schema: schema.clone() });
        let DbEvent::Objects { result, .. } = meta.wait(|e| matches!(e, DbEvent::Objects { .. }), 10).await else {
            unreachable!()
        };
        assert_eq!(result.expect("objects").stats.get(table), st.stats().as_ref(), "{table}: the list's are the same");
        (st.estimated_rows, st.total_bytes, locks, took)
    };
    let (rows, small, locks, _) = read("t").await;
    assert_eq!((rows, locks.as_str()), (Some(1000), "0"));
    let small = small.expect("analyzed: a size");
    assert_eq!(read("p").await.0, Some(2000));

    // 199000 more rows (counted with the 1000 counted again).
    write(&format!("INSERT INTO {s}.t SELECT g, repeat('x', 100) FROM generate_series(1001, 200000) g")).await;
    write(&format!("INSERT INTO {s}.p SELECT 1000000 + g, 'n' FROM generate_series(1, 48000) g")).await;
    write(flush).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let (rows, bytes, locks, _) = loop {
        let got = read("t").await;
        if got.0 != Some(1000) || Instant::now() > deadline {
            break got;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(locks, "0");
    let rows = rows.expect("rows");
    assert!((200_000..=201_000).contains(&rows), "the rows added since the ANALYZE: {rows}");
    let bytes = bytes.expect("a size");
    // What the server counts (it locks the table: fine here, nothing else holds it).
    let real = format!("SELECT pg_catalog.pg_total_relation_size('{s}.t')::int8");
    let mut sizer = Conn::open(&url, SessionRole::Query).await;
    let real: u64 = rows_of(sizer.run(1, &real).await)[0][0].as_deref().unwrap().parse().unwrap();
    assert!(bytes > small * 100, "grown with its rows: {small} then {bytes}");
    assert!(bytes <= real * 3 / 2 && bytes >= real * 2 / 3, "~{bytes} for {real} bytes");
    assert_eq!(bytes % 8192, 0, "whole pages");
    let rows = read("p").await.0.expect("rows");
    assert!((50_000..=51_000).contains(&rows), "the partitions' sum: {rows}");

    // Another session holds the table: the estimates come at once, no lock asked for.
    let mut holder = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(holder.run(1, "BEGIN").await, DbEvent::Done { .. }));
    let lock = format!("LOCK TABLE {s}.p_b IN ACCESS EXCLUSIVE MODE");
    assert!(matches!(holder.run(2, &lock).await, DbEvent::Done { .. }));
    let (held, _, locks, took) = read("p").await;
    assert_eq!((held, locks.as_str()), (Some(rows), "0"), "a partition held");
    assert!(took < Duration::from_secs(1), "at once: {took:?}");
    assert!(matches!(holder.run(3, "ROLLBACK").await, DbEvent::Done { .. }));

    // Rows deleted since: the last ANALYZE's estimate stays the floor.
    write(&format!("DELETE FROM {s}.t WHERE id > 500")).await;
    write(flush).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let rows = loop {
        let rows = read("t").await.0;
        if rows.is_some_and(|r| r < 200_000) || Instant::now() > deadline {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(rows, Some(1000), "never below the ANALYZE's");
}

/// A table's structure, and the list of a schema's objects with their estimates, stay fast in a
/// database of some thousand relations (a partitioned table of 2000 partitions and hundreds of
/// small tables, all in one schema): their planned cost grows with the catalog, not with its
/// square, so it stays under `jit_above_cost`, and the metadata session's transaction turns JIT
/// off anyway (a JIT compilation of them costs far more than running them).
/// Here the database compiles every statement (`jit_above_cost = 0`, inlined and optimized),
/// so a read that JIT compiled would take hundreds of milliseconds.
#[tokio::test(flavor = "multi_thread")]
async fn table_structure_stays_fast_with_thousands_of_relations() {
    use datarig_driver_postgres::catalog_sql::{BEGIN_READ, SCHEMA_OBJECTS, TABLE_STRUCTURE};
    let Some(url) = pg_url("table_structure_stays_fast_with_thousands_of_relations") else { return };
    let db = format!("zz_many_{}", std::process::id());
    let _db_guard = DatabaseGuard { url: url.clone(), name: db.clone() };
    pg_clean::run_fresh(&url, &format!("CREATE DATABASE {db}")).unwrap();
    for setting in ["jit_above_cost", "jit_inline_above_cost", "jit_optimize_above_cost"] {
        pg_clean::run_fresh(&url, &format!("ALTER DATABASE {db} SET {setting} = 0")).unwrap();
    }
    let ctx = datarig_core::driver::SessionContext { database: Some(db.clone()), schema: None };
    let mut q = Conn::open_in(&url, SessionRole::Query, false, ctx.clone()).await;
    let mut id = 0;
    let mut run = async |sql: &str| {
        id += 1;
        match q.run(id, sql).await {
            DbEvent::Failed { error, .. } => panic!("{sql}: {error:?}"),
            ev => ev,
        }
    };
    // In slices, so no transaction holds thousands of locks.
    for i in 0..3 {
        run(&format!(
            "DO $$BEGIN FOR i IN {} .. {} LOOP \
             EXECUTE format('CREATE TABLE zz_small_%s (id int PRIMARY KEY, note text)', i); END LOOP; END$$",
            i * 200,
            i * 200 + 199
        ))
        .await;
    }
    run("CREATE TABLE zz_t (id int PRIMARY KEY, note text)").await;
    run("INSERT INTO zz_t SELECT g, 'n' FROM generate_series(1, 100) g").await;
    run("CREATE TABLE zz_part (id int, note text) PARTITION BY RANGE (id)").await;
    for i in 0..10 {
        run(&format!(
            "DO $$BEGIN FOR i IN {} .. {} LOOP EXECUTE format(\
             'CREATE TABLE zz_part_%s PARTITION OF zz_part FOR VALUES FROM (%s) TO (%s)', i, i * 10, i * 10 + 10); \
             END LOOP; END$$",
            i * 200,
            i * 200 + 199
        ))
        .await;
    }
    run("INSERT INTO zz_part SELECT g, 'n' FROM generate_series(0, 19999) g").await;
    run("ANALYZE zz_t, zz_part").await;
    let rows = rows_of(run("SELECT count(*)::text, pg_catalog.pg_jit_available()::text FROM pg_class").await);
    assert!(rows[0][0].as_deref().unwrap().parse::<u32>().unwrap() > 7000, "{rows:?}");
    let jit_available = rows[0][1].as_deref() == Some("true");

    let objects = SCHEMA_OBJECTS.replace("$1", "'public'");
    for (table, sql) in [
        ("zz_t", TABLE_STRUCTURE.replace("$1", "'public'").replace("$2", "'zz_t'")),
        ("zz_part", TABLE_STRUCTURE.replace("$1", "'public'").replace("$2", "'zz_part'")),
        ("the objects of public", objects),
    ] {
        let explain = |sql: &str| format!("EXPLAIN (ANALYZE, FORMAT JSON) {sql}");
        let plan =
            |ev: DbEvent| -> serde_json::Value { serde_json::from_str(rows_of(ev)[0][0].as_deref().unwrap()).unwrap() };
        // The planned cost, under the default `jit_above_cost` (100000).
        let bare = plan(run(&explain(&sql)).await);
        let cost = bare[0]["Plan"]["Total Cost"].as_f64().unwrap();
        assert!(cost < 100_000.0, "{table}: planned cost {cost}");
        // This database compiles every statement, but not in the metadata session's transaction.
        assert_eq!(bare[0].get("JIT").is_some(), jit_available, "{table}: {bare}");
        for begin in BEGIN_READ.split(';') {
            run(begin).await;
        }
        let read = plan(run(&explain(&sql)).await);
        run("COMMIT").await;
        assert!(read[0].get("JIT").is_none(), "{table}: {read}");
    }

    let mut meta = Conn::open_in(&url, SessionRole::Meta, false, ctx).await;
    meta.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    for (table, rows, budget) in [("zz_t", 100, 100), ("zz_part", 20_000, 200)] {
        let mut best = Duration::MAX;
        for _ in 0..3 {
            let t0 = Instant::now();
            meta.session.send(DbCommand::LoadStructure { schema: "public".into(), table: table.into() });
            let DbEvent::Structure { result, .. } = meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 30).await
            else {
                unreachable!()
            };
            best = best.min(t0.elapsed());
            let st = result.expect(table);
            assert_eq!(st.estimated_rows, Some(rows), "{table}");
            assert!(st.total_bytes.is_some_and(|b| b > 0), "{table}");
        }
        assert!(best < Duration::from_millis(budget), "{table}: {best:?} (budget {budget} ms)");
    }
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let t0 = Instant::now();
        meta.session.send(DbCommand::LoadObjects { schema: "public".into() });
        let DbEvent::Objects { result, .. } = meta.wait(|e| matches!(e, DbEvent::Objects { .. }), 30).await else {
            unreachable!()
        };
        best = best.min(t0.elapsed());
        let objects = result.expect("objects");
        assert_eq!(objects.tables.len(), 2602);
        assert_eq!(objects.stats["zz_part"].rows, Some(20_000));
        assert_eq!(objects.stats["zz_part_7"].rows, Some(10));
        assert_eq!(objects.stats["zz_t"].rows, Some(100));
    }
    assert!(best < Duration::from_millis(300), "the objects of public: {best:?} (budget 300 ms)");
}

/// One connection per session, named after its role; the metadata session refuses statements.
#[tokio::test(flavor = "multi_thread")]
async fn each_session_is_one_connection_named_after_its_role() {
    let Some(url) = pg_url("each_session_is_one_connection_named_after_its_role") else { return };
    let tag = format!("role{}", std::process::id());
    let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.clone()), ..ConnectionConfig::test_db() };
    let open = |role| {
        let (tx, rx) = unbounded_channel();
        let session = PgDriver.connect(&cfg, role, ConnectOptions::new(PAGE, role, &tag).dialer(dialer()), tx);
        Conn { session, rx }
    };
    let mut meta = open(SessionRole::Meta);
    meta.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    let mut q = open(SessionRole::Query);
    q.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    let sql = format!("SELECT application_name FROM pg_stat_activity WHERE application_name LIKE '%-{tag}' ORDER BY 1");
    let DbEvent::Page { rows, .. } = q.run(1, &sql).await else { panic!("pg_stat_activity") };
    let names: Vec<_> = rows.iter().map(|r| r[0].clone().unwrap_or_default()).collect();
    assert_eq!(names, [format!("datarig-meta-{tag}"), format!("datarig-q-{tag}")]);
    let ev = meta.run(2, "SELECT 1").await;
    assert!(matches!(ev, DbEvent::Failed { id: 2, cancelled: false, .. }), "{ev:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn portal_paging_fetches_500_rows_per_page() {
    let Some(url) = pg_url("portal_paging_fetches_500_rows_per_page") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT * FROM analytics.events".into()],
        paging: PagingMode::Hold,
    });
    c.wait(|e| matches!(e, DbEvent::TxOpen(true)), 10).await;
    let t0 = Instant::now();
    let DbEvent::Page { columns: Some(cols), rows, more, .. } = c.result(1).await else {
        panic!("expected first page")
    };
    // The whole table would take much longer; the bound is loose for loaded machines.
    assert!(t0.elapsed() < Duration::from_secs(5), "first page took {:?}", t0.elapsed());
    assert_eq!(cols.len(), 6);
    assert_eq!(cols[0].type_name, "int8");
    assert!(cols[0].numeric && cols[4].json);
    assert_eq!(rows.len(), PAGE);
    assert!(more);

    c.session.send(DbCommand::FetchMore { id: 1 });
    let DbEvent::Page { columns: None, rows: page2, more, .. } = c.result(1).await else {
        panic!("expected next page")
    };
    assert_eq!(page2.len(), PAGE);
    assert!(more);
    let ids: std::collections::HashSet<_> = rows.iter().chain(page2.iter()).map(|r| r[0].clone()).collect();
    assert_eq!(ids.len(), 2 * PAGE, "pages must not overlap");

    // Running another statement closes the implicit transaction first.
    c.session.send(DbCommand::Execute { id: 2, statements: vec!["SELECT 1 AS one".into()], paging: PagingMode::Hold });
    c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
    let DbEvent::Page { rows, more, .. } = c.result(2).await else { panic!() };
    assert_eq!(rows, vec![vec![Some("1".to_string())]]);
    assert!(!more);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_request_stops_slow_function() {
    let Some(url) = pg_url("cancel_request_stops_slow_function") else { return };
    let tag = format!("cancel{}", std::process::id());
    let (mut c, mut obs) = tagged(&url, &tag).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT analytics.slow(30)".into()],
        paging: PagingMode::Hold,
    });
    // Cancel once the server runs it (a cancel that arrives before the statement is lost).
    started(&mut obs, &tag).await;
    let t0 = Instant::now();
    c.session.cancel();
    let ev = c.result(1).await;
    // The statement would run 30s; a bound this loose holds on a loaded machine too.
    assert!(t0.elapsed() < Duration::from_secs(10), "cancel took {:?}", t0.elapsed());
    assert!(matches!(ev, DbEvent::Failed { cancelled: true, .. }), "{ev:?}");
    // The connection is immediately usable again.
    let DbEvent::Page { rows, .. } = c.run(2, "SELECT 42").await else { panic!() };
    assert_eq!(rows[0][0].as_deref(), Some("42"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cjk_and_emoji_edge_rows_decode() {
    let Some(url) = pg_url("cjk_and_emoji_edge_rows_decode") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let DbEvent::Page { rows, .. } = c
        .run(1, "SELECT id, name, nickname, address, bio, profile, is_active FROM shop.users WHERE id <= 8 ORDER BY id")
        .await
    else {
        panic!()
    };
    assert_eq!(rows.len(), 8);
    let v = |r: usize, c: usize| rows[r][c].clone();
    assert_eq!(v(0, 1).as_deref(), Some("陳大文"));
    assert_eq!(v(0, 2).as_deref(), Some("大文🐘"));
    assert_eq!(v(1, 2), None, "NULL nickname stays NULL");
    assert_eq!(v(2, 2).as_deref(), Some("👨‍👩‍👧‍👦 family"));
    assert!(v(3, 4).unwrap().contains("🇯🇵🇹🇼🇺🇸"));
    assert_eq!(v(4, 1).as_deref(), Some("山田太郎"));
    assert_eq!(v(5, 1).as_deref(), Some("王小明"));
    assert_eq!(v(7, 3).as_deref(), Some("タブ\t入り住所 (tab inside)"));
    assert_eq!(v(7, 4).as_deref(), Some("複数行\nテキスト\n三行目"));
    assert_eq!(v(2, 6).as_deref(), Some("false"));
    let profile: serde_json::Value = serde_json::from_str(&v(0, 5).unwrap()).unwrap();
    assert_eq!(profile["lang"], "zh-TW");
}

#[tokio::test(flavor = "multi_thread")]
async fn value_formats_and_outcomes() {
    let Some(url) = pg_url("value_formats_and_outcomes") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let DbEvent::Page { rows, .. } = c
        .run(1, "SELECT 1234.50::numeric(10,2), -0.001::numeric, ARRAY[1,2,NULL], '2024-01-02'::date, '1 day 02:00'::interval, ''::text, 'a,b'::text")
        .await
    else {
        panic!()
    };
    let r: Vec<Option<&str>> = rows[0].iter().map(|v| v.as_deref()).collect();
    assert_eq!(
        r,
        [
            Some("1234.50"),
            Some("-0.001"),
            Some("{1,2,NULL}"),
            Some("2024-01-02"),
            Some("1 day 02:00:00"),
            Some(""),
            Some("a,b")
        ]
    );

    // DDL / DML outcomes on the same (query) connection; temp table is session-local.
    let ev = c.run(2, "CREATE TEMP TABLE it_tmp (x int)").await;
    assert!(matches!(ev, DbEvent::Done { outcome: Outcome::Command(ref t), .. } if t == "CREATE TABLE"), "{ev:?}");
    let ev = c.run(3, "INSERT INTO it_tmp VALUES (1), (2)").await;
    assert!(matches!(ev, DbEvent::Done { outcome: Outcome::Affected(2), .. }), "{ev:?}");

    // Visual selection of two statements: last one's result.
    c.session.send(DbCommand::Execute {
        id: 4,
        statements: vec!["SELECT 1".into(), "SELECT 'last'".into()],
        paging: PagingMode::Hold,
    });
    let DbEvent::Page { rows, .. } = c.result(4).await else { panic!() };
    assert_eq!(rows[0][0].as_deref(), Some("last"));

    let ev = c.run(5, "SELECT * FROM no_such_table").await;
    match ev {
        DbEvent::Failed { error, cancelled: false, .. } => {
            assert!(matches!(&error, DbError::Server(s) if s.contains("no_such_table")), "{error:?}")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_port_reports_failure() {
    let Some(_) = pg_url("wrong_port_reports_failure") else { return };
    let cfg = ConnectionConfig { port: 1, ..ConnectionConfig::test_db() };
    let (tx, mut rx) = unbounded_channel();
    let _session = PgDriver.connect(&cfg, SessionRole::Query, opts(SessionRole::Query), tx);
    let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("no event").expect("closed");
    assert!(matches!(ev, DbEvent::ConnectFailed { auth: false, .. }), "{ev:?}");
    // Before `Connected` a failure is only `ConnectFailed` (never `Lost`).
    let next = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(matches!(next, Ok(None)), "{next:?}");
}

// ── test connection (profile manager "Test") ─────────────────────────────────

fn url_profile(url: &str) -> ConnectionConfig {
    ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connection_reports_server_version_and_latency() {
    let Some(url) = pg_url("test_connection_reports_server_version_and_latency") else { return };
    let info =
        PgDriver.ping(&url_profile(&url), Duration::from_secs(5), dialer()).await.expect("test connection succeeds");
    assert!(info.server_version.starts_with(|c: char| c.is_ascii_digit()), "{info:?}");
    assert!(info.latency > Duration::ZERO && info.latency < Duration::from_secs(5), "{info:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connection_wrong_port_fails_within_timeout() {
    let Some(_) = pg_url("test_connection_wrong_port_fails_within_timeout") else { return };
    let cfg = ConnectionConfig { port: 1, ..ConnectionConfig::test_db() };
    let t0 = Instant::now();
    let r = PgDriver.ping(&cfg, Duration::from_secs(5), dialer()).await;
    assert!(t0.elapsed() < Duration::from_secs(6), "took {:?}", t0.elapsed());
    match r {
        Err(PingError::Failed(DbError::Connection(_))) if dialer().is_none() => {}
        Err(PingError::Failed(DbError::Transport(DialError::Failed(_)))) if dialer().is_some() => {}
        other => panic!("expected a readable failure, got {other:?}"),
    }
}

/// A listener that accepts TCP but never speaks the protocol: the test must give up at the
/// timeout instead of hanging, and dropping the future must stop it (cancel button).
#[tokio::test(flavor = "multi_thread")]
async fn test_connection_times_out_and_can_be_cancelled() {
    let Some(_) = pg_url("test_connection_times_out_and_can_be_cancelled") else { return };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hold = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            open.push(sock); // keep the socket open, never answer
        }
    });
    let cfg = ConnectionConfig { port, ..ConnectionConfig::test_db() };
    let t0 = Instant::now();
    let r = PgDriver.ping(&cfg, Duration::from_millis(400), dialer()).await;
    assert_eq!(r, Err(PingError::Timeout(Duration::from_millis(400))));
    assert!(t0.elapsed() < Duration::from_secs(5), "took {:?}", t0.elapsed());

    let task = tokio::spawn(PgDriver.ping(&cfg, Duration::from_secs(30), dialer()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let t1 = Instant::now();
    task.abort();
    let err = task.await.expect_err("aborted");
    assert!(err.is_cancelled());
    assert!(t1.elapsed() < Duration::from_secs(2), "abort took {:?}", t1.elapsed());
    hold.abort();
}

// ── session lifetime and transactions ────────────────────────────

/// A query session with its own `application_name` tag, and an observer session.
async fn tagged(url: &str, tag: &str) -> (Conn, Conn) {
    let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
    let (tx, rx) = unbounded_channel();
    let o = ConnectOptions::new(PAGE, SessionRole::Query, tag).dialer(dialer());
    let session = PgDriver.connect(&cfg, SessionRole::Query, o, tx);
    let mut q = Conn { session, rx };
    q.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    (q, Conn::open(url, SessionRole::Query).await)
}

/// Wait (at most 10s) until the session tagged `tag` runs a statement on the server.
async fn started(observer: &mut Conn, tag: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut n = 1_000;
    while active_queries(observer, n, tag).await.is_empty() {
        assert!(Instant::now() < deadline, "the statement never started");
        n += 1;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Drops a scratch schema with everything in it when it goes out of scope (also when an
/// assertion fails).
struct SchemaGuard {
    url: String,
    schema: String,
}

impl SchemaGuard {
    fn new(url: &str, schema: &str) -> Self {
        Self { url: url.to_string(), schema: schema.to_string() }
    }
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        let sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema);
        if let Err(e) = pg_clean::run_fresh(&self.url, &sql) {
            let _ = writeln!(std::io::stderr(), "warning: {sql}: {e}");
        }
    }
}

/// Statements of the session tagged `tag` that the server still runs.
async fn active_queries(observer: &mut Conn, id: u64, tag: &str) -> Vec<String> {
    let sql =
        format!("SELECT query FROM pg_stat_activity WHERE application_name = 'datarig-q-{tag}' AND state = 'active'");
    let DbEvent::Page { rows, .. } = observer.run(id, &sql).await else { panic!("pg_stat_activity") };
    rows.into_iter().map(|r| r[0].clone().unwrap_or_default()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_session_cancels_the_running_statement() {
    let Some(url) = pg_url("closing_the_session_cancels_the_running_statement") else { return };
    let tag = format!("close{}", std::process::id());
    let (q, mut obs) = tagged(&url, &tag).await;
    q.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT pg_sleep(60)".into()],
        paging: PagingMode::Hold,
    });
    started(&mut obs, &tag).await;
    let mut n = 100;
    let Conn { session, mut rx } = q;
    while rx.try_recv().is_ok() {} // TxOpen(true) of the portal
    let t0 = Instant::now();
    session.close();
    loop {
        n += 1;
        if active_queries(&mut obs, n, &tag).await.is_empty() {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "still running {:?} after close", t0.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // No event after `close` (the UI closed it on purpose).
    let late = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    assert!(matches!(late, Ok(None) | Err(_)), "{late:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminated_backend_fails_the_statement_then_reports_lost_once() {
    let Some(url) = pg_url("a_terminated_backend_fails_the_statement_then_reports_lost_once") else { return };
    let tag = format!("lost{}", std::process::id());
    let (mut q, mut obs) = tagged(&url, &tag).await;
    q.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT pg_sleep(30)".into()],
        paging: PagingMode::Hold,
    });
    started(&mut obs, &tag).await;
    let kill =
        format!("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = 'datarig-q-{tag}'");
    let DbEvent::Page { .. } = obs.run(1, &kill).await else { panic!("terminate") };
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), q.rx.recv()).await {
            Ok(Some(DbEvent::TxOpen(_))) => {}
            Ok(Some(ev)) => seen.push(ev),
            Ok(None) | Err(_) => break,
        }
    }
    assert!(matches!(&seen[..], [DbEvent::Failed { id: 1, .. }, DbEvent::Lost { .. }]), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reading_inside_a_transaction_keeps_it_open() {
    let Some(url) = pg_url("reading_inside_a_transaction_keeps_it_open") else { return };
    let tag = format!("tx{}", std::process::id());
    let table = format!("public.it_tx_{tag}");
    // Dropped last: even a failed assertion leaves no committed table behind.
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let (mut q, mut obs) = tagged(&url, &tag).await;
    let visible = |n| format!("SELECT count(*) FROM pg_class WHERE relname = 'it_tx_{tag}' AND {n} > 0");
    assert!(matches!(q.run(1, "BEGIN").await, DbEvent::Done { .. }));
    q.wait(|e| matches!(e, DbEvent::TxOpen(true)), 5).await;
    assert!(matches!(q.run(2, &format!("CREATE TABLE {table} (x int)")).await, DbEvent::Done { .. }));
    assert!(matches!(q.run(3, &format!("INSERT INTO {table} VALUES (1)")).await, DbEvent::Done { .. }));
    // A row-returning statement inside the block: its portal must not commit the block.
    let DbEvent::Page { rows, .. } = q.run(4, &format!("SELECT x FROM {table}")).await else { panic!() };
    assert_eq!(rows, vec![vec![Some("1".to_string())]]);
    let DbEvent::Page { rows, .. } = obs.run(1, &visible(1)).await else { panic!() };
    assert_eq!(rows[0][0].as_deref(), Some("0"), "another session must not see the uncommitted table");
    // An error aborts the block; it stays open until ROLLBACK.
    assert!(matches!(q.run(5, "SELECT 1/0").await, DbEvent::Failed { .. }));
    assert!(matches!(q.run(6, "SELECT 1").await, DbEvent::Failed { .. }), "aborted block refuses statements");
    q.session.send(DbCommand::Execute { id: 7, statements: vec!["ROLLBACK".into()], paging: PagingMode::Hold });
    q.wait(|e| matches!(e, DbEvent::TxOpen(false)), 5).await;
    let DbEvent::Page { rows, .. } =
        q.run(8, &format!("SELECT count(*) FROM pg_class WHERE relname = 'it_tx_{tag}'")).await
    else {
        panic!()
    };
    assert_eq!(rows[0][0].as_deref(), Some("0"), "rolled back");
}

// ── the transaction indicator (probe only when the state can change) ─────────

/// A query session that follows `TxOpen` like the UI's transaction indicator does.
struct Indicator {
    c: Conn,
    open: bool,
    id: u64,
}

impl Indicator {
    async fn open(url: &str) -> Indicator {
        Indicator { c: Conn::open(url, SessionRole::Query).await, open: false, id: 0 }
    }

    /// Run `sql` and return its result, following `TxOpen` on the way.
    async fn run(&mut self, sql: &str) -> DbEvent {
        self.id += 1;
        let id = self.id;
        self.c.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: PagingMode::Hold });
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.c.rx.recv()).await {
                Ok(Some(DbEvent::TxOpen(open))) => self.open = open,
                Ok(Some(ev @ (DbEvent::Page { .. } | DbEvent::Done { .. } | DbEvent::Failed { .. }))) => return ev,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("{sql:?} did not finish within 30s"),
            }
        }
    }

    /// The indicator once every event of the statements before has arrived: a statement the
    /// session never asks about (`SET`, no rows, no `TxOpen` of its own) runs after them, so its
    /// result comes after all of their events. In a failed block it fails, which changes
    /// nothing either (the block stays open).
    async fn settled(&mut self) -> bool {
        self.run("SET lock_timeout = 0").await;
        self.open
    }

    async fn done(&mut self, sql: &str) {
        let ev = self.run(sql).await;
        assert!(matches!(ev, DbEvent::Done { .. }), "{sql:?}: {ev:?}");
    }

    async fn fails(&mut self, sql: &str) -> String {
        match self.run(sql).await {
            // A statement's failure is the server's own message.
            DbEvent::Failed { error: DbError::Server(s), .. } => s,
            ev => panic!("{sql:?} should fail: {ev:?}"),
        }
    }

    async fn scalar(&mut self, sql: &str) -> Option<String> {
        match self.run(sql).await {
            DbEvent::Page { rows, .. } => rows[0][0].clone(),
            ev => panic!("{sql:?}: {ev:?}"),
        }
    }
}

/// A `COMMIT` that fails (a deferred foreign key) ends the block: the indicator must close, every
/// time. The error arrives before the server's `ReadyForQuery`, so this is asked after the
/// failure, never read from the failed request.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_commit_closes_the_transaction_every_time() {
    let Some(url) = pg_url("a_failed_commit_closes_the_transaction_every_time") else { return };
    let mut s = Indicator::open(&url).await;
    s.done("CREATE TEMP TABLE parent (id int PRIMARY KEY)").await;
    s.done("CREATE TEMP TABLE child (p int REFERENCES parent DEFERRABLE INITIALLY DEFERRED)").await;
    for i in 0..200 {
        s.done("BEGIN").await;
        assert!(s.settled().await, "run {i}: BEGIN opens the block");
        s.done("INSERT INTO child VALUES (1)").await;
        let error = s.fails("COMMIT").await;
        assert!(error.contains("foreign key"), "run {i}: {error}");
        assert!(!s.settled().await, "run {i}: the failed COMMIT ended the block");
    }
    assert_eq!(s.scalar("SELECT count(*) FROM child").await.as_deref(), Some("0"));
}

/// An error inside a block aborts it: the indicator stays on while the server refuses
/// statements, `ROLLBACK TO SAVEPOINT` repairs the block, and only its end turns it off.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_block_stays_open_until_it_ends() {
    let Some(url) = pg_url("an_aborted_block_stays_open_until_it_ends") else { return };
    let mut s = Indicator::open(&url).await;
    s.done("CREATE TEMP TABLE t (x int PRIMARY KEY)").await;
    s.done("BEGIN").await;
    s.done("INSERT INTO t VALUES (1)").await;
    s.done("SAVEPOINT sp").await;
    // A failing statement without rows, then one with rows.
    s.fails("INSERT INTO t VALUES (1)").await;
    assert!(s.settled().await, "aborted, still a block");
    let error = s.fails("SELECT x FROM t").await;
    assert!(error.contains("current transaction is aborted"), "{error}");
    assert!(s.settled().await);
    s.done("ROLLBACK TO SAVEPOINT sp").await;
    assert!(s.settled().await, "repaired, still a block");
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("1"));
    s.fails("SELECT 1/0").await;
    assert!(s.settled().await);
    // COMMIT of an aborted block rolls it back, and ends it.
    s.done("COMMIT").await;
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("0"));
}

/// `COMMIT AND CHAIN` commits and opens the next block at once.
#[tokio::test(flavor = "multi_thread")]
async fn commit_and_chain_keeps_a_block_open() {
    let Some(url) = pg_url("commit_and_chain_keeps_a_block_open") else { return };
    let mut s = Indicator::open(&url).await;
    s.done("CREATE TEMP TABLE t (x int)").await;
    s.done("BEGIN ISOLATION LEVEL REPEATABLE READ").await;
    s.done("INSERT INTO t VALUES (1)").await;
    s.done("COMMIT AND CHAIN").await;
    assert!(s.settled().await, "the chained block is open");
    // Only possible in a block, and the chained one keeps the isolation level.
    s.done("SAVEPOINT sp").await;
    assert_eq!(s.scalar("SHOW transaction_isolation").await.as_deref(), Some("repeatable read"));
    s.done("INSERT INTO t VALUES (2)").await;
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT string_agg(x::text, ',') FROM t").await.as_deref(), Some("1"), "1 committed");
}

/// A procedure may commit: after `CALL` the session asks, whatever happened inside.
#[tokio::test(flavor = "multi_thread")]
async fn call_of_a_procedure_that_commits() {
    let Some(url) = pg_url("call_of_a_procedure_that_commits") else { return };
    let mut s = Indicator::open(&url).await;
    s.done("CREATE TEMP TABLE t (x int)").await;
    s.done(
        "CREATE PROCEDURE pg_temp.commit_then(fail bool) LANGUAGE plpgsql AS $$
         BEGIN
           INSERT INTO t VALUES (1);
           COMMIT;
           INSERT INTO t VALUES (2);
           IF fail THEN RAISE EXCEPTION 'after the commit'; END IF;
         END $$",
    )
    .await;
    // Outside a block the procedure's COMMIT works, and nothing stays open.
    s.done("CALL pg_temp.commit_then(false)").await;
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("2"));
    // A failure after the COMMIT keeps what was committed; still nothing open.
    s.fails("CALL pg_temp.commit_then(true)").await;
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("3"));
    // Inside the user's block a procedure cannot commit: the block is aborted, still open.
    s.done("BEGIN").await;
    let error = s.fails("CALL pg_temp.commit_then(false)").await;
    assert!(error.contains("invalid transaction termination"), "{error}");
    assert!(s.settled().await);
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
    // DO blocks commit the same way.
    s.done("DO $$ BEGIN INSERT INTO t VALUES (9); COMMIT; END $$").await;
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("4"));
}

// ── round trips per statement (wire-level count) ─────────────────────────────

/// Protocol messages seen by [`wire_proxy`]. Every simple `Query` and every `Sync` from the
/// client is answered by one `ReadyForQuery`, i.e. is one request.
#[derive(Default)]
struct Wire {
    /// Simple-protocol `Query` messages from the client.
    queries: std::sync::atomic::AtomicUsize,
    /// Extended-protocol `Sync` messages from the client.
    syncs: std::sync::atomic::AtomicUsize,
    /// Simple queries that ask for the transaction state (`statement_timestamp()`).
    probes: std::sync::atomic::AtomicUsize,
    /// Round trips: reads from the client that follow something from the server (what the
    /// client sends before it hears back is one flight, however it is split up).
    flights: std::sync::atomic::AtomicUsize,
    /// The server spoke since the client's last read.
    server_spoke: std::sync::atomic::AtomicBool,
    /// `flights` when the proxy last passed something from the server on to the client: a
    /// statement's result is in front of the client after this many round trips, whatever it
    /// sends right after (a `COMMIT` that is not waited for).
    delivered: std::sync::atomic::AtomicUsize,
    /// The text of every simple `Query` and every `Parse` from the client, in order.
    texts: std::sync::Mutex<Vec<String>>,
}

impl Wire {
    /// (requests, probes) so far.
    fn get(&self) -> (usize, usize) {
        use std::sync::atomic::Ordering::SeqCst;
        (self.queries.load(SeqCst) + self.syncs.load(SeqCst), self.probes.load(SeqCst))
    }

    fn flights(&self) -> usize {
        self.flights.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn delivered(&self) -> usize {
        self.delivered.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The texts sent so far ([`Wire::texts`]).
    fn texts(&self) -> Vec<String> {
        self.texts.lock().unwrap().clone()
    }
}

/// Relay one direction, splitting the client's byte stream into messages and counting them
/// (the startup message has no type byte). Each read is passed on `delay` after it arrived,
/// without holding up the reads behind it: a stand-in for a remote server's latency.
async fn relay(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    wire: std::sync::Arc<Wire>,
    client: bool,
    delay: Duration,
) {
    use std::sync::atomic::Ordering::SeqCst;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (late_tx, mut late_rx) = unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    let w = wire.clone();
    tokio::spawn(async move {
        while let Some((at, bytes)) = late_rx.recv().await {
            tokio::time::sleep_until(at).await;
            if to.write_all(&bytes).await.is_err() {
                return;
            }
            if !client {
                w.delivered.store(w.flights(), SeqCst);
            }
        }
    });
    let mut buf = Vec::new();
    let mut untyped = true;
    let mut chunk = [0u8; 8192];
    loop {
        let n = match from.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if !client {
            wire.server_spoke.store(true, SeqCst);
        } else {
            if wire.server_spoke.swap(false, SeqCst) {
                wire.flights.fetch_add(1, SeqCst);
            }
            buf.extend_from_slice(&chunk[..n]);
            loop {
                let head = if untyped { 0 } else { 1 };
                if buf.len() < head + 4 {
                    break;
                }
                let len = u32::from_be_bytes(buf[head..head + 4].try_into().unwrap()) as usize;
                if buf.len() < head + len {
                    break;
                }
                match (untyped, buf[0]) {
                    (true, _) => {}
                    (false, b'Q') => {
                        wire.queries.fetch_add(1, SeqCst);
                        if buf[5..1 + len].windows(19).any(|w| w == b"statement_timestamp") {
                            wire.probes.fetch_add(1, SeqCst);
                        }
                        let text = buf[5..1 + len].split(|b| *b == 0).next().unwrap_or_default();
                        wire.texts.lock().unwrap().push(String::from_utf8_lossy(text).into_owned());
                    }
                    // `Parse`: the statement's name, then its text.
                    (false, b'P') => {
                        let text = buf[5..1 + len].split(|b| *b == 0).nth(1).unwrap_or_default();
                        wire.texts.lock().unwrap().push(String::from_utf8_lossy(text).into_owned());
                    }
                    (false, b'S') => _ = wire.syncs.fetch_add(1, SeqCst),
                    _ => {}
                }
                untyped = false;
                buf.drain(..head + len);
            }
        }
        if late_tx.send((tokio::time::Instant::now() + delay, chunk[..n].to_vec())).is_err() {
            return;
        }
    }
}

/// A TCP proxy in front of the test server that counts the client's requests and round trips
/// and delays each direction by `delay`. Returns its port.
async fn wire_proxy(host: String, port: u16, wire: std::sync::Arc<Wire>, delay: Duration) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((down, _)) = listener.accept().await {
            let Ok(up) = tokio::net::TcpStream::connect((host.as_str(), port)).await else { return };
            let (dr, dw) = down.into_split();
            let (ur, uw) = up.into_split();
            tokio::spawn(relay(dr, uw, wire.clone(), true, delay));
            tokio::spawn(relay(ur, dw, wire.clone(), false, delay));
        }
    });
    local
}

/// A query session behind [`wire_proxy`], and the proxy's counts.
async fn wired(url: &str, delay: Duration) -> (Indicator, std::sync::Arc<Wire>) {
    wired_with(url, delay, false).await
}

/// [`wired`] for a profile whose policy is read-only (`read_only`) or not.
async fn wired_with(url: &str, delay: Duration, read_only: bool) -> (Indicator, std::sync::Arc<Wire>) {
    let d = datarig_core::profile::dsn::parse(url).expect("a postgres:// URL");
    let wire = std::sync::Arc::new(Wire::default());
    let port = wire_proxy(d.host.clone(), d.port.unwrap_or(5432), wire.clone(), delay).await;
    let cfg = ConnectionConfig {
        name: "it-wire".into(),
        host: "127.0.0.1".into(),
        port,
        user: d.user.clone(),
        password: d.password.clone().unwrap_or_default(),
        database: d.database.clone(),
        sslmode: "disable".into(),
        ..ConnectionConfig::test_db()
    };
    let (tx, rx) = unbounded_channel();
    let session = PgDriver.connect(&cfg, SessionRole::Query, opts(SessionRole::Query).read_only(read_only), tx);
    let mut c = Conn { session, rx };
    c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    (Indicator { c, open: false, id: 0 }, wire)
}

/// Statements that cannot change the transaction state, and transaction control, cost no
/// question about it: counted on the wire (requests and probes) and timed behind a proxy that
/// adds `DELAY` to every answer.
#[tokio::test(flavor = "multi_thread")]
async fn only_statements_that_can_change_the_transaction_ask_for_it() {
    const DELAY: Duration = Duration::from_millis(20);
    const N: usize = 20;
    let Some(url) = pg_url("only_statements_that_can_change_the_transaction_ask_for_it") else { return };
    let (mut s, wire) = wired(&url, DELAY).await;
    s.done("CREATE TEMP TABLE t (x int)").await;
    // Requests and probes of `sqls`, counted up to the result of the settling statement after
    // them (it runs once every probe of theirs is answered, and asks nothing itself), and the
    // time per statement.
    let mut cost = async |sqls: &[&str]| {
        s.settled().await;
        let before = wire.get();
        let t0 = Instant::now();
        for sql in sqls {
            s.run(sql).await;
        }
        let per = t0.elapsed() / sqls.len() as u32;
        s.settled().await;
        let after = wire.get();
        (after.0 - before.0, after.1 - before.1, per)
    };
    let inserts = vec!["INSERT INTO t VALUES (1)"; N];
    let (requests, probes, per) = cost(&inserts).await;
    let _ = writeln!(
        std::io::stderr(),
        "wire: {N} INSERTs: {requests} requests, {probes} probes, {per:?} per INSERT (answers delayed {DELAY:?})"
    );
    assert_eq!(probes, 0, "DML never asks");
    // Parse, Describe, Bind, Execute and Sync in one request, plus at most
    // the settling statements' own.
    assert!((N..=N + 6).contains(&requests), "{requests}");
    let ddl_set = ["CREATE TEMP TABLE u (x int)", "ALTER TABLE u ADD COLUMN y int", "SET search_path = public"];
    assert_eq!(cost(&ddl_set).await.1, 0, "DDL and SET never ask");
    let (_, probes, per_select) = cost(&["SELECT 1"; 5]).await;
    assert_eq!(probes, 0, "a successful SELECT never asks");
    let _ = writeln!(std::io::stderr(), "wire: {per_select:?} per SELECT");
    // Transaction control sets the state without asking; so does DML inside the block.
    assert_eq!(cost(&["BEGIN"]).await.1, 0);
    assert_eq!(cost(&["UPDATE t SET x = 2", "DELETE FROM t WHERE x = 0"]).await.1, 0);
    assert_eq!(cost(&["SAVEPOINT a"; N]).await.1, 0);
    assert_eq!(cost(&["RELEASE a", "COMMIT"]).await.1, 0);
    // For comparison, statements that ask (the probe runs after the result, so it holds up
    // the next statement). Timings are only printed: they depend on the machine's load.
    let (requests, probes, per_do) = cost(&["DO $$ BEGIN NULL; END $$"; N]).await;
    assert_eq!(probes, N);
    let _ = writeln!(std::io::stderr(), "wire: {N} DOs: {requests} requests, {probes} probes, {per_do:?} per DO");
    // Every failure asks.
    assert_eq!(cost(&["INSERT INTO t VALUES ('x')", "SELECT 1/0"]).await.1, 2);
    assert!(!s.open);
}

/// Every successful transaction control statement sets the indicator from the statement alone
/// (no probe on the wire), in every state it can run in; failures still ask.
#[tokio::test(flavor = "multi_thread")]
async fn transaction_control_sets_the_indicator_without_asking() {
    let Some(url) = pg_url("transaction_control_sets_the_indicator_without_asking") else { return };
    let (mut s, wire) = wired(&url, Duration::ZERO).await;
    let probes = || wire.get().1;
    // `sql` succeeds, asks nothing, and leaves the indicator at `open`. The settling statement
    // runs after any probe of `sql` would have been sent, and asks nothing itself (a success
    // never leaves an aborted block, where it would fail).
    macro_rules! control {
        ($sql:expr, $open:expr) => {{
            let before = probes();
            s.done($sql).await;
            assert_eq!(s.settled().await, $open, "{}: indicator", $sql);
            assert_eq!(probes() - before, 0, "{}: probes", $sql);
        }};
    }
    // `sql` fails and asks once; returns once that probe has been sent.
    macro_rules! failure {
        ($sql:expr) => {{
            let before = probes();
            s.fails($sql).await;
            let deadline = Instant::now() + Duration::from_secs(5);
            while probes() == before {
                assert!(Instant::now() < deadline, "{}: no probe after the failure", $sql);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }};
    }
    // BEGIN / START, also inside a block (a warning); COMMIT / END / ROLLBACK / ABORT, also
    // outside one (a warning).
    control!("BEGIN", true);
    control!("BEGIN", true);
    control!("COMMIT", false);
    control!("COMMIT", false);
    control!("ROLLBACK", false);
    control!("START TRANSACTION", true);
    control!("END", false);
    control!("BEGIN WORK", true);
    control!("ABORT", false);
    // COMMIT of an aborted block rolls it back and ends it.
    control!("BEGIN", true);
    failure!("SELECT 1/0");
    assert!(s.open);
    control!("COMMIT", false);
    // AND CHAIN: a new block, also from an aborted one.
    control!("BEGIN", true);
    control!("COMMIT AND CHAIN", true);
    control!("ROLLBACK AND CHAIN", true);
    failure!("SELECT 1/0");
    control!("COMMIT AND CHAIN", true);
    assert_eq!(s.scalar("SELECT 1").await.as_deref(), Some("1"), "the chained block is not aborted");
    failure!("SELECT 1/0");
    control!("ROLLBACK AND CHAIN", true);
    control!("END AND CHAIN", true);
    control!("ABORT AND CHAIN", true);
    control!("ROLLBACK", false);
    // Outside a block AND CHAIN fails: asked, still idle.
    failure!("COMMIT AND CHAIN");
    assert!(!s.settled().await);
    // Savepoints: ROLLBACK TO repairs an aborted block.
    control!("BEGIN", true);
    control!("SAVEPOINT a", true);
    failure!("SELECT 1/0");
    control!("ROLLBACK TO SAVEPOINT a", true);
    assert_eq!(s.scalar("SELECT 1").await.as_deref(), Some("1"), "repaired");
    control!("RELEASE SAVEPOINT a", true);
    control!("SAVEPOINT b", true);
    control!("ROLLBACK WORK TO b", true);
    control!("RELEASE b", true);
    control!("COMMIT", false);
    failure!("SAVEPOINT c");
    assert!(!s.settled().await);
    // PREPARE TRANSACTION outside a block or in an aborted one rolls back (a warning outside).
    control!("PREPARE TRANSACTION 'it_none'", false);
    control!("BEGIN", true);
    failure!("SELECT 1/0");
    control!("PREPARE TRANSACTION 'it_none'", false);
    // COMMIT / ROLLBACK PREPARED of nothing, and inside a block: failures, asked.
    failure!("COMMIT PREPARED 'it_none'");
    assert!(!s.settled().await);
    control!("BEGIN", true);
    failure!("ROLLBACK PREPARED 'it_none'");
    // Aborted, still a block (settling would fail and ask as well).
    assert!(s.open);
    control!("ROLLBACK", false);
}

/// Nothing runs a query between the user's transaction control and the user's next statement:
/// `SET TRANSACTION` must come before the transaction's first query.
#[tokio::test(flavor = "multi_thread")]
async fn set_transaction_works_right_after_transaction_control() {
    let Some(url) = pg_url("set_transaction_works_right_after_transaction_control") else { return };
    let mut s = Indicator::open(&url).await;
    s.done("BEGIN").await;
    s.done("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").await;
    assert_eq!(s.scalar("SHOW transaction_isolation").await.as_deref(), Some("repeatable read"));
    s.done("ROLLBACK").await;
    s.done("START TRANSACTION").await;
    s.done("SET TRANSACTION READ ONLY").await;
    assert_eq!(s.scalar("SHOW transaction_read_only").await.as_deref(), Some("on"));
    s.done("ROLLBACK").await;
    s.done("BEGIN READ ONLY").await;
    s.done("SET TRANSACTION READ WRITE").await;
    assert_eq!(s.scalar("SHOW transaction_read_only").await.as_deref(), Some("off"));
    // The chained transaction is a new one: its characteristics can be set again.
    s.done("COMMIT AND CHAIN").await;
    s.done("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").await;
    assert_eq!(s.scalar("SHOW transaction_isolation").await.as_deref(), Some("serializable"));
    s.done("ROLLBACK AND CHAIN").await;
    s.done("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").await;
    assert_eq!(s.scalar("SHOW transaction_isolation").await.as_deref(), Some("repeatable read"));
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
}

/// A `REPEATABLE READ` transaction takes its snapshot at the user's first query, not at `BEGIN`:
/// a row another session commits in between is visible, one committed after it is not.
#[tokio::test(flavor = "multi_thread")]
async fn repeatable_read_takes_its_snapshot_at_the_first_query() {
    let Some(url) = pg_url("repeatable_read_takes_its_snapshot_at_the_first_query") else { return };
    let table = format!("public.it_rrsnap_{}", std::process::id());
    // Dropped last: even a failed assertion leaves no committed table behind.
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let mut other = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(other.run(1, &format!("CREATE TABLE {table} (x int)")).await, DbEvent::Done { .. }));
    let mut s = Indicator::open(&url).await;
    let count = format!("SELECT count(*) FROM {table}");
    for (i, begin) in ["BEGIN ISOLATION LEVEL REPEATABLE READ", "START TRANSACTION ISOLATION LEVEL REPEATABLE READ"]
        .iter()
        .enumerate()
    {
        let id = 10 * (i as u64 + 1);
        assert!(matches!(other.run(id, &format!("TRUNCATE {table}")).await, DbEvent::Done { .. }));
        s.done(begin).await;
        assert!(matches!(other.run(id + 1, &format!("INSERT INTO {table} VALUES (1)")).await, DbEvent::Done { .. }));
        assert_eq!(s.scalar(&count).await.as_deref(), Some("1"), "{begin}: committed before the first query");
        assert!(matches!(other.run(id + 2, &format!("INSERT INTO {table} VALUES (2)")).await, DbEvent::Done { .. }));
        assert_eq!(s.scalar(&count).await.as_deref(), Some("1"), "{begin}: the snapshot holds");
        s.done("COMMIT").await;
        assert_eq!(s.scalar(&count).await.as_deref(), Some("2"));
    }
}

/// A host name that does not resolve is told apart from other connection failures (no server
/// needed: `.invalid` never resolves).
#[tokio::test(flavor = "multi_thread")]
async fn a_host_name_that_does_not_resolve_is_host_not_found() {
    let cfg = ConnectionConfig { host: "datarig-no-such-host.invalid".into(), ..ConnectionConfig::test_db() };
    // The direct path's own lookup (a tunnel resolves names at its far end).
    match PgDriver.ping(&cfg, Duration::from_secs(5), None).await {
        Err(PingError::Failed(DbError::Connection(f))) => assert_eq!(f.kind, FaultKind::HostNotFound, "{f:?}"),
        other => panic!("expected a failed lookup, got {other:?}"),
    }
}

/// Every value is shown as PostgreSQL's own text output: types decoded from binary match
/// the type's output function (`format('%s', …)`; `::text` of `inet` adds the mask) exactly (infinity and BC dates, far years), and every other type (bit strings,
/// ranges, multiranges, geometry, text search, network, money, `"char"`, `reg*`, …) comes in
/// the text format, never as raw binary.
#[tokio::test(flavor = "multi_thread")]
async fn values_are_the_servers_own_text() {
    let Some(url) = pg_url("values_are_the_servers_own_text") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let exprs = [
        "B'1011'::bit(4)",
        "B'101'::varbit",
        "int4range(1, 10)",
        "numrange(1.5, NULL, '[]')",
        "'[2024-01-01,infinity)'::daterange",
        "'[\"2024-01-01 00:00\",\"2024-01-02 00:00\")'::tsrange",
        "'{[1,3), [5,7)}'::int4multirange",
        "'(1.5,2)'::point",
        "'{1,2,3}'::line",
        "'[(0,0),(1,1)]'::lseg",
        "'(1,1),(0,0)'::box",
        "'<(0,0),2>'::circle",
        "'((0,0),(1,0),(1,1))'::polygon",
        "'[(0,0),(1,1)]'::path",
        "'a fat cat:2A sat'::tsvector",
        "'fat & (rat | cat)'::tsquery",
        "'192.168.0.1/24'::inet",
        "'10.0.0.0/8'::cidr",
        "'::1'::inet",
        "'08:00:2b:01:02:03'::macaddr",
        "'08:00:2b:01:02:03:04:05'::macaddr8",
        "12.34::money",
        "'x'::\"char\"",
        "'pg_class'::regclass",
        "'int4'::regtype",
        "'16/B374D848'::pg_lsn",
        "'<a>1</a>'::xml",
        "'$.a'::jsonpath",
        "'infinity'::date",
        "'-infinity'::date",
        "'0044-03-15 BC'::date",
        "'0001-01-01 BC'::date",
        "'15689-12-12'::date",
        "'infinity'::timestamp",
        "'-infinity'::timestamp",
        "'0044-03-15 12:00:00.5 BC'::timestamp",
        "'294276-12-31 23:59:59.999999'::timestamp",
        "'infinity'::timestamptz",
        "'Infinity'::float8",
        "'-Infinity'::float4",
        "'1.7976931348623157e308'::float8",
        "'5e-324'::float8",
        "'2.2250738585072014e-308'::float8",
        "'1e15'::float8",
        "'123456789012345'::float8",
        "'0.00001'::float8",
        "'-0'::float8",
        "'3.4028235e38'::float4",
        "'1e-45'::float4",
        "'1234567'::float4",
        "'123456'::float4",
        "'0.1'::float4",
        "'NaN'::float8",
        "'NaN'::numeric",
        "'Infinity'::numeric",
        "'-1 year -2 mons +3 days -04:05:06.5'::interval",
        "'1 year'::interval",
        "'-1 year'::interval",
        "'1 mon -1 day'::interval",
        "'100:00:00'::interval",
        "'0'::interval",
        "'-00:00:01'::interval",
        "'1 day 00:00:01'::interval",
        "'infinity'::interval",
        "ARRAY['(1,2)'::point, NULL]",
        "ARRAY[int4range(1, 2)]",
        "ARRAY['a b', 'tab\there', '', 'NULL', 'q\"uote', 'back\\slash', 'c,d']",
        "ARRAY[[1,2],[3,4]]",
        "'{{{1},{2}},{{3},{NULL}}}'::int[]",
        "'[0:1]={7,8}'::int[]",
        "'[-2:-1][1:2]={{1,2},{3,4}}'::int8[]",
        "'{}'::int[]",
        "ARRAY[[true,false]]",
        "ARRAY[['a b','{x}'],['NULL',NULL]]",
        "ARRAY[['2024-01-01'::date,'infinity']]",
    ];
    let sql =
        format!("SELECT {}", exprs.iter().map(|e| format!("{e}, format('%s', {e})")).collect::<Vec<_>>().join(", "));
    let DbEvent::Page { rows, .. } = c.run(1, &sql).await else { panic!("{sql}") };
    for (i, e) in exprs.iter().enumerate() {
        let (got, want) = (&rows[0][2 * i], &rows[0][2 * i + 1]);
        assert!(got.as_deref().is_some_and(|v| !v.contains('\0')), "{e}: {got:?}");
        assert_eq!(got, want, "{e}");
    }
}

// ── pipelining ────────────────────────────────────────────

/// One way latency of the pipelining tests: long enough that the pieces of one flight always
/// reach the proxy before the server can answer any of them.
const ONE_WAY: Duration = Duration::from_millis(15);

impl Indicator {
    /// Round trips until `sql`'s result was in front of the client, counted once the requests
    /// of the statement before (a trailing `COMMIT`, closes) have been answered.
    async fn rtt(&mut self, wire: &Wire, sql: &str) -> (usize, DbEvent) {
        tokio::time::sleep(ONE_WAY * 8).await;
        let before = wire.flights();
        let ev = self.run(sql).await;
        (wire.delivered() - before, ev)
    }

    /// Round trips until the next page of result `id`.
    async fn fetch_rtt(&mut self, wire: &Wire, id: u64) -> (usize, DbEvent) {
        tokio::time::sleep(ONE_WAY * 8).await;
        let before = wire.flights();
        self.c.session.send(DbCommand::FetchMore { id });
        let ev = self.c.result(id).await;
        (wire.delivered() - before, ev)
    }
}

/// The first page of a row-returning statement costs two round trips when the statement is new
/// to the session (Parse, then BEGIN + Bind + Execute in one write) and one when it is prepared
/// already; each next page one; a statement without rows one.
#[tokio::test(flavor = "multi_thread")]
async fn first_pages_and_commands_cost_one_or_two_round_trips() {
    let Some(url) = pg_url("first_pages_and_commands_cost_one_or_two_round_trips") else { return };
    let (mut s, wire) = wired(&url, ONE_WAY).await;
    let (n, ev) = s.rtt(&wire, "CREATE TEMP TABLE zz_rtt (x int)").await;
    assert!(matches!(ev, DbEvent::Done { .. }), "{ev:?}");
    assert_eq!(n, 1, "DDL: one round trip");
    for i in 0..3 {
        let (n, ev) = s.rtt(&wire, &format!("INSERT INTO zz_rtt VALUES ({i})")).await;
        assert!(matches!(ev, DbEvent::Done { outcome: Outcome::Affected(1), .. }), "{ev:?}");
        assert_eq!(n, 1, "INSERT: one round trip");
    }
    let small = "SELECT x FROM zz_rtt ORDER BY x";
    let (n, ev) = s.rtt(&wire, small).await;
    let DbEvent::Page { rows, more: false, .. } = ev else { panic!("{ev:?}") };
    assert_eq!(rows.len(), 3);
    assert_eq!(n, 2, "a new SELECT: Parse, then the first page");
    for _ in 0..3 {
        let (n, ev) = s.rtt(&wire, small).await;
        assert!(matches!(ev, DbEvent::Page { more: false, .. }), "{ev:?}");
        assert_eq!(n, 1, "a prepared SELECT: one round trip");
    }
    assert!(!s.settled().await, "nothing stays open after a complete result");
    // A large result: the first page, then one round trip per page.
    let big = "SELECT * FROM analytics.events";
    let (n, ev) = s.rtt(&wire, big).await;
    assert!(matches!(ev, DbEvent::Page { more: true, .. }), "{ev:?}");
    assert_eq!(n, 2);
    let id = s.id;
    for _ in 0..3 {
        let (n, ev) = s.fetch_rtt(&wire, id).await;
        let DbEvent::Page { rows, more: true, .. } = ev else { panic!("{ev:?}") };
        assert_eq!(rows.len(), PAGE);
        assert_eq!(n, 1, "a next page: one round trip");
    }
    assert!(s.open, "the portal's transaction holds it open");
    s.c.session.send(DbCommand::ClosePortal { id });
    assert!(!s.settled().await, "closing the portal ends its transaction");
    let (n, ev) = s.rtt(&wire, big).await;
    assert!(matches!(ev, DbEvent::Page { more: true, .. }), "{ev:?}");
    assert_eq!(n, 1, "prepared: the first page of a large result in one round trip");
    s.c.session.send(DbCommand::ClosePortal { id: s.id });
    // Inside the user's block a statement is prepared again (a stale one would abort the block).
    s.done("BEGIN").await;
    let (n, ev) = s.rtt(&wire, small).await;
    assert!(matches!(ev, DbEvent::Page { more: false, .. }), "{ev:?}");
    assert_eq!(n, 2);
    let (n, _) = s.rtt(&wire, "INSERT INTO zz_rtt VALUES (9)").await;
    assert_eq!(n, 1);
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
}

/// Behind latency, pipelined statements keep every transaction rule: a result that fits in a
/// page leaves nothing open, a larger one holds its portal's transaction until it is closed,
/// the user's block is never ended by reading, and rows of a statement that may write are shown
/// only when its COMMIT succeeded.
#[tokio::test(flavor = "multi_thread")]
async fn pipelined_statements_keep_the_transaction_rules() {
    let Some(url) = pg_url("pipelined_statements_keep_the_transaction_rules") else { return };
    let (mut s, _wire) = wired(&url, ONE_WAY).await;
    s.done("CREATE TEMP TABLE t (x int)").await;
    s.done("CREATE TEMP TABLE parent (id int PRIMARY KEY)").await;
    s.done("CREATE TEMP TABLE child (p int REFERENCES parent DEFERRABLE INITIALLY DEFERRED)").await;
    assert_eq!(s.scalar("SELECT 1").await.as_deref(), Some("1"));
    assert!(!s.settled().await);
    let DbEvent::Page { more: true, .. } = s.run("SELECT * FROM analytics.events").await else { panic!() };
    assert!(s.open);
    s.c.session.send(DbCommand::ClosePortal { id: s.id });
    assert!(!s.settled().await);
    // The user's block: reading keeps it, a large result's portal lives in it, closing that
    // portal keeps the block.
    s.done("BEGIN").await;
    s.done("INSERT INTO t VALUES (1)").await;
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("1"));
    assert!(s.settled().await);
    let DbEvent::Page { more: true, .. } = s.run("SELECT * FROM analytics.events").await else { panic!() };
    s.c.session.send(DbCommand::ClosePortal { id: s.id });
    assert!(s.settled().await, "the block stays open");
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT count(*) FROM t").await.as_deref(), Some("0"), "rolled back");
    // INSERT ... RETURNING whose COMMIT fails (a deferred foreign key): the failure, never the
    // rows first.
    s.id += 1;
    let id = s.id;
    s.c.session.send(DbCommand::Execute {
        id,
        statements: vec!["INSERT INTO child VALUES (1) RETURNING p".into()],
        paging: PagingMode::Hold,
    });
    match s.c.result(id).await {
        DbEvent::Failed { error: DbError::Server(e), .. } => assert!(e.contains("foreign key"), "{e}"),
        ev => panic!("the rows of a failed COMMIT were shown: {ev:?}"),
    }
    assert!(!s.settled().await);
    assert_eq!(s.scalar("SELECT count(*) FROM child").await.as_deref(), Some("0"));
    // Committed rows of INSERT ... RETURNING are shown once the COMMIT succeeded.
    s.done("INSERT INTO parent VALUES (1)").await;
    let DbEvent::Page { rows, .. } = s.run("INSERT INTO child VALUES (1), (1) RETURNING p").await else { panic!() };
    assert_eq!(rows.len(), 2);
    assert_eq!(s.scalar("SELECT count(*) FROM child").await.as_deref(), Some("2"));
    assert!(!s.settled().await);
}

/// Cancelling a first page that is still running (its BEGIN, Bind and Execute went out in one
/// write) fails it as cancelled and ends the transaction it started; inside the user's block the
/// block stays, aborted, until the user ends it.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_first_page_ends_its_transaction() {
    let Some(url) = pg_url("cancelling_a_first_page_ends_its_transaction") else { return };
    let (mut s, _wire) = wired(&url, ONE_WAY).await;
    let slow = "SELECT pg_sleep(0.5), x FROM generate_series(1, 20) x";
    for round in 0..2 {
        s.id += 1;
        let id = s.id;
        s.c.session.send(DbCommand::Execute { id, statements: vec![slow.into()], paging: PagingMode::Hold });
        tokio::time::sleep(Duration::from_millis(400)).await;
        s.c.session.cancel();
        match s.c.result(id).await {
            DbEvent::Failed { cancelled: true, .. } => {}
            ev => panic!("round {round}: {ev:?}"),
        }
        assert!(!s.settled().await, "round {round}: nothing stays open");
        assert_eq!(s.scalar("SELECT 1").await.as_deref(), Some("1"));
    }
    s.done("BEGIN").await;
    assert!(s.settled().await);
    s.id += 1;
    let id = s.id;
    s.c.session.send(DbCommand::Execute { id, statements: vec![slow.into()], paging: PagingMode::Hold });
    tokio::time::sleep(Duration::from_millis(400)).await;
    s.c.session.cancel();
    let ev = s.c.result(id).await;
    assert!(matches!(ev, DbEvent::Failed { cancelled: true, .. }), "{ev:?}");
    assert!(s.settled().await, "the user's block stays, aborted");
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
}

/// A failure anywhere in a pipelined request (parsing, binding, executing after some rows)
/// fails the statement, leaves no transaction of its own open, and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_failure_in_the_middle_of_a_pipeline() {
    let Some(url) = pg_url("a_failure_in_the_middle_of_a_pipeline") else { return };
    let (mut s, _wire) = wired(&url, ONE_WAY).await;
    for sql in [
        "SELEC 1",
        "SELECT 1/(x - 3) FROM generate_series(1, 5) x",
        // Fails before anything is sent (no value for the parameter).
        "SELECT $1::int",
        "INSERT INTO no_such_table VALUES (1)",
        "SELECT * FROM analytics.events WHERE id = 1/(id - 700)",
    ] {
        for _ in 0..2 {
            let ev = s.run(sql).await;
            assert!(matches!(ev, DbEvent::Failed { cancelled: false, .. }), "{sql}: {ev:?}");
            assert!(!s.settled().await, "{sql}: nothing stays open");
            assert_eq!(s.scalar("SELECT 2").await.as_deref(), Some("2"), "{sql}: the session goes on");
        }
    }
    // Inside the user's block a failure aborts the block, which stays until the user ends it.
    s.done("BEGIN").await;
    s.fails("SELECT 1/(x - 3) FROM generate_series(1, 5) x").await;
    assert!(s.settled().await);
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
}

/// A prepared statement is run again after the table changed: a changed result type makes the
/// server refuse it before it runs, and it is prepared and run once more; renamed columns and
/// another table through `search_path` are described as they are now, never as prepared.
/// Inside the user's block the statement is prepared again, so DDL there never aborts the block.
#[tokio::test(flavor = "multi_thread")]
async fn prepared_statements_follow_the_schema() {
    let Some(url) = pg_url("prepared_statements_follow_the_schema") else { return };
    let (a, b) = (format!("zz_pipe_a_{}", std::process::id()), format!("zz_pipe_b_{}", std::process::id()));
    let _ga = SchemaGuard::new(&url, &a);
    let _gb = SchemaGuard::new(&url, &b);
    let (mut s, _wire) = wired(&url, ONE_WAY).await;
    s.done("CREATE TEMP TABLE zz_s (a int)").await;
    s.done("INSERT INTO zz_s VALUES (1)").await;
    let star = "SELECT * FROM zz_s";
    let columns = |ev: &DbEvent| match ev {
        DbEvent::Page { columns: Some(c), rows, .. } => {
            (c.iter().map(|c| (c.name.clone(), c.type_name.clone(), c.origin)).collect::<Vec<_>>(), rows.clone())
        }
        ev => panic!("{ev:?}"),
    };
    let (c, _) = columns(&s.run(star).await);
    assert_eq!((c[0].0.as_str(), c[0].1.as_str()), ("a", "int4"));
    s.done("ALTER TABLE zz_s ALTER a TYPE text USING a::text || 'x'").await;
    let (c, rows) = columns(&s.run(star).await);
    assert_eq!((c[0].0.as_str(), c[0].1.as_str()), ("a", "text"), "prepared again");
    assert_eq!(rows[0][0].as_deref(), Some("1x"));
    assert!(!s.settled().await);
    s.done("ALTER TABLE zz_s RENAME a TO b").await;
    let (c, _) = columns(&s.run(star).await);
    assert_eq!(c[0].0, "b", "the portal's own description");
    // The same text, another table.
    for (schema, v) in [(&a, "from a"), (&b, "from b")] {
        s.done(&format!("CREATE SCHEMA {schema}")).await;
        s.done(&format!("CREATE TABLE {schema}.t (v text)")).await;
        s.done(&format!("INSERT INTO {schema}.t VALUES ('{v}')")).await;
    }
    s.done(&format!("SET search_path = {a}")).await;
    let (ca, rows) = columns(&s.run("SELECT v FROM t").await);
    assert_eq!(rows[0][0].as_deref(), Some("from a"));
    s.done(&format!("SET search_path = {b}")).await;
    let (cb, rows) = columns(&s.run("SELECT v FROM t").await);
    assert_eq!(rows[0][0].as_deref(), Some("from b"));
    assert!(ca[0].2.is_some() && cb[0].2.is_some() && ca[0].2 != cb[0].2, "{ca:?} {cb:?}");
    s.done("RESET search_path").await;
    // Inside the user's block.
    s.done("BEGIN").await;
    assert!(matches!(s.run(star).await, DbEvent::Page { .. }));
    s.done("ALTER TABLE zz_s ALTER b TYPE int USING length(b)").await;
    let (c, rows) = columns(&s.run(star).await);
    assert_eq!((c[0].1.as_str(), rows[0][0].as_deref()), ("int4", Some("2")));
    assert!(s.settled().await, "the block is not aborted");
    assert_eq!(s.scalar("SELECT 1").await.as_deref(), Some("1"));
    s.done("ROLLBACK").await;
    assert!(!s.settled().await);
}

/// Every run ends with exactly one terminal event, also while another session changes the
/// table's column type over and over: through the latency proxy a
/// type change lands between a statement's Parse and its Bind, also right after it was prepared
/// again. Each run answers with its rows or with `SchemaChanged` within a bounded time, never
/// with nothing (the tab would stay "running" and ignore cancel), and nothing stays open.
#[tokio::test(flavor = "multi_thread")]
async fn every_run_ends_while_the_table_keeps_changing() {
    const RUNS: usize = 60;
    const PER_RUN: Duration = Duration::from_secs(10);
    let Some(url) = pg_url("every_run_ends_while_the_table_keeps_changing") else { return };
    let table = format!("public.it_flip_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let mut alter = Indicator::open(&url).await;
    alter.done(&format!("CREATE TABLE {table} (a int4)")).await;
    alter.done(&format!("INSERT INTO {table} SELECT generate_series(1, 3)")).await;
    let (mut s, _wire) = wired(&url, Duration::from_millis(50)).await;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flips = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (stop2, flips2, table2) = (stop.clone(), flips.clone(), table.clone());
    let flipper = tokio::spawn(async move {
        use std::sync::atomic::Ordering::SeqCst;
        while !stop2.load(SeqCst) {
            let ty = if flips2.fetch_add(1, SeqCst).is_multiple_of(2) { "int8" } else { "int4" };
            alter.done(&format!("ALTER TABLE {table2} ALTER a TYPE {ty}")).await;
        }
    });
    let sql = format!("SELECT a FROM {table}");
    let (mut rows, mut changed) = (0, 0);
    let t0 = Instant::now();
    for i in 0..RUNS {
        s.id += 1;
        let id = s.id;
        s.c.session.send(DbCommand::Execute { id, statements: vec![sql.clone()], paging: PagingMode::Hold });
        let ended = tokio::time::timeout(PER_RUN, async {
            loop {
                match s.c.rx.recv().await {
                    Some(DbEvent::TxOpen(open)) => s.open = open,
                    Some(ev @ (DbEvent::Page { .. } | DbEvent::Done { .. } | DbEvent::Failed { .. })) => return ev,
                    Some(_) => {}
                    None => panic!("event channel closed"),
                }
            }
        })
        .await;
        match ended {
            Ok(DbEvent::Page { rows: r, more: false, .. }) => {
                assert_eq!(r.len(), 3, "run {i}");
                rows += 1;
            }
            Ok(DbEvent::Failed { error: DbError::SchemaChanged, cancelled: false, .. }) => changed += 1,
            Ok(ev) => panic!("run {i}: {ev:?}"),
            Err(_) => panic!("run {i}: no terminal event within {PER_RUN:?} (the tab would hang)"),
        }
    }
    let elapsed = t0.elapsed();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    flipper.await.unwrap();
    let flips = flips.load(std::sync::atomic::Ordering::SeqCst);
    let _ = writeln!(
        std::io::stderr(),
        "flip: {RUNS} runs in {elapsed:?}: {rows} with rows, {changed} SchemaChanged, {flips} type changes meanwhile"
    );
    assert_eq!(rows + changed, RUNS);
    assert!(rows > 0, "some runs get their rows");
    assert!(flips > RUNS, "the table kept changing: {flips}");
    assert!(elapsed < Duration::from_secs(90), "{elapsed:?}");
    assert!(!s.settled().await, "nothing stays open");
    assert_eq!(s.scalar("SELECT 1").await.as_deref(), Some("1"), "the session goes on");
}

/// Statements prepared inside the user's block are not kept: a block full of other SELECTs
/// does not evict the statements runs outside it reuse, which still cost one round trip.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_does_not_evict_the_statements_kept_outside_it() {
    let Some(url) = pg_url("a_block_does_not_evict_the_statements_kept_outside_it") else { return };
    let (mut s, wire) = wired(&url, ONE_WAY).await;
    let sql = "SELECT 1 AS kept";
    assert!(matches!(s.run(sql).await, DbEvent::Page { .. }));
    assert_eq!(s.rtt(&wire, sql).await.0, 1);
    s.done("BEGIN").await;
    for i in 0..80 {
        assert!(matches!(s.run(&format!("SELECT {i} AS in_block")).await, DbEvent::Page { .. }));
    }
    s.done("COMMIT").await;
    let (n, ev) = s.rtt(&wire, sql).await;
    assert!(matches!(ev, DbEvent::Page { .. }), "{ev:?}");
    assert_eq!(n, 1, "still prepared from before the block");
}

/// Prepared statements the server dropped: after `DEALLOCATE ALL`, `DISCARD ALL` or `DISCARD
/// PLANS` run in the session, the session forgets its statements and prepares again at once
/// (two round trips, no failed attempt first); after one run behind its back (a `DO` block),
/// the server's `26000` makes it prepare again once. The first run after either works.
#[tokio::test(flavor = "multi_thread")]
async fn statements_the_server_dropped_are_prepared_again() {
    let Some(url) = pg_url("statements_the_server_dropped_are_prepared_again") else { return };
    let (mut s, wire) = wired(&url, ONE_WAY).await;
    let sql = "SELECT 7 AS x";
    let seven = |ev: &DbEvent| matches!(ev, DbEvent::Page { rows, .. } if rows[0][0].as_deref() == Some("7"));
    for reset in ["DEALLOCATE ALL", "DISCARD ALL", "DISCARD PLANS", "DEALLOCATE PREPARE ALL"] {
        assert!(seven(&s.run(sql).await));
        assert_eq!(s.rtt(&wire, sql).await.0, 1, "{reset}: prepared");
        s.done(reset).await;
        let (n, ev) = s.rtt(&wire, sql).await;
        assert!(seven(&ev), "{reset}: {ev:?}");
        assert_eq!(n, 2, "{reset}: prepared again at once");
        assert_eq!(s.rtt(&wire, sql).await.0, 1, "{reset}: and kept");
    }
    s.done("DO $$ BEGIN EXECUTE 'DEALLOCATE ALL'; END $$").await;
    let ev = s.run(sql).await;
    assert!(seven(&ev), "{ev:?}");
    assert_eq!(s.rtt(&wire, sql).await.0, 1, "kept");
    assert!(!s.settled().await);
}

/// DML the lexer expects to return no rows runs in one round trip, also when a rule turns it
/// into a SELECT (the extended protocol returns the command's outcome, never the rule's rows,
/// as before). VACUUM and CREATE INDEX CONCURRENTLY, which refuse to run in a transaction block,
/// run on their own.
#[tokio::test(flavor = "multi_thread")]
async fn statements_expected_without_rows() {
    let Some(url) = pg_url("statements_expected_without_rows") else { return };
    let table = format!("public.it_norows_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let (mut s, wire) = wired(&url, ONE_WAY).await;
    s.done("CREATE TEMP TABLE zz_r (x int)").await;
    s.done("CREATE RULE zz_r_rows AS ON INSERT TO zz_r DO INSTEAD SELECT n FROM generate_series(1, 600) n").await;
    let (n, ev) = s.rtt(&wire, "INSERT INTO zz_r VALUES (1)").await;
    assert!(matches!(ev, DbEvent::Done { outcome: Outcome::Affected(0), .. }), "{ev:?}");
    assert_eq!(n, 1);
    assert!(!s.settled().await);
    s.done(&format!("CREATE TABLE {table} (x int)")).await;
    s.done(&format!("VACUUM {table}")).await;
    s.done(&format!("CREATE INDEX CONCURRENTLY ON {table} (x)")).await;
    s.done("SET lock_timeout = 0").await;
    assert!(!s.settled().await);
}

// ── read-only policy ──────────────────────────────────────

/// Drops a function (`name(args)`) on a fresh connection when it goes out of scope.
struct FunctionGuard(String, String);

impl Drop for FunctionGuard {
    fn drop(&mut self) {
        let _ = pg_clean::run_fresh(&self.0, &format!("DROP FUNCTION IF EXISTS {}", self.1));
    }
}

/// The text of the first cell of a one-row result.
fn cell(ev: &DbEvent) -> Option<String> {
    match ev {
        DbEvent::Page { rows, .. } => rows.first().and_then(|r| r.first()).cloned().flatten(),
        _ => None,
    }
}

fn read_only_refusal(ev: &DbEvent) -> bool {
    // SQLSTATE 25006 read_only_sql_transaction; the message is the server's own.
    matches!(ev, DbEvent::Failed { error: DbError::Server(m), .. } if m.contains("read-only transaction"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_refuses_writes_the_text_does_not_show() {
    let Some(url) = pg_url("a_read_only_session_refuses_writes_the_text_does_not_show") else { return };
    let tag = format!("ro{}", std::process::id());
    let table = format!("public.it_ro_{tag}");
    let func = format!("public.it_ro_write_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let _func = FunctionGuard(url.clone(), format!("{func}()"));
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} (id serial, x int)")).unwrap();
    // A plain-string body: no dependency on the table, so either can go first.
    let body = format!("INSERT INTO {table} (x) VALUES (1) RETURNING x");
    pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {func}() RETURNS int LANGUAGE sql VOLATILE AS $${body}$$"))
        .unwrap();
    // The DSN asks for the opposite: the policy's option comes after it and wins.
    let sep = if url.contains('?') { '&' } else { '?' };
    let dsn = format!("{url}{sep}options=-c%20default_transaction_read_only%3Doff");
    let mut q = Conn::open_with(&dsn, SessionRole::Query, true).await;
    assert_eq!(cell(&q.run(1, "SHOW default_transaction_read_only").await).as_deref(), Some("on"));
    // Functions called from a SELECT that write: the text is a read, the server refuses.
    let ev = q.run(2, &format!("SELECT {func}()")).await;
    assert!(read_only_refusal(&ev), "{ev:?}");
    let ev = q.run(3, &format!("SELECT nextval('{table}_id_seq')")).await;
    assert!(read_only_refusal(&ev), "{ev:?}");
    let ev = q.run(4, &format!("INSERT INTO {table} (x) VALUES (1)")).await;
    assert!(read_only_refusal(&ev), "{ev:?}");
    // RESET and DISCARD go back to the value the session started with.
    assert!(matches!(q.run(5, "RESET ALL").await, DbEvent::Done { .. }));
    assert!(matches!(q.run(6, "DISCARD ALL").await, DbEvent::Done { .. }));
    assert_eq!(cell(&q.run(7, "SHOW default_transaction_read_only").await).as_deref(), Some("on"));
    // A transaction is read-only too, and so is the metadata session.
    assert!(matches!(q.run(8, "BEGIN").await, DbEvent::Done { .. }));
    assert_eq!(cell(&q.run(9, "SHOW transaction_read_only").await).as_deref(), Some("on"));
    assert!(matches!(q.run(10, "ROLLBACK").await, DbEvent::Done { .. }));
    let _meta = Conn::open_with(&dsn, SessionRole::Meta, true).await;
    // Nothing was written.
    let mut rw = Conn::open(&url, SessionRole::Query).await;
    assert_eq!(cell(&rw.run(1, &format!("SELECT count(*) FROM {table}")).await).as_deref(), Some("0"));
    assert_eq!(
        cell(&rw.run(2, "SHOW default_transaction_read_only").await).as_deref(),
        Some("off"),
        "other sessions keep theirs"
    );
}

// ── read-only per transaction (a known escape) ──────

/// A read-only session makes every transaction it opens `READ ONLY` on the server: what a
/// statement does to the session's default (a function that calls `set_config()`, which the
/// app cannot see) does not make any later transaction writable, implicit or the user's own.
/// Statements that ask for read-write are not even sent.
#[tokio::test(flavor = "multi_thread")]
async fn every_transaction_of_a_read_only_session_is_read_only() {
    let Some(url) = pg_url("every_transaction_of_a_read_only_session_is_read_only") else { return };
    let tag = format!("rt{}", std::process::id());
    let table = format!("public.it_ro_{tag}");
    let write = format!("public.it_ro_write_{tag}");
    let off = format!("public.it_ro_off_{tag}");
    let tx_off = format!("public.it_ro_txoff_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let _fns = [&write, &off, &tx_off].map(|f| FunctionGuard(url.clone(), format!("{f}()")));
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} (id serial, x int)")).unwrap();
    let body = format!("INSERT INTO {table} (x) VALUES (1) RETURNING x");
    pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {write}() RETURNS int LANGUAGE sql VOLATILE AS $${body}$$"))
        .unwrap();
    for (f, setting) in [(&off, "default_transaction_read_only"), (&tx_off, "transaction_read_only")] {
        let local = setting == "transaction_read_only";
        let body = format!("SELECT set_config('{setting}', 'off', {local})");
        pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {f}() RETURNS text LANGUAGE sql VOLATILE AS $${body}$$"))
            .unwrap();
    }
    let mut q = Conn::open_with(&url, SessionRole::Query, true).await;
    // A known escape, spelled out: the driver does not send it.
    let ev = q.run(1, "SELECT set_config('default_transaction_read_only','off',false)").await;
    assert!(matches!(ev, DbEvent::Failed { error: DbError::ReadWriteRefused, .. }), "{ev:?}");
    for (id, sql) in [
        (2, "SET default_transaction_read_only = off"),
        (3, "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE"),
        (4, "RESET default_transaction_read_only"),
    ] {
        let ev = q.run(id, sql).await;
        assert!(matches!(ev, DbEvent::Failed { error: DbError::ReadWriteRefused, .. }), "{sql}: {ev:?}");
    }
    // The same through a function the text does not show: the default is off now...
    assert!(matches!(q.run(5, &format!("SELECT {off}()")).await, DbEvent::Page { .. }));
    assert_eq!(cell(&q.run(6, "SHOW default_transaction_read_only").await).as_deref(), Some("off"));
    // ...and still every implicit transaction is read-only.
    assert_eq!(cell(&q.run(7, "SELECT current_setting('transaction_read_only')").await).as_deref(), Some("on"));
    for (id, sql) in [
        (8, format!("SELECT {write}()")),
        (9, format!("SELECT nextval('{table}_id_seq')")),
        (10, format!("INSERT INTO {table} (x) VALUES (1)")),
        (11, format!("WITH i AS (INSERT INTO {table} (x) VALUES (1) RETURNING x) SELECT * FROM i")),
        (12, "EXECUTE nothing_prepared".to_string()),
    ] {
        let ev = q.run(id, &sql).await;
        let refused = read_only_refusal(&ev)
            || matches!(&ev, DbEvent::Failed { error: DbError::Server(m), .. } if m.contains("does not exist"));
        assert!(refused, "{sql}: {ev:?}");
    }
    // A new transaction of the user's: read-only too, before and after a query.
    assert!(matches!(q.run(13, "BEGIN").await, DbEvent::Done { .. }));
    assert_eq!(cell(&q.run(14, "SHOW transaction_read_only").await).as_deref(), Some("on"));
    assert!(read_only_refusal(&q.run(15, &format!("INSERT INTO {table} (x) VALUES (1)")).await));
    assert!(matches!(q.run(16, "ROLLBACK").await, DbEvent::Done { .. }));
    assert!(matches!(q.run(17, "BEGIN").await, DbEvent::Done { .. }));
    // Its first statement asks for read-write: not sent (before a query the server would allow it).
    let ev = q.run(18, "SET transaction_read_only = off").await;
    assert!(matches!(ev, DbEvent::Failed { error: DbError::ReadWriteRefused, .. }), "{ev:?}");
    let ev = q.run(19, &format!("SELECT {write}()")).await;
    assert!(read_only_refusal(&ev), "{ev:?}");
    assert!(matches!(q.run(20, "ROLLBACK").await, DbEvent::Done { .. }));
    // A function that turns the transaction read-write: too late, a query already runs.
    assert!(matches!(q.run(21, "BEGIN").await, DbEvent::Done { .. }));
    let ev = q.run(22, &format!("SELECT {tx_off}()")).await;
    assert!(
        matches!(&ev, DbEvent::Failed { error: DbError::Server(m), .. } if m.contains("before any query")),
        "{ev:?}"
    );
    assert!(matches!(q.run(23, "ROLLBACK").await, DbEvent::Done { .. }));
    // EXPLAIN ANALYZE inside the block: read-only before its savepoint.
    assert!(matches!(q.run(24, "BEGIN").await, DbEvent::Done { .. }));
    let ev = q.run(25, &format!("EXPLAIN ANALYZE INSERT INTO {table} (x) VALUES (1)")).await;
    assert!(read_only_refusal(&ev), "{ev:?}");
    assert_eq!(cell(&q.run(26, "SHOW transaction_read_only").await).as_deref(), Some("on"));
    assert!(matches!(q.run(27, "ROLLBACK").await, DbEvent::Done { .. }));
    // Settings and session statements run as before (DISCARD ALL cannot run in a block).
    assert!(matches!(q.run(28, "DISCARD ALL").await, DbEvent::Done { .. }));
    assert!(matches!(q.run(29, "SET work_mem = '8MB'").await, DbEvent::Done { .. }));
    assert_eq!(cell(&q.run(30, "SHOW default_transaction_read_only").await).as_deref(), Some("on"), "DISCARD ALL");
    // Nothing was written.
    let mut rw = Conn::open(&url, SessionRole::Query).await;
    assert_eq!(cell(&rw.run(1, &format!("SELECT count(*) FROM {table}")).await).as_deref(), Some("0"));
    assert_eq!(cell(&rw.run(2, &format!("SELECT last_value FROM {table}_id_seq")).await).as_deref(), Some("1"));
}

/// A TCP proxy in front of the test server that drops the `options` startup parameter, as a
/// connection pooler may. Returns its port.
async fn options_dropping_proxy(host: String, port: u16) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut down, _)) = listener.accept().await {
            let Ok(mut up) = tokio::net::TcpStream::connect((host.as_str(), port)).await else { return };
            tokio::spawn(async move {
                // Untyped messages until the startup message (an SSL request is answered by the
                // server with one byte).
                loop {
                    let mut len = [0u8; 4];
                    if down.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let n = u32::from_be_bytes(len) as usize;
                    let mut body = vec![0u8; n - 4];
                    if down.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let code = u32::from_be_bytes(body[..4].try_into().unwrap());
                    if code != 196608 {
                        let _ = up.write_all(&[&len[..], &body[..]].concat()).await;
                        let mut answer = [0u8; 1];
                        if up.read_exact(&mut answer).await.is_err() || down.write_all(&answer).await.is_err() {
                            return;
                        }
                        continue;
                    }
                    let mut fields: Vec<&[u8]> = body[4..].split(|b| *b == 0).collect();
                    fields.retain(|f| !f.is_empty());
                    let mut out = body[..4].to_vec();
                    for pair in fields.chunks(2) {
                        if pair[0] != b"options" {
                            for f in pair {
                                out.extend_from_slice(f);
                                out.push(0);
                            }
                        }
                    }
                    out.push(0);
                    let mut msg = ((out.len() + 4) as u32).to_be_bytes().to_vec();
                    msg.extend_from_slice(&out);
                    if up.write_all(&msg).await.is_err() {
                        return;
                    }
                    break;
                }
                let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
            });
        }
    });
    local
}

/// A pooler that drops the read-only startup option: the session opens (the UI is told), and
/// every transaction is still read-only.
#[tokio::test(flavor = "multi_thread")]
async fn a_pooler_that_drops_the_startup_option_still_gets_read_only_transactions() {
    let Some(url) = pg_url("a_pooler_that_drops_the_startup_option_still_gets_read_only_transactions") else {
        return;
    };
    let tag = format!("rp{}", std::process::id());
    let table = format!("public.it_ro_{tag}");
    let write = format!("public.it_ro_write_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let _fn = FunctionGuard(url.clone(), format!("{write}()"));
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} (id serial, x int)")).unwrap();
    let body = format!("INSERT INTO {table} (x) VALUES (1) RETURNING x");
    pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {write}() RETURNS int LANGUAGE sql VOLATILE AS $${body}$$"))
        .unwrap();
    let d = datarig_core::profile::dsn::parse(&url).expect("a postgres:// URL");
    let port = options_dropping_proxy(d.host.clone(), d.port.unwrap_or(5432)).await;
    let cfg = ConnectionConfig {
        name: "it-pooler".into(),
        host: "127.0.0.1".into(),
        port,
        user: d.user.clone(),
        password: d.password.clone().unwrap_or_default(),
        database: d.database.clone(),
        sslmode: "disable".into(),
        ..ConnectionConfig::test_db()
    };
    let connect = |role: SessionRole, read_only: bool| {
        let (tx, rx) = unbounded_channel();
        Conn { session: PgDriver.connect(&cfg, role, opts(role).read_only(read_only), tx), rx }
    };
    for role in [SessionRole::Meta, SessionRole::Query] {
        let mut c = connect(role, true);
        c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
        c.wait(|e| matches!(e, DbEvent::ReadOnlyPerTransaction), 5).await;
    }
    let mut q = connect(SessionRole::Query, true);
    q.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    assert_eq!(cell(&q.run(1, "SHOW default_transaction_read_only").await).as_deref(), Some("off"), "dropped");
    assert!(read_only_refusal(&q.run(2, &format!("SELECT {write}()")).await));
    assert!(read_only_refusal(&q.run(3, &format!("INSERT INTO {table} (x) VALUES (1)")).await));
    assert!(matches!(q.run(4, "BEGIN").await, DbEvent::Done { .. }));
    assert!(read_only_refusal(&q.run(5, &format!("UPDATE {table} SET x = 2")).await));
    assert!(matches!(q.run(6, "ROLLBACK").await, DbEvent::Done { .. }));
    // A profile that is not read-only is told nothing.
    let mut rw = connect(SessionRole::Query, false);
    rw.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    assert!(matches!(rw.run(1, "SELECT 1").await, DbEvent::Page { .. }));
    while let Ok(ev) = rw.rx.try_recv() {
        assert!(!matches!(ev, DbEvent::ReadOnlyPerTransaction));
    }
    let mut check = Conn::open(&url, SessionRole::Query).await;
    assert_eq!(cell(&check.run(1, &format!("SELECT count(*) FROM {table}")).await).as_deref(), Some("0"));
}

/// Making each transaction read-only costs no round trip: `BEGIN READ ONLY` goes out with the
/// first page, `SET TRANSACTION READ ONLY` with the first statement of the user's block.
#[tokio::test(flavor = "multi_thread")]
async fn read_only_transactions_cost_no_round_trips() {
    let Some(url) = pg_url("read_only_transactions_cost_no_round_trips") else { return };
    let small = "SELECT id FROM analytics.events WHERE id <= 5";
    let mut counts = Vec::new();
    for read_only in [false, true] {
        let (mut s, wire) = wired_with(&url, ONE_WAY, read_only).await;
        let mut n = Vec::new();
        for sql in [small, small, "SET work_mem = '8MB'", "SHOW work_mem", "BEGIN", small, small, "COMMIT"] {
            let (rtt, ev) = s.rtt(&wire, sql).await;
            assert!(matches!(ev, DbEvent::Page { .. } | DbEvent::Done { .. }), "{sql}: {ev:?}");
            n.push(rtt);
        }
        let (rtt, ev) = s.rtt(&wire, "SELECT * FROM analytics.events").await;
        assert!(matches!(ev, DbEvent::Page { more: true, .. }), "{ev:?}");
        n.push(rtt);
        s.c.session.send(DbCommand::ClosePortal { id: s.id });
        assert!(!s.settled().await);
        counts.push(n);
    }
    let _ = writeln!(std::io::stderr(), "rtt read-write {:?}, read-only {:?}", counts[0], counts[1]);
    assert_eq!(counts[0], counts[1]);
}

/// The classifier's list of built-in functions (`risk::BUILTINS`, which a call may name
/// without making a tab's prepared statements unknown) is exactly the server's: every function
/// of `pg_catalog` that initdb creates, less the ones that run SQL or code they are given
/// (`risk::RUNS_CODE`). Each name of `RUNS_CODE`, `SERVER_FILES` and `SERVER_ACTIONS` is one of
/// the server's built-ins.
#[tokio::test(flavor = "multi_thread")]
async fn the_builtin_functions_are_the_servers() {
    use datarig_core::sql::risk::{BUILTINS, RUNS_CODE, SERVER_ACTIONS, SERVER_FILES};
    let Some(url) = pg_url("the_builtin_functions_are_the_servers") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let list = |names: &[&str]| names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ");
    let catalog = "FROM pg_proc WHERE pronamespace = 'pg_catalog'::regnamespace AND oid < 10000";
    let sql = format!(
        "SELECT string_agg(DISTINCT proname::text COLLATE \"C\", E'\\n' ORDER BY proname::text COLLATE \"C\") \
         {catalog} AND proname NOT IN ({})",
        list(RUNS_CODE)
    );
    let DbEvent::Page { rows, .. } = c.run(1, &sql).await else { panic!("{sql}") };
    assert_eq!(rows[0][0].as_deref(), Some(BUILTINS.trim_end()));
    for (what, names) in [("RUNS_CODE", RUNS_CODE), ("SERVER_FILES", SERVER_FILES), ("SERVER_ACTIONS", SERVER_ACTIONS)]
    {
        let sql = format!(
            "SELECT string_agg(n, ',' ORDER BY n) FROM unnest(ARRAY[{}]::text[]) n \
             WHERE NOT EXISTS (SELECT {catalog} AND proname = n)",
            list(names)
        );
        let DbEvent::Page { rows, .. } = c.run(2, &sql).await else { panic!("{sql}") };
        assert_eq!(rows[0][0], None, "{what} names functions the server does not have");
    }
}

/// The allowlist's volatile built-ins (`risk::repeat::VOLATILE`, which the app never runs
/// again or counts on the user's behalf) are exactly the server's: every
/// function of `pg_catalog` that initdb creates with `provolatile = 'v'`.
#[tokio::test(flavor = "multi_thread")]
async fn the_volatile_builtins_are_the_servers() {
    use datarig_core::sql::risk::repeat::VOLATILE;
    let Some(url) = pg_url("the_volatile_builtins_are_the_servers") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let sql = "SELECT string_agg(DISTINCT proname::text COLLATE \"C\", E'\\n' ORDER BY proname::text COLLATE \"C\") \
               FROM pg_proc WHERE pronamespace = 'pg_catalog'::regnamespace AND oid < 10000 AND provolatile = 'v'";
    let DbEvent::Page { rows, .. } = c.run(1, sql).await else { panic!("{sql}") };
    assert_eq!(rows[0][0].as_deref(), Some(VOLATILE.trim_end()));
}

/// An audit, pinned: every volatile built-in (a function with side effects is
/// volatile) is either classified (`risk::RUNS_CODE`, `SERVER_FILES`, `SERVER_ACTIONS`) or one
/// of these, read one by one and found harmless for the classifier. A PostgreSQL that adds a
/// volatile built-in fails here until it is read too.
#[tokio::test(flavor = "multi_thread")]
async fn every_volatile_builtin_was_read() {
    use datarig_core::sql::risk::{RUNS_CODE, SERVER_ACTIONS, SERVER_FILES};
    const HARMLESS: &[&str] = &[
        // Access method and tablesample handlers, validators: they take `internal` or check.
        "amvalidate",
        "bernoulli",
        "brinhandler",
        "bthandler",
        "ginhandler",
        "gisthandler",
        "hashhandler",
        "heap_tableam_handler",
        "spghandler",
        "system",
        // Fail outside initdb, pg_upgrade and extension scripts.
        "binary_upgrade_add_sub_rel_state",
        "binary_upgrade_create_empty_extension",
        "binary_upgrade_logical_slot_has_caught_up",
        "binary_upgrade_replorigin_advance",
        "binary_upgrade_set_missing_value",
        "binary_upgrade_set_next_array_pg_type_oid",
        "binary_upgrade_set_next_heap_pg_class_oid",
        "binary_upgrade_set_next_heap_relfilenode",
        "binary_upgrade_set_next_index_pg_class_oid",
        "binary_upgrade_set_next_index_relfilenode",
        "binary_upgrade_set_next_multirange_array_pg_type_oid",
        "binary_upgrade_set_next_multirange_pg_type_oid",
        "binary_upgrade_set_next_pg_authid_oid",
        "binary_upgrade_set_next_pg_enum_oid",
        "binary_upgrade_set_next_pg_tablespace_oid",
        "binary_upgrade_set_next_pg_type_oid",
        "binary_upgrade_set_next_toast_pg_class_oid",
        "binary_upgrade_set_next_toast_relfilenode",
        "binary_upgrade_set_record_init_privs",
        "pg_extension_config_dump",
        "pg_stop_making_pinned_objects",
        // Values, clocks and random numbers; the session's own state.
        "array_sample",
        "array_shuffle",
        "clock_timestamp",
        "current_query",
        "gen_random_uuid",
        "pg_nextoid",
        "pg_sleep",
        "pg_sleep_for",
        "pg_sleep_until",
        "pg_stat_clear_snapshot",
        "pg_stat_force_next_flush",
        "random",
        "random_normal",
        "set_config",
        "setseed",
        "timeofday",
        // Sequences: `nextval` and `setval` are refused by a read-only transaction.
        "currval",
        "lastval",
        "nextval",
        "pg_sequence_last_value",
        "setval",
        // Large objects: the writes are refused by a read-only transaction.
        "lo_close",
        "lo_creat",
        "lo_create",
        "lo_from_bytea",
        "lo_get",
        "lo_lseek",
        "lo_lseek64",
        "lo_open",
        "lo_put",
        "lo_tell",
        "lo_tell64",
        "lo_truncate",
        "lo_truncate64",
        "lo_unlink",
        "loread",
        "lowrite",
        // Locks the session holds, a notification, a snapshot for other sessions to import.
        "pg_advisory_lock",
        "pg_advisory_lock_shared",
        "pg_advisory_unlock",
        "pg_advisory_unlock_all",
        "pg_advisory_unlock_shared",
        "pg_advisory_xact_lock",
        "pg_advisory_xact_lock_shared",
        "pg_try_advisory_lock",
        "pg_try_advisory_lock_shared",
        "pg_try_advisory_xact_lock",
        "pg_try_advisory_xact_lock_shared",
        "pg_notify",
        "pg_export_snapshot",
        // Read the server's state, or one fixed file of it.
        "currtid2",
        "pg_available_wal_summaries",
        "pg_blocking_pids",
        "pg_collation_actual_version",
        "pg_control_checkpoint",
        "pg_control_init",
        "pg_control_recovery",
        "pg_control_system",
        "pg_current_logfile",
        "pg_current_wal_flush_lsn",
        "pg_current_wal_insert_lsn",
        "pg_current_wal_lsn",
        "pg_database_collation_actual_version",
        "pg_database_size",
        "pg_get_backend_memory_contexts",
        "pg_get_multixact_members",
        "pg_get_shmem_allocations",
        "pg_get_wait_events",
        "pg_get_wal_replay_pause_state",
        "pg_get_wal_resource_managers",
        "pg_get_wal_summarizer_state",
        "pg_hba_file_rules",
        "pg_ident_file_mappings",
        "pg_indexes_size",
        "pg_is_in_recovery",
        "pg_is_wal_replay_paused",
        "pg_isolation_test_session_is_blocked",
        "pg_jit_available",
        "pg_last_committed_xact",
        "pg_last_wal_receive_lsn",
        "pg_last_wal_replay_lsn",
        "pg_last_xact_replay_timestamp",
        "pg_lock_status",
        "pg_logical_slot_peek_binary_changes",
        "pg_logical_slot_peek_changes",
        "pg_notification_queue_usage",
        "pg_partition_ancestors",
        "pg_partition_tree",
        "pg_prepared_xact",
        "pg_relation_size",
        "pg_replication_origin_progress",
        "pg_replication_origin_session_is_setup",
        "pg_replication_origin_session_progress",
        "pg_safe_snapshot_blocking_pids",
        "pg_show_all_file_settings",
        "pg_show_replication_origin_status",
        "pg_table_size",
        "pg_tablespace_size",
        "pg_total_relation_size",
        "pg_wal_summary_contents",
        "pg_xact_commit_timestamp",
        "pg_xact_commit_timestamp_origin",
        "pg_xact_status",
        "txid_status",
    ];
    let Some(url) = pg_url("every_volatile_builtin_was_read") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let read: Vec<String> =
        [RUNS_CODE, SERVER_FILES, SERVER_ACTIONS, HARMLESS].concat().iter().map(|n| format!("'{n}'")).collect();
    // Trigger functions only run as triggers; the statistics getters only read.
    let sql = format!(
        "SELECT string_agg(DISTINCT proname::text, ',' ORDER BY proname::text) FROM pg_proc \
         WHERE pronamespace = 'pg_catalog'::regnamespace AND oid < 10000 AND provolatile = 'v' \
         AND prorettype <> 'trigger'::regtype AND proname !~ '^pg_stat_(get|have)_' \
         AND proname NOT IN ({})",
        read.join(", ")
    );
    let DbEvent::Page { rows, .. } = c.run(1, &sql).await else { panic!("{sql}") };
    assert_eq!(rows[0][0], None, "volatile built-ins nobody read");
}

// ── the user's block, counts and resumed results ─────────────────────────

impl Conn {
    /// Every event that arrives within `ms` milliseconds of quiet.
    async fn drain(&mut self, ms: u64) -> Vec<DbEvent> {
        let mut out = Vec::new();
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(ms), self.rx.recv()).await {
            out.push(ev);
        }
        out
    }

    /// Send `Count` for result `id` and wait for its answer (and nothing but one).
    async fn count(&mut self, id: u64, sql: &str) -> Result<u64, DbError> {
        self.count_in(id, sql).await.0
    }

    /// [`Conn::count`], with whether it counted in a transaction with one snapshot.
    async fn count_in(&mut self, id: u64, sql: &str) -> (Result<u64, DbError>, bool) {
        self.session.send(DbCommand::Count { id, sql: sql.to_string() });
        let DbEvent::Counted { id: got, result, snapshot } =
            self.wait(|e| matches!(e, DbEvent::Counted { .. }), 30).await
        else {
            unreachable!()
        };
        assert_eq!(got, id);
        let more = self.drain(150).await;
        assert!(!more.iter().any(|e| matches!(e, DbEvent::Counted { .. })), "one answer per count: {more:?}");
        (result, snapshot)
    }
}

fn count_of(sql: &str) -> String {
    datarig_core::sql::risk::repeat::count_query(sql).expect("on the allowlist")
}

/// `Block` reports the user's own block only: a portal's transaction of its own is `TxOpen`
/// alone, `BEGIN` and `COMMIT`/`ROLLBACK` move `Block`, and it comes before the `TxOpen` of the
/// same change.
#[tokio::test(flavor = "multi_thread")]
async fn block_reports_the_users_transaction_only() {
    let Some(url) = pg_url("block_reports_the_users_transaction_only") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT g FROM generate_series(1, 2000) g".into()],
        paging: PagingMode::Hold,
    });
    let DbEvent::Page { more: true, .. } = c.result(1).await else { panic!("a page with more") };
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::Block(_))), "{events:?}");
    c.session.send(DbCommand::ClosePortal { id: 1 });
    c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;

    c.session.send(DbCommand::Execute { id: 2, statements: vec!["BEGIN".into()], paging: PagingMode::Hold });
    let mut seen = Vec::new();
    while seen.len() < 3 {
        let ev = c.wait(|e| matches!(e, DbEvent::Block(_) | DbEvent::TxOpen(_) | DbEvent::Done { .. }), 10).await;
        seen.push(ev);
    }
    let block = seen.iter().position(|e| matches!(e, DbEvent::Block(true))).expect("Block(true)");
    let open = seen.iter().position(|e| matches!(e, DbEvent::TxOpen(true))).expect("TxOpen(true)");
    assert!(block < open, "{seen:?}");
    // A failure in the block keeps it open (aborted); ROLLBACK ends it.
    let _ = c.run(3, "SELECT 1/0").await;
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::Block(false))), "{events:?}");
    c.session.send(DbCommand::Execute { id: 4, statements: vec!["ROLLBACK".into()], paging: PagingMode::Hold });
    c.wait(|e| matches!(e, DbEvent::Block(false)), 10).await;
    c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
}

/// A count while a result pages runs in the portal's transaction under a savepoint: it
/// answers once, and the portal pages on from where it was, also after a count that failed.
#[tokio::test(flavor = "multi_thread")]
async fn a_count_while_paging_keeps_the_portal() {
    let Some(url) = pg_url("a_count_while_paging_keeps_the_portal") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let sql = "SELECT g FROM generate_series(1, 2345) g ORDER BY g";
    c.session.send(DbCommand::Execute { id: 1, statements: vec![sql.into()], paging: PagingMode::Hold });
    let DbEvent::Page { rows, more: true, .. } = c.result(1).await else { panic!("a page with more") };
    assert_eq!(rows.last().unwrap()[0].as_deref(), Some("500"));
    assert_eq!(c.count(1, &count_of(sql)).await, Ok(2345));
    // A count that fails (division by zero on its way) is rolled back to its savepoint.
    let failing = count_of("SELECT g FROM generate_series(1, 10) g WHERE 1 / (g - 7) > 0");
    assert!(matches!(c.count(1, &failing).await, Err(DbError::Server(_))));
    c.session.send(DbCommand::FetchMore { id: 1 });
    let DbEvent::Page { columns: None, rows, more: true, .. } = c.result(1).await else { panic!("the next page") };
    assert_eq!(rows[0][0].as_deref(), Some("501"));
    // Text off the allowlist is never sent.
    assert_eq!(c.count(1, "SELECT nextval('zz_none')").await, Err(DbError::NotSupported));
    assert_eq!(c.count(1, "SELECT 1; DROP TABLE zz_none").await, Err(DbError::NotSupported));
    c.session.send(DbCommand::ClosePortal { id: 1 });
    c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
}

/// Inside the user's block a count never aborts the block nor loses its changes; without a
/// portal it runs on its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_count_in_the_users_block_keeps_the_block() {
    let Some(url) = pg_url("a_count_in_the_users_block_keeps_the_block") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let _ = c.run(1, "CREATE TEMP TABLE zz_count (x int)").await;
    let _ = c.run(2, "BEGIN").await;
    let _ = c.run(3, "INSERT INTO zz_count SELECT g FROM generate_series(1, 3) g").await;
    assert_eq!(c.count(3, &count_of("SELECT * FROM zz_count")).await, Ok(3));
    let failing = count_of("SELECT x FROM zz_count WHERE 1 / (x - 2) > 0");
    assert!(matches!(c.count(3, &failing).await, Err(DbError::Server(_))));
    // Not aborted: the block still sees its own insert.
    let DbEvent::Page { rows, .. } = c.run(4, "SELECT count(*) FROM zz_count").await else { panic!() };
    assert_eq!(rows[0][0].as_deref(), Some("3"));
    let _ = c.run(5, "ROLLBACK").await;
    // Outside any transaction.
    assert_eq!(c.count(5, &count_of("SELECT * FROM zz_count")).await, Ok(0));
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::TxOpen(true))), "{events:?}");
}

/// On a read-only session a count runs in a read-only transaction.
#[tokio::test(flavor = "multi_thread")]
async fn a_count_on_a_read_only_session_is_read_only() {
    let Some(url) = pg_url("a_count_on_a_read_only_session_is_read_only") else { return };
    let mut c = Conn::open_with(&url, SessionRole::Query, true).await;
    let sql = "SELECT current_setting('transaction_read_only') AS ro FROM generate_series(1, 3) WHERE \
               current_setting('transaction_read_only') = 'on'";
    assert_eq!(c.count(1, &count_of(sql)).await, Ok(3));
}

/// `Resume` runs the statement again, drops `skip` rows (over several chunks too) and answers
/// like a run: the next page with its columns, `more` while rows follow; an empty last page
/// when the result now ends within the skipped rows.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_result_starts_after_the_skipped_rows() {
    let Some(url) = pg_url("a_resumed_result_starts_after_the_skipped_rows") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let sql = "SELECT g FROM generate_series(1, 25000) g ORDER BY g";
    c.session.send(DbCommand::Resume { id: 1, sql: sql.into(), skip: 12_000, paging: PagingMode::Hold });
    let DbEvent::Page { columns: Some(cols), rows, more: true, .. } = c.result(1).await else { panic!("a page") };
    assert_eq!(cols[0].name, "g");
    assert_eq!(rows.len(), PAGE);
    assert_eq!(rows[0][0].as_deref(), Some("12001"));
    c.session.send(DbCommand::FetchMore { id: 1 });
    let DbEvent::Page { rows, .. } = c.result(1).await else { panic!("the next page") };
    assert_eq!(rows[0][0].as_deref(), Some("12501"));
    c.session.send(DbCommand::ClosePortal { id: 1 });
    c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
    // Past a chunk and just short of one: the page is topped up to its size.
    for (id, skip) in [(4, 9_800), (5, 10_000), (6, 1)] {
        c.session.send(DbCommand::Resume { id, sql: sql.into(), skip, paging: PagingMode::Hold });
        let DbEvent::Page { rows, more: true, .. } = c.result(id).await else { panic!("{skip}") };
        assert_eq!(rows.len(), PAGE, "{skip}");
        assert_eq!(rows[0][0].as_deref(), Some((skip + 1).to_string().as_str()), "{skip}");
        assert_eq!(rows[PAGE - 1][0].as_deref(), Some((skip + PAGE as u64).to_string().as_str()), "{skip}");
        c.session.send(DbCommand::FetchMore { id });
        let DbEvent::Page { rows, .. } = c.result(id).await else { panic!("{skip}") };
        assert_eq!(rows[0][0].as_deref(), Some((skip + PAGE as u64 + 1).to_string().as_str()), "{skip}");
        c.session.send(DbCommand::ClosePortal { id });
        c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
    }
    // The last rows: a short last page without more.
    c.session.send(DbCommand::Resume { id: 7, sql: sql.into(), skip: 24_900, paging: PagingMode::Hold });
    let DbEvent::Page { rows, more: false, .. } = c.result(7).await else { panic!("the last page") };
    assert_eq!(rows.len(), 100);
    for (id, skip) in [(2, 25_000), (3, 30_000)] {
        c.session.send(DbCommand::Resume { id, sql: sql.into(), skip, paging: PagingMode::Hold });
        let DbEvent::Page { columns: Some(_), rows, more: false, .. } = c.result(id).await else { panic!("{skip}") };
        assert!(rows.is_empty(), "{skip}");
    }
    // Every command ended once: nothing is left open.
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::Page { .. } | DbEvent::Failed { .. })), "{events:?}");
}

/// A resumed result on a read-only session is read in a read-only transaction.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_result_is_read_only_on_a_read_only_session() {
    let Some(url) = pg_url("a_resumed_result_is_read_only_on_a_read_only_session") else { return };
    let mut c = Conn::open_with(&url, SessionRole::Query, true).await;
    let sql = "SELECT current_setting('transaction_read_only') FROM generate_series(1, 700)";
    c.session.send(DbCommand::Resume { id: 1, sql: sql.into(), skip: 1, paging: PagingMode::Hold });
    let DbEvent::Page { rows, more: true, .. } = c.result(1).await else { panic!("a page") };
    assert_eq!(rows[0][0].as_deref(), Some("on"));
}

/// A cancelled count answers `Cancelled`, once, and a portal open meanwhile pages on.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_count_answers_once_and_the_portal_pages_on() {
    let Some(url) = pg_url("a_cancelled_count_answers_once_and_the_portal_pages_on") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let sql = "SELECT g FROM generate_series(1, 2000) g ORDER BY g";
    c.session.send(DbCommand::Execute { id: 1, statements: vec![sql.into()], paging: PagingMode::Hold });
    let DbEvent::Page { more: true, .. } = c.result(1).await else { panic!("a page with more") };
    let slow = count_of("SELECT g FROM generate_series(1, 2000000000) g");
    c.session.send(DbCommand::Count { id: 1, sql: slow });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.session.cancel();
    let DbEvent::Counted { result, .. } = c.wait(|e| matches!(e, DbEvent::Counted { .. }), 20).await else {
        unreachable!()
    };
    assert_eq!(result, Err(DbError::Cancelled));
    c.session.send(DbCommand::FetchMore { id: 1 });
    let DbEvent::Page { rows, .. } = c.result(1).await else { panic!("the next page") };
    assert_eq!(rows[0][0].as_deref(), Some("501"));
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::Counted { .. })), "{events:?}");
}

/// Inside the user's block a statement run again for the user (`Resume`) runs under a
/// savepoint, as a count does: when it fails while it skips, on its first page or on a later
/// page, the block is not aborted and keeps its changes; when it succeeds the savepoint is
/// released with its portal. On a read-only session the block stays read-only.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_result_in_the_users_block_keeps_the_block() {
    let Some(url) = pg_url("a_resumed_result_in_the_users_block_keeps_the_block") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let _ = c.run(1, "CREATE TEMP TABLE zz_resume (x int)").await;
    let _ = c.run(2, "BEGIN").await;
    let _ = c.run(3, "INSERT INTO zz_resume SELECT g FROM generate_series(1, 3) g").await;
    let rows_kept = async |c: &mut Conn, id: u64| {
        let DbEvent::Page { rows, .. } = c.run(id, "SELECT count(*) FROM zz_resume").await else {
            panic!("the block is aborted")
        };
        rows[0][0].clone()
    };
    // Fails while it skips (row 1,000 of the rows it drops).
    let failing = "SELECT 1 / (g - 1000) AS q FROM generate_series(1, 2000) g";
    c.session.send(DbCommand::Resume { id: 4, sql: failing.into(), skip: 500, paging: PagingMode::Hold });
    assert!(matches!(c.result(4).await, DbEvent::Failed { .. }));
    assert_eq!(rows_kept(&mut c, 5).await.as_deref(), Some("3"));
    // Fails on its first page.
    c.session.send(DbCommand::Resume { id: 6, sql: failing.into(), skip: 700, paging: PagingMode::Hold });
    assert!(matches!(c.result(6).await, DbEvent::Failed { .. }));
    assert_eq!(rows_kept(&mut c, 7).await.as_deref(), Some("3"));
    // Fails on a later page (row 1,800).
    let later = "SELECT 1 / (g - 1800) AS q FROM generate_series(1, 3000) g";
    c.session.send(DbCommand::Resume { id: 8, sql: later.into(), skip: 500, paging: PagingMode::Hold });
    let DbEvent::Page { more: true, .. } = c.result(8).await else { panic!("a page with more") };
    c.session.send(DbCommand::FetchMore { id: 8 });
    let DbEvent::Page { columns: None, more: true, .. } = c.result(8).await else { panic!("the next page") };
    c.session.send(DbCommand::FetchMore { id: 8 });
    assert!(matches!(c.result(8).await, DbEvent::Failed { .. }));
    assert_eq!(rows_kept(&mut c, 9).await.as_deref(), Some("3"));
    // Succeeds: the savepoint is gone once the portal ended (the next statement ended it).
    c.session.send(DbCommand::Resume {
        id: 10,
        sql: "SELECT x FROM zz_resume ORDER BY x".into(),
        skip: 1,
        paging: PagingMode::Hold,
    });
    let DbEvent::Page { rows, more: false, .. } = c.result(10).await else { panic!("the rest") };
    assert_eq!(rows.iter().map(|r| r[0].as_deref()).collect::<Vec<_>>(), [Some("2"), Some("3")]);
    c.session.send(DbCommand::Resume {
        id: 11,
        sql: "SELECT g FROM generate_series(1, 2000) g".into(),
        skip: 1,
        paging: PagingMode::Hold,
    });
    let DbEvent::Page { more: true, .. } = c.result(11).await else { panic!("a page with more") };
    assert_eq!(rows_kept(&mut c, 12).await.as_deref(), Some("3"));
    let DbEvent::Failed { error: DbError::Server(e), .. } = c.run(13, "RELEASE SAVEPOINT datarig_resume").await else {
        panic!("no savepoint is left")
    };
    assert!(e.contains("datarig_resume"), "{e}");
    let _ = c.run(14, "ROLLBACK").await;

    // Read-only: the block is made read-only before the savepoint, and stays so after a failure.
    let mut c = Conn::open_with(&url, SessionRole::Query, true).await;
    let _ = c.run(1, "BEGIN").await;
    c.session.send(DbCommand::Resume { id: 2, sql: failing.into(), skip: 500, paging: PagingMode::Hold });
    assert!(matches!(c.result(2).await, DbEvent::Failed { .. }));
    let DbEvent::Page { rows, .. } = c.run(3, "SELECT current_setting('transaction_read_only')").await else {
        panic!("the block is aborted")
    };
    assert_eq!(rows[0][0].as_deref(), Some("on"));
    let _ = c.run(4, "ROLLBACK").await;
    let _ = c.run(5, "BEGIN").await;
    let ro = "SELECT current_setting('transaction_read_only') FROM generate_series(1, 700)";
    c.session.send(DbCommand::Resume { id: 6, sql: ro.into(), skip: 1, paging: PagingMode::Hold });
    let DbEvent::Page { rows, more: true, .. } = c.result(6).await else { panic!("a page") };
    assert_eq!(rows[0][0].as_deref(), Some("on"));
    let _ = c.run(7, "ROLLBACK").await;
}

/// Right before the app runs a statement again or counts it, the server is asked what only it
/// can tell for the allowlist (`risk::repeat::check_query`): a view, a table
/// with row-level security (also on a table that inherits from the one read), a relation it
/// cannot find, a function of the user's with the name of a built-in the statement calls (an
/// overload), one it may call in attribute notation, an operator of the user's that takes a
/// built-in type with the name of one it uses, a type of the user's ahead of `pg_catalog` in
/// `search_path`, and a column type of the user's that functions of the user's take all refuse
/// it (`NotRepeatable`, nothing is run); a count or a re-run inside the user's block that is
/// refused leaves the block as it was. What the text shows (an operator or a type of the
/// user's) is refused without asking. Plain tables, an enum column without code of the user's
/// and `WITH` queries pass.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_is_asked_before_a_statement_runs_again() {
    use datarig_core::sql::risk::repeat::NotRepeatable as N;
    let Some(url) = pg_url("the_server_is_asked_before_a_statement_runs_again") else { return };
    let schema = format!("zz_rep_{}", std::process::id());
    let _guard = SchemaGuard::new(&url, &schema);
    let setup = [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.t AS SELECT g AS id FROM generate_series(1, 700) g"),
        format!("CREATE VIEW {schema}.v AS SELECT * FROM {schema}.t"),
        format!("CREATE FUNCTION {schema}.attr({schema}.t) RETURNS int LANGUAGE sql AS 'SELECT 1'"),
        format!("CREATE FUNCTION {schema}.abs(text) RETURNS text LANGUAGE sql AS 'SELECT $1'"),
        format!("CREATE FUNCTION {schema}.plus(int, text) RETURNS int LANGUAGE sql AS 'SELECT $1'"),
        format!("CREATE OPERATOR {schema}.+ (LEFTARG = int, RIGHTARG = text, FUNCTION = {schema}.plus)"),
        format!("CREATE TABLE {schema}.p (x int)"),
        format!("CREATE TABLE {schema}.kid () INHERITS ({schema}.p)"),
        format!("ALTER TABLE {schema}.kid ENABLE ROW LEVEL SECURITY"),
        format!("CREATE TYPE {schema}.mood AS ENUM ('a', 'b')"),
        format!("CREATE TYPE {schema}.hue AS ENUM ('r', 'g')"),
        format!("CREATE FUNCTION {schema}.sad({schema}.mood) RETURNS bool LANGUAGE sql AS 'SELECT true'"),
        format!("CREATE TABLE {schema}.m AS SELECT 'a'::{schema}.mood AS x"),
        format!("CREATE TABLE {schema}.h AS SELECT 'r'::{schema}.hue AS x FROM generate_series(1, 3)"),
        format!("CREATE DOMAIN {schema}.int8 AS bigint"),
    ];
    for sql in &setup {
        pg_clean::run_fresh(&url, sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let _ = c.run(1, &format!("SET search_path = {schema}, pg_catalog, public")).await;
    let refused = |r: Result<u64, DbError>| match r {
        Err(DbError::NotRepeatable(n)) => n,
        other => panic!("not refused: {other:?}"),
    };
    for (sql, why) in [
        ("SELECT * FROM v", N::NotATable("v".into())),
        ("SELECT * FROM zz_nowhere", N::NotATable("zz_nowhere".into())),
        ("SELECT * FROM p", N::RowSecurity("kid".into())),
        ("SELECT abs('x')", N::Shadowed("abs".into())),
        ("SELECT t.attr FROM t", N::Shadowed("attr".into())),
        ("SELECT (t).attr FROM t", N::Shadowed("attr".into())),
        ("SELECT id + 1 FROM t", N::Shadowed("+".into())),
        ("SELECT '1'::int8", N::UserType("int8".into())),
        ("SELECT * FROM m", N::UserColumnType("mood".into())),
    ] {
        assert_eq!(refused(c.count(2, &count_of(sql)).await), why, "{sql}");
        c.session.send(DbCommand::Resume { id: 3, sql: sql.into(), skip: 1, paging: PagingMode::Hold });
        let DbEvent::Failed { error: DbError::NotRepeatable(n), cancelled: false, .. } = c.result(3).await else {
            panic!("{sql}: run again")
        };
        assert_eq!(n, why, "{sql}");
    }
    // What the text shows is refused without asking the server.
    for (sql, why) in [
        (format!("SELECT 1 OPERATOR({schema}.+) 'x'"), N::UserOperator(format!("{schema}.+"))),
        ("SELECT 1::mood".to_string(), N::UserType("mood".into())),
        ("SELECT * FROM json_to_record('{}') AS r(c mood)".to_string(), N::UserType("mood".into())),
    ] {
        assert_eq!(c.count(4, &sql).await, Err(DbError::NotSupported), "{sql}");
        c.session.send(DbCommand::Resume { id: 5, sql: sql.clone(), skip: 1, paging: PagingMode::Hold });
        let DbEvent::Failed { error: DbError::NotRepeatable(n), .. } = c.result(5).await else { panic!("{sql}") };
        assert_eq!(n, why, "{sql}");
    }
    // Plain tables, an enum column no code of the user's takes, `WITH` queries, built-in
    // operators and types in every form.
    for (sql, n) in [
        ("SELECT id FROM t WHERE id BETWEEN 1 AND 700 ORDER BY id", 700),
        ("SELECT * FROM h", 3),
        ("WITH w AS (SELECT id FROM t) SELECT * FROM w WHERE id OPERATOR(pg_catalog.<=) 5", 5),
        ("SELECT id::text, '1'::pg_catalog.int8, date '2020-01-01' FROM pg_catalog.pg_class, t LIMIT 2", 2),
    ] {
        assert_eq!(c.count(6, &count_of(sql)).await, Ok(n), "{sql}");
    }
    c.session.send(DbCommand::Resume {
        id: 7,
        sql: "SELECT id FROM t ORDER BY id".into(),
        skip: 10,
        paging: PagingMode::Hold,
    });
    let DbEvent::Page { rows, more: true, .. } = c.result(7).await else { panic!("a page") };
    assert_eq!(rows[0][0].as_deref(), Some("11"));
    let _ = c.run(8, "SELECT 1").await;
    // Refused inside the user's block: the block is as it was, and no savepoint is left.
    let _ = c.run(9, "BEGIN").await;
    let _ = c.run(10, "CREATE TEMP TABLE zz_kept (x int)").await;
    let _ = c.run(11, "INSERT INTO zz_kept VALUES (1)").await;
    assert_eq!(refused(c.count(12, &count_of("SELECT * FROM v")).await), N::NotATable("v".into()));
    c.session.send(DbCommand::Resume { id: 13, sql: "SELECT abs('x')".into(), skip: 1, paging: PagingMode::Hold });
    assert!(matches!(c.result(13).await, DbEvent::Failed { error: DbError::NotRepeatable(_), .. }));
    assert_eq!(cell(&c.run(14, "SELECT count(*) FROM zz_kept").await).as_deref(), Some("1"));
    for sp in ["datarig_resume", "datarig_count"] {
        let DbEvent::Failed { error: DbError::Server(e), .. } = c.run(15, &format!("RELEASE SAVEPOINT {sp}")).await
        else {
            panic!("{sp} is left")
        };
        assert!(e.contains(sp), "{e}");
        let _ = c.run(16, "ROLLBACK").await;
        let _ = c.run(17, "BEGIN").await;
    }
    let _ = c.run(18, "ROLLBACK").await;
}

/// The allowlist's built-in operators and types (`risk::repeat::OPERATORS`, `TYPES`) are
/// exactly the server's: every operator and type of `pg_catalog` that initdb creates.
#[tokio::test(flavor = "multi_thread")]
async fn the_builtin_operators_and_types_are_the_servers() {
    use datarig_core::sql::risk::repeat::{OPERATORS, TYPES};
    let Some(url) = pg_url("the_builtin_operators_and_types_are_the_servers") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    for (sql, list) in [
        (
            "SELECT string_agg(DISTINCT oprname::text COLLATE \"C\", E'\\n' ORDER BY oprname::text COLLATE \"C\") \
             FROM pg_operator WHERE oprnamespace = 'pg_catalog'::regnamespace AND oid < 10000",
            OPERATORS,
        ),
        (
            "SELECT string_agg(DISTINCT typname::text COLLATE \"C\", E'\\n' ORDER BY typname::text COLLATE \"C\") \
             FROM pg_type WHERE typnamespace = 'pg_catalog'::regnamespace AND oid < 10000",
            TYPES,
        ),
    ] {
        let DbEvent::Page { rows, .. } = c.run(1, sql).await else { panic!("{sql}") };
        assert_eq!(rows[0][0].as_deref(), Some(list.trim_end()), "{sql}");
    }
}

/// What a count counts: the rows committed when it runs, except in the
/// user's block at `REPEATABLE READ` or `SERIALIZABLE`, whose one snapshot the pages were read
/// in too. 1,000 rows, a portal open at row 500, another session commits 100 more, then a
/// count: (i) outside a transaction (the portal's own, at the default `READ COMMITTED`): 1,100,
/// not the pages' (`snapshot` false); (ii) in the user's `BEGIN` at `READ COMMITTED`: 1,100,
/// `snapshot` false; (iii) in a `REPEATABLE READ` block: 1,000, `snapshot` true; after the
/// portal closed, outside a transaction: 1,100, `snapshot` false. The pages are the 1,000 rows
/// every time, and the count costs the round trips it did before (the isolation is asked in the
/// allowlist's request).
#[tokio::test(flavor = "multi_thread")]
async fn a_count_says_whether_it_counted_the_rows_being_paged() {
    let Some(url) = pg_url("a_count_says_whether_it_counted_the_rows_being_paged") else { return };
    let table = format!("public.it_snap_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let sql = format!("SELECT x FROM {table}");
    let count = count_of(&sql);
    for (case, begin, want, snapshot) in [
        ("(i) outside a transaction", None, 1100, false),
        ("(ii) BEGIN at READ COMMITTED", Some("BEGIN"), 1100, false),
        ("(iii) BEGIN ISOLATION LEVEL REPEATABLE READ", Some("BEGIN ISOLATION LEVEL REPEATABLE READ"), 1000, true),
    ] {
        pg_clean::run_fresh(&url, &format!("DROP TABLE IF EXISTS {table}")).unwrap();
        pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 1000) AS x")).unwrap();
        let mut c = Conn::open(&url, SessionRole::Query).await;
        if let Some(b) = begin {
            let _ = c.run(1, b).await;
        }
        let DbEvent::Page { rows, more: true, .. } = c.run(3, &sql).await else { panic!("{case}: a page") };
        let mut paged = rows.len();
        pg_clean::run_fresh(&url, &format!("INSERT INTO {table} SELECT generate_series(1001, 1100)")).unwrap();
        assert_eq!(c.count_in(3, &count).await, (Ok(want), snapshot), "{case}");
        loop {
            c.session.send(DbCommand::FetchMore { id: 3 });
            let DbEvent::Page { rows, more, .. } = c.result(3).await else { panic!("{case}: fetch") };
            paged += rows.len();
            if !more {
                break;
            }
        }
        assert_eq!(paged, 1000, "{case}: the pages");
        if begin.is_some() {
            let _ = c.run(9, "ROLLBACK").await;
        }
    }
    // After the portal closed, outside a transaction: the rows committed now.
    pg_clean::run_fresh(&url, &format!("DROP TABLE IF EXISTS {table}")).unwrap();
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 1000) AS x")).unwrap();
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let DbEvent::Page { more: true, .. } = c.run(3, &sql).await else { panic!("a page") };
    c.session.send(DbCommand::ClosePortal { id: 3 });
    c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
    pg_clean::run_fresh(&url, &format!("INSERT INTO {table} SELECT generate_series(1001, 1100)")).unwrap();
    assert_eq!(c.count_in(3, &count).await, (Ok(1100), false));
}

/// A plain `SELECT` pages exactly as psql reads it:
/// a plain `BEGIN` with its first page, no `LOCK TABLE` and no isolation of the app's, so the
/// session's default isolation holds (never raised, never lowered). On a table of 60 partitions
/// the open portal holds the locks a cursor of psql holds for the same `SELECT` (the parent and
/// the partitions the plan reads), not one per partition: a `LOCK TABLE` of the parent would
/// lock all 61 (other sessions ran out of shared memory for locks).
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_select_pages_without_a_lock_or_an_isolation_of_its_own() {
    let Some(url) = pg_url("a_plain_select_pages_without_a_lock_or_an_isolation_of_its_own") else { return };
    let parent = format!("it_part_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&format!("public.{parent}")]);
    pg_clean::run_fresh(&url, &format!("CREATE TABLE public.{parent} (k int, v text) PARTITION BY RANGE (k)")).unwrap();
    let partitions = format!(
        "DO $$ BEGIN FOR i IN 0..59 LOOP EXECUTE format('CREATE TABLE public.{parent}_%s PARTITION OF \
         public.{parent} FOR VALUES FROM (%s) TO (%s)', i, i * 100, (i + 1) * 100); END LOOP; END $$"
    );
    pg_clean::run_fresh(&url, &partitions).unwrap();
    let rows = format!("INSERT INTO public.{parent} SELECT g, 'r' || g FROM generate_series(0, 5999) g");
    pg_clean::run_fresh(&url, &rows).unwrap();
    let sql = format!("SELECT k, v FROM public.{parent} WHERE k < 1500");
    risk_allows(&sql);
    // Relation locks of the backend `pid` on the table and its partitions.
    let locks = |pid: &str| {
        format!(
            "SELECT count(*) FROM pg_catalog.pg_locks l JOIN pg_catalog.pg_class c ON c.oid = l.relation \
             WHERE l.pid = {pid} AND l.locktype = 'relation' AND c.relname LIKE '{parent}%'"
        )
    };
    let scalar = |ev: DbEvent| match ev {
        DbEvent::Page { rows, .. } => rows[0][0].clone().unwrap_or_default(),
        other => panic!("{other:?}"),
    };
    let mut observer = Conn::open(&url, SessionRole::Query).await;

    // psql's way: a cursor in a block of its own, one row fetched.
    let mut psql = Conn::open(&url, SessionRole::Query).await;
    let psql_pid = scalar(psql.run(1, "SELECT pg_backend_pid()").await);
    let _ = psql.run(2, "BEGIN").await;
    let _ = psql.run(3, &format!("DECLARE zz_c CURSOR FOR {sql}")).await;
    let _ = psql.run(4, "FETCH 1 FROM zz_c").await;
    let want = scalar(observer.run(1, &locks(&psql_pid)).await);
    let _ = psql.run(5, "ROLLBACK").await;
    assert!(want.parse::<u32>().unwrap() < 61, "the plan reads a few partitions: {want}");

    // The app's portal, outside the user's block, behind a proxy that keeps what it sends.
    let (mut s, wire) = wired(&url, Duration::ZERO).await;
    let pid = s.scalar("SELECT pg_backend_pid()").await.unwrap();
    let DbEvent::Page { rows, more: true, .. } = s.run(&sql).await else { panic!("a first page") };
    assert_eq!(rows.len(), PAGE);
    let id = s.id;
    assert!(s.open, "the portal's transaction is open");
    assert_eq!(scalar(observer.run(2, &locks(&pid)).await), want, "the locks psql's read holds");
    let sent = wire.texts();
    assert!(sent.iter().all(|t| !t.to_ascii_uppercase().contains("LOCK")), "no LOCK TABLE: {sent:?}");
    assert!(sent.iter().all(|t| !t.to_ascii_uppercase().contains("ISOLATION")), "no isolation: {sent:?}");
    assert!(sent.iter().any(|t| t == "BEGIN"), "a plain BEGIN: {sent:?}");
    s.c.session.send(DbCommand::ClosePortal { id });
    s.c.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;

    // The session's default isolation holds, whatever it is.
    let iso = "SELECT current_setting('transaction_isolation') FROM generate_series(1, 700)";
    assert_eq!(s.scalar(iso).await.as_deref(), Some("read committed"));
    s.c.session.send(DbCommand::ClosePortal { id: s.id });
    let _ = s.run("SET default_transaction_isolation = 'serializable'").await;
    assert_eq!(s.scalar(iso).await.as_deref(), Some("serializable"), "never lowered");
    s.c.session.send(DbCommand::ClosePortal { id: s.id });
    let mut r = Conn::open_with(&url, SessionRole::Query, true).await;
    let ro = "SELECT current_setting('transaction_isolation') || ' ' || current_setting('transaction_read_only') \
              FROM generate_series(1, 700)";
    assert_eq!(scalar(r.run(1, ro).await), "read committed on", "read-only as before");
}

/// `sql` is on the plain-`SELECT` allowlist: the statements that were once paged in a snapshot.
fn risk_allows(sql: &str) {
    assert!(datarig_core::sql::risk::repeat::repeatable(sql).is_ok(), "{sql}");
}

/// Drops database `name` when the test ends (`WITH (FORCE)`: its sessions are closed first).
struct DatabaseGuard {
    url: String,
    name: String,
}

impl Drop for DatabaseGuard {
    fn drop(&mut self) {
        let sql = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name);
        if let Err(e) = pg_clean::run_fresh(&self.url, &sql) {
            let _ = writeln!(std::io::stderr(), "warning: {sql}: {e}");
        }
    }
}

fn rows_of(ev: DbEvent) -> Vec<Vec<Option<String>>> {
    match ev {
        DbEvent::Page { rows, .. } => rows,
        other => panic!("rows expected: {other:?}"),
    }
}

/// A session opens in a schema and a database of the profile's server. The schema
/// is the search path's first schema (quoted: a name with a capital, a space and a quote),
/// `public` after it, set as a startup option so `RESET ALL` returns to it,
/// `pg_catalog` still first; the session says
/// where it works (`Context`), a missing schema is missing there; a read-only session in a
/// schema stays read-only; the metadata session lists the databases the user may connect to.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_opens_in_a_database_and_schema() {
    use datarig_core::driver::SessionContext;
    let Some(url) = pg_url("a_session_opens_in_a_database_and_schema") else { return };
    let pid = std::process::id();
    let schema = format!("zz_Ctx \"q\" {pid}");
    let quoted = format!("\"{}\"", schema.replace('"', "\"\""));
    let _guard = SchemaGuard::new(&url, &quoted);
    for sql in [
        format!("CREATE SCHEMA {quoted}"),
        format!("CREATE TABLE {quoted}.zz_t (x int)"),
        format!("INSERT INTO {quoted}.zz_t VALUES (7)"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let ctx = |db: Option<&str>, s: Option<&str>| SessionContext {
        database: db.map(str::to_string),
        schema: s.map(str::to_string),
    };
    let mut c = Conn::open_in(&url, SessionRole::Query, false, ctx(None, Some(&schema))).await;
    let DbEvent::Context { database, schemas } = c.wait(|e| matches!(e, DbEvent::Context { .. }), 10).await else {
        unreachable!()
    };
    assert_eq!((database.as_str(), schemas.clone()), ("datarig", vec![schema.clone(), "public".to_string()]));
    assert_eq!(rows_of(c.run(1, "SELECT x FROM zz_t").await), [[Some("7".to_string())]]);
    let path = rows_of(c.run(2, "SELECT pg_catalog.current_schemas(true)::text").await);
    assert!(path[0][0].as_deref().unwrap().starts_with("{pg_catalog,"), "{path:?}");
    // `RESET ALL` goes back to the startup option, not to the server's default.
    c.run(3, "RESET ALL").await;
    assert_eq!(rows_of(c.run(4, "SELECT x FROM zz_t").await), [[Some("7".to_string())]]);
    drop(c);
    // Read-only in a schema: reads work, writes are refused.
    let mut ro = Conn::open_in(&url, SessionRole::Query, true, ctx(None, Some(&schema))).await;
    assert_eq!(rows_of(ro.run(1, "SELECT x FROM zz_t").await), [[Some("7".to_string())]]);
    let DbEvent::Failed { error, .. } = ro.run(2, "INSERT INTO zz_t VALUES (8)").await else { panic!("a write ran") };
    assert!(error.raw().contains("read-only"), "{error:?}");
    drop(ro);
    // A schema that does not exist: the session opens, and its context says it is missing.
    let mut none = Conn::open_in(&url, SessionRole::Query, false, ctx(None, Some("zz_no_such_schema"))).await;
    let DbEvent::Context { schemas, .. } = none.wait(|e| matches!(e, DbEvent::Context { .. }), 10).await else {
        unreachable!()
    };
    assert_eq!(schemas, ["public"], "only public is there");
    drop(none);
    // Another database: a connection of its own.
    let db = format!("zz_ctxdb_{pid}");
    let _db_guard = DatabaseGuard { url: url.clone(), name: db.clone() };
    pg_clean::run_fresh(&url, &format!("CREATE DATABASE {db}")).unwrap();
    let mut meta = Conn::open(&url, SessionRole::Meta).await;
    meta.session.send(DbCommand::LoadDatabases);
    let DbEvent::Databases(Ok(dbs)) = meta.wait(|e| matches!(e, DbEvent::Databases(_)), 10).await else {
        panic!("databases failed")
    };
    assert!(dbs.contains(&"datarig".to_string()) && dbs.contains(&db), "{dbs:?}");
    assert!(!dbs.iter().any(|d| d.starts_with("template")), "{dbs:?}");
    drop(meta);
    let mut other = Conn::open_in(&url, SessionRole::Query, false, ctx(Some(&db), Some("public"))).await;
    let DbEvent::Context { database, schemas } = other.wait(|e| matches!(e, DbEvent::Context { .. }), 10).await else {
        unreachable!()
    };
    assert_eq!((database.as_str(), schemas), (db.as_str(), vec!["public".to_string()]));
    assert_eq!(rows_of(other.run(1, "SELECT current_database()::text").await), [[Some(db.clone())]]);
    drop(other);
    // The metadata session of another database lists that database's schemas.
    let mut meta = Conn::open_in(&url, SessionRole::Meta, false, ctx(Some(&db), None)).await;
    let DbEvent::Schemas(Ok(schemas)) = meta.wait(|e| matches!(e, DbEvent::Schemas(_)), 10).await else {
        panic!("schemas failed")
    };
    assert_eq!(schemas, ["public"]);
}

/// Drops a table on a fresh connection when it goes out of scope.
struct TableGuard(String, String);

impl Drop for TableGuard {
    fn drop(&mut self) {
        let _ = pg_clean::run_fresh(&self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
    }
}

/// `public` follows the chosen schema. With schema `s` chosen, a function only
/// `public` has resolves unqualified, a table of `s` wins over `public`'s of the same name, and
/// `pg_catalog` stays first (a function of `s` named like a built-in does not shadow it).
#[tokio::test(flavor = "multi_thread")]
async fn public_follows_the_chosen_schema() {
    use datarig_core::driver::SessionContext;
    let Some(url) = pg_url("public_follows_the_chosen_schema") else { return };
    let pid = std::process::id();
    let schema = format!("zz_pubctx_{pid}");
    let function = format!("public.zz_pubfn_{pid}()");
    let table = format!("zz_pubt_{pid}");
    let _schema = SchemaGuard::new(&url, &schema);
    let _function = FunctionGuard(url.clone(), function.clone());
    let _table = TableGuard(url.clone(), format!("public.{table}"));
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE FUNCTION {function} RETURNS int LANGUAGE sql AS 'SELECT 42'"),
        format!("CREATE TABLE public.{table} (w text)"),
        format!("INSERT INTO public.{table} VALUES ('public')"),
        format!("CREATE TABLE {schema}.{table} (w text)"),
        format!("INSERT INTO {schema}.{table} VALUES ('chosen')"),
        format!("CREATE FUNCTION {schema}.lower(text) RETURNS text LANGUAGE sql AS $$SELECT 'shadow'$$"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let ctx = SessionContext { database: None, schema: Some(schema.clone()) };
    let mut c = Conn::open_in(&url, SessionRole::Query, false, ctx).await;
    let DbEvent::Context { schemas, .. } = c.wait(|e| matches!(e, DbEvent::Context { .. }), 10).await else {
        unreachable!()
    };
    assert_eq!(schemas, [schema.clone(), "public".to_string()]);
    assert_eq!(cell(&c.run(1, &format!("SELECT zz_pubfn_{pid}()")).await).as_deref(), Some("42"));
    assert_eq!(cell(&c.run(2, &format!("SELECT w FROM {table}")).await).as_deref(), Some("chosen"));
    assert_eq!(cell(&c.run(3, "SELECT lower('X')").await).as_deref(), Some("x"), "pg_catalog first");
    let path = c.run(4, "SELECT pg_catalog.current_setting('search_path')").await;
    assert_eq!(cell(&path), Some(format!("\"{schema}\", public")));
    // `public` itself: the path is `public` alone.
    let ctx = SessionContext { database: None, schema: Some("public".to_string()) };
    let mut p = Conn::open_in(&url, SessionRole::Query, false, ctx).await;
    let path = p.run(1, "SELECT pg_catalog.current_setting('search_path')").await;
    assert_eq!(cell(&path).as_deref(), Some("\"public\""));
}

/// Behind a pooler that drops the startup `options` (the in-process proxy of
/// `pg_proxy`), a session in a schema never sets `search_path` for the session (on a pooled
/// connection it would stay for other clients, and later transactions may land elsewhere): it
/// says `ContextPerTransaction` and sends `SET LOCAL search_path …` in every transaction, in the
/// same write as what goes there anyway. Every path is exercised: a prepared statement and its
/// portal, a statement without rows, one the lexer does not know returns none (`EXECUTE`),
/// `PREPARE`, the user's block (with `SET TRANSACTION ISOLATION LEVEL` after it, which still
/// works: `SET LOCAL` takes no snapshot), a count and a statement run again; `RESET ALL` and
/// `DISCARD ALL` no longer lose the schema; a read-only profile still refuses writes. The wire
/// shows no session-level set and `SET LOCAL` in each block.
#[tokio::test(flavor = "multi_thread")]
async fn behind_a_pooler_the_path_is_set_in_each_transaction() {
    use datarig_core::driver::SessionContext;
    let Some(url) = pg_url("behind_a_pooler_the_path_is_set_in_each_transaction") else { return };
    let pid = std::process::id();
    let schema = format!("zz_pool_{pid}");
    let _guard = SchemaGuard::new(&url, &schema);
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.zz_pt (x int)"),
        format!("INSERT INTO {schema}.zz_pt SELECT generate_series(1, 3)"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let proxy = pg_proxy::Proxy::start(&pg_proxy::upstream(&url)).await;
    let purl = proxy.url(&url);
    let ctx = SessionContext { database: None, schema: Some(schema.clone()) };
    let mut c = Conn::open_in(&purl, SessionRole::Query, false, ctx.clone()).await;
    assert_eq!(*proxy.dropped.lock().unwrap(), 1, "the proxy dropped the options");
    c.wait(|e| matches!(e, DbEvent::ContextPerTransaction), 10).await;
    let DbEvent::Context { schemas, .. } = c.wait(|e| matches!(e, DbEvent::Context { .. }), 10).await else {
        unreachable!()
    };
    assert_eq!(schemas, [schema.clone(), "public".to_string()], "asked with the path set");
    let one = |ev: DbEvent| cell(&ev);
    // A prepared statement and its portal (twice: prepared again, then reused).
    assert_eq!(one(c.run(1, "SELECT count(*) FROM zz_pt").await).as_deref(), Some("3"));
    assert_eq!(one(c.run(2, "SELECT count(*) FROM zz_pt").await).as_deref(), Some("3"));
    // Without rows (known and not), and `PREPARE`.
    assert!(matches!(c.run(3, "INSERT INTO zz_pt VALUES (4)").await, DbEvent::Done { .. }));
    assert!(matches!(c.run(4, "PREPARE zz_ins AS INSERT INTO zz_pt VALUES (5)").await, DbEvent::Done { .. }));
    assert!(matches!(c.run(5, "EXECUTE zz_ins").await, DbEvent::Done { .. }));
    assert_eq!(one(c.run(6, "SELECT count(*) FROM zz_pt").await).as_deref(), Some("5"));
    // `RESET ALL` and `DISCARD ALL` (outside a block) keep the schema: each transaction sets it.
    for (id, sql) in [(7, "RESET ALL"), (9, "DISCARD ALL")] {
        assert!(matches!(c.run(id, sql).await, DbEvent::Done { .. }), "{sql}");
        assert_eq!(one(c.run(id + 1, "SELECT count(*) FROM zz_pt").await).as_deref(), Some("5"), "after {sql}");
    }
    // The user's block: the path first, then `SET TRANSACTION` still works.
    assert!(matches!(c.run(11, "BEGIN").await, DbEvent::Done { .. }));
    assert!(matches!(c.run(12, "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").await, DbEvent::Done { .. }));
    let iso = c.run(13, "SELECT current_setting('transaction_isolation')").await;
    assert_eq!(cell(&iso).as_deref(), Some("serializable"));
    assert_eq!(one(c.run(14, "SELECT count(*) FROM zz_pt").await).as_deref(), Some("5"));
    assert!(matches!(c.run(15, "DELETE FROM zz_pt WHERE x > 3").await, DbEvent::Done { .. }));
    assert!(matches!(c.run(16, "COMMIT").await, DbEvent::Done { .. }));
    // A count, and a statement run again past rows it skips.
    assert_eq!(c.count(17, &count_of("SELECT x FROM zz_pt")).await, Ok(3));
    c.session.send(DbCommand::Resume {
        id: 18,
        sql: "SELECT x FROM zz_pt ORDER BY x".into(),
        skip: 1,
        paging: PagingMode::Hold,
    });
    let DbEvent::Page { rows, .. } = c.result(18).await else { panic!("resume failed") };
    assert_eq!(rows, [[Some("2".to_string())], [Some("3".to_string())]]);
    // What the user reads is the path of their transaction (the session's own is untouched:
    // the wire check below). A `SET` statement stores it as the server quotes it.
    let shown = c.run(19, "SHOW search_path").await;
    assert_eq!(cell(&shown), Some(format!("{schema}, public")));
    drop(c);
    // Read-only behind the pooler: each transaction read-only and in the schema.
    let mut ro = Conn::open_in(&purl, SessionRole::Query, true, ctx).await;
    assert_eq!(one(ro.run(1, "SELECT count(*) FROM zz_pt").await).as_deref(), Some("3"));
    let DbEvent::Failed { error, .. } = ro.run(2, "INSERT INTO zz_pt VALUES (9)").await else { panic!("a write ran") };
    assert!(error.raw().contains("read-only"), "{error:?}");
    drop(ro);
    let sent = proxy.sent();
    // `DATARIG_TEST_WIRE_LOG=<file>`: what went over the wire, for a person to read.
    if let Ok(path) = std::env::var("DATARIG_TEST_WIRE_LOG") {
        let text: String = sent.iter().map(|s| format!("{s:?}\n")).collect();
        std::fs::write(path, text).expect("wire log");
    }
    let blocks = pg_proxy::check_wire(&sent, "zz_pt");
    assert!(blocks >= 10, "{blocks} blocks: {sent:#?}");
    // The profile's defaults behind the pooler: nothing is added.
    let before = proxy.sent().len();
    let mut d = Conn::open(&purl, SessionRole::Query).await;
    assert_eq!(one(d.run(1, "SELECT 1").await).as_deref(), Some("1"));
    let added = proxy.sent()[before..].to_vec();
    assert!(!added.iter().any(|s| format!("{s:?}").contains("search_path")), "{added:?}");
}

/// Behind a pooler that drops the startup `options`, `COMMIT AND CHAIN` and
/// `ROLLBACK AND CHAIN` open a new transaction, which gets `SET LOCAL search_path …` with its
/// next statement like any new block (the old one's ended with it): unqualified names resolve in
/// the tab's schema there too, and the wire shows the path in every block.
#[tokio::test(flavor = "multi_thread")]
async fn behind_a_pooler_a_chained_transaction_gets_the_path() {
    use datarig_core::driver::SessionContext;
    let Some(url) = pg_url("behind_a_pooler_a_chained_transaction_gets_the_path") else { return };
    let pid = std::process::id();
    let schema = format!("zz_chain_{pid}");
    let _guard = SchemaGuard::new(&url, &schema);
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.zz_ct (x int)"),
        format!("INSERT INTO {schema}.zz_ct SELECT generate_series(1, 4)"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let proxy = pg_proxy::Proxy::start(&pg_proxy::upstream(&url)).await;
    let purl = proxy.url(&url);
    for read_only in [false, true] {
        let ctx = SessionContext { database: None, schema: Some(schema.clone()) };
        let mut c = Conn::open_in(&purl, SessionRole::Query, read_only, ctx).await;
        c.wait(|e| matches!(e, DbEvent::ContextPerTransaction), 10).await;
        // As a `SET` statement stores it (the server quotes it only where it must).
        let path = format!("{schema}, public");
        assert!(matches!(c.run(1, "BEGIN").await, DbEvent::Done { .. }));
        assert_eq!(cell(&c.run(2, "SELECT count(*) FROM zz_ct").await).as_deref(), Some("4"));
        for (id, chain) in [(3, "COMMIT AND CHAIN"), (6, "ROLLBACK AND CHAIN")] {
            assert!(matches!(c.run(id, chain).await, DbEvent::Done { .. }), "{chain}");
            let shown = c.run(id + 1, "SELECT pg_catalog.current_setting('search_path')").await;
            assert_eq!(cell(&shown).as_deref(), Some(path.as_str()), "after {chain} (read-only: {read_only})");
            let counted = c.run(id + 2, "SELECT count(*) FROM zz_ct").await;
            assert_eq!(cell(&counted).as_deref(), Some("4"), "after {chain} (read-only: {read_only})");
        }
        if read_only {
            // The chained transaction keeps the block's read-only characteristic.
            let DbEvent::Failed { error, .. } = c.run(9, "INSERT INTO zz_ct VALUES (5)").await else {
                panic!("a write ran")
            };
            assert!(error.raw().contains("read-only"), "{error:?}");
        }
        assert!(matches!(c.run(10, "ROLLBACK").await, DbEvent::Done { .. }));
    }
    let sent = proxy.sent();
    let blocks = pg_proxy::check_wire(&sent, "zz_ct");
    assert!(blocks >= 6, "{blocks} blocks: {sent:#?}");
}

/// Whether the startup option applied is told by where the path comes from, not
/// by its value alone. A database whose default path is exactly the tab's (as a pooled
/// connection may carry another client's `set_config(…, false)`) fooled the check behind a pooler
/// that drops the option: the session took the value as its own and set nothing per
/// transaction. Now `pg_settings.source` must say `client`: behind the proxy the session sets the
/// path in each transaction; connected directly, the option applied and nothing more is sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_path_the_client_did_not_set_is_not_taken_as_applied() {
    use datarig_core::driver::SessionContext;
    let Some(url) = pg_url("a_path_the_client_did_not_set_is_not_taken_as_applied") else { return };
    let pid = std::process::id();
    let db = format!("zz_srcdb_{pid}");
    // A capital: the server quotes it in the value as the session's option does.
    let schema = format!("zz_Src_{pid}");
    let _db_guard = DatabaseGuard { url: url.clone(), name: db.clone() };
    pg_clean::run_fresh(&url, &format!("CREATE DATABASE {db}")).unwrap();
    // The database's default: the very value the session asks for.
    let default = format!("ALTER DATABASE {db} SET search_path TO \"{schema}\", public");
    pg_clean::run_fresh(&url, &default).unwrap();
    let ctx = SessionContext { database: Some(db.clone()), schema: Some(schema.clone()) };
    // Every event up to the session's `Context`: whether it said `ContextPerTransaction`.
    async fn per_transaction(c: &mut Conn) -> bool {
        let mut seen = false;
        loop {
            match c.wait(|_| true, 10).await {
                DbEvent::ContextPerTransaction => seen = true,
                DbEvent::Context { .. } => return seen,
                _ => {}
            }
        }
    }
    let proxy = pg_proxy::Proxy::start(&pg_proxy::upstream(&url)).await;
    let mut pooled = Conn::open_in(&proxy.url(&url), SessionRole::Query, false, ctx.clone()).await;
    assert!(per_transaction(&mut pooled).await, "behind the pooler the path is set per transaction");
    assert!(matches!(pooled.run(1, "SELECT 1").await, DbEvent::Page { .. }));
    drop(pooled);
    let sent = proxy.sent();
    assert!(sent.iter().any(|s| format!("{s:?}").contains("SET LOCAL search_path")), "{sent:#?}");
    let mut direct = Conn::open_in(&url, SessionRole::Query, false, ctx).await;
    assert!(!per_transaction(&mut direct).await, "connected directly the option applied");
}

/// Behind a pooler, in a session with a schema of its own, a
/// statement that cannot run in a transaction block (`VACUUM`, `CREATE INDEX CONCURRENTLY`) and a
/// procedure that commits are still sent as they are, and the server refuses them in the
/// transaction that sets the path: that failure is `NeedsNoTransaction` (the UI says why and how to
/// run it), and nothing is left open. The same statement in the user's own block is a plain server
/// error; connected directly (the option applied) it runs.
#[tokio::test(flavor = "multi_thread")]
async fn behind_a_pooler_a_statement_that_needs_no_transaction_says_so() {
    use datarig_core::driver::SessionContext;
    let Some(url) = pg_url("behind_a_pooler_a_statement_that_needs_no_transaction_says_so") else { return };
    let pid = std::process::id();
    let schema = format!("zz_notx_{pid}");
    let _guard = SchemaGuard::new(&url, &schema);
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.zz_nt (x int)"),
        format!("CREATE PROCEDURE {schema}.zz_commits() LANGUAGE plpgsql AS $$ BEGIN COMMIT; END $$"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let ctx = SessionContext { database: None, schema: Some(schema.clone()) };
    let proxy = pg_proxy::Proxy::start(&pg_proxy::upstream(&url)).await;
    let mut c = Conn::open_in(&proxy.url(&url), SessionRole::Query, false, ctx.clone()).await;
    c.wait(|e| matches!(e, DbEvent::ContextPerTransaction), 10).await;
    let needs = |ev: &DbEvent| matches!(ev, DbEvent::Failed { error: DbError::NeedsNoTransaction(m), cancelled: false, .. } if m.starts_with("ERROR"));
    for (id, sql) in [
        (1, "VACUUM zz_nt".to_string()),
        (2, format!("CREATE INDEX CONCURRENTLY zz_nt_i_{pid} ON zz_nt (x)")),
        (3, "CALL zz_commits()".to_string()),
    ] {
        let ev = c.run(id, &sql).await;
        assert!(needs(&ev), "{sql}: {ev:?}");
    }
    // Nothing stays open: the next statement runs.
    assert_eq!(cell(&c.run(4, "SELECT count(*) FROM zz_nt").await).as_deref(), Some("0"));
    // In the user's own block the server says the same, as a plain error.
    assert!(matches!(c.run(5, "BEGIN").await, DbEvent::Done { .. }));
    let ev = c.run(6, "VACUUM zz_nt").await;
    assert!(matches!(&ev, DbEvent::Failed { error: DbError::Server(_), .. }), "{ev:?}");
    assert!(matches!(c.run(7, "ROLLBACK").await, DbEvent::Done { .. }));
    drop(c);
    // Connected directly the option applied: it runs.
    let mut d = Conn::open_in(&url, SessionRole::Query, false, ctx).await;
    assert!(matches!(d.run(1, "VACUUM zz_nt").await, DbEvent::Done { .. }));
    assert!(matches!(d.run(2, "CALL zz_commits()").await, DbEvent::Done { .. }));
}

/// The statement cache off (a profile behind a pooler in transaction mode):
/// runs outside and inside the user's block, of new and repeated statements, a table's row
/// type (a type lookup) and a statement without rows the lexer does not know leave no prepared
/// statement on the connection; with the cache on the same runs keep theirs.
#[tokio::test(flavor = "multi_thread")]
async fn statement_cache_off_leaves_no_prepared_statement() {
    let Some(url) = pg_url("statement_cache_off_leaves_no_prepared_statement") else { return };
    for cache in [false, true] {
        let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.clone()), ..ConnectionConfig::test_db() };
        let (tx, rx) = unbounded_channel();
        let o = opts(SessionRole::Query).statement_cache(cache);
        let mut c = Conn { session: PgDriver.connect(&cfg, SessionRole::Query, o, tx), rx };
        c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
        // `DEALLOCATE` is not one the lexer knows returns no rows: it takes the prepared path.
        let runs = [
            "DEALLOCATE ALL",
            "SELECT id FROM shop.users ORDER BY id LIMIT 3",
            "SELECT id FROM shop.users ORDER BY id LIMIT 3",
            "SELECT u FROM shop.users u ORDER BY u.id LIMIT 2",
            "SELECT pg_catalog.pg_sleep(0)",
            "BEGIN",
            "DEALLOCATE ALL",
            "SELECT id FROM shop.products ORDER BY id LIMIT 3",
            "SELECT u FROM shop.orders u ORDER BY u.id LIMIT 2",
            "COMMIT",
            "VALUES (1), (2)",
        ];
        for (i, sql) in runs.into_iter().enumerate() {
            let ev = c.run(i as u64 + 1, sql).await;
            assert!(!matches!(ev, DbEvent::Failed { .. }), "cache {cache}: {sql}: {ev:?}");
        }
        let DbEvent::Page { rows, .. } = c.run(99, "SELECT count(*) FROM pg_catalog.pg_prepared_statements").await
        else {
            panic!()
        };
        let left = rows[0][0].as_deref().unwrap_or_default().to_string();
        if cache {
            assert_ne!(left, "0", "the cache keeps what it prepared");
        } else {
            assert_eq!(left, "0", "nothing may outlive its transaction with the cache off");
        }
    }
}

/// Through a dialer (an SSH tunnel), every connection goes through it: the
/// query session (paging as usual), its cancel request, the cancel that closing it sends, a
/// metadata session and a test connection. Counted by the dialer itself.
#[tokio::test(flavor = "multi_thread")]
async fn sessions_cancels_and_tests_go_through_the_dialer() {
    let Some(url) = pg_url("sessions_cancels_and_tests_go_through_the_dialer") else { return };
    let counting = Arc::new(TcpDialer::default());
    let through = || Some(DialerRef(counting.clone() as Arc<dyn Dialer>));
    let dials = || counting.dials.load(std::sync::atomic::Ordering::SeqCst);
    let tag = format!("dial{}", std::process::id());
    let cfg = url_profile(&url);
    let open = |tag: &str| {
        let (tx, rx) = unbounded_channel();
        let o = ConnectOptions::new(PAGE, SessionRole::Query, tag).dialer(through());
        Conn { session: PgDriver.connect(&cfg, SessionRole::Query, o, tx), rx }
    };
    let mut q = open(&tag);
    q.wait(|e| matches!(e, DbEvent::Connected), 10).await;
    assert_eq!(dials(), 1, "the session's connection");
    let mut obs = Conn::open(&url, SessionRole::Query).await;

    let DbEvent::Page { rows, more: true, .. } = q.run(1, "SELECT g FROM generate_series(1, 1200) g").await else {
        panic!("first page")
    };
    assert_eq!(rows.len(), PAGE);
    q.session.send(DbCommand::FetchMore { id: 1 });
    let DbEvent::Page { rows, .. } = q.wait(|e| matches!(e, DbEvent::Page { id: 1, .. }), 10).await else { panic!() };
    assert_eq!(rows[0][0].as_deref(), Some(&*(PAGE + 1).to_string()));

    q.session.send(DbCommand::Execute {
        id: 2,
        statements: vec!["SELECT pg_sleep(60)".into()],
        paging: PagingMode::Hold,
    });
    started(&mut obs, &tag).await;
    let t0 = Instant::now();
    q.session.cancel();
    let ev = q.wait(|e| matches!(e, DbEvent::Failed { id: 2, .. }), 10).await;
    assert!(matches!(ev, DbEvent::Failed { cancelled: true, .. }), "{ev:?}");
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert_eq!(dials(), 2, "the cancel request dialed");

    // Closing the session while it runs cancels the statement through the dialer too. (Waited
    // for by its own text: the cancelled one may still show as active for a moment.)
    q.session.send(DbCommand::Execute {
        id: 3,
        statements: vec!["SELECT pg_sleep(61)".into()],
        paging: PagingMode::Hold,
    });
    let mut n = 20_000;
    while !active_queries(&mut obs, n, &tag).await.iter().any(|t| t.contains("pg_sleep(61)")) {
        n += 1;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let Conn { session, .. } = q;
    let t0 = Instant::now();
    session.close();
    let mut n = 10_000;
    while !active_queries(&mut obs, n, &tag).await.is_empty() {
        n += 1;
        assert!(t0.elapsed() < Duration::from_secs(10), "still running {:?} after close", t0.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(dials(), 3, "closing sent its cancel through the dialer");

    let (tx, rx) = unbounded_channel();
    let o = ConnectOptions::new(PAGE, SessionRole::Meta, &tag).dialer(through());
    let mut meta = Conn { session: PgDriver.connect(&cfg, SessionRole::Meta, o, tx), rx };
    let DbEvent::Schemas(Ok(schemas)) = meta.wait(|e| matches!(e, DbEvent::Schemas(_)), 10).await else {
        panic!("schemas")
    };
    assert!(schemas.contains(&"shop".to_string()));
    assert_eq!(dials(), 4);
    PgDriver.ping(&cfg, Duration::from_secs(5), through()).await.expect("test connection");
    assert_eq!(dials(), 5);
}

/// A dialer that cannot give a stream fails the connect with its reason, and a session whose
/// server is not one TCP host is refused before anything is dialed.
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_that_fails_fails_the_connect() {
    struct Closed;
    impl Dialer for Closed {
        fn dial(
            &self,
            _: &str,
            _: u16,
        ) -> futures::future::BoxFuture<'static, Result<datarig_core::transport::BoxedStream, DialError>> {
            Box::pin(async { Err(DialError::NotOpen) })
        }
    }
    let closed = Some(DialerRef(Arc::new(Closed) as Arc<dyn Dialer>));
    let cfg = ConnectionConfig::test_db();
    let (tx, mut rx) = unbounded_channel();
    let _s = PgDriver.connect(&cfg, SessionRole::Query, opts(SessionRole::Query).dialer(closed.clone()), tx);
    match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
        Ok(Some(DbEvent::ConnectFailed { error: DbError::Transport(DialError::NotOpen), auth: false })) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(
        PgDriver.ping(&cfg, Duration::from_secs(5), closed.clone()).await,
        Err(PingError::Failed(DbError::Transport(DialError::NotOpen)))
    );
    let two = ConnectionConfig { dsn: Some("postgres://u@a,b/db".into()), ..ConnectionConfig::test_db() };
    match PgDriver.ping(&two, Duration::from_secs(5), closed).await {
        Err(PingError::Failed(DbError::Settings(f))) => assert!(f.detail.contains("several hosts"), "{f:?}"),
        other => panic!("{other:?}"),
    }
}

// ── results not held (`PagingMode::NoHold`) ───────────────────────────────

/// What the server shows of the session tagged `tag`: its state in `pg_stat_activity` (`idle`,
/// `idle in transaction`, …) and how many locks it holds or waits for in `pg_locks`.
async fn server_view(observer: &mut Conn, id: u64, tag: &str) -> (String, String) {
    let sql = format!(
        "SELECT a.state, (SELECT count(*) FROM pg_catalog.pg_locks l WHERE l.pid = a.pid) \
         FROM pg_catalog.pg_stat_activity a WHERE a.application_name = 'datarig-q-{tag}'"
    );
    let DbEvent::Page { rows, .. } = observer.run(id, &sql).await else { panic!("pg_stat_activity") };
    assert_eq!(rows.len(), 1, "one session tagged {tag}: {rows:?}");
    (rows[0][0].clone().unwrap_or_default(), rows[0][1].clone().unwrap_or_default())
}

/// Run `ALTER TABLE` on `table` from `migration` as a deploy would, giving up after a short
/// `lock_timeout`; whether it went through (`Err` with the server's message when it did not).
async fn alter_quickly(migration: &mut Conn, id: u64, table: &str, column: &str) -> Result<(), String> {
    let statements = vec![
        "BEGIN".to_string(),
        "SET LOCAL lock_timeout = '500ms'".to_string(),
        format!("ALTER TABLE {table} ADD COLUMN {column} int"),
        "COMMIT".to_string(),
    ];
    migration.session.send(DbCommand::Execute { id, statements, paging: PagingMode::Hold });
    match migration.result(id).await {
        DbEvent::Done { .. } => Ok(()),
        DbEvent::Failed { error, .. } => {
            assert!(matches!(migration.run(id + 1, "ROLLBACK").await, DbEvent::Done { .. }));
            Err(error.raw().to_string())
        }
        ev => panic!("{ev:?}"),
    }
}

/// By default nothing of a result stays on the server: its first page comes with the portal
/// and its transaction already ended (`Released` right before the page, no `TxOpen`), so the
/// session is idle (not idle in transaction) and holds no lock while the page is on screen, and
/// a deploy's `ALTER TABLE` with a short `lock_timeout` goes through. The next page runs the
/// statement again (`Resume`), continues after the rows shown, and leaves nothing either; the
/// last page is not released (nothing was left to release).
#[tokio::test(flavor = "multi_thread")]
async fn a_result_not_held_leaves_no_lock_and_no_transaction() {
    let Some(url) = pg_url("a_result_not_held_leaves_no_lock_and_no_transaction") else { return };
    let tag = format!("nohold{}", std::process::id());
    let table = format!("public.it_nohold_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let (mut q, mut obs) = tagged(&url, &tag).await;
    let create = format!("CREATE TABLE {table} AS SELECT g AS x FROM generate_series(1, 1200) g");
    assert!(matches!(obs.run(1, &create).await, DbEvent::Done { .. }));
    let sql = format!("SELECT x FROM {table} ORDER BY x");
    q.session.send(DbCommand::Execute { id: 1, statements: vec![sql.clone()], paging: PagingMode::NoHold });
    let mut events = Vec::new();
    loop {
        let ev = q.wait(|_| true, 30).await;
        let page = matches!(ev, DbEvent::Page { id: 1, .. } | DbEvent::Failed { id: 1, .. });
        events.push(ev);
        if page {
            break;
        }
    }
    assert!(matches!(events[..], [.., DbEvent::Released { id: 1 }, DbEvent::Page { more: true, .. }]), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, DbEvent::TxOpen(true))), "never reported open: {events:?}");
    let DbEvent::Page { rows, .. } = events.last().unwrap() else { unreachable!() };
    assert_eq!((rows.len(), rows[0][0].as_deref(), rows[PAGE - 1][0].as_deref()), (PAGE, Some("1"), Some("500")));
    assert_eq!(server_view(&mut obs, 2, &tag).await, ("idle".to_string(), "0".to_string()), "nothing held");
    let mut migration = Conn::open(&url, SessionRole::Query).await;
    assert_eq!(alter_quickly(&mut migration, 1, &table, "y").await, Ok(()), "the deploy is not blocked");
    // The next page: the statement again, after the 500 rows shown.
    q.session.send(DbCommand::Resume { id: 2, sql: sql.clone(), skip: 500, paging: PagingMode::NoHold });
    let released = q.wait(|e| matches!(e, DbEvent::Released { .. } | DbEvent::Page { .. }), 30).await;
    assert!(matches!(released, DbEvent::Released { id: 2 }), "{released:?}");
    let DbEvent::Page { columns: Some(_), rows, more: true, .. } = q.result(2).await else { panic!("page 2") };
    assert_eq!((rows[0][0].as_deref(), rows[PAGE - 1][0].as_deref()), (Some("501"), Some("1000")));
    assert_eq!(server_view(&mut obs, 3, &tag).await, ("idle".to_string(), "0".to_string()));
    assert_eq!(alter_quickly(&mut migration, 3, &table, "z").await, Ok(()));
    // The last page ends the result: nothing to release.
    q.session.send(DbCommand::Resume { id: 3, sql, skip: 1000, paging: PagingMode::NoHold });
    let DbEvent::Page { rows, more: false, .. } = q.result(3).await else { panic!("the last page") };
    assert_eq!((rows.len(), rows[199][0].as_deref()), (200, Some("1200")));
    let events = q.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::Released { .. } | DbEvent::TxOpen(true))), "{events:?}");
    assert_eq!(server_view(&mut obs, 4, &tag).await, ("idle".to_string(), "0".to_string()));
}

/// `PagingMode::Hold` is the behaviour before no-hold became the default: the portal stays
/// open in a transaction of its own, which holds `ACCESS SHARE` on the table, so a deploy's
/// `ALTER TABLE` waits and gives up at its `lock_timeout`; once the portal closes it goes
/// through.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_result_keeps_its_transaction_and_lock_until_closed() {
    let Some(url) = pg_url("a_held_result_keeps_its_transaction_and_lock_until_closed") else { return };
    let tag = format!("hold{}", std::process::id());
    let table = format!("public.it_hold_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let (mut q, mut obs) = tagged(&url, &tag).await;
    let create = format!("CREATE TABLE {table} AS SELECT g AS x FROM generate_series(1, 1200) g");
    assert!(matches!(obs.run(1, &create).await, DbEvent::Done { .. }));
    let sql = format!("SELECT x FROM {table} ORDER BY x");
    q.session.send(DbCommand::Execute { id: 1, statements: vec![sql], paging: PagingMode::Hold });
    q.wait(|e| matches!(e, DbEvent::TxOpen(true)), 10).await;
    let DbEvent::Page { more: true, .. } = q.result(1).await else { panic!("a page with more") };
    let (state, locks) = server_view(&mut obs, 2, &tag).await;
    assert_eq!(state, "idle in transaction");
    assert_ne!(locks, "0", "the portal's transaction holds its locks");
    let mut migration = Conn::open(&url, SessionRole::Query).await;
    let blocked = alter_quickly(&mut migration, 1, &table, "y").await;
    assert!(blocked.as_ref().is_err_and(|e| e.contains("lock timeout")), "{blocked:?}");
    q.session.send(DbCommand::FetchMore { id: 1 });
    let DbEvent::Page { rows, .. } = q.result(1).await else { panic!("the next page") };
    assert_eq!(rows[0][0].as_deref(), Some("501"), "fetched from the open portal");
    q.session.send(DbCommand::ClosePortal { id: 1 });
    q.wait(|e| matches!(e, DbEvent::TxOpen(false)), 10).await;
    assert_eq!(server_view(&mut obs, 3, &tag).await, ("idle".to_string(), "0".to_string()));
    assert_eq!(alter_quickly(&mut migration, 3, &table, "y").await, Ok(()));
}

/// Inside the user's block a result is never released, whatever the paging: its portal lives
/// in the block and pages from it.
#[tokio::test(flavor = "multi_thread")]
async fn a_result_in_the_users_block_is_held_without_hold_too() {
    let Some(url) = pg_url("a_result_in_the_users_block_is_held_without_hold_too") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    assert!(matches!(c.run(1, "BEGIN").await, DbEvent::Done { .. }));
    let sql = "SELECT g FROM generate_series(1, 1200) g";
    c.session.send(DbCommand::Execute { id: 2, statements: vec![sql.into()], paging: PagingMode::NoHold });
    let DbEvent::Page { more: true, .. } = c.result(2).await else { panic!("a page with more") };
    c.session.send(DbCommand::FetchMore { id: 2 });
    let DbEvent::Page { columns: None, rows, more: true, .. } = c.result(2).await else { panic!("the next page") };
    assert_eq!(rows[0][0].as_deref(), Some("501"));
    assert!(matches!(c.run(3, "ROLLBACK").await, DbEvent::Done { .. }));
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::Released { .. })), "{events:?}");
}

/// On a read-only session a result not held reads in a read-only transaction, its first page
/// and the statement run again alike.
#[tokio::test(flavor = "multi_thread")]
async fn a_result_not_held_is_read_only_on_a_read_only_session() {
    let Some(url) = pg_url("a_result_not_held_is_read_only_on_a_read_only_session") else { return };
    let mut c = Conn::open_with(&url, SessionRole::Query, true).await;
    let sql = "SELECT current_setting('transaction_read_only') FROM generate_series(1, 700)";
    c.session.send(DbCommand::Execute { id: 1, statements: vec![sql.into()], paging: PagingMode::NoHold });
    let DbEvent::Page { rows, more: true, .. } = c.result(1).await else { panic!("a page") };
    assert_eq!(rows[0][0].as_deref(), Some("on"));
    c.session.send(DbCommand::Resume { id: 2, sql: sql.into(), skip: 500, paging: PagingMode::NoHold });
    let DbEvent::Page { rows, more: false, .. } = c.result(2).await else { panic!("the rest") };
    assert_eq!((rows.len(), rows[0][0].as_deref()), (200, Some("on")));
}

/// A statement that writes and returns more rows than a page, not held: its first page is
/// shown once its `COMMIT` (sent with it) succeeded, and the rest of its rows are gone with the
/// transaction (what it wrote stays). When that `COMMIT` fails (a deferred constraint) the run
/// fails, no page is shown, and the session goes on outside any transaction. A big skip (more
/// than one request of skipped rows) is ended right after its page too, before the page.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_not_held_is_shown_only_once_committed() {
    let Some(url) = pg_url("a_write_not_held_is_shown_only_once_committed") else { return };
    let mut c = Conn::open(&url, SessionRole::Query).await;
    let create = "CREATE TEMP TABLE zz_nohold_w (x int UNIQUE DEFERRABLE INITIALLY DEFERRED)";
    assert!(matches!(c.run(1, create).await, DbEvent::Done { .. }));
    let insert = "INSERT INTO zz_nohold_w SELECT g FROM generate_series(1, 700) g RETURNING x";
    c.session.send(DbCommand::Execute { id: 2, statements: vec![insert.into()], paging: PagingMode::NoHold });
    let DbEvent::Page { rows, more: true, .. } = c.result(2).await else { panic!("a page with more") };
    assert_eq!(rows.len(), PAGE);
    let DbEvent::Page { rows, .. } = c.run(3, "SELECT count(*) FROM zz_nohold_w").await else { panic!() };
    assert_eq!(rows[0][0].as_deref(), Some("700"), "committed");
    let twice = "INSERT INTO zz_nohold_w SELECT 1 FROM generate_series(1, 600) RETURNING x";
    c.session.send(DbCommand::Execute { id: 4, statements: vec![twice.into()], paging: PagingMode::NoHold });
    let DbEvent::Failed { error: DbError::Server(e), .. } = c.result(4).await else { panic!("the COMMIT fails") };
    assert!(e.contains("duplicate key"), "{e}");
    let DbEvent::Page { rows, .. } = c.run(5, "SELECT count(*) FROM zz_nohold_w").await else {
        panic!("the session goes on, outside any transaction")
    };
    assert_eq!(rows[0][0].as_deref(), Some("700"));
    let sql = "SELECT g FROM generate_series(1, 25000) g ORDER BY g";
    c.session.send(DbCommand::Resume { id: 6, sql: sql.into(), skip: 12_000, paging: PagingMode::NoHold });
    let DbEvent::Page { rows, more: true, .. } = c.result(6).await else { panic!("a page") };
    assert_eq!(rows[0][0].as_deref(), Some("12001"));
    let events = c.drain(150).await;
    assert!(!events.iter().any(|e| matches!(e, DbEvent::TxOpen(true))), "nothing stays open: {events:?}");
    // Ended: a statement that cannot run in a transaction block runs.
    assert!(matches!(c.run(7, "VACUUM zz_nohold_w").await, DbEvent::Done { .. }), "no transaction open");
}
