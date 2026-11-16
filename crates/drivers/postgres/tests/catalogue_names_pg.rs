//! The metadata session's reads against a real PostgreSQL (`DATARIG_TEST_PG_URL`, see
//! `integration_pg.rs`) whose search path holds a schema with operators of its own: each one
//! matches the argument types of a comparison or a concatenation of a catalogue read exactly,
//! where the server's own needs a cast, so it would be chosen over the server's wherever the
//! read leaves the name to the path. None of them runs.
//!
//! Everything is created in a database of the test's own (`zz_catnamesdb_<pid>`), dropped by a
//! guard.

#[allow(dead_code)]
mod pg_clean;

use datarig_core::driver::ddl::DdlObject;
use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Driver, Session, SessionContext, SessionRole};
use datarig_core::profile::ConnectionConfig;
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

/// Drops the test's database when it goes out of scope (also when an assertion fails).
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

/// A database of the test's own and `url` with it in place of the shared one.
fn own_database(url: &str) -> (DatabaseGuard, String) {
    let name = format!("zz_catnamesdb_{}", std::process::id());
    let guard = DatabaseGuard { url: url.to_string(), name: name.clone() };
    for sql in [format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"), format!("CREATE DATABASE {name}")] {
        pg_clean::run_fresh(url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let host = url.rfind('@').map_or(url.find("//").unwrap() + 2, |i| i + 1);
    let path = url[host..].find(['/', '?']).map_or(url.len(), |i| host + i);
    let query = url[path..].find('?').map_or("", |i| &url[path + i..]);
    (guard, format!("{}/{name}{query}", &url[..path]))
}

struct Meta {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Meta {
    /// Every event until the first one `pred` matches, that one included.
    async fn until(&mut self, pred: impl Fn(&DbEvent) -> bool) -> Vec<DbEvent> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(ev)) => {
                    let done = pred(&ev) || matches!(ev, DbEvent::ConnectFailed { .. });
                    seen.push(ev);
                    if done {
                        return seen;
                    }
                }
                Ok(None) => panic!("event channel closed: {seen:?}"),
                Err(_) => panic!("timed out: {seen:?}"),
            }
        }
    }
}

/// The operators of the schema: `(name, left, right)`, each for the comparison or the
/// concatenation of a catalogue read it would take the place of.
const PLANTED: &[(&str, &str, &str)] = &[
    // The schemas left out of the explorer: `nspname NOT LIKE 'pg\_toast%'`.
    ("!~~", "name", "name"),
    // The row and size estimates: `reltuples >= 0`, `reltuples > 0`, `2 * reltuples`.
    (">=", "float8", "integer"),
    (">", "float8", "integer"),
    ("*", "integer", "float8"),
    // Index collations and domains: `indcollation[k] <> 0`, `typbasetype = 0`.
    ("<>", "oid", "integer"),
    ("=", "oid", "integer"),
    // A table and a type named by text: `oid = '…'::regclass`, `oid = '…'::regtype`.
    ("=", "oid", "regclass"),
    ("=", "oid", "regtype"),
    // A trigger's function `nspname || '.' || proname`; a DDL target `'r:' || oid`.
    ("||", "name", "name"),
    ("||", "oid", "oid"),
];

