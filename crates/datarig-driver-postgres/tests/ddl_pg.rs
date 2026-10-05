//! The DDL of objects (`DbCommand::LoadDdl`) against a real PostgreSQL loaded with
//! `dev/init/*.sql` (`DATARIG_TEST_PG_URL`, see `integration_pg.rs`): written out, it creates
//! the same objects again (the seed's tables in a database of their own, the test's own objects
//! in their schema), and reading it never waits for a lock.
//!
//! Everything a test creates is named `zz_ddl_…` with the process id and dropped by a guard.

// Its table guard is not used here: these tests drop schemas and databases of their own.
#[allow(dead_code)]
mod pg_clean;

use datarig_core::driver::ddl::{DdlObject, DdlSource};
use datarig_core::driver::structure::RelationKind;
use datarig_core::driver::{ConnectOptions, DbCommand, DbError, DbEvent, Driver, Session, SessionContext, SessionRole};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::ddl::ddl_text;
use datarig_driver_postgres::PgDriver;
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
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: set DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig"
            );
            None
        }
    }
}

/// `url` with another database.
fn in_database(url: &str, database: &str) -> String {
    let (base, _) = url.rsplit_once('/').expect("a URL with a database");
    format!("{base}/{database}")
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    next: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, tag: &str) -> Conn {
        let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
        let (tx, rx) = unbounded_channel();
        let opts = ConnectOptions::new(500, role, tag).context(SessionContext::default());
        let session = PgDriver.connect(&cfg, role, opts, tx);
        let mut c = Conn { session, rx, next: 0 };
        let ready = match role {
            SessionRole::Meta => |e: &DbEvent| matches!(e, DbEvent::Keys(_)),
            SessionRole::Query => |e: &DbEvent| matches!(e, DbEvent::Connected),
        };
        c.wait(ready, 30).await;
        c
    }

    async fn wait(&mut self, pred: impl Fn(&DbEvent) -> bool, secs: u64) -> DbEvent {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("connect failed: {error:?}"),
                Ok(Some(ev)) if pred(&ev) => return ev,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out after {secs}s waiting for an event"),
            }
        }
    }

    /// Run `statements` in order; the last one's rows (as text), or the first failure.
    async fn run(&mut self, statements: Vec<String>) -> Result<Vec<Vec<Option<String>>>, String> {
        self.next += 1;
        let id = self.next;
        self.session.send(DbCommand::Execute { id, statements });
        let ev = self
            .wait(
                |e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
                60,
            )
            .await;
        match ev {
            DbEvent::Page { rows, .. } => Ok(rows),
            DbEvent::Done { .. } => Ok(Vec::new()),
            DbEvent::Failed { error, .. } => Err(format!("{error:?}")),
            _ => unreachable!(),
        }
    }

    async fn one(&mut self, sql: &str) -> String {
        let rows = self.run(vec![sql.to_string()]).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
        rows.first().and_then(|r| r[0].clone()).unwrap_or_default()
    }

    /// The DDL of `object`, and how long it took.
    async fn ddl(&mut self, object: DdlObject) -> (Result<DdlSource, DbError>, Duration) {
        self.next += 1;
        let id = self.next;
        let t0 = Instant::now();
        self.session.send(DbCommand::LoadDdl { id, object });
        match self.wait(|e| matches!(e, DbEvent::Ddl { id: i, .. } if *i == id), 30).await {
            DbEvent::Ddl { result, .. } => (result, t0.elapsed()),
            _ => unreachable!(),
        }
    }

    async fn text(&mut self, object: DdlObject) -> String {
        let what = format!("{object:?}");
        ddl_text(&self.ddl(object).await.0.unwrap_or_else(|e| panic!("{what}: {e:?}")))
    }
}

/// The statements of a DDL text, to run one by one.
fn statements(text: &str) -> Vec<String> {
    datarig_core::sql::split::split(text).iter().map(|s| s.body(text).to_string()).collect()
}

fn relation(schema: &str, name: &str) -> DdlObject {
    DdlObject::Relation { schema: schema.into(), name: name.into() }
}

/// Runs `sql` on a fresh connection when it goes out of scope (a `DROP …`); declared before
/// what it drops, so it runs after the sessions that use it are gone.
struct Cleanup {
    url: String,
    sql: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Err(e) = pg_clean::run_fresh(&self.url, &self.sql) {
            let _ = writeln!(std::io::stderr(), "warning: {}: {e}", self.sql);
        }
    }
}

fn cleanup(url: &str, sql: String) -> Cleanup {
    Cleanup { url: url.to_string(), sql }
}

