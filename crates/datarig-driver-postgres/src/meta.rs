//! The *meta* connection: schemas, relations and columns for the explorer tree and completion,
//! the key constraints of the tables (`Capabilities::key_metadata`), and one table's structure
//! when its node opens (`Capabilities::structure`, [`structure`]). Every read reports its
//! failure: a catalog or key list that could not be read is never sent as an empty one.
//!
//! Every read is one unnamed statement, parsed, bound and run in one round trip
//! (`Client::query_typed`): nothing depends on a statement prepared in an earlier transaction,
//! which a pooler in transaction mode may have run on another server connection.

use crate::connect::db_error;
use crate::link::{Link, Next};
use datarig_core::driver::keys::{Generated, KeyCatalog, KeyKind};
use datarig_core::driver::{DbCommand, DbError, DbEvent, SchemaObjects};
use datarig_core::sql::complete::{Catalog, ColumnInfo, Relation};
use tokio::sync::mpsc::UnboundedSender;
use tokio_postgres::Client;
use tokio_postgres::types::Type;

mod structure;

const HIDDEN_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";

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
    let rows = client.query_typed(&sql, &[]).await.map_err(|e| db_error(&e))?;
    Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
}

/// The databases the user may connect to (templates and databases that refuse connections
/// left out), by name.
async fn load_databases(client: &Client) -> Result<Vec<String>, DbError> {
    let rows = client
        .query_typed(
            "SELECT d.datname::text FROM pg_catalog.pg_database d \
             WHERE d.datallowconn AND NOT d.datistemplate \
             AND pg_catalog.has_database_privilege(d.datname, 'CONNECT') ORDER BY 1",
            &[],
        )
        .await
        .map_err(|e| db_error(&e))?;
    Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
}

async fn load_objects(client: &Client, schema: &str) -> Result<SchemaObjects, DbError> {
    let rows = client
        .query_typed(
            "SELECT c.relname, c.relkind::text FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'f', 'v', 'm') ORDER BY c.relname",
            &[(&schema, Type::TEXT)],
        )
        .await
        .map_err(|e| db_error(&e))?;
    let mut out = SchemaObjects::default();
    for r in rows {
        let name: String = r.get(0);
        match r.get::<_, String>(1).as_str() {
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

async fn load_catalog(client: &Client) -> Result<Catalog, DbError> {
    let schemas = load_schemas(client).await?;
    let sql = format!(
        "SELECT n.nspname, c.relname, c.relkind::text, a.attname, format_type(a.atttypid, a.atttypmod) \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.relkind IN ('r', 'p', 'f', 'v', 'm') AND {HIDDEN_SCHEMAS} \
         ORDER BY n.nspname, c.relname, a.attnum"
    );
    let rows = client.query_typed(&sql, &[]).await.map_err(|e| db_error(&e))?;
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
    let rows = client.query_typed(&sql, &[]).await.map_err(|e| db_error(&e))?;
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
    for r in client.query_typed(&indexes, &[]).await.map_err(|e| db_error(&e))? {
        let kind = if r.get::<_, bool>(1) { KeyKind::Primary } else { KeyKind::Unique };
        keys.mark(r.get(0), &r.get::<_, Vec<i16>>(2), kind);
    }
    let foreign = format!(
        "SELECT con.conrelid, con.conkey FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE con.contype = 'f' AND {HIDDEN_SCHEMAS}"
    );
    for r in client.query_typed(&foreign, &[]).await.map_err(|e| db_error(&e))? {
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