#[tokio::test(flavor = "multi_thread")]
async fn no_operator_of_a_schema_on_the_path_runs_in_a_catalogue_read() {
    let Some(url) = pg_url("no_operator_of_a_schema_on_the_path_runs_in_a_catalogue_read") else { return };
    let (_database, url) = own_database(&url);
    let s = format!("zz_catnames_{}", std::process::id());
    let mut setup = vec![
        format!("CREATE SCHEMA {s}"),
        format!("CREATE TABLE {s}.t (id int PRIMARY KEY, v text, w text)"),
        format!("CREATE INDEX t_v ON {s}.t (v COLLATE \"C\")"),
        format!("CREATE TABLE {s}.child (id int PRIMARY KEY, t_id int REFERENCES {s}.t)"),
        format!("CREATE DOMAIN {s}.d AS int"),
        format!("ALTER TABLE {s}.t ADD COLUMN dv {s}.d"),
        format!("CREATE FUNCTION {s}.touch() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$"),
        format!("CREATE TRIGGER t_touch BEFORE UPDATE ON {s}.t FOR EACH ROW EXECUTE FUNCTION {s}.touch()"),
        format!("INSERT INTO {s}.t SELECT g, g::text, g::text FROM generate_series(1, 20) g"),
        format!("ANALYZE {s}.t"),
    ];
    for (i, (op, left, right)) in PLANTED.iter().enumerate() {
        setup.push(format!(
            "CREATE FUNCTION {s}.planted{i}({left}, {right}) RETURNS boolean LANGUAGE plpgsql AS \
             $$BEGIN RAISE EXCEPTION 'zz_catnames planted % ran', {i}; END$$"
        ));
        setup.push(format!(
            "CREATE OPERATOR {s}.{op} (LEFTARG = {left}, RIGHTARG = {right}, FUNCTION = {s}.planted{i})"
        ));
    }
    for sql in &setup {
        pg_clean::run_fresh(&url, sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }

    // The profile's schema is the session's: first on the path after the implicit `pg_catalog`.
    let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.clone()), ..ConnectionConfig::test_db() };
    let context = SessionContext { database: None, schema: Some(s.clone()) };
    let tag = format!("catnames{}", std::process::id());
    let (tx, rx) = unbounded_channel();
    let opts = ConnectOptions::new(500, SessionRole::Meta, &tag).context(context.clone());
    let session = PgDriver.connect(&cfg, SessionRole::Meta, opts, tx);
    // A query session in the same schema: the operators are live there (the user's own SQL
    // resolves through the path, as with any client).
    let (qtx, mut qrx) = unbounded_channel();
    let qopts = ConnectOptions::new(500, SessionRole::Query, &tag).context(context);
    let query = PgDriver.connect(&cfg, SessionRole::Query, qopts, qtx);
    let mut meta = Meta { session, rx };
    loop {
        match tokio::time::timeout(Duration::from_secs(30), qrx.recv()).await {
            Ok(Some(DbEvent::Connected)) => break,
            Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("{error:?}"),
            Ok(Some(_)) => {}
            other => panic!("{other:?}"),
        }
    }
    query.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT 1::oid <> 0".into()],
        paging: datarig_core::driver::PagingMode::Hold,
        tx: datarig_core::driver::TxRule::Auto,
        max_rows: None,
    });
    let live = loop {
        match tokio::time::timeout(Duration::from_secs(30), qrx.recv()).await {
            Ok(Some(ev @ (DbEvent::Failed { .. } | DbEvent::Page { .. }))) => break ev,
            Ok(Some(_)) => {}
            other => panic!("{other:?}"),
        }
    };
    assert!(format!("{live:?}").contains("zz_catnames planted"), "the schema's operators are live: {live:?}");

    let mut events = meta.until(|e| matches!(e, DbEvent::Keys(_))).await;
    meta.session.send(DbCommand::LoadDatabases);
    events.extend(meta.until(|e| matches!(e, DbEvent::Databases(_))).await);
    meta.session.send(DbCommand::LoadObjects { schema: s.clone() });
    events.extend(meta.until(|e| matches!(e, DbEvent::Objects { .. })).await);
    meta.session.send(DbCommand::LoadStructure { schema: s.clone(), table: "t".into() });
    events.extend(meta.until(|e| matches!(e, DbEvent::Structure { .. })).await);
    meta.session.send(DbCommand::LoadTableKeys { id: 2, schema: s.clone(), table: "t".into() });
    events.extend(meta.until(|e| matches!(e, DbEvent::TableKeys { .. })).await);
    let ddl = [
        DdlObject::Relation { schema: s.clone(), name: "t".into() },
        DdlObject::Index { schema: s.clone(), name: "t_v".into() },
        DdlObject::Trigger { schema: s.clone(), table: "t".into(), name: "t_touch".into() },
        DdlObject::TriggerFunction { schema: s.clone(), table: "t".into(), trigger: "t_touch".into() },
        DdlObject::Named { name: "child".into(), schema: Some(s.clone()) },
        DdlObject::Named { name: "touch()".into(), schema: Some(s.clone()) },
    ];
    for (i, object) in ddl.into_iter().enumerate() {
        let id = 10 + i as u64;
        meta.session.send(DbCommand::LoadDdl { id, object });
        events.extend(meta.until(|e| matches!(e, DbEvent::Ddl { id: d, .. } if *d == id)).await);
    }

    let failed: Vec<String> = events
        .iter()
        .map(|e| format!("{e:?}"))
        .filter(|e| e.contains("zz_catnames planted") || e.contains("Err(") || e.contains("ConnectFailed"))
        .collect();
    assert!(failed.is_empty(), "reads that failed:\n{}", failed.join("\n"));
    let objects = events.iter().find_map(|e| match e {
        DbEvent::Objects { result: Ok(o), .. } => Some(o),
        _ => None,
    });
    let objects = objects.expect("the schema's objects");
    assert_eq!(objects.tables, ["child", "t"]);
    assert_eq!(objects.stats["t"].rows, Some(20), "the estimates are read");
    let schemas = events.iter().find_map(|e| match e {
        DbEvent::Schemas(Ok(list)) => Some(list.clone()),
        _ => None,
    });
    assert!(schemas.expect("the schemas").contains(&s), "the schema is listed");
}
