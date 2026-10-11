//! An object's DDL (`DbCommand::LoadDdl`): the server's own `CREATE` statement, from `SHOW
//! CREATE TABLE` (a table or a view), `SHOW CREATE TRIGGER`, `SHOW CREATE PROCEDURE`, `SHOW CREATE
//! FUNCTION` or `SHOW CREATE EVENT`, sent as it is ([`DdlSource::Verbatim`]) and named
//! `database.object`.
//!
//! The text is ready to run again: a table's or a view's statement ends with `;`; a trigger's,
//! a routine's or an event's body may hold statements of its own, so it is written as
//! `mysqldump` writes it, between `DELIMITER ;;` and `DELIMITER ;`, ended with `;;` on a line of
//! its own (the server keeps a comment at the end of a body, which would swallow an end on its
//! line). Such an object keeps the `sql_mode` it was made under, which changes how its body
//! reads: one that is not the metadata session's is said in a comment line first.
//!
//! MySQL has no DDL of an index of its own (an index is part of its table's `CREATE TABLE`), and
//! a trigger calls no function: the explorer asks for the table's DDL on an index, and never for
//! a trigger's function ([`DbError::NotSupported`] for either).
//!
//! A name the user typed (`:ddl [kind] name`, the name bare or quoted, `name` or
//! `database.name`) is looked up first, in one `information_schema` statement over the tables
//! and views, routines, triggers and events of the database it names (the session's without
//! one): a name of more than one of them is [`DbError::Ambiguous`] unless a kind word picks one,
//! of none [`DbError::NotFound`].
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
/// database and name as the server has them, one row each; `{none}` the row that says the session
/// has no database to look in.
const FIND_NAMED: &str = "{none}SELECT TABLE_TYPE, TABLE_SCHEMA, TABLE_NAME FROM information_schema.TABLES \
     WHERE TABLE_SCHEMA = {schema} AND TABLE_NAME = {name} \
     UNION ALL SELECT ROUTINE_TYPE, ROUTINE_SCHEMA, ROUTINE_NAME FROM information_schema.ROUTINES \
     WHERE ROUTINE_SCHEMA = {schema} AND ROUTINE_NAME = {name} \
     UNION ALL SELECT 'TRIGGER', TRIGGER_SCHEMA, TRIGGER_NAME FROM information_schema.TRIGGERS \
     WHERE TRIGGER_SCHEMA = {schema} AND TRIGGER_NAME = {name} \
     UNION ALL SELECT 'EVENT', EVENT_SCHEMA, EVENT_NAME FROM information_schema.EVENTS \
     WHERE EVENT_SCHEMA = {schema} AND EVENT_NAME = {name}";

/// The kind [`FIND_NAMED`] gives when the session has no database and the name names none.
const NO_DATABASE: &str = "NO DATABASE";

/// The DDL of `object`, its names written as `mode` reads them; `sql_mode` is the metadata
/// session's (`None`: not known).
pub(crate) async fn load_ddl(
    conn: &mut Conn,
    mode: MySqlMode,
    sql_mode: Option<&str>,
    object: &DdlObject,
) -> Result<DdlSource, DbError> {
    let (kind, schema, name) = match object {
        DdlObject::Relation { schema, name } => (ObjectKind::Table, schema.clone(), name.clone()),
        DdlObject::Trigger { schema, name, .. } => (ObjectKind::Trigger, schema.clone(), name.clone()),
        DdlObject::Named { name, schema } => find(conn, mode, name, schema.as_deref()).await?,
        DdlObject::Index { .. } | DdlObject::TriggerFunction { .. } => return Err(DbError::NotSupported),
    };
    show(conn, kind, &schema, &name, sql_mode).await
}

/// What a typed name asks for: the kind its first word names (`procedure p`), if any, then its
/// database (`None`: the one it is looked up in) and name. A name of more parts, or no name, is
/// [`DbError::NotAName`]; one with a character above U+FFFF (which no MySQL name has: its names
/// are `utf8mb3`) names nothing ([`DbError::NotFound`]) and is not looked up.
fn typed_name(typed: &str, d: Dialect) -> Result<(Option<ObjectKind>, Option<String>, String), DbError> {
    let typed = typed.trim();
    let (kind, rest) = match typed.split_once(char::is_whitespace) {
        Some((word, rest)) => match kind_word(word) {
            Some(kind) => (Some(kind), rest),
            None => (None, typed),
        },
        None => (None, typed),
    };
    let mut parts = name_parts(rest, d).ok_or(DbError::NotAName)?;
    if parts.iter().any(|p| p.chars().any(|c| u32::from(c) > 0xFFFF)) {
        return Err(DbError::NotFound);
    }
    let name = parts.pop().ok_or(DbError::NotAName)?;
    match parts.pop() {
        None => Ok((kind, None, name)),
        Some(database) if parts.is_empty() => Ok((kind, Some(database), name)),
        Some(_) => Err(DbError::NotAName),
    }
}

