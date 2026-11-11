//! The keys of one table that make an order total (`DbCommand::LoadTableKeys`): what a sorted
//! table tab appends as its tie-breaker. Asked for by the app on its own (a table tab opening),
//! so it must never disturb another session: one `information_schema` statement on `TABLES`,
//! `STATISTICS` (no `CARDINALITY`) and `COLUMNS`, which the data dictionary answers without
//! opening the table, in autocommit like every read of the metadata session.
//!
//! A unique key counts when every part is a whole column (no expression, no prefix); the
//! primary key always, another one when all its columns are `NOT NULL`.
//!
//! With them come the table's columns in its order ([`OrderKeys::columns`]): `COLUMNS` (type,
//! `NOT NULL`, an `INVISIBLE` column, which `SELECT *` leaves out) and `KEY_COLUMN_USAGE` (the
//! columns of its foreign keys). `COLUMNS` lists only the columns the user has a privilege on.

use datarig_core::driver::DbError;
use datarig_core::driver::keys::{CatalogColumn, OrderKeys};
use datarig_core::sql::dialect::{Dialect, MySqlMode};
use mysql_async::Conn;
use std::collections::BTreeMap;

const SQL: &str = "SELECT 'T', NULL, NULL, NULL, NULL, NULL
FROM information_schema.TABLES t WHERE t.TABLE_SCHEMA = {schema} AND t.TABLE_NAME = {table}
UNION ALL
SELECT 'K', x.INDEX_NAME, CAST(x.SEQ_IN_INDEX AS CHAR), x.COLUMN_NAME, CAST(x.SUB_PART AS CHAR), c.IS_NULLABLE
FROM information_schema.STATISTICS x
LEFT JOIN information_schema.COLUMNS c
  ON c.TABLE_SCHEMA = x.TABLE_SCHEMA AND c.TABLE_NAME = x.TABLE_NAME AND c.COLUMN_NAME = x.COLUMN_NAME
WHERE x.TABLE_SCHEMA = {schema} AND x.TABLE_NAME = {table} AND x.NON_UNIQUE = 0
UNION ALL
SELECT 'C', c.COLUMN_NAME, CAST(c.ORDINAL_POSITION AS CHAR), c.COLUMN_TYPE, c.IS_NULLABLE, c.EXTRA
FROM information_schema.COLUMNS c WHERE c.TABLE_SCHEMA = {schema} AND c.TABLE_NAME = {table}
UNION ALL
SELECT 'F', NULL, NULL, u.COLUMN_NAME, NULL, NULL
FROM information_schema.KEY_COLUMN_USAGE u
WHERE u.TABLE_SCHEMA = {schema} AND u.TABLE_NAME = {table} AND u.REFERENCED_TABLE_NAME IS NOT NULL";

/// One row: `T` (the table is there); `K` with a unique key's name, the part's place, its
/// column (none for an expression), its prefix length and whether the column takes NULL; `C`
/// with a column's name, place, type, whether it takes NULL and its extra (`INVISIBLE`, …); or
/// `F` with a column of a foreign key in the fourth place.
type RawRow = (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);

/// The statement for `schema.table`, the names written as `mode` reads literals.
pub(crate) fn statement(mode: MySqlMode, schema: &str, table: &str) -> String {
    let d = Dialect::MySql(mode);
    super::fill(SQL, &[("schema", &d.quote_literal(schema)), ("table", &d.quote_literal(table))])
}

/// The keys the rows describe. No `T` row: there is no such table to the user
/// ([`DbError::NotFound`]), never "no key".
pub(crate) fn keys_of(rows: Vec<RawRow>) -> Result<OrderKeys, DbError> {
    if !rows.iter().any(|r| r.0 == "T") {
        return Err(DbError::NotFound);
    }
    // Per key: (place, column, prefix, nullable) of each part.
    type Part = (u32, Option<String>, Option<String>, Option<String>);
    let mut parts: BTreeMap<String, Vec<Part>> = BTreeMap::new();
    let mut rows_c = Vec::new();
    let mut foreign = Vec::new();
    for (kind, index, seq, column, prefix, nullable) in rows {
        match kind.as_str() {
            "C" => {
                rows_c.push((index, seq, column, prefix, nullable));
                continue;
            }
            "F" => {
                foreign.extend(column);
                continue;
            }
            "K" => {}
            _ => continue,
        }
        let (Some(index), Some(seq)) = (index, seq.and_then(|s| s.parse::<u32>().ok())) else {
            return Err(DbError::Server("key metadata: a unique key part".to_string().into()));
        };
        parts.entry(index).or_default().push((seq, column, prefix, nullable));
    }
    let mut keys = OrderKeys::default();
    let primary: Vec<String> =
        parts.get("PRIMARY").map(|p| p.iter().filter_map(|x| x.1.clone()).collect()).unwrap_or_default();
    keys.columns = columns_of(&rows_c, &primary, &foreign);
    for (index, mut p) in parts {
        p.sort_by_key(|x| x.0);
        if p.iter().any(|x| x.1.is_none() || x.2.is_some()) {
            continue;
        }
        let columns: Vec<String> = p.iter().filter_map(|x| x.1.clone()).collect();
        if index == "PRIMARY" {
            keys.primary_key = columns;
        } else if p.iter().all(|x| x.3.as_deref() == Some("NO")) {
            keys.unique_not_null.push(columns);
        }
    }
    keys.unique_not_null.sort();
    Ok(keys)
}

