//! The *meta* connection: schemas, relations and columns for the explorer tree and completion,
//! the key constraints of the tables (`Capabilities::key_metadata`), and one table's structure
//! when its node opens (`Capabilities::structure`, [`structure`]). Every read reports its
//! failure: a catalog or key list that could not be read is never sent as an empty one.
//!
//! Every read is one unnamed statement, parsed, bound and run in one round trip
//! (`Client::query_typed`): nothing depends on a statement prepared in an earlier transaction,
//! which a pooler in transaction mode may have run on another server connection.
//!
//! A lookup never waits on another session: every read runs in a transaction of its own whose
//! `lock_timeout` is short ([`read`]), so a read that would wait for a lock (a catalog locked
//! by a `VACUUM FULL`, a table an `ALTER TABLE` holds) gives up with [`DbError::Locked`] instead
//! of hanging, and of holding up the sessions queued behind it. The structure also checks for
//! such a lock before it asks for one ([`structure`]).

use crate::connect::db_error;
use crate::link::{Link, Next};
use datarig_core::driver::keys::{Generated, KeyCatalog, KeyKind};
use datarig_core::driver::structure::RelationStats;
use datarig_core::driver::{DbCommand, DbError, DbEvent, SchemaObjects};
use datarig_core::sql::complete::{Catalog, ColumnInfo, Relation};
use tokio::sync::mpsc::UnboundedSender;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{Client, Row};

/// The estimates of rows and size of the relations of a CTE `roots (oid, kind)` (`kind` its
/// `relkind` as text), as CTEs to follow it in a `WITH RECURSIVE`: `stats (root, known_rows,
/// est_rows, known_size, bytes)`, one row per root with storage of its own or through its
/// partitions; `stats_rows!` and `stats_bytes!` read it (`LEFT JOIN stats s`). The rules are
/// the table structure's ([`structure`]) and the explorer's list of a schema's objects alike:
///
/// * `heaps` are the relations whose statistics are a root's: itself (a table, a materialized
///   view), or a partitioned table's leaf partitions (found in `pg_inherits`, at any depth);
/// * a heap's rows are `reltuples` (the last `VACUUM` or `ANALYZE`), or the live rows the
///   cumulative statistics count (`n_live_tup` of `pg_stat_all_tables`, read with
///   `pg_stat_get_live_tuples`) when they are more than twice as many: rows added since count
///   before the next `ANALYZE`. Only then, as the counters are estimates too: they start again
///   from 0 when the statistics are reset (a crash, `pg_stat_reset`) while `reltuples` stays,
///   and they count again the rows an `ANALYZE` saw that a session had not reported yet (at
///   most as many as it saw). Rows deleted since count at the next `ANALYZE`;
/// * rows are counted only where every heap has an estimate (`reltuples` `-1`: never vacuumed
///   or analyzed), and the size where every one has statistics of its own (the same, unless its
///   own `relpages` is set, by a `CREATE INDEX`; a foreign table has no storage): its indexes and
///   its TOAST table's index have pages from their creation on, which are not its size;
/// * the size is `relpages` of the heaps, their TOAST tables and all their indexes, times
///   `block_size`, each heap's grown as its rows did since `reltuples` (its pages hold as many
///   rows each as they did then, as the planner assumes), in whole pages.
///
/// It reads `pg_class`, `pg_inherits`, `pg_index` and the cumulative statistics only, and locks
/// no relation (the functions that give the exact size, `pg_total_relation_size` and
/// `pg_partition_tree`, lock each one and wait behind an `ALTER TABLE` or a `VACUUM FULL`).
/// Indexes are found in one pass, grouped by heap, never looked up per heap: the planned cost
/// stays linear in the catalog, under `jit_above_cost` with thousands of relations, for a table
/// of thousands of partitions and for a schema of thousands of tables.
macro_rules! stats_ctes {
    () => {
        "tree AS (
  SELECT r.oid AS root, r.oid FROM roots r WHERE r.kind = 'p'
  UNION ALL
  SELECT tree.root, i.inhrelid FROM pg_catalog.pg_inherits i JOIN tree ON i.inhparent = tree.oid
), heaps AS (
  SELECT m.root, h.oid, h.relkind, h.reltuples::float8 AS reltuples, h.relpages::int8 AS relpages, h.reltoastrelid,
         pg_catalog.pg_stat_get_live_tuples(h.oid)::float8 AS live
  FROM (SELECT r.oid AS root, r.oid FROM roots r WHERE r.kind IN ('r', 'm')
        UNION ALL SELECT tree.root, tree.oid FROM tree) m
  JOIN pg_catalog.pg_class h ON h.oid = m.oid
  WHERE h.relkind <> 'p'
), index_pages AS (
  SELECT x.root, x.heap, sum(ic.relpages)::int8 AS pages
  FROM (SELECT y.root, y.oid AS heap, y.oid FROM heaps y
        UNION ALL SELECT y.root, y.oid, y.reltoastrelid FROM heaps y) x
  JOIN pg_catalog.pg_index i ON i.indrelid = x.oid
  JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
  GROUP BY x.root, x.heap
), sized AS (
  SELECT h.root, h.relkind, h.reltuples, h.relpages,
         CASE WHEN h.live > 2 * h.reltuples AND h.reltuples >= 0 THEN h.live WHEN h.reltuples >= 0 THEN h.reltuples END
           AS est_rows,
         (h.relpages + coalesce(t.relpages, 0) + coalesce(ix.pages, 0))::float8 AS pages
  FROM heaps h
  LEFT JOIN pg_catalog.pg_class t ON t.oid = h.reltoastrelid
  LEFT JOIN index_pages ix ON ix.root = h.root AND ix.heap = h.oid
), stats AS (
  SELECT z.root, bool_and(z.est_rows IS NOT NULL) AS known_rows, sum(z.est_rows)::float8 AS est_rows,
         bool_and(z.relkind = 'f' OR z.reltuples >= 0 OR z.relpages > 0) AS known_size,
         pg_catalog.round(sum(CASE WHEN z.reltuples > 0 THEN z.pages * z.est_rows / z.reltuples ELSE z.pages END))::int8
           * pg_catalog.current_setting('block_size')::int8 AS bytes
  FROM sized z
  GROUP BY z.root
)"
    };
}

