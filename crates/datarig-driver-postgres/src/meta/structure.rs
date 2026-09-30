//! One table's structure (`DbCommand::LoadStructure`): a single catalog statement that builds
//! the whole structure as one JSON document on the server, so opening a table's node costs
//! one round trip. It reads the catalog only (never the table), and it never waits for a lock
//! another session holds or waits for:
//!
//! * the row and size estimates are the statistics in `pg_class` and the cumulative statistics,
//!   as the explorer's list of a schema's objects has them (`stats_ctes!`: no lock on any
//!   relation);
//! * the server deparses defaults, check constraints, indexes and trigger conditions only
//!   against the open table: `pg_get_expr` with a relation, `pg_get_indexdef`, and
//!   `pg_get_constraintdef` of a check and `pg_get_triggerdef` of a trigger with `WHEN` lock the
//!   table (`AccessShareLock`, PostgreSQL 17). They run only when `pg_locks` shows no
//!   `AccessExclusiveLock` on the table, held or asked for (the only mode that conflicts); when
//!   there is one the statement answers `locked` and asks for no lock ([`DbError::Locked`]).
//!   One taken between that check and the deparsing ends the wait after the metadata session's
//!   `lock_timeout` (the same error).
//!
//! The rest (`regclass`, `format_type`, the catalogs' own rows, `pg_get_constraintdef` of a
//! key) takes no lock on the table: an index key's order, operator class and collation come
//! from `pg_index` (not `pg_get_indexdef`), as do the table's columns each key and `INCLUDE`
//! column is (`indkey`, 0 for an expression); a trigger's `UPDATE OF` columns from `tgattr`. Its
//! `WHEN` condition only `pg_get_triggerdef` prints ([`when_condition`]).

use datarig_core::driver::DbError;
use datarig_core::driver::structure::{
    CheckConstraint, ColumnFill, FkAction, ForeignKey, Index, KeyConstraint, RelationKind, StructureColumn,
    TableStructure, Trigger, TriggerEvent, TriggerTiming,
};
use serde::Deserialize;
use tokio_postgres::Client;
use tokio_postgres::types::Type;

/// The statement. `$1` and `$2` are the schema and the table; the cast to `regclass` (which
/// locks nothing) fails with the server's own "does not exist" when the table is gone. The row
/// and size estimates are the explorer's list's (`stats_ctes!`, for the one root).
pub const SQL: &str = concat!(
    "WITH RECURSIVE rel AS (
  SELECT c.oid, c.relkind::text AS kind,
         EXISTS (SELECT 1 FROM pg_catalog.pg_locks l
           WHERE l.locktype = 'relation' AND l.relation = c.oid AND l.mode = 'AccessExclusiveLock'
             AND l.database = (SELECT d.oid FROM pg_catalog.pg_database d
                               WHERE d.datname = pg_catalog.current_database())) AS locked
  FROM pg_catalog.pg_class c
  WHERE c.oid = pg_catalog.format('%I.%I', $1::text, $2::text)::regclass
), roots AS (
  SELECT rel.oid, rel.kind FROM rel
), ",
    stats_ctes!(),
    "
