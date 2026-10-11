//! An object's DDL (`DbCommand::LoadDdl`): the server's own `CREATE` statement, from `SHOW
//! CREATE TABLE` (a table or a view), `SHOW CREATE TRIGGER`, `SHOW CREATE PROCEDURE`, `SHOW CREATE
//! FUNCTION` or `SHOW CREATE EVENT`, sent as it is ([`DdlSource::Verbatim`]) and named
//! `database.object`.
//!
//! The text is ready to run again: a table's or a view's statement ends with `;`; a trigger's,
//! a routine's or an event's body may hold statements of its own, so it is written as
//! `mysqldump` writes it, between `DELIMITER ;;` and `DELIMITER ;`, ended with `;;`.
//!
//! MySQL has no DDL of an index of its own (an index is part of its table's `CREATE TABLE`), and
//! a trigger calls no function: the explorer asks for the table's DDL on an index, and never for
//! a trigger's function ([`DbError::NotSupported`] for either).
//!
//! A name the user typed (`:ddl name`) is looked up first, in one `information_schema` statement
//! over the tables and views, routines, triggers and events of the database it names (the
//! session's without one): a name of more than one of them is [`DbError::Ambiguous`], of none
//! [`DbError::NotFound`].
//!
//! Like every read of the metadata session each statement runs in autocommit, gives up after the
//! session's `lock_wait_timeout` behind another session's metadata lock ([`DbError::Locked`]) and
//! holds none once it has answered.

use datarig_core::driver::DbError;
use datarig_core::driver::ddl::{DdlObject, DdlSource, ObjectKind};
use datarig_core::fault::Fault;
use datarig_core::sql::dialect::{Dialect, MySqlMode};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Row};

/// The server's errors for an object that is not there: an unknown database, table, routine
/// (`ER_SP_DOES_NOT_EXIST`), trigger or event.
const NOT_THERE: [u16; 5] = [1049, 1146, 1305, 1360, 1539];

/// What `name` names in database `{schema}` (the session's, `DATABASE()`, without one): its kind,
/// database and name as the server has them, one row each.
const FIND_NAMED: &str = "SELECT TABLE_TYPE, TABLE_SCHEMA, TABLE_NAME FROM information_schema.TABLES \
     WHERE TABLE_SCHEMA = {schema} AND TABLE_NAME = {name} \
     UNION ALL SELECT ROUTINE_TYPE, ROUTINE_SCHEMA, ROUTINE_NAME FROM information_schema.ROUTINES \
     WHERE ROUTINE_SCHEMA = {schema} AND ROUTINE_NAME = {name} \
     UNION ALL SELECT 'TRIGGER', TRIGGER_SCHEMA, TRIGGER_NAME FROM information_schema.TRIGGERS \
     WHERE TRIGGER_SCHEMA = {schema} AND TRIGGER_NAME = {name} \
     UNION ALL SELECT 'EVENT', EVENT_SCHEMA, EVENT_NAME FROM information_schema.EVENTS \
     WHERE EVENT_SCHEMA = {schema} AND EVENT_NAME = {name}";

/// The DDL of `object`, its names written as `mode` reads them.
pub(crate) async fn load_ddl(conn: &mut Conn, mode: MySqlMode, object: &DdlObject) -> Result<DdlSource, DbError> {
    let (kind, schema, name) = match object {
        DdlObject::Relation { schema, name } => (ObjectKind::Table, schema.clone(), name.clone()),
        DdlObject::Trigger { schema, name, .. } => (ObjectKind::Trigger, schema.clone(), name.clone()),
        DdlObject::Named { name, schema } => find(conn, mode, name, schema.as_deref()).await?,
        DdlObject::Index { .. } | DdlObject::TriggerFunction { .. } => return Err(DbError::NotSupported),
    };
    show(conn, kind, &schema, &name).await
}

/// The one object `name` (`name` or `database.name`, each part bare or quoted) names in its
/// database, `schema` without one (the session's database when `None`): its kind, database and
/// name.
async fn find(
    conn: &mut Conn,
    mode: MySqlMode,
    name: &str,
    schema: Option<&str>,
) -> Result<(ObjectKind, String, String), DbError> {
    let d = Dialect::MySql(mode);
    let (database, name) = match name_parts(name, d).as_deref() {
        Some([name]) => (schema.map(|s| d.quote_literal(s)), d.quote_literal(name)),
        Some([database, name]) => (Some(d.quote_literal(database)), d.quote_literal(name)),
        _ => return Err(DbError::NotFound),
    };
    let sql = FIND_NAMED.replace("{schema}", database.as_deref().unwrap_or("DATABASE()")).replace("{name}", &name);
    let rows: Vec<(String, String, String)> = super::rows_of(conn, sql).await?;
    let mut kinds: Vec<ObjectKind> = rows.iter().map(|(k, ..)| kind_of(k)).collect();
    kinds.sort();
    kinds.dedup();
    match (rows.len(), rows.into_iter().next()) {
        (1, Some((kind, schema, name))) => Ok((kind_of(&kind), schema, name)),
        (0, _) => Err(DbError::NotFound),
        _ => Err(DbError::Ambiguous(kinds)),
    }
}

