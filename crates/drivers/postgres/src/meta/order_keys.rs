//! The keys of one table that make an order total (`DbCommand::LoadTableKeys`): what a sorted
//! table tab appends as its tie-breaker. Asked for by the app on its own (a table tab opening),
//! so it must never disturb another session: it reads `pg_class`, `pg_namespace`, `pg_index` and
//! `pg_attribute` by the table's name and nothing else. No function that opens the relation
//! (`pg_get_indexdef`, `pg_get_expr`, a `regclass` cast of a name) is called, so no lock is
//! taken on the table and none is waited for: an `ALTER TABLE` holding it does not stop the
//! read, and the read does not stop the `ALTER TABLE`.
//!
//! A unique index counts when it is on whole columns (no expression), for every row (no
//! predicate), checked at once (not deferred) and valid, and compares each column as an
//! `ORDER BY` of it does (the column's collation, and the btree class the server picks for the
//! column's type: its own default, else the one of a type it converts to without a function);
//! only its key columns count (not those of `INCLUDE`). The other unique keys count only when all their columns are `NOT NULL`.

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
  -- Every key column found (an expression's place, 0, has none), each compared as an ORDER BY
  -- of the column compares it: in the column's collation, with the operator class an ORDER BY
  -- picks for its type (a key unique only under another collation or class proves nothing).
  -- The class is picked as the server picks a default one: of the type with its domains
  -- resolved, the btree default class for exactly that type; only when it has none, the class
  -- of `anyenum` for an enum or `anyarray` for an array, else the default class of a type it
  -- converts to implicitly without a function (one, or of several its category's preferred
  -- type's). Anything else (a range, no single class) is not taken as the same order.
  HAVING pg_catalog.count(*) = i.indnkeyatts
    AND pg_catalog.bool_and(i.indcollation[k.k] = a.attcollation AND EXISTS (
      WITH RECURSIVE dom (t, base) AS (
        SELECT ty.oid, ty.typbasetype FROM pg_catalog.pg_type ty WHERE ty.oid = a.atttypid
        UNION ALL
        SELECT ty.oid, ty.typbasetype FROM dom JOIN pg_catalog.pg_type ty ON ty.oid = dom.base WHERE dom.base <> 0
      ), defaults AS (
        SELECT d.oid, d.opcintype FROM pg_catalog.pg_opclass d
        JOIN pg_catalog.pg_am m ON m.oid = d.opcmethod
        WHERE m.amname = 'btree' AND d.opcdefault
      )
      SELECT 1 FROM dom b
      JOIN pg_catalog.pg_type bt ON bt.oid = b.t
      WHERE b.base = 0 AND i.indclass[k.k] = COALESCE(
        (SELECT d.oid FROM defaults d WHERE d.opcintype = b.t),
        -- An enum or an array: the class of `anyenum` or `anyarray` (the server's polymorphic
        -- rule, not a cast). Ranges stay without one (not taken as the same order).
        (SELECT d.oid FROM defaults d WHERE bt.typtype = 'e' AND d.opcintype = 'pg_catalog.anyenum'::pg_catalog.regtype),
        (SELECT d.oid FROM defaults d
         WHERE bt.typelem <> 0 AND bt.typlen = -1 AND bt.typcategory = 'A'
           AND d.opcintype = 'pg_catalog.anyarray'::pg_catalog.regtype),
        -- Else a type it converts to implicitly without a function: the only one, or of
        -- several the preferred type of its own category (varchar converts to text and bpchar
        -- alike, text is the strings' preferred type; regclass to oid and integer, oid is the
        -- numbers' preferred type).
        (SELECT (pg_catalog.array_agg(d.oid))[1] FROM defaults d
         JOIN pg_catalog.pg_cast cs ON cs.castsource = b.t AND cs.casttarget = d.opcintype
           AND cs.castmethod = 'b' AND cs.castcontext = 'i'
         HAVING pg_catalog.count(*) = 1),
        (SELECT (pg_catalog.array_agg(d.oid))[1] FROM defaults d
         JOIN pg_catalog.pg_cast cs ON cs.castsource = b.t AND cs.casttarget = d.opcintype
           AND cs.castmethod = 'b' AND cs.castcontext = 'i'
         JOIN pg_catalog.pg_type pt ON pt.oid = d.opcintype AND pt.typispreferred AND pt.typcategory = bt.typcategory
         HAVING pg_catalog.count(*) = 1))))
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

    /// A key counts only where it compares each column as an ORDER BY of it does, and on whole,
    /// immediate, valid, full keys (the live test reads such tables).
    #[test]
    fn the_read_keeps_only_keys_an_order_by_compares_alike() {
        for part in [
            "i.indcollation[k.k] = a.attcollation",
            "WHERE m.amname = 'btree' AND d.opcdefault",
            "(SELECT d.oid FROM defaults d WHERE d.opcintype = b.t)",
            "AND cs.castmethod = 'b' AND cs.castcontext = 'i'",
            "pt.typcategory = bt.typcategory",
            "'pg_catalog.anyenum'::pg_catalog.regtype",
            "'pg_catalog.anyarray'::pg_catalog.regtype",
            "pg_catalog.count(*) = 1",
            "pt.typispreferred",
            "ty.typbasetype",
            "i.indimmediate AND i.indisvalid",
            "i.indpred IS NULL AND i.indexprs IS NULL",
            "pg_catalog.generate_series(0, i.indnkeyatts - 1)",
        ] {
            assert!(ORDER_KEYS.contains(part), "{part}");
        }
        // Nothing that opens the relation.
        for f in ["pg_get_", "::regclass", "::pg_catalog.regclass", "pg_relation_size", "pg_total_relation_size"] {
            assert!(!ORDER_KEYS.contains(f), "{f}");
        }
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