SELECT CASE WHEN rel.locked THEN pg_catalog.json_build_object('kind', rel.kind, 'locked', true)
ELSE pg_catalog.json_build_object(
  'kind', rel.kind,
  'rows', ",
    stats_rows!("rel.kind"),
    ",
  'bytes', ",
    stats_bytes!("rel.kind"),
    ",
  'columns', (
    SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
      'name', a.attname, 'type', pg_catalog.format_type(a.atttypid, a.atttypmod),
      'not_null', a.attnotnull, 'default', pg_catalog.pg_get_expr(d.adbin, d.adrelid, true),
      'generated', a.attgenerated::text, 'identity', a.attidentity::text) ORDER BY a.attnum)
    FROM pg_catalog.pg_attribute a
    LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
    WHERE a.attrelid = rel.oid AND a.attnum > 0 AND NOT a.attisdropped),
  'constraints', (
    SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
      'name', con.conname, 'type', con.contype::text,
      'columns', (SELECT pg_catalog.json_agg(a.attname ORDER BY k.i)
        FROM pg_catalog.unnest(con.conkey) WITH ORDINALITY k(n, i)
        JOIN pg_catalog.pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n),
      'ref_schema', rn.nspname, 'ref_table', rc.relname,
      'ref_columns', (SELECT pg_catalog.json_agg(a.attname ORDER BY k.i)
        FROM pg_catalog.unnest(con.confkey) WITH ORDINALITY k(n, i)
        JOIN pg_catalog.pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.n),
      'on_delete', con.confdeltype::text, 'on_update', con.confupdtype::text,
      'expression', CASE WHEN con.contype = 'c' THEN pg_catalog.pg_get_expr(con.conbin, con.conrelid, true) END,
      'definition', pg_catalog.pg_get_constraintdef(con.oid, true)) ORDER BY con.conname)
    FROM pg_catalog.pg_constraint con
    LEFT JOIN pg_catalog.pg_class rc ON rc.oid = con.confrelid
    LEFT JOIN pg_catalog.pg_namespace rn ON rn.oid = rc.relnamespace
    WHERE con.conrelid = rel.oid AND con.contype IN ('p', 'f', 'u', 'c')),
  'indexes', (
    SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
      'name', ic.relname, 'unique', i.indisunique, 'primary', i.indisprimary,
      'constraint', EXISTS (SELECT 1 FROM pg_catalog.pg_constraint x
        WHERE x.conrelid = i.indrelid AND x.conindid = i.indexrelid AND x.contype IN ('p', 'u', 'x')),
      'method', am.amname,
      'columns', (SELECT pg_catalog.json_agg(pg_catalog.pg_get_indexdef(i.indexrelid, k, true) ORDER BY k)
        FROM pg_catalog.generate_series(1, i.indnkeyatts) k),
      'options', (SELECT pg_catalog.json_agg(pg_catalog.concat_ws(' ',
          CASE WHEN i.indcollation[k - 1] <> 0 AND i.indcollation[k - 1] <> coalesce(ta.attcollation, ty.typcollation)
            THEN 'COLLATE ' || pg_catalog.quote_ident(co.collname) END,
          CASE WHEN NOT oc.opcdefault THEN pg_catalog.quote_ident(oc.opcname) END,
          CASE WHEN pg_catalog.pg_indexam_has_property(am.oid, 'can_order') THEN
            CASE i.indoption[k - 1]::int & 3 WHEN 1 THEN 'DESC NULLS LAST' WHEN 2 THEN 'NULLS FIRST' WHEN 3 THEN 'DESC' END
          END) ORDER BY k)
        FROM pg_catalog.generate_series(1, i.indnkeyatts) k
        LEFT JOIN pg_catalog.pg_attribute ta ON ta.attrelid = i.indrelid AND ta.attnum = i.indkey[k - 1]
        LEFT JOIN pg_catalog.pg_attribute ia ON ia.attrelid = i.indexrelid AND ia.attnum = k
        LEFT JOIN pg_catalog.pg_type ty ON ty.oid = ia.atttypid
        LEFT JOIN pg_catalog.pg_opclass oc ON oc.oid = i.indclass[k - 1]
        LEFT JOIN pg_catalog.pg_collation co ON co.oid = i.indcollation[k - 1]),
      'include', (SELECT pg_catalog.json_agg(pg_catalog.pg_get_indexdef(i.indexrelid, k, true) ORDER BY k)
        FROM pg_catalog.generate_series(i.indnkeyatts + 1, i.indnatts) k),
      'key_columns', (SELECT pg_catalog.json_agg(ta.attname ORDER BY k)
        FROM pg_catalog.generate_series(1, i.indnkeyatts) k
        LEFT JOIN pg_catalog.pg_attribute ta ON ta.attrelid = i.indrelid AND ta.attnum = i.indkey[k - 1]),
      'include_columns', (SELECT pg_catalog.json_agg(ta.attname ORDER BY k)
        FROM pg_catalog.generate_series(i.indnkeyatts + 1, i.indnatts) k
        JOIN pg_catalog.pg_attribute ta ON ta.attrelid = i.indrelid AND ta.attnum = i.indkey[k - 1]),
      'predicate', pg_catalog.pg_get_expr(i.indpred, i.indrelid, true),
      'definition', pg_catalog.pg_get_indexdef(i.indexrelid)) ORDER BY ic.relname)
    FROM pg_catalog.pg_index i
    JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
    JOIN pg_catalog.pg_am am ON am.oid = ic.relam
    WHERE i.indrelid = rel.oid),
  'triggers', (
    SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
      'name', t.tgname, 'type', t.tgtype, 'enabled', t.tgenabled::text,
      'function', pn.nspname || '.' || p.proname,
      'update_columns', (SELECT pg_catalog.json_agg(pg_catalog.quote_ident(a.attname) ORDER BY k.i)
        FROM pg_catalog.unnest(t.tgattr::pg_catalog.int2[]) WITH ORDINALITY k(n, i)
        JOIN pg_catalog.pg_attribute a ON a.attrelid = t.tgrelid AND a.attnum = k.n),
      'when', t.tgqual IS NOT NULL,
      'definition', pg_catalog.pg_get_triggerdef(t.oid, true)) ORDER BY t.tgname)
    FROM pg_catalog.pg_trigger t
    JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid
    JOIN pg_catalog.pg_namespace pn ON pn.oid = p.pronamespace
    WHERE t.tgrelid = rel.oid AND NOT t.tgisinternal)
) END::text
FROM rel LEFT JOIN stats s ON s.root = rel.oid"
);

