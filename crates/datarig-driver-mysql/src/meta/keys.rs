//! The key metadata of every database the user may see (`DbEvent::Keys`): which columns are
//! part of a primary key, a foreign key or a unique key, which are generated, which tables are
//! views, and how the server compares names. One `information_schema` statement (its parts
//! joined by `UNION ALL`), read at connect after the catalog and again on `DbCommand::LoadKeys`.
//! Like every read of the metadata session it reads the data dictionary only (no `TABLE_ROWS`,
//! no `CARDINALITY`: nothing that opens a table) and holds no lock once it has answered.
//!
//! Tables get ids of their own (MySQL has no table numbers); a column's number is its
//! `ORDINAL_POSITION`. A unique key marks its columns only when it is on whole columns: a key on
//! a prefix (`b(10)`) or an expression does not make a column unique. A `VIRTUAL` or `STORED`
//! generated column takes no value ([`Generated::Expression`]); an `AUTO_INCREMENT` one does
//! ([`Generated::No`]).
//!
//! Names compare as the server compares them ([`NameRule::MySql`]): a column's never minds the
//! case, a database's or a table's does unless `lower_case_table_names` is 1 or 2. With 2 (macOS,
//! Windows) the result metadata and `information_schema` may write a name in other cases; that
//! setting is not tested. `COLUMNS` lists only the columns the user has a privilege on.

use super::HIDDEN;
use datarig_core::driver::DbError;
use datarig_core::driver::keys::{Generated, KeyCatalog, KeyKind, NameRule};
use mysql_async::Conn;
use std::collections::BTreeMap;

/// One row of the statement: what it is (`L` the server's `lower_case_table_names`, `C` a
/// column, `P` a primary key column, `F` a foreign key column, `U` a unique key part), its
/// database and table, and three values by kind (`C`: the position, the name, `EXTRA`; `P`/`F`:
/// -, the column; `U`: the key, the column or NULL for an expression, the prefix length), and
/// the table's type (`C`).
type RawRow = (String, String, String, Option<String>, Option<String>, Option<String>, Option<String>);

/// The statement.
fn statement() -> String {
    format!(
        "SELECT 'L', '', '', CAST(@@lower_case_table_names AS CHAR), NULL, NULL, NULL \
         UNION ALL SELECT 'C', c.TABLE_SCHEMA, c.TABLE_NAME, CAST(c.ORDINAL_POSITION AS CHAR), c.COLUMN_NAME, \
           c.EXTRA, t.TABLE_TYPE \
         FROM information_schema.COLUMNS c JOIN information_schema.TABLES t \
           ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
         WHERE c.TABLE_SCHEMA NOT IN {HIDDEN} \
         UNION ALL SELECT IF(k.CONSTRAINT_NAME = 'PRIMARY', 'P', 'F'), k.TABLE_SCHEMA, k.TABLE_NAME, NULL, \
           k.COLUMN_NAME, NULL, NULL \
         FROM information_schema.KEY_COLUMN_USAGE k \
         WHERE k.TABLE_SCHEMA NOT IN {HIDDEN} \
           AND (k.CONSTRAINT_NAME = 'PRIMARY' OR k.REFERENCED_TABLE_NAME IS NOT NULL) \
         UNION ALL SELECT 'U', x.TABLE_SCHEMA, x.TABLE_NAME, x.INDEX_NAME, x.COLUMN_NAME, \
           CAST(x.SUB_PART AS CHAR), NULL \
         FROM information_schema.STATISTICS x \
         WHERE x.TABLE_SCHEMA NOT IN {HIDDEN} AND x.NON_UNIQUE = 0 AND x.INDEX_NAME <> 'PRIMARY'"
    )
}

/// Read the key metadata in one round trip.
pub(crate) async fn load_keys(conn: &mut Conn) -> Result<KeyCatalog, DbError> {
    let rows: Vec<RawRow> = super::rows_of(conn, statement()).await?;
    catalog(rows)
}

