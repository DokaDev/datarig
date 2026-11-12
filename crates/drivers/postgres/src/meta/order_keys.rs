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
//!
//! With them comes, per column, whether its collation is deterministic (`pg_collation`, by the
//! column's `attcollation`; a column of no collation is): a pattern without case (`ILIKE`) is
//! written only on such a column, the server refuses it on another.

use super::read;
use datarig_core::driver::DbError;
use datarig_core::driver::keys::OrderKeys;
use tokio_postgres::Client;
use tokio_postgres::types::Type;

/// The pick of the default class among `cands (oid, exact, preferred)`, as PostgreSQL's
/// `GetDefaultOpClass` does: the class of exactly the type; else the only one whose type is the
/// preferred type of the column type's category; else the only one; else none (`NULL`).
macro_rules! pick {
    () => {
        "COALESCE(
          (SELECT (pg_catalog.array_agg(c.oid))[1] FROM cands c WHERE c.exact HAVING pg_catalog.count(*) = 1),
          (SELECT (pg_catalog.array_agg(c.oid))[1] FROM cands c WHERE c.preferred HAVING pg_catalog.count(*) = 1),
          (SELECT (pg_catalog.array_agg(c.oid))[1] FROM cands c HAVING pg_catalog.count(*) = 1))"
    };
}

/// [`pick!`] as text: the live tests run it on candidates of their own.
pub const PICK: &str = pick!();

/// One row per usable unique index of the table, or one row with NULL index columns when it has
/// none; no row when there is no such table. Columns: whether reading it reads other tables'
/// rows (inheritance children of a plain table), whether the index is the primary key, its key
/// columns in order, whether they are all `NOT NULL`; then the table's columns and whether the
/// collation of each is deterministic (the same on every row).
pub const ORDER_KEYS: &str = concat!(
    "SELECT c.relhassubclass AND c.relkind = 'r', x.indisprimary, x.columns, x.not_null, d.names, d.deterministic
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
  -- The class is picked as the server picks a default one ([`PICK`]) among the classes the
  -- type, its domains resolved, is binary compatible with. Anything else (a range, no single
  -- class) is not taken as the same order.
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
      WHERE b.base = 0 AND i.indclass[k.k] = (
        -- Every default btree class the base type is binary compatible with, as the server
        -- collects them: its own, the polymorphic one an enum (`anyenum`) or an array
        -- (`anyarray`) matches, and those of types it converts to implicitly without a
        -- function. Ranges match none here (not taken as the same order).
        WITH cands (oid, exact, preferred) AS (
          SELECT d.oid, d.opcintype = b.t, pt.typispreferred AND pt.typcategory = bt.typcategory
          FROM defaults d JOIN pg_catalog.pg_type pt ON pt.oid = d.opcintype
          WHERE d.opcintype = b.t
             OR (bt.typtype = 'e' AND d.opcintype = 'pg_catalog.anyenum'::pg_catalog.regtype)
             OR (bt.typelem <> 0 AND bt.typlen = -1 AND bt.typcategory = 'A'
                 AND d.opcintype = 'pg_catalog.anyarray'::pg_catalog.regtype)
             OR EXISTS (SELECT 1 FROM pg_catalog.pg_cast cs
                        WHERE cs.castsource = b.t AND cs.casttarget = d.opcintype
                          AND cs.castmethod = 'b' AND cs.castcontext = 'i')
        )
        SELECT ",
    pick!(),
    "
      )))
) x ON true
LEFT JOIN LATERAL (
  -- Per column, whether its collation is deterministic: a column of no collation (0) is.
  SELECT pg_catalog.array_agg(a.attname::text ORDER BY a.attnum) AS names,
         pg_catalog.array_agg(co.collisdeterministic IS NOT FALSE ORDER BY a.attnum) AS deterministic
  FROM pg_catalog.pg_attribute a
  LEFT JOIN pg_catalog.pg_collation co ON co.oid = a.attcollation
  WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
) d ON true
WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'p', 'f', 'v', 'm')"
);

/// One row of [`ORDER_KEYS`]: `(inherited, primary, columns, all NOT NULL, the table's
/// columns, whether each one's collation is deterministic)`.
type RawRow = (bool, Option<bool>, Option<Vec<String>>, Option<bool>, Option<Vec<String>>, Option<Vec<bool>>);