/// Read the structure of `schema.table` in one round trip.
pub(crate) async fn load_structure(client: &Client, schema: &str, table: &str) -> Result<TableStructure, DbError> {
    let rows = super::read(client, SQL, &[(&schema, Type::TEXT), (&table, Type::TEXT)]).await?;
    match rows.first() {
        Some(row) => parse(row.get(0)),
        None => Err(DbError::NoResult),
    }
}

#[derive(Deserialize)]
struct Raw {
    kind: String,
    /// Another session holds or asked for an `AccessExclusiveLock` on the table: nothing was
    /// deparsed.
    #[serde(default)]
    locked: bool,
    rows: Option<f64>,
    bytes: Option<i64>,
    columns: Option<Vec<RawColumn>>,
    constraints: Option<Vec<RawConstraint>>,
    indexes: Option<Vec<RawIndex>>,
    triggers: Option<Vec<RawTrigger>>,
}

#[derive(Deserialize)]
struct RawColumn {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    not_null: bool,
    default: Option<String>,
    generated: String,
    identity: String,
}

#[derive(Deserialize)]
struct RawConstraint {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    columns: Option<Vec<String>>,
    ref_schema: Option<String>,
    ref_table: Option<String>,
    ref_columns: Option<Vec<String>>,
    on_delete: String,
    on_update: String,
    expression: Option<String>,
    definition: String,
}

#[derive(Deserialize)]
struct RawIndex {
    name: String,
    unique: bool,
    primary: bool,
    constraint: bool,
    method: String,
    columns: Option<Vec<String>>,
    options: Option<Vec<String>>,
    include: Option<Vec<String>>,
    /// Each key's column (`null`: an expression, `indkey` 0).
    key_columns: Option<Vec<Option<String>>>,
    include_columns: Option<Vec<String>>,
    predicate: Option<String>,
    definition: String,
}

#[derive(Deserialize)]
struct RawTrigger {
    name: String,
    #[serde(rename = "type")]
    tgtype: i32,
    enabled: String,
    function: String,
    update_columns: Option<Vec<String>>,
    /// It has a `WHEN` condition, which only `definition` prints.
    #[serde(default)]
    when: bool,
    definition: String,
}