/// The row estimate of a root of kind `kind` joined to its `stats_ctes!` row as `s`: `-1`
/// unknown, `NULL` without storage (a view, a foreign table). A partitioned table without
/// partitions (no heaps, no row) has none: 0.
macro_rules! stats_rows {
    ($kind:literal) => {
        concat!(
            "CASE WHEN ",
            $kind,
            " IN ('r', 'm', 'p') THEN CASE WHEN s.root IS NULL THEN 0 WHEN s.known_rows THEN s.est_rows ELSE -1 END END"
        )
    };
}

/// The size estimate in bytes, as `stats_rows!`: `NULL` unknown or without storage.
macro_rules! stats_bytes {
    ($kind:literal) => {
        concat!(
            "CASE WHEN ",
            $kind,
            " IN ('r', 'm', 'p') THEN CASE WHEN s.root IS NULL THEN 0 WHEN s.known_size THEN s.bytes END END"
        )
    };
}

#[macro_use]
pub(crate) mod structure;
pub(crate) mod ddl;

const HIDDEN_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";

/// What each read starts with: its own read-only transaction, whose waits for a lock end after
/// 2 s, without JIT compilation (PostgreSQL 11 and later: `set_config(…, true)` is a `SET
/// LOCAL` the older servers, which have no `jit`, skip). A catalog read is short, but a
/// catalog of some thousand relations can raise its planned cost over `jit_above_cost`, and
/// compiling it costs far more than running it. Both settings end with the transaction, so
/// nothing is left on a server connection a pooler hands to another client.
pub const BEGIN_READ: &str = "BEGIN READ ONLY; SET LOCAL lock_timeout = '2s'; \
     SELECT pg_catalog.set_config('jit', 'off', true) \
     WHERE pg_catalog.current_setting('server_version_num')::int >= 110000";

