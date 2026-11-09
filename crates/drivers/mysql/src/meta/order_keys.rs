//! The keys of one table that make an order total (`DbCommand::LoadTableKeys`): what a sorted
//! table tab appends as its tie-breaker. Asked for by the app on its own (a table tab opening),
//! so it must never disturb another session: one `information_schema` statement on `TABLES`,
//! `STATISTICS` (no `CARDINALITY`) and `COLUMNS`, which the data dictionary answers without
//! opening the table, in autocommit like every read of the metadata session.
//!
//! A unique key counts when every part is a whole column (no expression, no prefix); the
//! primary key always, another one when all its columns are `NOT NULL`.

use datarig_core::driver::DbError;
use datarig_core::driver::keys::OrderKeys;
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
WHERE x.TABLE_SCHEMA = {schema} AND x.TABLE_NAME = {table} AND x.NON_UNIQUE = 0";

/// One row: `T` (the table is there), or `K` with a unique key's name, the part's place, its
/// column (none for an expression), its prefix length and whether the column takes NULL.
type RawRow = (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);

/// The statement for `schema.table`, the names written as `mode` reads literals.
pub(crate) fn statement(mode: MySqlMode, schema: &str, table: &str) -> String {
    let d = Dialect::MySql(mode);
    SQL.replace("{schema}", &d.quote_literal(schema)).replace("{table}", &d.quote_literal(table))
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
    for (kind, index, seq, column, prefix, nullable) in rows {
        if kind != "K" {
            continue;
        }
        let (Some(index), Some(seq)) = (index, seq.and_then(|s| s.parse::<u32>().ok())) else {
            return Err(DbError::Server("key metadata: a unique key part".to_string().into()));
        };
        parts.entry(index).or_default().push((seq, column, prefix, nullable));
    }
    let mut keys = OrderKeys::default();
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
        assert_eq!(keys_of(vec![t]).unwrap(), OrderKeys::default());
        assert_eq!(keys_of(Vec::new()), Err(DbError::NotFound));
    }
}