/// The document the statement built, as the model. A kind of relation the explorer does not
/// list (an index, a sequence) is not supported.
fn parse(json: &str) -> Result<TableStructure, DbError> {
    let raw: Raw = serde_json::from_str(json).map_err(|e| DbError::Server(format!("table structure: {e}")))?;
    if raw.locked {
        return Err(DbError::Locked);
    }
    let kind = match raw.kind.as_str() {
        "r" => RelationKind::Table,
        "p" => RelationKind::PartitionedTable,
        "f" => RelationKind::ForeignTable,
        "v" => RelationKind::View,
        "m" => RelationKind::MaterializedView,
        _ => return Err(DbError::NotSupported),
    };
    let mut s = TableStructure::new(kind);
    let stats = super::relation_stats(raw.rows, raw.bytes);
    (s.estimated_rows, s.total_bytes) = (stats.rows, stats.bytes);
    s.columns = raw
        .columns
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            let fill = match (c.generated.as_str(), c.identity.as_str()) {
                ("s", _) => ColumnFill::Stored(c.default.clone().unwrap_or_default()),
                ("v", _) => ColumnFill::Virtual(c.default.clone().unwrap_or_default()),
                (_, "a") => ColumnFill::IdentityAlways,
                (_, "d") => ColumnFill::IdentityByDefault,
                _ => ColumnFill::Default,
            };
            // A generated column's expression is its fill, not a default.
            let default = if matches!(fill, ColumnFill::Default) { c.default } else { None };
            StructureColumn { name: c.name, type_name: c.type_name, not_null: c.not_null, default, fill }
        })
        .collect();
    for c in raw.constraints.unwrap_or_default() {
        let columns = c.columns.unwrap_or_default();
        match c.kind.as_str() {
            "p" => s.primary_key = Some(KeyConstraint { name: c.name, columns, definition: c.definition }),
            "u" => s.unique_constraints.push(KeyConstraint { name: c.name, columns, definition: c.definition }),
            "c" => s.checks.push(CheckConstraint {
                name: c.name,
                expression: c.expression.unwrap_or_default(),
                columns,
                definition: c.definition,
            }),
            "f" => s.foreign_keys.push(ForeignKey {
                name: c.name,
                columns,
                ref_schema: c.ref_schema.unwrap_or_default(),
                ref_table: c.ref_table.unwrap_or_default(),
                ref_columns: c.ref_columns.unwrap_or_default(),
                on_delete: fk_action(&c.on_delete),
                on_update: fk_action(&c.on_update),
                definition: c.definition,
            }),
            _ => {}
        }
    }
    s.indexes = raw
        .indexes
        .unwrap_or_default()
        .into_iter()
        .map(|i| Index {
            name: i.name,
            columns: i.columns.unwrap_or_default(),
            options: i.options.unwrap_or_default(),
            include: i.include.unwrap_or_default(),
            key_columns: i.key_columns.unwrap_or_default(),
            include_columns: i.include_columns.unwrap_or_default(),
            unique: i.unique,
            method: i.method,
            predicate: i.predicate,
            primary: i.primary,
            constraint: i.constraint,
            definition: i.definition,
        })
        .collect();
    s.triggers = raw.triggers.unwrap_or_default().into_iter().map(trigger).collect();
    Ok(s)
}

/// `pg_constraint.confdeltype` / `confupdtype`.
fn fk_action(code: &str) -> FkAction {
    match code {
        "r" => FkAction::Restrict,
        "c" => FkAction::Cascade,
        "n" => FkAction::SetNull,
        "d" => FkAction::SetDefault,
        _ => FkAction::NoAction,
    }
}

/// A trigger from `pg_trigger`: `tgtype` is a bit set (`TRIGGER_TYPE_*` of the server's
/// `pg_trigger.h`), `tgenabled` is `D` when it is disabled.
fn trigger(t: RawTrigger) -> Trigger {
    const ROW: i32 = 1 << 0;
    const BEFORE: i32 = 1 << 1;
    const INSERT: i32 = 1 << 2;
    const DELETE: i32 = 1 << 3;
    const UPDATE: i32 = 1 << 4;
    const TRUNCATE: i32 = 1 << 5;
    const INSTEAD: i32 = 1 << 6;
    let timing = if t.tgtype & INSTEAD != 0 {
        TriggerTiming::InsteadOf
    } else if t.tgtype & BEFORE != 0 {
        TriggerTiming::Before
    } else {
        TriggerTiming::After
    };
    let events = [
        (INSERT, TriggerEvent::Insert),
        (UPDATE, TriggerEvent::Update),
        (DELETE, TriggerEvent::Delete),
        (TRUNCATE, TriggerEvent::Truncate),
    ]
    .into_iter()
    .filter(|(bit, _)| t.tgtype & bit != 0)
    .map(|(_, e)| e)
    .collect();
    Trigger {
        name: t.name,
        timing,
        events,
        for_each_row: t.tgtype & ROW != 0,
        function: t.function,
        enabled: t.enabled != "D",
        update_columns: t.update_columns.unwrap_or_default(),
        condition: if t.when { when_condition(&t.definition) } else { None },
        definition: t.definition,
    }
}

/// The condition of the `WHEN (…)` clause of `definition` (`pg_get_triggerdef`), without its
/// parentheses. The clause is the first `WHEN (` outside quoted names and string literals (a
/// name or an argument may contain the words), up to its matching parenthesis.
fn when_condition(definition: &str) -> Option<String> {
    let bytes = definition.as_bytes();
    let (mut quote, mut depth, mut start) = (None, 0usize, None);
    for (i, &b) in bytes.iter().enumerate() {
        match (quote, b) {
            // A doubled quote inside a name or a literal closes and reopens it: the same.
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'(') => {
                if start.is_none() && depth == 0 && definition[..i].ends_with(" WHEN ") {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            (None, b')') => {
                depth = depth.saturating_sub(1);
                if let Some(s) = start.filter(|_| depth == 0) {
                    return Some(definition[s..i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests;