/// Run the catalog read `sql` in a transaction of its own ([`BEGIN_READ`]). The `BEGIN`, the
/// statement and the `COMMIT` go out together (one round trip); a failed statement leaves the
/// transaction aborted, which the `COMMIT` rolls back. A lock wait that timed out (SQLSTATE
/// `55P03`) is [`DbError::Locked`].
async fn read(client: &Client, sql: &str, params: &[(&(dyn ToSql + Sync), Type)]) -> Result<Vec<Row>, DbError> {
    let (begun, rows, _) =
        tokio::join!(client.batch_execute(BEGIN_READ), client.query_typed(sql, params), client.batch_execute("COMMIT"));
    let error = |e: tokio_postgres::Error| {
        if e.code() == Some(&SqlState::LOCK_NOT_AVAILABLE) { DbError::Locked } else { db_error(&e) }
    };
    begun.map_err(error)?;
    rows.map_err(error)
}

pub(crate) async fn meta_loop(client: Client, mut link: Link, events: UnboundedSender<DbEvent>) {
    let Ok(schemas) = link.guard(None, load_schemas(&client)).await else { return };
    let _ = events.send(DbEvent::Schemas(schemas));
    let Ok(catalog) = link.guard(None, load_catalog(&client)).await else { return };
    let _ = events.send(DbEvent::Catalog(catalog));
    // The server's version, read once per connection: key metadata needs PostgreSQL 12.
    let Ok(version) = link.guard(None, server_version(&client)).await else { return };
    let Ok(keys) = link.guard(None, load_keys(&client, &version)).await else { return };
    let _ = events.send(DbEvent::Keys(keys));
    loop {
        let cmd = match link.next().await {
            // No portal is ever open here.
            Next::Command(DbCommand::ClosePortal { .. }) => continue,
            Next::Command(c) => c,
            Next::Closed => return,
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        let request = async {
            match cmd {
                DbCommand::LoadSchemas => DbEvent::Schemas(load_schemas(&client).await),
                DbCommand::LoadObjects { schema } => {
                    let result = load_objects(&client, &schema).await;
                    DbEvent::Objects { schema, result }
                }
                DbCommand::LoadCatalog => DbEvent::Catalog(load_catalog(&client).await),
                DbCommand::LoadKeys => DbEvent::Keys(load_keys(&client, &version).await),
                DbCommand::LoadDatabases => DbEvent::Databases(load_databases(&client).await),
                DbCommand::LoadStructure { schema, table } => {
                    // It reads `attgenerated` and `pg_partition_tree`, as the keys do (12).
                    let result = match version.clone().and_then(keys_supported) {
                        Ok(()) => structure::load_structure(&client, &schema, &table).await.map(Box::new),
                        Err(e) => Err(e),
                    };
                    DbEvent::Structure { schema, table, result }
                }
                DbCommand::LoadDdl { id, object } => {
                    // It reads the structure's parts (12).
                    let result = match version.clone().and_then(keys_supported) {
                        Ok(()) => ddl::load_ddl(&client, &object).await,
                        Err(e) => Err(e),
                    };
                    DbEvent::Ddl { id, result }
                }
                // Statements run on a tab's query session, never on the shared metadata one.
                DbCommand::Execute { id, .. }
                | DbCommand::Resume { id, .. }
                | DbCommand::FetchMore { id }
                | DbCommand::ClosePortal { id } => {
                    DbEvent::Failed { id, error: DbError::NotSupported, cancelled: false }
                }
                DbCommand::Count { id, .. } => {
                    DbEvent::Counted { id, result: Err(DbError::NotSupported), snapshot: false }
                }
            }
        };
        let Ok(ev) = link.guard(None, request).await else { return };
        if events.send(ev).is_err() {
            return;
        }
    }
}

async fn load_schemas(client: &Client) -> Result<Vec<String>, DbError> {
    let sql = format!("SELECT n.nspname FROM pg_namespace n WHERE {HIDDEN_SCHEMAS} ORDER BY n.nspname");
    let rows = read(client, &sql, &[]).await?;
    Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
}

/// The databases the user may connect to (templates and databases that refuse connections
/// left out), by name.
async fn load_databases(client: &Client) -> Result<Vec<String>, DbError> {
    let rows = read(
        client,
        "SELECT d.datname::text FROM pg_catalog.pg_database d \
         WHERE d.datallowconn AND NOT d.datistemplate \
         AND pg_catalog.has_database_privilege(d.datname, 'CONNECT') ORDER BY 1",
        &[],
    )
    .await?;
    Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
}

/// A schema's relations with the estimates of those with storage (`stats_ctes!`), in one
/// statement.
pub const SCHEMA_OBJECTS: &str = concat!(
    "WITH RECURSIVE roots AS (
  SELECT c.oid, c.relname, c.relkind::text AS kind
  FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'f', 'v', 'm')
), ",
    stats_ctes!(),
    "
