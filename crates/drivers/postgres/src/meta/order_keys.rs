//! The keys of one table that make an order total (`DbCommand::LoadTableKeys`): what a sorted
//! table tab appends as its tie-breaker. Asked for by the app on its own (a table tab opening),
//! so it must never disturb another session: it reads `pg_class`, `pg_namespace`, `pg_index` and
//! `pg_attribute` by the table's name and nothing else. No function that opens the relation
//! (`pg_get_indexdef`, `pg_get_expr`, a `regclass` cast of a name) is called, so no lock is
//! taken on the table and none is waited for: an `ALTER TABLE` holding it does not stop the
//! read, and the read does not stop the `ALTER TABLE`.
//!
//! A unique index counts when it is on whole columns (no expression), for every row (no
//! predicate), checked at once (not deferred) and valid; only its key columns count (not those
//! of `INCLUDE`). The other unique keys count only when all their columns are `NOT NULL`.

use super::read;
use datarig_core::driver::DbError;
use datarig_core::driver::keys::OrderKeys;
use tokio_postgres::Client;
use tokio_postgres::types::Type;

/// One row per usable unique index of the table, or one row with NULL index columns when it has
/// none; no row when there is no such table. Columns: whether reading it reads other tables'
/// rows (inheritance children of a plain table), whether the index is the primary key, its key
/// columns in order, whether they are all `NOT NULL`.
pub const ORDER_KEYS: &str = "SELECT c.relhassubclass AND c.relkind = 'r', x.indisprimary, x.columns, x.not_null
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN LATERAL (
  SELECT i.indisprimary,
         pg_catalog.array_agg(a.attname::text ORDER BY k.k) AS columns,
         pg_catalog.bool_and(a.attnotnull) AS not_null
  FROM pg_catalog.pg_index i
  CROSS JOIN LATERAL pg_catalog.generate_series(0, i.indnkeyatts - 1) AS k(k)
  JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[k.k]
  WHERE i.indrelid = c.oid AND i.indisunique AND i.indimmediate AND i.indisvalid
    AND i.indpred IS NULL AND i.indexprs IS NULL
  GROUP BY i.indexrelid, i.indisprimary, i.indnkeyatts
  -- Every key column found (an expression's place, 0, has none).
  HAVING pg_catalog.count(*) = i.indnkeyatts
) x ON true
WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'p', 'f', 'v', 'm')";

/// One row of [`ORDER_KEYS`]: `(inherited, primary, columns, all NOT NULL)`.
type RawRow = (bool, Option<bool>, Option<Vec<String>>, Option<bool>);

/// The rows of [`ORDER_KEYS`] as the keys.
/// No row: there is no such table ([`DbError::NotFound`]).
pub(crate) fn keys_of(rows: Vec<RawRow>) -> Result<OrderKeys, DbError> {
    let Some(first) = rows.first() else { return Err(DbError::NotFound) };
    let mut keys = OrderKeys { inherited: first.0, ..OrderKeys::default() };
    for (_, primary, columns, not_null) in rows {
        let (Some(primary), Some(columns)) = (primary, columns) else { continue };
        if columns.is_empty() {
            continue;
        }
        if primary {
            keys.primary_key = columns;
        } else if not_null == Some(true) && !keys.unique_not_null.contains(&columns) {
            keys.unique_not_null.push(columns);
        }
    }
    keys.unique_not_null.sort();
    Ok(keys)
}

/// Read the keys of `schema.table` ([`ORDER_KEYS`], in a read-only transaction of its own).
pub(crate) async fn load_order_keys(client: &Client, schema: &str, table: &str) -> Result<OrderKeys, DbError> {
    let rows = read(client, ORDER_KEYS, &[(&schema, Type::TEXT), (&table, Type::TEXT)]).await?;
    keys_of(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(c: &[&str]) -> Option<Vec<String>> {
        Some(c.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn the_rows_give_the_primary_key_and_the_not_null_unique_keys() {
        let rows = vec![
            (false, Some(false), cols(&["email"]), Some(true)),
            (false, Some(true), cols(&["id"]), Some(true)),
            (false, Some(false), cols(&["nick"]), Some(false)),
        ];
        let k = keys_of(rows).unwrap();
        assert_eq!(k.primary_key, ["id"]);
        assert_eq!(k.unique_not_null, vec![vec!["email".to_string()]]);
        assert!(!k.inherited);
        // A table without a usable key: one row without an index.
        let k = keys_of(vec![(true, None, None, None)]).unwrap();
        assert_eq!(k, OrderKeys { inherited: true, ..OrderKeys::default() });
        // No such table: not found, never "no key".
        assert_eq!(keys_of(Vec::new()), Err(DbError::NotFound));
    }
}
