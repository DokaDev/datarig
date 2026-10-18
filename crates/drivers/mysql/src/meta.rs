//! The *meta* connection: the server's databases (MySQL's schemas: a database is the schema),
//! the tables and views of one with the estimates of their rows and size, and the columns of
//! every table for completion. Every read reports its failure: a list that could not be read is
//! never sent as an empty one.
//!
//! Every read is one statement in autocommit (`information_schema`, never `SHOW TABLE STATUS` nor
//! `ANALYZE`), so no transaction is ever open and a metadata lock a read takes ends with it. The
//! session was set up so that a read gives up after `lock_wait_timeout` (2 s) behind a metadata
//! lock another session holds or waits for (an `ALTER TABLE`, `LOCK TABLES … WRITE`), instead of
//! queueing behind it ([`DbError::Locked`]), a `SELECT` stops after `max_execution_time`, and
//! the session is read-only (`session::init_sql`). The estimates are the server's own
//! (`TABLE_ROWS`, `DATA_LENGTH + INDEX_LENGTH`, kept for `information_schema_stats_expiry`): the
//! explorer labels them approximate. A table's structure is one statement too
//! ([`structure`]).
//!
//! An object's DDL is the server's own `SHOW CREATE …` (after a lookup, for a name the user
//! typed): a read like the others, one statement each ([`ddl`]).

use crate::link::{Link, Next};
use crate::session::{Server, Tracked, quit};
use datarig_core::driver::structure::RelationStats;
use datarig_core::driver::{DbCommand, DbError, DbEvent, SchemaObjects};
use datarig_core::fault::Fault;
use datarig_core::sql::complete::{Catalog, ColumnInfo, Relation};
use datarig_core::sql::dialect::{Dialect, MySqlMode};
use datarig_core::sql::lexer::{Tok, lex_in};
use mysql_async::prelude::{FromRow, Queryable};
use mysql_async::{Conn, Row};
use tokio::sync::mpsc::UnboundedSender;

mod ddl;
mod keys;
mod structure;

/// The databases of the server's own, never listed.
const HIDDEN: &str = "('information_schema', 'mysql', 'performance_schema', 'sys')";

/// ER_LOCK_WAIT_TIMEOUT: a read gave up behind another session's lock.
const LOCK_WAIT_TIMEOUT: u16 = 1205;

/// A failed read as the UI hears it: a lock it gave up on is [`DbError::Locked`]. One the server
/// stopped after the session's `max_execution_time` keeps the server's message (MySQL's 3024
/// says that the time limit was exceeded).
fn read_error(e: &mysql_async::Error) -> DbError {
    match e {
        mysql_async::Error::Server(s) if s.code == LOCK_WAIT_TIMEOUT => DbError::Locked,
        e => crate::session::my_error(e),
    }
}

/// The rows of `sql` as `T`. A value of another type than the read expects (a server or a proxy
/// that answers otherwise) fails the read instead of the session.
async fn rows_of<T: FromRow>(conn: &mut Conn, sql: String) -> Result<Vec<T>, DbError> {
    let rows: Vec<Row> = conn.query(sql).await.map_err(|e| read_error(&e))?;
    rows.into_iter()
        .map(|r| mysql_async::from_row_opt(r).map_err(|e| DbError::Connection(Fault::other(e.to_string()))))
        .collect()
}