SELECT r.relname, r.kind, (",
    stats_rows!("r.kind"),
    ")::float8, (",
    stats_bytes!("r.kind"),
    ")::int8
FROM roots r LEFT JOIN stats s ON s.root = r.oid
ORDER BY r.relname"
);

async fn load_objects(client: &Client, schema: &str) -> Result<SchemaObjects, DbError> {
    let rows = read(client, SCHEMA_OBJECTS, &[(&schema, Type::TEXT)]).await?;
    let mut out = SchemaObjects::default();
    for r in rows {
        let name: String = r.get(0);
        let kind: String = r.get(1);
        if matches!(kind.as_str(), "r" | "p" | "m") {
            out.stats.insert(name.clone(), relation_stats(r.get(2), r.get(3)));
        }
        match kind.as_str() {
            "m" => {
                out.materialized.insert(name.clone());
                out.views.push(name);
            }
            "v" => out.views.push(name),
            _ => out.tables.push(name),
        }
    }
    Ok(out)
}

/// The estimates as `stats_rows!` and `stats_bytes!` read them: a row estimate of `-1` is
/// none yet (never vacuumed or analyzed), which is not "no rows".
pub(crate) fn relation_stats(rows: Option<f64>, bytes: Option<i64>) -> RelationStats {
    RelationStats { rows: rows.filter(|r| *r >= 0.0).map(|r| r.round() as u64), bytes: bytes.map(|b| b.max(0) as u64) }
}

async fn load_catalog(client: &Client) -> Result<Catalog, DbError> {
    let schemas = load_schemas(client).await?;
    let sql = format!(
        "SELECT n.nspname, c.relname, c.relkind::text, a.attname, format_type(a.atttypid, a.atttypmod) \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.relkind IN ('r', 'p', 'f', 'v', 'm') AND {HIDDEN_SCHEMAS} \
         ORDER BY n.nspname, c.relname, a.attnum"
    );
    let rows = read(client, &sql, &[]).await?;
    let mut relations: Vec<Relation> = Vec::new();
    for r in rows {
        let (schema, name): (String, String) = (r.get(0), r.get(1));
        let is_new = relations.last().is_none_or(|l| l.schema != schema || l.name != name);
        if is_new {
            let kind: String = r.get(2);
            relations.push(Relation { schema, name, is_view: matches!(kind.as_str(), "v" | "m"), columns: Vec::new() });
        }
        if let (Some(col), Some(ty)) = (r.get::<_, Option<String>>(3), r.get::<_, Option<String>>(4))
            && let Some(rel) = relations.last_mut()
        {
            rel.columns.push(ColumnInfo { name: col, type_name: ty });
        }
    }
    Ok(Catalog { schemas, relations })
}