/// A `C` row's name, place, type, whether it takes NULL and extra.
type ColumnRow = (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);

/// The table's columns in its order from its `C` rows (name, place, type, nullable, extra),
/// the columns of its primary key and those of its foreign keys. `None` (not known) when a row
/// does not read: no name or type, a place that is no number or one taken twice.
fn columns_of(rows: &[ColumnRow], primary: &[String], foreign: &[String]) -> Option<Vec<CatalogColumn>> {
    let mut placed = Vec::with_capacity(rows.len());
    for (name, place, type_name, nullable, extra) in rows {
        let place: u32 = place.as_deref()?.trim().parse().ok()?;
        let (name, type_name) = (name.clone()?, type_name.clone()?);
        // `EXTRA` lists words such as `auto_increment`, `DEFAULT_GENERATED`, `INVISIBLE`.
        let invisible =
            extra.as_deref().is_some_and(|e| e.split_whitespace().any(|w| w.eq_ignore_ascii_case("INVISIBLE")));
        let column = CatalogColumn {
            primary_key: primary.contains(&name),
            foreign_key: foreign.contains(&name),
            not_null: nullable.as_deref() == Some("NO"),
            name,
            type_name,
            invisible,
        };
        placed.push((place, column));
    }
    placed.sort_by_key(|(place, _)| *place);
    if placed.windows(2).any(|w| w[0].0 == w[1].0) {
        return None;
    }
    Some(placed.into_iter().map(|(_, c)| c).collect())
}

/// Read the keys of `schema.table` in one round trip.
pub(crate) async fn load_order_keys(
    conn: &mut Conn,
    mode: MySqlMode,
    schema: &str,
    table: &str,
) -> Result<OrderKeys, DbError> {
    let rows: Vec<RawRow> = super::rows_of(conn, statement(mode, schema, table)).await?;
    keys_of(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(index: &str, seq: &str, col: Option<&str>, prefix: Option<&str>, nullable: &str) -> RawRow {
        let s = |v: &str| Some(v.to_string());
        ("K".into(), s(index), s(seq), col.map(str::to_string), prefix.map(str::to_string), s(nullable))
    }

    #[test]
    fn whole_column_keys_count_and_nullable_ones_only_as_the_primary_key() {
        let t: RawRow = ("T".into(), None, None, None, None, None);
        let rows = vec![
            t.clone(),
            part("PRIMARY", "2", Some("b"), None, "NO"),
            part("PRIMARY", "1", Some("a"), None, "NO"),
            part("email", "1", Some("email"), None, "NO"),
            part("nick", "1", Some("nick"), None, "YES"),
            part("pre", "1", Some("name"), Some("10"), "NO"),
            part("fn", "1", None, None, "NO"),
        ];
        let k = keys_of(rows).unwrap();
        assert_eq!(k.primary_key, ["a", "b"]);
        assert_eq!(k.unique_not_null, vec![vec!["email".to_string()]]);
        assert_eq!(keys_of(vec![t]).unwrap(), OrderKeys { columns: Some(Vec::new()), ..OrderKeys::default() });
        assert_eq!(keys_of(Vec::new()), Err(DbError::NotFound));
    }

    fn column(name: &str, place: &str, type_name: &str, nullable: &str, extra: &str) -> RawRow {
        let s = |v: &str| Some(v.to_string());
        ("C".into(), s(name), s(place), s(type_name), s(nullable), s(extra))
    }

    #[test]
    fn the_columns_come_in_the_tables_order_with_their_keys_and_invisible_ones() {
        let t: RawRow = ("T".into(), None, None, None, None, None);
        let fk: RawRow = ("F".into(), None, None, Some("owner".into()), None, None);
        let rows = vec![
            t.clone(),
            column("owner", "3", "bigint unsigned", "YES", ""),
            column("id", "1", "int", "NO", "auto_increment"),
            column("note", "2", "varchar(20)", "YES", "INVISIBLE"),
            part("PRIMARY", "1", Some("id"), None, "NO"),
            fk,
        ];
        let cols = keys_of(rows).unwrap().columns.unwrap();
        let got: Vec<(&str, &str, bool, bool, bool, bool)> = cols
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str(), c.not_null, c.primary_key, c.foreign_key, c.invisible))
            .collect();
        assert_eq!(
            got,
            [
                ("id", "int", true, true, false, false),
                ("note", "varchar(20)", false, false, false, true),
                ("owner", "bigint unsigned", false, false, true, false),
            ]
        );
        // A place that does not read, or one taken twice: not known.
        assert_eq!(keys_of(vec![t.clone(), column("a", "x", "int", "NO", "")]).unwrap().columns, None);
        let twice = vec![t, column("a", "1", "int", "NO", ""), column("b", "1", "int", "NO", "")];
        assert_eq!(keys_of(twice).unwrap().columns, None);
    }
}