pub(crate) async fn meta_loop(mut conn: Conn, mut link: Link, events: UnboundedSender<DbEvent>, tracked: Tracked) {
    let server = Server { version: conn.server_version(), mariadb: conn.is_mariadb() };
    let mode = tracked.mode(&server);
    // The database the session opened in; its current one moves for the DDL reads ([`ddl`]).
    let home: Option<String> = conn.opts().db_name().filter(|d| !d.is_empty()).map(str::to_string);
    let mut current = home.clone();
    let Ok(schemas) = link.guard(None, load_schemas(&mut conn)).await else { return };
    let _ = events.send(DbEvent::Schemas(schemas));
    let Ok(catalog) = link.guard(None, load_catalog(&mut conn)).await else { return };
    let _ = events.send(DbEvent::Catalog(catalog));
    let Ok(keys) = link.guard(None, keys::load_keys(&mut conn)).await else { return };
    let _ = events.send(DbEvent::Keys(keys));
    loop {
        let cmd = match link.next().await {
            // No result is ever held here.
            Next::Command(DbCommand::ClosePortal { .. }) => continue,
            Next::Command(c) => c,
            Next::Closed => {
                quit(conn).await;
                return;
            }
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        let request = async {
            match cmd {
                DbCommand::LoadSchemas => DbEvent::Schemas(load_schemas(&mut conn).await),
                DbCommand::LoadObjects { schema } => {
                    let result = load_objects(&mut conn, &schema, mode).await;
                    DbEvent::Objects { schema, result }
                }
                DbCommand::LoadCatalog => DbEvent::Catalog(load_catalog(&mut conn).await),
                // A database is the schema: the server's databases are its schemas.
                DbCommand::LoadDatabases => DbEvent::Databases(load_schemas(&mut conn).await),
                DbCommand::LoadKeys => DbEvent::Keys(keys::load_keys(&mut conn).await),
                DbCommand::LoadStructure { schema, table } => {
                    let result = structure::load_structure(&mut conn, server, mode, &schema, &table).await;
                    DbEvent::Structure { schema, table, result: result.map(Box::new) }
                }
                DbCommand::LoadDdl { id, object } => {
                    let s = ddl::Session {
                        mode,
                        sql_mode: tracked.sql_mode.as_deref(),
                        home: home.as_deref(),
                        current: &mut current,
                    };
                    let result = ddl::load_ddl(&mut conn, s, &object).await;
                    DbEvent::Ddl { id, result }
                }
                // Statements run on a tab's query session, never on the shared metadata one.
                DbCommand::Execute { id, .. }
                | DbCommand::Resume { id, .. }
                | DbCommand::FetchMore { id }
                | DbCommand::ClosePortal { id }
                | DbCommand::EndTx { id, .. } => DbEvent::Failed { id, error: DbError::NotSupported, cancelled: false },
                DbCommand::Count { id, .. } => {
                    DbEvent::Counted { id, result: Err(DbError::NotSupported), snapshot: false }
                }
                DbCommand::CheckRepeat { id, .. } => DbEvent::RepeatChecked { id, result: Err(DbError::NotSupported) },
                DbCommand::Export { id, .. } => {
                    DbEvent::Exported { id, result: Err(DbError::NotSupported), cancelled: false }
                }
            }
        };
        let Ok(ev) = link.guard(None, request).await else { return };
        let lost = conn.is_disconnected();
        if events.send(ev).is_err() {
            return;
        }
        if lost {
            let _ = events.send(DbEvent::Lost { error: DbError::Closed });
            return;
        }
    }
}

/// The databases the user may see, by name, but the server's own.
async fn load_schemas(conn: &mut Conn) -> Result<Vec<String>, DbError> {
    let sql = format!(
        "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN {HIDDEN} ORDER BY SCHEMA_NAME"
    );
    rows_of(conn, sql).await
}

/// The tables and views of database `schema`, with the estimates of each table: one statement.
const SCHEMA_OBJECTS: &str = "SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS, DATA_LENGTH + INDEX_LENGTH \
     FROM information_schema.TABLES WHERE TABLE_SCHEMA = {schema} ORDER BY TABLE_NAME";

async fn load_objects(conn: &mut Conn, schema: &str, mode: MySqlMode) -> Result<SchemaObjects, DbError> {
    let sql = SCHEMA_OBJECTS.replace("{schema}", &Dialect::MySql(mode).quote_literal(schema));
    let rows: Vec<(String, String, Option<u64>, Option<u64>)> = rows_of(conn, sql).await?;
    let mut out = SchemaObjects::default();
    for (name, kind, rows, bytes) in rows {
        if kind == "BASE TABLE" {
            out.stats.insert(name.clone(), RelationStats { rows, bytes });
            out.tables.push(name);
        } else {
            out.views.push(name);
        }
    }
    Ok(out)
}

/// The tables and views of every database but the server's own, with their columns in order:
/// one statement.
async fn load_catalog(conn: &mut Conn) -> Result<Catalog, DbError> {
    let schemas = load_schemas(conn).await?;
    let sql = format!(
        "SELECT c.TABLE_SCHEMA, c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.COLUMN_TYPE \
         FROM information_schema.COLUMNS c JOIN information_schema.TABLES t \
         ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
         WHERE c.TABLE_SCHEMA NOT IN {HIDDEN} ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION"
    );
    let rows: Vec<(String, String, String, String, String)> = rows_of(conn, sql).await?;
    let mut relations: Vec<Relation> = Vec::new();
    for (schema, name, kind, column, ty) in rows {
        if relations.last().is_none_or(|l| l.schema != schema || l.name != name) {
            relations.push(Relation { schema, name, is_view: kind != "BASE TABLE", columns: Vec::new() });
        }
        if let Some(rel) = relations.last_mut() {
            let values = enum_values(&ty).map(Into::into);
            rel.columns.push(ColumnInfo { values, ..ColumnInfo::new(column, ty) });
        }
    }
    Ok(Catalog { schemas, relations })
}

/// The values of an `ENUM` column from its `COLUMN_TYPE` (`enum('a','it''s','b\\c')`: each
/// value a string literal as the server writes it, in its default mode), in their order;
/// `None` for another type, or one that does not read so.
fn enum_values(column_type: &str) -> Option<Vec<String>> {
    let list = column_type.get(..5).filter(|p| p.eq_ignore_ascii_case("enum("))?;
    let d = Dialect::MySql(MySqlMode::default());
    let body = column_type[list.len()..].strip_suffix(')')?;
    let mut values = Vec::new();
    let mut comma = true;
    for t in lex_in(body, d).iter().filter(|t| !t.is_trivia()) {
        match (comma, t.kind) {
            (true, Tok::Str) => values.push(d.unquote_literal(t.text(body))?),
            (false, Tok::Comma) => {}
            _ => return None,
        }
        comma = !comma;
    }
    (!values.is_empty() && !comma).then_some(values)
}

#[cfg(test)]
mod tests;