/// The kind of a row of [`FIND_NAMED`]: a table's `TABLE_TYPE` (anything but a view is read as
/// a table, as `SHOW CREATE TABLE` shows it), a routine's `ROUTINE_TYPE`, or the trigger's and
/// event's own.
fn kind_of(kind: &str) -> ObjectKind {
    match kind {
        "VIEW" | "SYSTEM VIEW" => ObjectKind::View,
        "PROCEDURE" => ObjectKind::Procedure,
        "FUNCTION" => ObjectKind::Function,
        "TRIGGER" => ObjectKind::Trigger,
        "EVENT" => ObjectKind::Event,
        _ => ObjectKind::Table,
    }
}

/// The parts of a name as SQL writes it (`shop.users`, `` `my db`.`a.b` ``), each as the name it
/// stands for; `None` when it is not one.
fn name_parts(name: &str, d: Dialect) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut rest = name.trim();
    loop {
        let after = match d.opening_quote(rest) {
            Some(q) => {
                // The closing quote: the first that is not doubled.
                let mut at = q.len_utf8();
                let end = loop {
                    let i = at + rest[at..].find(q)?;
                    if rest[i + q.len_utf8()..].starts_with(q) {
                        at = i + 2 * q.len_utf8();
                    } else {
                        break i + q.len_utf8();
                    }
                };
                parts.push(d.unquote(&rest[..end])?);
                &rest[end..]
            }
            None => {
                let end = rest.find('.').unwrap_or(rest.len());
                let part = rest[..end].trim_end();
                if part.is_empty() || part.contains(|c: char| c.is_whitespace() || matches!(c, '`' | '"' | '\'')) {
                    return None;
                }
                parts.push(d.fold(part));
                &rest[end..]
            }
        };
        let after = after.trim_start();
        if after.is_empty() {
            return Some(parts);
        }
        rest = after.strip_prefix('.')?.trim_start();
    }
}

/// `SHOW CREATE …` of object `schema.name` of `kind`: its text, ready to run again.
async fn show(conn: &mut Conn, kind: ObjectKind, schema: &str, name: &str) -> Result<DdlSource, DbError> {
    let what = match kind {
        // A view's too: the server answers with the view's statement.
        ObjectKind::Table | ObjectKind::View => "TABLE",
        ObjectKind::Procedure => "PROCEDURE",
        ObjectKind::Function => "FUNCTION",
        ObjectKind::Trigger => "TRIGGER",
        ObjectKind::Event => "EVENT",
    };
    let d = Dialect::MySql(MySqlMode::default());
    let sql = format!("SHOW CREATE {what} {}.{}", d.force_quote_ident(schema), d.force_quote_ident(name));
    let rows: Vec<Row> = conn.query(sql).await.map_err(|e| match &e {
        mysql_async::Error::Server(s) if NOT_THERE.contains(&s.code) => DbError::NotFound,
        e => super::read_error(e),
    })?;
    let row = rows.first().ok_or(DbError::NotFound)?;
    let text_of = |i: usize| row.get_opt::<Option<String>, _>(i).and_then(Result::ok);
    let column = row.columns_ref().iter().position(|c| {
        matches!(
            &*c.name_str(),
            "Create Table"
                | "Create View"
                | "SQL Original Statement"
                | "Create Procedure"
                | "Create Function"
                | "Create Event"
        )
    });
    // An answer of another shape than the server's (a proxy's) fails the read.
    let unexpected = || DbError::Connection(Fault::other(format!("SHOW CREATE {what}: no statement in the answer")));
    let own = text_of(0).flatten().ok_or_else(unexpected)?;
    // A routine's statement is NULL to a user who may run it but not see it.
    let text = text_of(column.ok_or_else(unexpected)?).ok_or_else(unexpected)?.ok_or(DbError::DefinitionHidden)?;
    Ok(DdlSource::Verbatim { name: format!("{schema}.{own}"), text: runnable(kind, &text) })
}

/// `text`, the server's statement for an object of `kind`, as text to run again: a table's or a
/// view's ended with `;`, any other's (a body may hold statements of its own) between `DELIMITER`
/// lines as `mysqldump` writes it.
fn runnable(kind: ObjectKind, text: &str) -> String {
    match kind {
        ObjectKind::Table | ObjectKind::View => format!("{text};\n"),
        _ => format!("DELIMITER ;;\n{text} ;;\nDELIMITER ;\n"),
    }
}

#[cfg(test)]
mod tests;