/// A column of a table being built: its name, `EXTRA`.
struct Col {
    name: String,
    extra: String,
}

/// A table being built: whether it is a view, its columns by position.
#[derive(Default)]
struct Table {
    view: bool,
    columns: BTreeMap<i16, Col>,
}

/// The catalog the statement's rows describe. A row the model has no words for (a kind, a
/// position, a `lower_case_table_names` it cannot read) fails the read: the keys are then not
/// known, never taken for none.
pub(crate) fn catalog(rows: Vec<RawRow>) -> Result<KeyCatalog, DbError> {
    let bad = |what: &str| DbError::Server(format!("key metadata: {what}"));
    let mut rule = None;
    let mut tables: BTreeMap<(String, String), Table> = BTreeMap::new();
    // (database, table, column, kind) of each key column; unique key parts by key.
    let mut keys: Vec<(String, String, String, KeyKind)> = Vec::new();
    let mut unique: BTreeMap<(String, String, String), Vec<Option<String>>> = BTreeMap::new();
    for (kind, schema, table, a, b, c, d) in rows {
        match kind.as_str() {
            "L" => {
                rule = Some(match a.as_deref() {
                    Some("0") => NameRule::MySql { tables_ignore_case: false },
                    Some("1" | "2") => NameRule::MySql { tables_ignore_case: true },
                    other => return Err(bad(&format!("lower_case_table_names {other:?}"))),
                })
            }
            "C" => {
                let position = a.as_deref().and_then(|p| p.parse::<i16>().ok());
                let (Some(position), Some(name)) = (position, b) else {
                    return Err(bad(&format!("a column of {schema}.{table}")));
                };
                let t = tables.entry((schema, table)).or_default();
                t.view = d.as_deref() == Some("VIEW");
                t.columns.insert(position, Col { name, extra: c.unwrap_or_default() });
            }
            "P" | "F" => {
                let Some(column) = b else { return Err(bad(&format!("a key column of {schema}.{table}"))) };
                let k = if kind == "P" { KeyKind::Primary } else { KeyKind::Foreign };
                keys.push((schema, table, column, k));
            }
            "U" => {
                let Some(index) = a else { return Err(bad(&format!("a unique key of {schema}.{table}"))) };
                // A prefix part (a length) or an expression (no column) is no whole column.
                let part = if c.is_some() { None } else { b };
                unique.entry((schema, table, index)).or_default().push(part);
            }
            other => return Err(bad(&format!("a row of kind {other}"))),
        }
    }
    let rule = rule.ok_or_else(|| bad("no lower_case_table_names"))?;
    for ((schema, table, _), parts) in unique {
        // Every part a whole column, or the key marks none of them.
        if let Some(columns) = parts.into_iter().collect::<Option<Vec<String>>>() {
            keys.extend(columns.into_iter().map(|c| (schema.clone(), table.clone(), c, KeyKind::Unique)));
        }
    }
    let mut out = KeyCatalog::default();
    out.set_names(rule);
    let mut ids: BTreeMap<(String, String), u32> = BTreeMap::new();
    for (id, ((schema, name), t)) in (1u32..).zip(tables) {
        out.add_table(id, &schema, &name, t.columns.iter().map(|(n, c)| (*n, c.name.clone())));
        if t.view {
            out.set_view(id);
        }
        for (n, c) in &t.columns {
            // `VIRTUAL GENERATED`, `STORED GENERATED` (not `DEFAULT_GENERATED`, a default).
            if c.extra.split_whitespace().any(|w| w.eq_ignore_ascii_case("GENERATED")) {
                out.set_generated(id, *n, Generated::Expression);
            }
        }
        ids.insert((schema, name), id);
    }
    for (schema, table, column, kind) in keys {
        let Some(id) = ids.get(&(schema, table)).copied() else { continue };
        let number = out.table(id).and_then(|t| t.columns.iter().find(|(_, c)| c.name == column).map(|(n, _)| *n));
        if let Some(n) = number {
            out.mark(id, &[n], kind);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