/// The rows of [`ORDER_KEYS`] as the keys.
/// No row: there is no such table ([`DbError::NotFound`]).
pub(crate) fn keys_of(rows: Vec<RawRow>) -> Result<OrderKeys, DbError> {
    let Some(first) = rows.first() else { return Err(DbError::NotFound) };
    let mut keys = OrderKeys { inherited: first.0, ..OrderKeys::default() };
    // One answer per column, or none taken (not known).
    if let (Some(names), Some(det)) = (&first.4, &first.5)
        && names.len() == det.len()
    {
        keys.deterministic = names.iter().cloned().zip(det.iter().copied()).collect();
    }
    for (_, primary, columns, not_null, ..) in rows {
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

/// Read the keys of `schema.table` ([`ORDER_KEYS`], in a read-only transaction of its own) on a
/// server of version `version` (`server_version_num`).
pub(crate) async fn load_order_keys(
    client: &Client,
    schema: &str,
    table: &str,
    version: u32,
) -> Result<OrderKeys, DbError> {
    let rows = read(client, ORDER_KEYS, &[(&schema, Type::TEXT), (&table, Type::TEXT)]).await?;
    let keys = keys_of(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5))).collect())?;
    Ok(OrderKeys { like_nondeterministic: Some(like_nondeterministic(version)), ..keys })
}

/// Whether a server of version `version` takes `LIKE` on a nondeterministic collation: since 18
/// (`ILIKE` and the regular expressions still refuse it).
pub(crate) fn like_nondeterministic(version: u32) -> bool {
    version >= 180_000
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
            "WITH cands (oid, exact, preferred) AS (",
            "AND cs.castmethod = 'b' AND cs.castcontext = 'i'",
            "'pg_catalog.anyenum'::pg_catalog.regtype",
            "'pg_catalog.anyarray'::pg_catalog.regtype",
            "pt.typispreferred AND pt.typcategory = bt.typcategory",
            "ty.typbasetype",
            "i.indimmediate AND i.indisvalid",
            "i.indpred IS NULL AND i.indexprs IS NULL",
            "pg_catalog.generate_series(0, i.indnkeyatts - 1)",
            "co.collisdeterministic IS NOT FALSE",
            "LEFT JOIN pg_catalog.pg_collation co ON co.oid = a.attcollation",
            "a.attnum > 0 AND NOT a.attisdropped",
        ] {
            assert!(ORDER_KEYS.contains(part), "{part}");
        }
        // One pick, after every candidate is collected (no class of a kind decided first).
        assert_eq!(ORDER_KEYS.matches(PICK).count(), 1);
        assert_eq!(ORDER_KEYS.matches("COALESCE").count(), 1);
        // Nothing that opens the relation.
        for f in ["pg_get_", "::regclass", "::pg_catalog.regclass", "pg_relation_size", "pg_total_relation_size"] {
            assert!(!ORDER_KEYS.contains(f), "{f}");
        }
    }

    #[test]
    fn the_rows_give_the_primary_key_and_the_not_null_unique_keys() {
        let names = cols(&["id", "email", "nick"]);
        let det = Some(vec![true, true, false]);
        let rows = vec![
            (false, Some(false), cols(&["email"]), Some(true), names.clone(), det.clone()),
            (false, Some(true), cols(&["id"]), Some(true), names.clone(), det.clone()),
            (false, Some(false), cols(&["nick"]), Some(false), names.clone(), det.clone()),
        ];
        let k = keys_of(rows).unwrap();
        assert_eq!(k.primary_key, ["id"]);
        assert_eq!(k.unique_not_null, vec![vec!["email".to_string()]]);
        assert!(!k.inherited);
        let want = [("email", true), ("id", true), ("nick", false)].map(|(c, d)| (c.to_string(), d));
        assert_eq!(k.deterministic, want.into_iter().collect());
        // A table without a usable key: one row without an index, its columns still read.
        let k = keys_of(vec![(true, None, None, None, cols(&["a"]), Some(vec![false]))]).unwrap();
        let deterministic = [("a".to_string(), false)].into_iter().collect();
        assert_eq!(k, OrderKeys { inherited: true, deterministic, ..OrderKeys::default() });
        // Answers that do not pair up with the columns: none taken (not known).
        let k = keys_of(vec![(false, None, None, None, cols(&["a", "b"]), Some(vec![true]))]).unwrap();
        assert!(k.deterministic.is_empty());
        // No such table: not found, never "no key".
        assert_eq!(keys_of(Vec::new()), Err(DbError::NotFound));
    }

    #[test]
    fn like_takes_a_nondeterministic_collation_from_18() {
        assert!(!like_nondeterministic(120_000));
        assert!(!like_nondeterministic(170_006));
        assert!(like_nondeterministic(180_000));
        assert!(like_nondeterministic(190_001));
    }
}