/// The kind a word before a typed name asks for, whatever its case.
fn kind_word(word: &str) -> Option<ObjectKind> {
    match word.to_ascii_lowercase().as_str() {
        "table" => Some(ObjectKind::Table),
        "view" => Some(ObjectKind::View),
        "procedure" => Some(ObjectKind::Procedure),
        "function" => Some(ObjectKind::Function),
        "trigger" => Some(ObjectKind::Trigger),
        "event" => Some(ObjectKind::Event),
        _ => None,
    }
}

/// The one object `typed` ([`typed_name`]) names in its database, `schema` without one (the
/// session's database when `None`): its kind, database and name.
async fn find(
    conn: &mut Conn,
    mode: MySqlMode,
    typed: &str,
    schema: Option<&str>,
) -> Result<(ObjectKind, String, String), DbError> {
    let d = Dialect::MySql(mode);
    let (wanted, database, name) = typed_name(typed, d)?;
    let database = database.as_deref().or(schema).map(|s| d.quote_literal(s));
    let none = match database {
        Some(_) => String::new(),
        None => format!("SELECT '{NO_DATABASE}', '', '' FROM DUAL WHERE DATABASE() IS NULL UNION ALL "),
    };
    let sql = FIND_NAMED
        .replace("{none}", &none)
        .replace("{schema}", database.as_deref().unwrap_or("DATABASE()"))
        .replace("{name}", &d.quote_literal(&name));
    let rows: Vec<(String, String, String)> = super::rows_of(conn, sql).await?;
    if rows.iter().any(|(k, ..)| k == NO_DATABASE) {
        return Err(DbError::NoDatabase);
    }
    let mut found = Vec::new();
    for (kind, schema, name) in rows {
        let kind = match (kind_of(&kind), wanted) {
            (Ok(k), _) => k,
            // Not of the kind asked for.
            (Err(_), Some(_)) => continue,
            (Err(e), None) => return Err(e),
        };
        if wanted.is_none_or(|w| w == kind) {
            found.push((kind, schema, name));
        }
    }
    let mut kinds: Vec<ObjectKind> = found.iter().map(|(k, ..)| *k).collect();
    kinds.sort();
    kinds.dedup();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(DbError::NotFound),
        _ => Err(DbError::Ambiguous(kinds)),
    }
}

/// The kind of a row of [`FIND_NAMED`]: a table's `TABLE_TYPE` (a base table, MariaDB's
/// system-versioned table and sequence are tables to `SHOW CREATE TABLE`), a routine's
/// `ROUTINE_TYPE`, or the trigger's and event's own. Any other (MariaDB's `PACKAGE`) is
/// [`DbError::KindUnsupported`]: its DDL is not read.
fn kind_of(kind: &str) -> Result<ObjectKind, DbError> {
    match kind {
        "BASE TABLE" | "SYSTEM VERSIONED" | "SEQUENCE" => Ok(ObjectKind::Table),
        "VIEW" | "SYSTEM VIEW" => Ok(ObjectKind::View),
        "PROCEDURE" => Ok(ObjectKind::Procedure),
        "FUNCTION" => Ok(ObjectKind::Function),
        "TRIGGER" => Ok(ObjectKind::Trigger),
        "EVENT" => Ok(ObjectKind::Event),
        other => Err(DbError::KindUnsupported(other.to_string())),
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

/// `SHOW CREATE …` of object `schema.name` of `kind`: its text, ready to run again, with the
/// `sql_mode` it was made under when that is not `session_mode` ([`runnable`]).
async fn show(
    conn: &mut Conn,
    kind: ObjectKind,
    schema: &str,
    name: &str,
    session_mode: Option<&str>,
) -> Result<DdlSource, DbError> {
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
    let own_mode = match row.columns_ref().iter().position(|c| c.name_str() == "sql_mode") {
        Some(i) => text_of(i).flatten(),
        None => None,
    };
    let mode = own_mode.filter(|m| session_mode.is_none_or(|s| !same_mode(m, s)));
    Ok(DdlSource::Verbatim { name: format!("{schema}.{own}"), text: runnable(kind, &text, mode.as_deref()) })
}

/// Whether two `sql_mode` values set the same flags, in any order.
fn same_mode(a: &str, b: &str) -> bool {
    let flags = |m: &str| {
        let mut f: Vec<String> =
            m.split(',').map(|x| x.trim().to_ascii_uppercase()).filter(|x| !x.is_empty()).collect();
        f.sort();
        f
    };
    flags(a) == flags(b)
}

/// `text`, the server's statement for an object of `kind`, as text to run again: a table's or a
/// view's ended with `;`, any other's (a body may hold statements of its own, and end in a
/// comment) between `DELIMITER` lines as `mysqldump` writes it, its end on a line of its own.
/// `sql_mode`: the mode the object was made under, said first, when it is not the session's.
fn runnable(kind: ObjectKind, text: &str, sql_mode: Option<&str>) -> String {
    let mode = match sql_mode {
        Some("") => "-- sql_mode: ''\n".to_string(),
        Some(m) => format!("-- sql_mode: {m}\n"),
        None => String::new(),
    };
    match kind {
        ObjectKind::Table | ObjectKind::View => format!("{mode}{text};\n"),
        _ => format!("{mode}DELIMITER ;;\n{text}\n;;\nDELIMITER ;\n"),
    }
}

#[cfg(test)]
mod tests;
