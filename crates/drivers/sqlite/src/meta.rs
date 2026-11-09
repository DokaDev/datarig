//! The metadata session's answers: the schema tree and the completion catalog, and the `CREATE`
//! text of an object. SQLite has one schema, `main`; the catalog is read from `sqlite_schema`
//! (the engine's own table of objects) and `pragma_table_info`.

use crate::session::Ctx;
use datarig_core::driver::ddl::{DdlObject, DdlSource};
use datarig_core::driver::{DbCommand, DbError, DbEvent, SchemaObjects};
use datarig_core::sql::complete::{Catalog, ColumnInfo, Relation};
use datarig_core::sql::dialect::Dialect;
use rusqlite::{Connection, OptionalExtension};

/// The only schema of a SQLite file.
pub(crate) const SCHEMA: &str = "main";

/// The objects the user sees: tables and views, not SQLite's own `sqlite_*` tables.
const OBJECTS: &str = "SELECT name, type FROM sqlite_schema \
    WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name";

pub(crate) fn answer(conn: &Connection, ctx: &Ctx, cmd: DbCommand) {
    let event = match cmd {
        DbCommand::LoadSchemas => DbEvent::Schemas(Ok(vec![SCHEMA.to_string()])),
        DbCommand::LoadObjects { schema } => {
            let result = objects(conn, &schema);
            DbEvent::Objects { schema, result }
        }
        DbCommand::LoadCatalog => DbEvent::Catalog(catalog(conn)),
        DbCommand::LoadDdl { id, object } => DbEvent::Ddl { id, result: ddl(conn, &object) },
        other => other.not_supported(),
    };
    ctx.send(event);
}

fn objects(conn: &Connection, schema: &str) -> Result<SchemaObjects, DbError> {
    if schema != SCHEMA {
        return Err(DbError::NotFound);
    }
    let mut out = SchemaObjects::default();
    for (name, kind) in listed(conn)? {
        if kind == "view" {
            out.views.push(name);
        } else {
            out.tables.push(name);
        }
    }
    Ok(out)
}

fn catalog(conn: &Connection) -> Result<Catalog, DbError> {
    let mut relations = Vec::new();
    for (name, kind) in listed(conn)? {
        let columns = columns_of(conn, &name)?;
        relations.push(Relation { schema: SCHEMA.to_string(), name, is_view: kind == "view", columns });
    }
    Ok(Catalog { schemas: vec![SCHEMA.to_string()], relations })
}

/// The tables and views of the file, by name, with their kind (`table` or `view`).
fn listed(conn: &Connection) -> Result<Vec<(String, String)>, DbError> {
    let mut stmt = conn.prepare(OBJECTS).map_err(crate::open::server)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(crate::open::server)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(crate::open::server)
}

fn columns_of(conn: &Connection, table: &str) -> Result<Vec<ColumnInfo>, DbError> {
    let mut stmt =
        conn.prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid").map_err(crate::open::server)?;
    let rows = stmt
        .query_map([table], |row| Ok(ColumnInfo { name: row.get(0)?, type_name: row.get(1)?, values: None }))
        .map_err(crate::open::server)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(crate::open::server)
}

/// The `CREATE` text of a table, view or index as the file stores it.
fn ddl(conn: &Connection, object: &DdlObject) -> Result<DdlSource, DbError> {
    let (schema, name, kinds): (&str, &str, &[&str]) = match object {
        DdlObject::Relation { schema, name } => (schema, name, &["table", "view"]),
        DdlObject::Index { schema, name } => (schema, name, &["index"]),
        _ => return Err(DbError::NotSupported),
    };
    if schema != SCHEMA {
        return Err(DbError::NotFound);
    }
    let kind_list = kinds.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(", ");
    let sql = format!("SELECT sql FROM sqlite_schema WHERE name = ?1 AND type IN ({kind_list})");
    let text: Option<Option<String>> =
        conn.query_row(&sql, [name], |row| row.get(0)).optional().map_err(crate::open::server)?;
    // An automatic index (`sqlite_autoindex_…`) has no text of its own: it is not an object the
    // user wrote.
    let text = text.flatten().ok_or(DbError::NotFound)?;
    Ok(DdlSource::Verbatim {
        name: name.to_string(),
        text: format!("{text};"),
        database: None,
        written_in: Some(Dialect::Sqlite),
    })
}