/// `server_version_num` (`120000` for 12.0).
async fn server_version(client: &Client) -> Result<u32, DbError> {
    let row = client
        .query_typed_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .map_err(|e| db_error(&e))?;
    Ok(row.get::<_, i32>(0).max(0) as u32)
}

/// Key metadata reads `attgenerated`, which PostgreSQL has since 12.
fn keys_supported(version: u32) -> Result<(), DbError> {
    if version < 120_000 { Err(DbError::ServerTooOld) } else { Ok(()) }
}

/// The tables and views outside the system schemas with their columns (and which of them are
/// generated: `attgenerated`, `attidentity`, PostgreSQL 12 or later), and their keys: the
/// primary key and the unique keys (from their indexes: full ones on plain columns, so unique
/// constraints and unique indexes alike), and the foreign keys. Every column of a composite key
/// is marked.
async fn load_keys(client: &Client, version: &Result<u32, DbError>) -> Result<KeyCatalog, DbError> {
    keys_supported(version.clone()?)?;
    let sql = format!(
        "SELECT c.oid, n.nspname, c.relname, a.attnum, a.attname, a.attgenerated::text, a.attidentity::text, \
         c.relkind IN ('v', 'm') \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.relkind IN ('r', 'p', 'f', 'v', 'm') AND {HIDDEN_SCHEMAS} \
         ORDER BY c.oid, a.attnum"
    );
    let rows = read(client, &sql, &[]).await?;
    // A table and its columns (number, name), in the order they come.
    struct Table {
        schema: String,
        name: String,
        columns: Vec<(i16, String)>,
        generated: Vec<(i16, Generated)>,
        view: bool,
    }
    let mut tables: std::collections::BTreeMap<u32, Table> = std::collections::BTreeMap::new();
    for r in rows {
        let t = tables.entry(r.get(0)).or_insert_with(|| Table {
            schema: r.get(1),
            name: r.get(2),
            columns: Vec::new(),
            generated: Vec::new(),
            view: r.get(7),
        });
        let g = match (r.get::<_, &str>(5), r.get::<_, &str>(6)) {
            // `s` stored, `v` virtual (PostgreSQL 18).
            ("s" | "v", _) => Generated::Expression,
            (_, "a") => Generated::IdentityAlways,
            _ => Generated::No,
        };
        if g != Generated::No {
            t.generated.push((r.get(3), g));
        }
        t.columns.push((r.get(3), r.get(4)));
    }
    let mut keys = KeyCatalog::default();
    for (id, t) in tables {
        keys.add_table(id, &t.schema, &t.name, t.columns);
        if t.view {
            keys.set_view(id);
        }
        for (column, g) in t.generated {
            keys.set_generated(id, column, g);
        }
    }
    let indexes = format!(
        "SELECT i.indrelid, i.indisprimary, i.indkey::int2[] FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE i.indisunique AND i.indpred IS NULL AND i.indexprs IS NULL AND {HIDDEN_SCHEMAS}"
    );
    for r in read(client, &indexes, &[]).await? {
        let kind = if r.get::<_, bool>(1) { KeyKind::Primary } else { KeyKind::Unique };
        keys.mark(r.get(0), &r.get::<_, Vec<i16>>(2), kind);
    }
    let foreign = format!(
        "SELECT con.conrelid, con.conkey FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE con.contype = 'f' AND {HIDDEN_SCHEMAS}"
    );
    for r in read(client, &foreign, &[]).await? {
        keys.mark(r.get(0), &r.get::<_, Vec<i16>>(1), KeyKind::Foreign);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_metadata_needs_postgresql_12() {
        assert_eq!(keys_supported(110_022), Err(DbError::ServerTooOld));
        assert_eq!(keys_supported(96_024), Err(DbError::ServerTooOld));
        assert_eq!(keys_supported(120_000), Ok(()));
        assert_eq!(keys_supported(170_011), Ok(()));
    }
}