/// The server's major version.
async fn major(c: &mut Conn) -> u32 {
    c.one("SELECT current_setting('server_version_num')").await.parse::<u32>().unwrap() / 10_000
}

/// The seed's tables and view, written out from their DDL, are the same objects in a database
/// of their own: each one's DDL read there is the same text. The tables go in the order of
/// their foreign keys, the view after them.
#[tokio::test(flavor = "multi_thread")]
async fn the_seed_round_trips_through_its_ddl() {
    let Some(url) = pg_url("the_seed_round_trips_through_its_ddl") else { return };
    let tag = format!("zz_ddl_seed_{}", std::process::id());
    let mut meta = Conn::open(&url, SessionRole::Meta, &tag).await;
    let mut seed = Vec::new();
    for (schema, name) in [
        ("shop", "users"),
        ("shop", "products"),
        ("shop", "orders"),
        ("shop", "order_items"),
        ("shop", "reviews"),
        ("shop", "audit_log"),
        ("analytics", "events"),
        ("analytics", "daily_stats"),
        ("shop", "order_summary"),
    ] {
        let text = meta.text(relation(schema, name)).await;
        assert!(text.starts_with(datarig_core::sql::ddl::HEADER), "{text}");
        seed.push((schema, name, text));
    }
    let users = &seed[0].2;
    assert!(users.contains("CREATE SEQUENCE shop.users_id_seq;"), "{users}");
    assert!(
        users.contains(
            "CREATE TABLE shop.users (\n    id bigint DEFAULT nextval('shop.users_id_seq'::regclass) NOT NULL,"
        ),
        "{users}"
    );
    assert!(users.contains("CONSTRAINT users_pkey PRIMARY KEY (id)"), "{users}");
    assert!(users.contains("CREATE INDEX users_created_at_idx ON shop.users USING btree (created_at);"), "{users}");
    assert!(seed[2].2.contains("FOREIGN KEY (user_id) REFERENCES shop.users(id)"), "{}", seed[2].2);
    let view = &seed[8].2;
    assert!(view.contains("CREATE OR REPLACE VIEW shop.order_summary AS\n SELECT o.id AS order_id,"), "{view}");
    assert!(view.contains("FROM shop.orders o\n     JOIN shop.users u"), "every name qualified: {view}");

    let db = tag.clone();
    let _drop = cleanup(&url, format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
    pg_clean::run_fresh(&url, &format!("CREATE DATABASE {db}")).unwrap();
    let copy = in_database(&url, &db);
    let mut q = Conn::open(&copy, SessionRole::Query, &tag).await;
    q.run(vec!["CREATE SCHEMA shop".into(), "CREATE SCHEMA analytics".into()]).await.unwrap();
    for (schema, name, text) in &seed {
        q.run(statements(text)).await.unwrap_or_else(|e| panic!("{schema}.{name}: {e}\n{text}"));
    }
    let mut meta2 = Conn::open(&copy, SessionRole::Meta, &tag).await;
    for (schema, name, text) in &seed {
        assert_eq!(&meta2.text(relation(schema, name)).await, text, "{schema}.{name}");
    }
}

/// The test's own objects, which use every part of a DDL the catalog has (identity with
/// options, a stored generated column, a collation, storage and statistics settings, an
/// exclusion constraint, a partial expression index, a disabled trigger with `UPDATE OF` and
/// `WHEN`, row-level security with a policy that reads another table, comments, grants to a
/// role, `UNLOGGED`, storage parameters of the table and its TOAST table, a partitioned table
/// with a partition, inheritance, a view with options and a column default, a materialized
/// view with an index, the trigger's function): read, the schema dropped, their DDL run again
/// and read again, it is the same text.
#[tokio::test(flavor = "multi_thread")]
async fn every_part_of_a_ddl_round_trips() {
    let Some(url) = pg_url("every_part_of_a_ddl_round_trips") else { return };
    let pid = std::process::id();
    let (s, role) = (format!("zz_ddl_rt_{pid}"), format!("zz_ddl_role_{pid}"));
    let fdw = format!("zz_ddl_fdw_{pid}");
    let _role = cleanup(&url, format!("DROP ROLE IF EXISTS {role}"));
    let _fdw = cleanup(&url, format!("DROP FOREIGN DATA WRAPPER IF EXISTS {fdw} CASCADE"));
    let _schema = cleanup(&url, format!("DROP SCHEMA IF EXISTS {s} CASCADE"));
    let tag = format!("zzddlrt{pid}");
    let mut q = Conn::open(&url, SessionRole::Query, &tag).await;
    let fixture = [
        format!("CREATE ROLE {role} NOLOGIN"),
        format!("CREATE SCHEMA {s}"),
        format!(
            "CREATE TABLE {s}.teams (id int GENERATED ALWAYS AS IDENTITY (START WITH 10 INCREMENT BY 5) PRIMARY KEY, \
             name text COLLATE \"C\" NOT NULL UNIQUE, during int4range, EXCLUDE USING gist (during WITH &&)) \
             WITH (fillfactor = 70)"
        ),
        format!(
            "CREATE TABLE {s}.\"Users\" (id bigserial PRIMARY KEY, team_id int REFERENCES {s}.teams (id) ON DELETE SET NULL, \
             email text NOT NULL CHECK (email <> ''), \"Mixed Col\" varchar(20) DEFAULT 'x', \
             twice bigint GENERATED ALWAYS AS (id * 2) STORED, note text)"
        ),
        format!("ALTER TABLE {s}.\"Users\" ALTER COLUMN note SET STORAGE EXTERNAL"),
        format!("ALTER TABLE {s}.\"Users\" ALTER COLUMN note SET STATISTICS 500"),
        format!("ALTER TABLE {s}.\"Users\" ALTER COLUMN email SET (n_distinct = -1)"),
        format!("CREATE INDEX users_lower ON {s}.\"Users\" (lower(email)) WHERE note IS NOT NULL"),
        format!("CREATE UNIQUE INDEX users_email ON {s}.\"Users\" (email)"),
        format!("ALTER TABLE {s}.\"Users\" REPLICA IDENTITY USING INDEX users_email"),
        format!("CREATE FUNCTION {s}.touch() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$"),
        format!(
            "CREATE TRIGGER users_touch BEFORE UPDATE OF email ON {s}.\"Users\" FOR EACH ROW \
             WHEN (OLD.email IS DISTINCT FROM NEW.email) EXECUTE FUNCTION {s}.touch()"
        ),
        format!("ALTER TABLE {s}.\"Users\" DISABLE TRIGGER users_touch"),
        format!("ALTER TABLE {s}.\"Users\" ENABLE ROW LEVEL SECURITY"),
        format!(
            "CREATE POLICY own ON {s}.\"Users\" FOR SELECT TO {role} USING (team_id IN (SELECT t.id FROM {s}.teams t))"
        ),
        format!("COMMENT ON TABLE {s}.\"Users\" IS 'People''s accounts'"),
        format!("COMMENT ON COLUMN {s}.\"Users\".email IS 'unique'"),
        format!("COMMENT ON CONSTRAINT \"Users_email_check\" ON {s}.\"Users\" IS 'not empty'"),
        format!("COMMENT ON INDEX {s}.users_lower IS 'case-insensitive'"),
        format!("COMMENT ON TRIGGER users_touch ON {s}.\"Users\" IS 'touch'"),
        format!("COMMENT ON POLICY own ON {s}.\"Users\" IS 'own'"),
        format!("GRANT SELECT, UPDATE ON {s}.\"Users\" TO {role}"),
        format!("GRANT SELECT (name) ON {s}.teams TO {role} WITH GRANT OPTION"),
        format!(
            "CREATE UNLOGGED TABLE {s}.scratch (k serial) WITH (autovacuum_enabled = false, toast.autovacuum_enabled = false)"
        ),
        format!(
            "CREATE TABLE {s}.events (id int NOT NULL, at date NOT NULL, PRIMARY KEY (id, at)) PARTITION BY RANGE (at)"
        ),
        format!(
            "CREATE TABLE {s}.events_2024 PARTITION OF {s}.events (CONSTRAINT events_2024_id CHECK (id > 0)) \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')"
        ),
        format!("CREATE INDEX events_at ON {s}.events (at)"),
        format!("CREATE TABLE {s}.base (id int, b text)"),
        format!("CREATE TABLE {s}.child (extra text) INHERITS ({s}.base)"),
        format!(
            "CREATE VIEW {s}.team_names WITH (security_barrier = true) AS SELECT t.id, t.name FROM {s}.teams t \
             WHERE t.id > 0 WITH LOCAL CHECK OPTION"
        ),
        format!("ALTER VIEW {s}.team_names ALTER COLUMN name SET DEFAULT 'none'"),
        format!("CREATE MATERIALIZED VIEW {s}.team_count AS SELECT count(*) AS n FROM {s}.teams WITH NO DATA"),
        format!("CREATE INDEX team_count_n ON {s}.team_count (n)"),
    ];
    let v = major(&mut q).await;
    let mut fixture = fixture.to_vec();
    if v >= 14 {
        fixture.push(format!("ALTER TABLE {s}.\"Users\" ALTER COLUMN \"Mixed Col\" SET COMPRESSION pglz"));
    }
    if v >= 18 {
        fixture.push(format!("CREATE TABLE {s}.virt (a int, b int GENERATED ALWAYS AS (a * 3) VIRTUAL)"));
    }
    // A function whose privileges are not the defaults; a partition and an inheriting table
    // with a default and a NOT NULL of their own; a typed table; a foreign table with column
    // options; NOT NULL constraints with a name of their own and NO INHERIT (18).
    let person = format!("CREATE TYPE {s}.person AS (id int, name text)");
    fixture.extend([
        format!("CREATE FUNCTION {s}.pay(amount int) RETURNS int LANGUAGE sql SECURITY DEFINER AS 'SELECT amount'"),
        format!("REVOKE EXECUTE ON FUNCTION {s}.pay(int) FROM PUBLIC"),
        format!("GRANT EXECUTE ON FUNCTION {s}.pay(int) TO {role}"),
        format!("ALTER TABLE {s}.events ADD COLUMN note text"),
        format!(
            "CREATE TABLE {s}.events_2025 PARTITION OF {s}.events (at WITH OPTIONS DEFAULT '2025-06-01') \
             FOR VALUES FROM ('2025-01-01') TO ('2026-01-01')"
        ),
        format!("ALTER TABLE ONLY {s}.events_2025 ALTER COLUMN note SET NOT NULL"),
        format!("ALTER TABLE {s}.base ALTER COLUMN b SET DEFAULT 'parent'"),
        format!("ALTER TABLE ONLY {s}.child ALTER COLUMN b SET DEFAULT 'child'"),
        format!("ALTER TABLE ONLY {s}.child ALTER COLUMN id SET NOT NULL"),
        person.clone(),
        format!(
            "CREATE TABLE {s}.people OF {s}.person (id WITH OPTIONS PRIMARY KEY, name WITH OPTIONS DEFAULT 'anon')"
        ),
        format!("CREATE FOREIGN DATA WRAPPER {fdw}"),
        format!("CREATE SERVER {fdw}_srv FOREIGN DATA WRAPPER {fdw}"),
        format!(
            "CREATE FOREIGN TABLE {s}.remote (id int OPTIONS (column_name 'ID'), v text) \
             SERVER {fdw}_srv OPTIONS (table_name 'r')"
        ),
    ]);
    if v >= 18 {
        fixture.push(format!("CREATE TABLE {s}.nn (a int CONSTRAINT a_required NOT NULL, b int NOT NULL NO INHERIT)"));
    }
    q.run(fixture).await.unwrap();
    let trigger_fn =
        DdlObject::TriggerFunction { schema: s.clone(), table: "Users".into(), trigger: "users_touch".into() };
    // In the order they can be created again.
    let mut objects = vec![trigger_fn];
    objects.extend(
        ["teams", "Users", "scratch", "events", "events_2024", "base", "child", "team_names", "team_count"]
            .map(|n| relation(&s, n)),
    );
    if v >= 18 {
        objects.push(relation(&s, "virt"));
    }
    let pay = DdlObject::Named { name: format!("{s}.pay(int)"), schema: None };
    objects.push(pay.clone());
    objects.extend(["events_2025", "people", "remote"].map(|n| relation(&s, n)));
    if v >= 18 {
        objects.push(relation(&s, "nn"));
    }
    let mut meta = Conn::open(&url, SessionRole::Meta, &tag).await;
    let mut before = Vec::new();
    for o in &objects {
        before.push(meta.text(o.clone()).await);
    }
    let users = &before[2];
    for part in [
        format!("CREATE SEQUENCE {s}.\"Users_id_seq\";"),
        format!("ALTER SEQUENCE {s}.\"Users_id_seq\" OWNED BY {s}.\"Users\".id;"),
        "\"Mixed Col\" character varying(20) DEFAULT 'x'::character varying,".to_string(),
        "twice bigint GENERATED ALWAYS AS (id * 2) STORED,".to_string(),
        format!("ALTER TABLE ONLY {s}.\"Users\" ALTER COLUMN note SET STORAGE EXTERNAL;"),
        format!("ALTER TABLE ONLY {s}.\"Users\" ALTER COLUMN note SET STATISTICS 500;"),
        format!("ALTER TABLE ONLY {s}.\"Users\" ALTER COLUMN email SET (n_distinct=-1);"),
        format!("ALTER TABLE {s}.\"Users\" DISABLE TRIGGER users_touch;"),
        format!("ALTER TABLE ONLY {s}.\"Users\" REPLICA IDENTITY USING INDEX users_email;"),
        format!("ALTER TABLE {s}.\"Users\" ENABLE ROW LEVEL SECURITY;"),
        format!(
            "CREATE POLICY own ON {s}.\"Users\" FOR SELECT TO {role} USING ((team_id IN ( SELECT t.id\n   FROM {s}.teams t)));"
        ),
        format!("COMMENT ON TABLE {s}.\"Users\" IS 'People''s accounts';"),
        format!("COMMENT ON CONSTRAINT \"Users_email_check\" ON {s}.\"Users\" IS 'not empty';"),
        format!("COMMENT ON INDEX {s}.users_lower IS 'case-insensitive';"),
        format!("GRANT SELECT, UPDATE ON TABLE {s}.\"Users\" TO {role};"),
    ] {
        assert!(users.contains(&part), "{part}\n---\n{users}");
    }
    let teams = &before[1];
    for part in [
        "id integer GENERATED ALWAYS AS IDENTITY (INCREMENT BY 5 START WITH 10) NOT NULL,".to_string(),
        "name text COLLATE pg_catalog.\"C\" NOT NULL,".to_string(),
        "CONSTRAINT teams_during_excl EXCLUDE USING gist (during WITH &&)".to_string(),
        "WITH (fillfactor=70);".to_string(),
        format!("GRANT SELECT (name) ON TABLE {s}.teams TO {role} WITH GRANT OPTION;"),
    ] {
        assert!(teams.contains(&part), "{part}\n---\n{teams}");
    }
    assert!(before[0].contains(&format!("CREATE OR REPLACE FUNCTION {s}.touch()")), "{}", before[0]);
    assert!(before[3].contains(&format!("CREATE UNLOGGED TABLE {s}.scratch")), "{}", before[3]);
    assert!(before[5].contains(&format!("PARTITION OF {s}.events (")), "{}", before[5]);
    assert!(!before[5].contains("PRIMARY KEY"), "the parent's key: {}", before[5]);
    assert!(before[7].contains(&format!("    extra text\n)\nINHERITS ({s}.base);")), "{}", before[7]);
    assert!(before[8].contains("WITH (security_barrier=true, check_option=local) AS"), "{}", before[8]);
    assert!(before[9].contains("WITH NO DATA;"), "{}", before[9]);
    if v >= 14 {
        let part = format!("ALTER TABLE ONLY {s}.\"Users\" ALTER COLUMN \"Mixed Col\" SET COMPRESSION pglz;");
        assert!(users.contains(&part), "{part}\n---\n{users}");
    }
    if v >= 18 {
        assert!(before[10].contains("    b integer GENERATED ALWAYS AS (a * 3) VIRTUAL\n"), "{}", before[10]);
    }
    let text_of = |o: &DdlObject| before[objects.iter().position(|x| x == o).unwrap()].as_str();
    let pay_text = text_of(&pay);
    for part in [
        format!("REVOKE ALL ON FUNCTION {s}.pay(amount integer) FROM PUBLIC;"),
        format!("GRANT EXECUTE ON FUNCTION {s}.pay(amount integer) TO {role};"),
    ] {
        assert!(pay_text.contains(&part), "{part}\n---\n{pay_text}");
    }
    let p2025 = text_of(&relation(&s, "events_2025"));
    for part in [
        format!("ALTER TABLE ONLY {s}.events_2025 ALTER COLUMN at SET DEFAULT '2025-06-01'::date;"),
        format!("ALTER TABLE ONLY {s}.events_2025 ALTER COLUMN note SET NOT NULL;"),
    ] {
        assert!(p2025.contains(&part), "{part}\n---\n{p2025}");
    }
    assert!(!p2025.contains("COLUMN id"), "the parent's as it is: {p2025}");
    let child = text_of(&relation(&s, "child"));
    for part in [
        format!("ALTER TABLE ONLY {s}.child ALTER COLUMN b SET DEFAULT 'child'::text;"),
        format!("ALTER TABLE ONLY {s}.child ALTER COLUMN id SET NOT NULL;"),
    ] {
        assert!(child.contains(&part), "{part}\n---\n{child}");
    }
    let people = text_of(&relation(&s, "people"));
    assert!(people.contains(&format!("CREATE TABLE {s}.people OF {s}.person (")), "{people}");
    assert!(people.contains("    name WITH OPTIONS DEFAULT 'anon'::text,"), "{people}");
    let remote = text_of(&relation(&s, "remote"));
    assert!(remote.contains("    id integer OPTIONS (column_name 'ID'),"), "{remote}");
    let scratch = text_of(&relation(&s, "scratch"));
    let create = if v >= 15 { "CREATE UNLOGGED SEQUENCE" } else { "CREATE SEQUENCE" };
    assert!(scratch.contains(&format!("{create} {s}.scratch_k_seq\n    AS integer;")), "{scratch}");
    if v >= 18 {
        let nn = text_of(&relation(&s, "nn"));
        assert!(nn.contains("    CONSTRAINT a_required NOT NULL a,"), "{nn}");
        assert!(nn.contains("NOT NULL b NO INHERIT"), "{nn}");
        assert!(nn.contains("    a integer,\n"), "no inline NOT NULL: {nn}");
    }

    q.run(vec![format!("DROP SCHEMA {s} CASCADE"), format!("CREATE SCHEMA {s}"), person]).await.unwrap();
    for (o, text) in objects.iter().zip(&before) {
        q.run(statements(text)).await.unwrap_or_else(|e| panic!("{o:?}: {e}\n{text}"));
    }
    for (o, text) in objects.iter().zip(&before) {
        assert_eq!(&meta.text(o.clone()).await, text, "{o:?}");
    }
}

/// Every kind of object, its own way: an index (and the constraint an index backs), a
/// trigger, a trigger's function, a name typed by the user in the tab's schema (a relation, a
/// function), and what is not there.
#[tokio::test(flavor = "multi_thread")]
async fn each_kind_of_object_and_what_is_not_there() {
    let Some(url) = pg_url("each_kind_of_object_and_what_is_not_there") else { return };
    let tag = format!("zzddlkinds{}", std::process::id());
    let mut meta = Conn::open(&url, SessionRole::Meta, &tag).await;
    let index = DdlObject::Index { schema: "shop".into(), name: "users_created_at_idx".into() };
    let (Ok(DdlSource::Index(i)), _) = meta.ddl(index).await else { panic!("an index") };
    assert_eq!(i.definition, "CREATE INDEX users_created_at_idx ON shop.users USING btree (created_at)");
    assert_eq!((i.table.as_str(), i.constraint.is_none()), ("users", true));
    let pk = DdlObject::Index { schema: "shop".into(), name: "users_pkey".into() };
    let (Ok(DdlSource::Index(pk)), _) = meta.ddl(pk).await else { panic!("a key's index") };
    assert_eq!(pk.constraint, Some(("users_pkey".to_string(), "PRIMARY KEY (id)".to_string())));

    let named =
        |name: &str, schema: Option<&str>| DdlObject::Named { name: name.into(), schema: schema.map(str::to_string) };
    let (Ok(DdlSource::Relation(r)), _) = meta.ddl(named("users", Some("shop"))).await else { panic!("users") };
    assert_eq!((r.schema.as_str(), r.name.as_str(), r.structure.kind), ("shop", "users", RelationKind::Table));
    let (Ok(DdlSource::Relation(r)), _) = meta.ddl(named("shop.order_summary", None)).await else { panic!("view") };
    assert_eq!(r.structure.kind, RelationKind::View);
    let (Ok(DdlSource::Function(f)), _) = meta.ddl(named("slow", Some("analytics"))).await else { panic!("fn") };
    assert_eq!((f.name.as_str(), f.arguments.as_str()), ("slow", "seconds integer"));
    assert!(
        f.definition.starts_with("CREATE OR REPLACE FUNCTION analytics.slow(seconds integer DEFAULT 30)"),
        "{}",
        f.definition
    );
    let (Ok(DdlSource::Function(_)), _) = meta.ddl(named("analytics.slow(int)", None)).await else { panic!("sig") };

    assert_eq!(meta.ddl(named("no_such_thing", Some("shop"))).await.0, Err(DbError::NotFound));
    assert_eq!(meta.ddl(named("users", None)).await.0, Err(DbError::NotFound), "not in the search path");
    match meta.ddl(relation("shop", "no_such_table")).await.0 {
        Err(DbError::Server(e)) => assert!(e.contains("does not exist"), "{e}"),
        other => panic!("{other:?}"),
    }
    let trigger = DdlObject::Trigger { schema: "shop".into(), table: "users".into(), name: "no_such".into() };
    assert_eq!(meta.ddl(trigger).await.0, Err(DbError::NotFound));
    // The session still answers after those.
    assert!(meta.ddl(relation("shop", "users")).await.0.is_ok());
}

/// Faster than the metadata session's `lock_timeout` (2 s): an answer that waited for a lock
/// would come only after it (as `Locked`); this one did not wait (machine load allowed for).
const AT_ONCE: Duration = Duration::from_millis(1900);

/// A DDL never waits on another session, and never makes one wait. Another session's `ACCESS
/// EXCLUSIVE` lock, held or queued behind a reader, on the object or on a relation reading its
/// definition would lock (a view's base table, the table a policy's subquery reads, an index's
/// or a trigger's table, the table an SQL-standard function body reads): the DDL answers at
/// once with `DbError::Locked`, and the metadata session holds and asks for no lock on any
/// relation of the schema (`pg_locks`, looked at while it is asked and when it answered). A
/// function whose body is a string reads no relation: it answers while the table is locked.
#[tokio::test(flavor = "multi_thread")]
async fn a_ddl_never_waits_for_a_lock() {
    let Some(url) = pg_url("a_ddl_never_waits_for_a_lock") else { return };
    let pid = std::process::id();
    let s = format!("zz_ddl_lock_{pid}");
    let _schema = cleanup(&url, format!("DROP SCHEMA IF EXISTS {s} CASCADE"));
    let tag = format!("zzddllock{pid}");
    let mut q = Conn::open(&url, SessionRole::Query, &tag).await;
    let v = major(&mut q).await;
    let mut fixture = vec![
        format!("CREATE SCHEMA {s}"),
        format!("CREATE TABLE {s}.base (id int PRIMARY KEY, v text CHECK (v <> ''))"),
        format!("CREATE TABLE {s}.other (id int PRIMARY KEY)"),
        format!("CREATE TABLE {s}.guarded (id int)"),
        format!("ALTER TABLE {s}.guarded ENABLE ROW LEVEL SECURITY"),
        format!("CREATE POLICY p ON {s}.guarded USING (id IN (SELECT o.id FROM {s}.other o))"),
        format!("CREATE VIEW {s}.v AS SELECT b.id FROM {s}.base b"),
        format!("CREATE INDEX base_v ON {s}.base (lower(v))"),
        format!("CREATE FUNCTION {s}.f() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$"),
        format!(
            "CREATE TRIGGER t BEFORE UPDATE ON {s}.base FOR EACH ROW WHEN (OLD.v IS DISTINCT FROM NEW.v) \
             EXECUTE FUNCTION {s}.f()"
        ),
    ];
    if v >= 14 {
        fixture.push(format!(
            "CREATE FUNCTION {s}.n() RETURNS bigint LANGUAGE sql BEGIN ATOMIC SELECT count(*) FROM {s}.base; END"
        ));
    }
    q.run(fixture).await.unwrap();
    let mut meta = Conn::open(&url, SessionRole::Meta, &tag).await;
    let mut observer = Conn::open(&url, SessionRole::Query, &format!("{tag}o")).await;
    let locks_sql = format!(
        "SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
         WHERE a.application_name = 'datarig-meta-{tag}' AND l.locktype = 'relation' \
         AND l.relation IN (SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relnamespace = '{s}'::regnamespace)"
    );
    // The DDL of `object`, how long it took to come, and the relation locks of the schema the
    // metadata session held or asked for (300 ms in, or when it answered, whichever came first).
    let mut ask = async |meta: &mut Conn, object: DdlObject| {
        meta.next += 1;
        let id = meta.next;
        let t0 = Instant::now();
        meta.session.send(DbCommand::LoadDdl { id, object });
        let first = tokio::time::timeout(Duration::from_millis(300), meta.rx.recv()).await;
        let came = t0.elapsed();
        let locks = observer.one(&locks_sql).await;
        let (ev, took) = match first {
            Ok(Some(ev)) => (ev, came),
            _ => {
                let ev = meta.wait(|e| matches!(e, DbEvent::Ddl { .. }), 10).await;
                (ev, t0.elapsed())
            }
        };
        let DbEvent::Ddl { result, .. } = ev else { panic!("{ev:?}") };
        (result, took, locks)
    };
    let trigger = DdlObject::Trigger { schema: s.clone(), table: "base".into(), name: "t".into() };
    let function = DdlObject::TriggerFunction { schema: s.clone(), table: "base".into(), trigger: "t".into() };
    let index = DdlObject::Index { schema: s.clone(), name: "base_v".into() };
    let sql_body = DdlObject::Named { name: format!("{s}.n"), schema: None };
    // Not locked: every one answers.
    for o in [
        relation(&s, "base"),
        relation(&s, "v"),
        relation(&s, "guarded"),
        trigger.clone(),
        function.clone(),
        index.clone(),
    ] {
        let (r, _, _) = ask(&mut meta, o.clone()).await;
        assert!(r.is_ok(), "{o:?}: {r:?}");
    }

    // Held: an `ALTER TABLE` of the base table holds its lock.
    let mut holder = Conn::open(&url, SessionRole::Query, &format!("{tag}h")).await;
    holder.run(vec!["BEGIN".into(), format!("LOCK TABLE {s}.base IN ACCESS EXCLUSIVE MODE")]).await.unwrap();
    let mut locked = vec![relation(&s, "base"), relation(&s, "v"), trigger.clone(), index.clone()];
    if v >= 14 {
        locked.push(sql_body.clone());
    }
    for o in locked {
        let (r, took, locks) = ask(&mut meta, o.clone()).await;
        assert_eq!(r, Err(DbError::Locked), "{o:?}");
        assert!(took < AT_ONCE, "{o:?}: at once: {took:?}");
        assert_eq!(locks, "0", "{o:?}: no lock held or asked for");
    }
    // What does not read the base table answers.
    for o in [function.clone(), relation(&s, "guarded"), relation(&s, "other")] {
        let (r, took, locks) = ask(&mut meta, o.clone()).await;
        assert!(r.is_ok(), "{o:?}: {r:?}");
        assert!(took < AT_ONCE, "{o:?}: at once: {took:?}");
        assert_eq!(locks, "0", "{o:?}: nothing left held");
    }
    holder.run(vec!["ROLLBACK".into()]).await.unwrap();

    // Held on the table a policy's subquery reads.
    holder.run(vec!["BEGIN".into(), format!("LOCK TABLE {s}.other IN ACCESS EXCLUSIVE MODE")]).await.unwrap();
    let (r, took, locks) = ask(&mut meta, relation(&s, "guarded")).await;
    assert_eq!((r, locks.as_str()), (Err(DbError::Locked), "0"), "the policy's table");
    assert!(took < AT_ONCE, "at once: {took:?}");
    holder.run(vec!["ROLLBACK".into()]).await.unwrap();

    // Held on the view itself (an `ALTER VIEW`).
    holder.run(vec!["BEGIN".into(), format!("ALTER VIEW {s}.v SET (security_barrier = false)")]).await.unwrap();
    let (r, _, locks) = ask(&mut meta, relation(&s, "v")).await;
    assert_eq!((r, locks.as_str()), (Err(DbError::Locked), "0"), "the view");
    holder.run(vec!["ROLLBACK".into()]).await.unwrap();

    // Waited for: a reader holds the base table and a `LOCK TABLE` queues behind it.
    let mut reader = Conn::open(&url, SessionRole::Query, &format!("{tag}r")).await;
    reader.run(vec!["BEGIN".into(), format!("SELECT count(*) FROM {s}.base")]).await.unwrap();
    holder.next += 1;
    let waiting_id = holder.next;
    holder.session.send(DbCommand::Execute {
        id: waiting_id,
        statements: vec!["BEGIN".into(), format!("LOCK TABLE {s}.base IN ACCESS EXCLUSIVE MODE")],
    });
    let queued = format!(
        "SELECT count(*) FROM pg_locks WHERE relation = '{s}.base'::regclass \
         AND mode = 'AccessExclusiveLock' AND NOT granted"
    );
    let mut watch = Conn::open(&url, SessionRole::Query, &format!("{tag}w")).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while watch.one(&queued).await != "1" {
        assert!(Instant::now() < deadline, "the LOCK TABLE never queued");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for o in [relation(&s, "base"), relation(&s, "v")] {
        let (r, took, locks) = ask(&mut meta, o.clone()).await;
        assert_eq!(r, Err(DbError::Locked), "{o:?}");
        assert!(took < AT_ONCE, "{o:?}: at once: {took:?}");
        assert_eq!(locks, "0", "{o:?}: not queued behind the waiting lock");
    }
    reader.run(vec!["ROLLBACK".into()]).await.unwrap();
    holder.wait(|e| matches!(e, DbEvent::Done { id, .. } | DbEvent::Failed { id, .. } if *id == waiting_id), 10).await;
    holder.run(vec!["ROLLBACK".into()]).await.unwrap();

    // Asked again once the lock is gone.
    assert!(ask(&mut meta, relation(&s, "v")).await.0.is_ok());
}
