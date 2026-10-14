//! An object's DDL (`DbCommand::LoadDdl`): what its `CREATE` statement needs, read from the
//! catalog in one round trip and built as one JSON document on the server, like the table
//! structure ([`super::structure`]), whose parts a relation's DDL includes.
//!
//! The statements of one read go out together in one transaction ([`super::BEGIN_READ`]):
//! the object is looked up first (by name, in the tab's schema for a name the user typed) and
//! its kind and `oid` kept in a setting of the transaction (`datarig.ddl_target`, `r:<oid>` a
//! relation or an index, `t:` a trigger, `f:` a function); then the search path is emptied for
//! the rest of the transaction, so the server qualifies every name it prints (as `pg_dump`
//! does); then the statement of that kind builds the document. Both settings end with the
//! transaction.
//!
//! It never waits for a lock another session holds or waits for. Deparsing takes
//! `AccessShareLock` on relations, which waits behind an `AccessExclusiveLock` (an `ALTER
//! TABLE`, a `VACUUM FULL`); checked against PostgreSQL 13 to 18 by holding one on each
//! relation in turn and seeing which reads wait:
//!
//! * the table's own deparsing (defaults, checks, indexes, trigger conditions, policies,
//!   `pg_get_partkeydef`, a partition's bound) locks the table;
//! * `pg_get_viewdef` locks the view and every relation its query reads (kept until the
//!   transaction ends): the relations the view's rule depends on (`pg_depend` of its
//!   `pg_rewrite` row);
//! * a policy's expression locks the relations its subqueries read: those the policy depends on;
//! * `pg_get_indexdef` and `pg_get_triggerdef` lock the index's or the trigger's table;
//! * `pg_get_functiondef` of a function with an SQL-standard body (`BEGIN ATOMIC`, PostgreSQL
//!   14) locks the relations the body reads: those the function depends on. One whose body is a
//!   string (PL/pgSQL, `AS '…'`) locks nothing;
//! * `aclexplode`, `pg_get_userbyid`, `pg_get_function_identity_arguments`,
//!   `pg_get_constraintdef` of a key, `format_type`, `regclass` and the catalogs' own rows
//!   (comments, sequences, collations) lock no relation.
//!
//! So each statement first looks at `pg_locks` for an `AccessExclusiveLock`, held or asked for,
//! on any of those relations (the "guard"); when there is one it answers `locked` and deparses
//! nothing ([`DbError::Locked`]: the DDL view says so, never a part of the DDL). A lock taken
//! between that look and the deparsing ends the wait after the metadata session's
//! `lock_timeout` (the same error).

use super::structure::{Raw as RawStructure, model};
use datarig_core::driver::DbError;
use datarig_core::driver::ddl::{
    ColumnDdl, Comment, DdlObject, DdlSource, ForeignTable, FunctionDdl, Grant, IndexDdl, OwnedSequence, PartitionOf,
    Policy, RelationDdl, ReplicaIdentity, SequenceDdl, TriggerDdl, TriggerExtra, TriggerMode,
};
use datarig_core::driver::structure::RelationKind;
use serde::Deserialize;
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{ToSql, Type};

/// The kind and `oid` the lookup found (`datarig.ddl_target`): the CTE `target (class, oid)`.
macro_rules! target_cte {
    () => {
        "target AS (
  SELECT pg_catalog.split_part(v, ':', 1) AS class,
         NULLIF(pg_catalog.split_part(v, ':', 2), '')::pg_catalog.oid AS oid
  FROM pg_catalog.current_setting('datarig.ddl_target') v
)"
    };
}

/// Whether another session holds or asked for an `AccessExclusiveLock` (the only mode that
/// conflicts with the `AccessShareLock` of deparsing) on a relation of the CTE `guard (oid)`:
/// the CTE `locked (locked)`.
macro_rules! locked_cte {
    () => {
        "locked AS (
  SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_locks l JOIN guard g ON g.oid = l.relation
    WHERE l.locktype = 'relation' AND l.mode = 'AccessExclusiveLock'
      AND l.database = (SELECT d.oid FROM pg_catalog.pg_database d
                        WHERE d.datname = pg_catalog.current_database())) AS locked
)"
    };
}

/// The comment of object `$oid` of catalog `$class` (`$sub`: a column's number, else 0).
macro_rules! description {
    ($class:literal, $oid:literal, $sub:literal) => {
        concat!(
            "(SELECT dsc.description FROM pg_catalog.pg_description dsc WHERE dsc.classoid = 'pg_catalog.",
            $class,
            "'::pg_catalog.regclass AND dsc.objoid = ",
            $oid,
            " AND dsc.objsubid = ",
            $sub,
            ")"
        )
    };
}

/// A sequence `seq` (`pg_class`) with its namespace `seqn` and its row of `pg_sequence` `sq`, as
/// a JSON object.
macro_rules! sequence_json {
    () => {
        "pg_catalog.json_build_object('schema', seqn.nspname, 'name', seq.relname,
        'type', pg_catalog.format_type(sq.seqtypid, NULL), 'start', sq.seqstart, 'increment', sq.seqincrement,
        'min', sq.seqmin, 'max', sq.seqmax, 'cache', sq.seqcache, 'cycle', sq.seqcycle,
        'unlogged', seq.relpersistence = 'u')"
    };
}

/// The privileges of the ACL `$acl` granted to others than `$owner`: `[grantee, privilege,
/// grantable]` each (the grantee `null` for `PUBLIC`).
macro_rules! grants_json {
    ($acl:literal, $owner:literal) => {
        concat!(
            "(SELECT pg_catalog.json_agg(pg_catalog.json_build_array(
        CASE WHEN x.grantee = 0 THEN NULL ELSE pg_catalog.pg_get_userbyid(x.grantee) END,
        x.privilege_type, x.is_grantable))
      FROM pg_catalog.aclexplode(",
            $acl,
            ") x WHERE x.grantee <> ",
            $owner,
            ")"
        )
    };
}

/// A relation's or an index's DDL. The guard: the relation, an index's table, the relations its
/// view query and its policies depend on.
pub const RELATION: &str = concat!(
    "WITH ",
    target_cte!(),
    ", rel AS (
  SELECT c.oid, c.relkind::text AS kind
  FROM target JOIN pg_catalog.pg_class c ON c.oid = target.oid
  WHERE target.class = 'r'
), guard AS (
  SELECT rel.oid FROM rel
  UNION SELECT i.indrelid FROM rel JOIN pg_catalog.pg_index i ON i.indexrelid = rel.oid
  UNION SELECT dep.refobjid FROM rel
    JOIN pg_catalog.pg_rewrite w ON w.ev_class = rel.oid AND w.ev_type = '1'
    JOIN pg_catalog.pg_depend dep ON dep.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND dep.objid = w.oid
  WHERE dep.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
  UNION SELECT dep.refobjid FROM rel
    JOIN pg_catalog.pg_policy pol ON pol.polrelid = rel.oid
    JOIN pg_catalog.pg_depend dep ON dep.classid = 'pg_catalog.pg_policy'::pg_catalog.regclass AND dep.objid = pol.oid
  WHERE dep.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
), ",
    locked_cte!(),
    "
SELECT CASE WHEN locked.locked THEN pg_catalog.json_build_object('locked', true)
WHEN rel.kind IN ('i', 'I') THEN pg_catalog.json_build_object('index', (
  SELECT pg_catalog.json_build_object(
    'schema', n.nspname, 'name', c.relname, 'table', tc.relname,
    'definition', pg_catalog.pg_get_indexdef(c.oid),
    'constraint', (SELECT pg_catalog.json_build_array(con.conname, pg_catalog.pg_get_constraintdef(con.oid, true))
      FROM pg_catalog.pg_constraint con
      WHERE con.conindid = c.oid AND con.conrelid = i.indrelid AND con.contype IN ('p', 'u', 'x')),
    'constraint_comment', (SELECT ",
    description!("pg_constraint", "con.oid", "0"),
    " FROM pg_catalog.pg_constraint con
      WHERE con.conindid = c.oid AND con.conrelid = i.indrelid AND con.contype IN ('p', 'u', 'x')),
    'comment', ",
    description!("pg_class", "c.oid", "0"),
    ")
  FROM pg_catalog.pg_class c
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
  JOIN pg_catalog.pg_index i ON i.indexrelid = c.oid
  JOIN pg_catalog.pg_class tc ON tc.oid = i.indrelid
  WHERE c.oid = rel.oid))
ELSE pg_catalog.json_build_object('relation', pg_catalog.json_build_object(
  'kind', rel.kind,
",
    structure_parts!(),
    "
), 'ddl', (
  SELECT pg_catalog.json_build_object(
    'schema', n.nspname, 'name', c.relname,
    'owner', pg_catalog.pg_get_userbyid(c.relowner),
    'persistence', c.relpersistence::text,
    'am', CASE WHEN c.relkind IN ('r', 'm', 'p') THEN
      (SELECT am.amname FROM pg_catalog.pg_am am WHERE am.oid = c.relam) END,
    'options', c.reloptions,
    'toast_options', (SELECT t.reloptions FROM pg_catalog.pg_class t WHERE t.oid = c.reltoastrelid),
    'tablespace', CASE WHEN c.reltablespace <> 0 THEN
      (SELECT ts.spcname FROM pg_catalog.pg_tablespace ts WHERE ts.oid = c.reltablespace) END,
    'partition_key', CASE WHEN c.relkind = 'p' THEN pg_catalog.pg_get_partkeydef(c.oid) END,
    'of_type', CASE WHEN c.reloftype <> 0 THEN pg_catalog.format_type(c.reloftype, NULL) END,
    'partition_of', CASE WHEN c.relispartition THEN (
      SELECT pg_catalog.json_build_array(pn.nspname, p.relname, pg_catalog.pg_get_expr(c.relpartbound, c.oid))
      FROM pg_catalog.pg_inherits h
      JOIN pg_catalog.pg_class p ON p.oid = h.inhparent
      JOIN pg_catalog.pg_namespace pn ON pn.oid = p.relnamespace
      WHERE h.inhrelid = c.oid) END,
    'inherits', CASE WHEN NOT c.relispartition THEN (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_array(pn.nspname, p.relname) ORDER BY h.inhseqno)
      FROM pg_catalog.pg_inherits h
      JOIN pg_catalog.pg_class p ON p.oid = h.inhparent
      JOIN pg_catalog.pg_namespace pn ON pn.oid = p.relnamespace
      WHERE h.inhrelid = c.oid) END,
    'view', CASE WHEN c.relkind IN ('v', 'm') THEN pg_catalog.pg_get_viewdef(c.oid, true) END,
    'server', (SELECT s.srvname FROM pg_catalog.pg_foreign_table f
      JOIN pg_catalog.pg_foreign_server s ON s.oid = f.ftserver WHERE f.ftrelid = c.oid),
    'server_options', (SELECT f.ftoptions FROM pg_catalog.pg_foreign_table f WHERE f.ftrelid = c.oid),
    'columns', (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
        'name', a.attname, 'local', a.attislocal,
        'collation', CASE WHEN a.attcollation <> ty.typcollation THEN (
          SELECT pg_catalog.json_build_array(cn.nspname, co.collname) FROM pg_catalog.pg_collation co
          JOIN pg_catalog.pg_namespace cn ON cn.oid = co.collnamespace WHERE co.oid = a.attcollation) END,
        'storage', CASE WHEN a.attstorage <> ty.typstorage THEN a.attstorage::text END,
        'compression', pg_catalog.to_jsonb(a) ->> 'attcompression',
        'statistics', a.attstattarget,
        'options', a.attoptions,
        'fdw_options', a.attfdwoptions,
        'default_tree', (SELECT ad.adbin::text FROM pg_catalog.pg_attrdef ad
          WHERE ad.adrelid = c.oid AND ad.adnum = a.attnum),
        'parent', CASE WHEN NOT a.attislocal THEN (
          SELECT pg_catalog.json_build_object('default_tree', pd.adbin::text, 'not_null', pa.attnotnull)
          FROM pg_catalog.pg_inherits h
          JOIN pg_catalog.pg_attribute pa ON pa.attrelid = h.inhparent AND pa.attname = a.attname
            AND NOT pa.attisdropped
          LEFT JOIN pg_catalog.pg_attrdef pd ON pd.adrelid = pa.attrelid AND pd.adnum = pa.attnum
          WHERE h.inhrelid = c.oid ORDER BY h.inhseqno LIMIT 1) END,
        'comment', ",
    description!("pg_class", "c.oid", "a.attnum"),
    ",
        'grants', ",
    grants_json!("a.attacl", "c.relowner"),
    ",
        'identity', CASE WHEN a.attidentity <> '' THEN (
          SELECT ",
    sequence_json!(),
    "
          FROM pg_catalog.pg_depend dep
          JOIN pg_catalog.pg_class seq ON seq.oid = dep.objid AND seq.relkind = 'S'
          JOIN pg_catalog.pg_namespace seqn ON seqn.oid = seq.relnamespace
          JOIN pg_catalog.pg_sequence sq ON sq.seqrelid = seq.oid
          WHERE dep.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
            AND dep.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
            AND dep.refobjid = c.oid AND dep.refobjsubid = a.attnum AND dep.deptype = 'i'
          LIMIT 1) END) ORDER BY a.attnum)
      FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type ty ON ty.oid = a.atttypid
      WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped),
    'sequences', (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_object('column', a.attname, 'sequence', ",
    sequence_json!(),
    ") ORDER BY a.attnum, seq.relname)
      FROM pg_catalog.pg_depend dep
      JOIN pg_catalog.pg_class seq ON seq.oid = dep.objid AND seq.relkind = 'S'
      JOIN pg_catalog.pg_namespace seqn ON seqn.oid = seq.relnamespace
      JOIN pg_catalog.pg_sequence sq ON sq.seqrelid = seq.oid
      JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = dep.refobjsubid
      WHERE dep.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
        AND dep.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
        AND dep.refobjid = c.oid AND dep.deptype = 'a'),
    'constraints', (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
        'name', con.conname, 'type', con.contype::text,
        'inherited', NOT con.conislocal OR con.conparentid <> 0,
        'definition', CASE WHEN con.contype IN ('x', 'n') THEN pg_catalog.pg_get_constraintdef(con.oid, true) END,
        'column', CASE WHEN con.contype = 'n' THEN (SELECT ca.attname FROM pg_catalog.pg_attribute ca
          WHERE ca.attrelid = con.conrelid AND ca.attnum = con.conkey[1]) END,
        'no_inherit', con.connoinherit,
        'comment', ",
    description!("pg_constraint", "con.oid", "0"),
    ") ORDER BY con.conname)
      FROM pg_catalog.pg_constraint con
      WHERE con.conrelid = c.oid AND con.contype IN ('p', 'u', 'c', 'f', 'x', 'n')),
    'indexes', (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
        'name', ic.relname,
        'inherited', EXISTS (SELECT 1 FROM pg_catalog.pg_inherits h WHERE h.inhrelid = i.indexrelid),
        'replica', i.indisreplident,
        'comment', ",
    description!("pg_class", "ic.oid", "0"),
    ") ORDER BY ic.relname)
      FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
      WHERE i.indrelid = c.oid),
    'triggers', (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
        'name', t.tgname, 'enabled', t.tgenabled::text,
        'inherited', coalesce((pg_catalog.to_jsonb(t) ->> 'tgparentid')::pg_catalog.oid, 0) <> 0,
        'comment', ",
    description!("pg_trigger", "t.oid", "0"),
    ") ORDER BY t.tgname)
      FROM pg_catalog.pg_trigger t
      WHERE t.tgrelid = c.oid AND NOT t.tgisinternal),
    'replica_identity', c.relreplident::text,
    'row_security', c.relrowsecurity,
    'force_row_security', c.relforcerowsecurity,
    'policies', (
      SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
        'name', pol.polname, 'permissive', pol.polpermissive, 'command', pol.polcmd::text,
        'roles', (SELECT pg_catalog.json_agg(CASE WHEN r.role = 0 THEN NULL ELSE pg_catalog.pg_get_userbyid(r.role) END
                    ORDER BY r.i)
                  FROM pg_catalog.unnest(pol.polroles) WITH ORDINALITY r(role, i)),
        'using', pg_catalog.pg_get_expr(pol.polqual, pol.polrelid),
        'check', pg_catalog.pg_get_expr(pol.polwithcheck, pol.polrelid),
        'comment', ",
    description!("pg_policy", "pol.oid", "0"),
    ") ORDER BY pol.polname)
      FROM pg_catalog.pg_policy pol WHERE pol.polrelid = c.oid),
    'grants', ",
    grants_json!("c.relacl", "c.relowner"),
    ",
    'comment', ",
    description!("pg_class", "c.oid", "0"),
    ")
  FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
  WHERE c.oid = rel.oid))
END::text
FROM rel, locked"
);

/// A trigger's DDL. The guard: its table (`pg_get_triggerdef` of a trigger with `WHEN` locks it).
pub const TRIGGER: &str = concat!(
    "WITH ",
    target_cte!(),
    ", trg AS (
  SELECT t.oid, t.tgrelid
  FROM target JOIN pg_catalog.pg_trigger t ON t.oid = target.oid
  WHERE target.class = 't'
), guard AS (
  SELECT trg.tgrelid AS oid FROM trg
), ",
    locked_cte!(),
    "
SELECT CASE WHEN locked.locked THEN pg_catalog.json_build_object('locked', true)
ELSE pg_catalog.json_build_object('trigger', (
  SELECT pg_catalog.json_build_object(
    'schema', n.nspname, 'table', c.relname, 'name', t.tgname,
    'definition', pg_catalog.pg_get_triggerdef(t.oid, true),
    'enabled', t.tgenabled::text,
    'comment', ",
    description!("pg_trigger", "t.oid", "0"),
    ")
  FROM pg_catalog.pg_trigger t
  JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
  WHERE t.oid = trg.oid))
END::text
FROM trg, locked"
);

/// A function's DDL. The guard: the relations it depends on (an SQL-standard body's).
pub const FUNCTION: &str = concat!(
    "WITH ",
    target_cte!(),
    ", fn AS (
  SELECT p.oid
  FROM target JOIN pg_catalog.pg_proc p ON p.oid = target.oid
  WHERE target.class = 'f'
), guard AS (
  SELECT dep.refobjid AS oid FROM fn
  JOIN pg_catalog.pg_depend dep ON dep.classid = 'pg_catalog.pg_proc'::pg_catalog.regclass AND dep.objid = fn.oid
  WHERE dep.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
), ",
    locked_cte!(),
    "
SELECT CASE WHEN locked.locked THEN pg_catalog.json_build_object('locked', true)
ELSE pg_catalog.json_build_object('function', (
  SELECT pg_catalog.json_build_object(
    'schema', n.nspname, 'name', p.proname,
    'arguments', pg_catalog.pg_get_function_identity_arguments(p.oid),
    'procedure', p.prokind = 'p',
    'owner', pg_catalog.pg_get_userbyid(p.proowner),
    'definition', pg_catalog.pg_get_functiondef(p.oid),
    'default_acl', p.proacl IS NULL,
    'grants', ",
    grants_json!("p.proacl", "p.proowner"),
    ",
    'comment', ",
    description!("pg_proc", "p.oid", "0"),
    ")
  FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
  WHERE p.oid = fn.oid))
END::text
FROM fn, locked"
);

/// Look up a relation or an index `$1.$2` (the cast fails with the server's own "does not
/// exist" when it is gone).
const FIND_RELATION: &str = "SELECT pg_catalog.set_config('datarig.ddl_target', \
     'r:' || pg_catalog.format('%I.%I', $1::text, $2::text)::pg_catalog.regclass::pg_catalog.oid, true)";

/// Look up trigger `$3` of table `$1.$2` (`t:` without an `oid` when the table has none).
const FIND_TRIGGER: &str = "SELECT pg_catalog.set_config('datarig.ddl_target', 't:' || coalesce(( \
     SELECT t.oid::text FROM pg_catalog.pg_trigger t \
     WHERE t.tgrelid = pg_catalog.format('%I.%I', $1::text, $2::text)::pg_catalog.regclass AND t.tgname = $3), ''), true)";

/// Look up the function trigger `$3` of table `$1.$2` calls.
const FIND_TRIGGER_FUNCTION: &str = "SELECT pg_catalog.set_config('datarig.ddl_target', 'f:' || coalesce(( \
     SELECT t.tgfoid::text FROM pg_catalog.pg_trigger t \
     WHERE t.tgrelid = pg_catalog.format('%I.%I', $1::text, $2::text)::pg_catalog.regclass AND t.tgname = $3), ''), true)";

/// Look up what name `$1` names in the search path: a function by its signature (`f(int)`),
/// else a relation (or an index), else a function by its name alone (when there is only one of
/// that name). Before PostgreSQL 16 `to_regprocedure` and `to_regclass` fail on a name of the
/// other syntax: each gets only its own.
const FIND_NAMED: &str = "SELECT pg_catalog.set_config('datarig.ddl_target', coalesce(CASE \
     WHEN pg_catalog.strpos($1::text, '(') > 0 THEN 'f:' || pg_catalog.to_regprocedure($1::text)::pg_catalog.oid \
     ELSE coalesce('r:' || pg_catalog.to_regclass($1::text)::pg_catalog.oid, \
                   'f:' || pg_catalog.to_regproc($1::text)::pg_catalog.oid) END, ''), true)";

/// A name the user typed is looked up in the tab's schema (its search path, behind the
/// implicit `pg_catalog`).
const SET_PATH: &str = "SELECT pg_catalog.set_config('search_path', pg_catalog.quote_ident($1::text), true)";

/// From here on the server qualifies every name it prints.
const EMPTY_PATH: &str = "SELECT pg_catalog.set_config('search_path', '', true)";

/// Read the DDL of `object` in one round trip.
pub(crate) async fn load_ddl(client: &Client, object: &DdlObject) -> Result<DdlSource, DbError> {
    let text = |s: &String| s.clone();
    let (find, args, schema, statements): (&str, Vec<String>, Option<String>, &[&str]) = match object {
        DdlObject::Relation { schema, name } | DdlObject::Index { schema, name } => {
            (FIND_RELATION, vec![text(schema), text(name)], None, &[RELATION])
        }
        DdlObject::Trigger { schema, table, name } => {
            (FIND_TRIGGER, vec![text(schema), text(table), text(name)], None, &[TRIGGER])
        }
        DdlObject::TriggerFunction { schema, table, trigger } => {
            (FIND_TRIGGER_FUNCTION, vec![text(schema), text(table), text(trigger)], None, &[FUNCTION])
        }
        DdlObject::Named { name, schema } => (FIND_NAMED, vec![text(name)], schema.clone(), &[RELATION, FUNCTION]),
    };
    let params: Vec<(&(dyn ToSql + Sync), Type)> =
        args.iter().map(|a| (a as &(dyn ToSql + Sync), Type::TEXT)).collect();
    let path = async {
        match &schema {
            Some(s) => client.query_typed(SET_PATH, &[(s, Type::TEXT)]).await.map(drop),
            None => Ok(()),
        }
    };
    let reads = futures::future::join_all(statements.iter().map(|sql| client.query_typed(sql, &[])));
    // Everything goes out together: one round trip.
    let (begun, path, found, emptied, read, _) = tokio::join!(
        client.batch_execute(super::BEGIN_READ),
        path,
        client.query_typed(find, &params),
        client.query_typed(EMPTY_PATH, &[]),
        reads,
        client.batch_execute("COMMIT"),
    );
    let error = |e: tokio_postgres::Error| {
        if e.code() == Some(&SqlState::LOCK_NOT_AVAILABLE) { DbError::Locked } else { super::db_error(&e) }
    };
    begun.map_err(error)?;
    path.map_err(error)?;
    found.map_err(error)?;
    emptied.map_err(error)?;
    let mut answer = None;
    for rows in read {
        if let Some(row) = rows.map_err(error)?.first() {
            answer = answer.or(row.get::<_, Option<String>>(0));
        }
    }
    parse(&answer.ok_or(DbError::NotFound)?)
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    locked: bool,
    relation: Option<RawStructure>,
    ddl: Option<RawRelation>,
    index: Option<RawIndex>,
    trigger: Option<RawTrigger>,
    function: Option<RawFunction>,
}

#[derive(Deserialize)]
struct RawSequence {
    schema: String,
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    start: i64,
    increment: i64,
    min: i64,
    max: i64,
    cache: i64,
    cycle: bool,
    #[serde(default)]
    unlogged: bool,
}

impl From<RawSequence> for SequenceDdl {
    fn from(s: RawSequence) -> Self {
        SequenceDdl {
            schema: s.schema,
            name: s.name,
            type_name: s.type_name,
            start: s.start,
            increment: s.increment,
            min: s.min,
            max: s.max,
            cache: s.cache,
            cycle: s.cycle,
            unlogged: s.unlogged,
        }
    }
}

/// `[grantee, privilege, grantable]`.
type RawGrant = (Option<String>, String, bool);

#[derive(Deserialize)]
struct RawColumn {
    name: String,
    local: bool,
    collation: Option<(String, String)>,
    storage: Option<String>,
    compression: Option<String>,
    statistics: Option<i32>,
    options: Option<Vec<String>>,
    fdw_options: Option<Vec<String>>,
    /// Its default's stored expression (compared with its parent's, never deparsed here).
    default_tree: Option<String>,
    /// A column that comes from a parent: the parent's default and `NOT NULL`.
    parent: Option<RawParentColumn>,
    comment: Option<String>,
    grants: Option<Vec<RawGrant>>,
    identity: Option<RawSequence>,
}

#[derive(Deserialize)]
struct RawParentColumn {
    default_tree: Option<String>,
    not_null: bool,
}

#[derive(Deserialize)]
struct RawOwned {
    column: String,
    sequence: RawSequence,
}

#[derive(Deserialize)]
struct RawConstraint {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    inherited: bool,
    definition: Option<String>,
    /// A `NOT NULL` constraint's column.
    column: Option<String>,
    #[serde(default)]
    no_inherit: bool,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawIndexExtra {
    name: String,
    inherited: bool,
    replica: bool,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawTriggerExtra {
    name: String,
    enabled: String,
    inherited: bool,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawPolicy {
    name: String,
    permissive: bool,
    command: String,
    roles: Option<Vec<Option<String>>>,
    using: Option<String>,
    check: Option<String>,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawRelation {
    schema: String,
    name: String,
    owner: String,
    persistence: String,
    am: Option<String>,
    options: Option<Vec<String>>,
    toast_options: Option<Vec<String>>,
    tablespace: Option<String>,
    partition_key: Option<String>,
    of_type: Option<String>,
    partition_of: Option<(String, String, String)>,
    inherits: Option<Vec<(String, String)>>,
    view: Option<String>,
    server: Option<String>,
    server_options: Option<Vec<String>>,
    columns: Option<Vec<RawColumn>>,
    sequences: Option<Vec<RawOwned>>,
    constraints: Option<Vec<RawConstraint>>,
    indexes: Option<Vec<RawIndexExtra>>,
    triggers: Option<Vec<RawTriggerExtra>>,
    replica_identity: String,
    row_security: bool,
    force_row_security: bool,
    policies: Option<Vec<RawPolicy>>,
    grants: Option<Vec<RawGrant>>,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawIndex {
    schema: String,
    name: String,
    table: String,
    definition: String,
    constraint: Option<(String, String)>,
    constraint_comment: Option<String>,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawTrigger {
    schema: String,
    table: String,
    name: String,
    definition: String,
    enabled: String,
    comment: Option<String>,
}

#[derive(Deserialize)]
struct RawFunction {
    schema: String,
    name: String,
    arguments: String,
    procedure: bool,
    owner: String,
    definition: String,
    /// `proacl` is `NULL`: the default privileges.
    default_acl: bool,
    grants: Option<Vec<RawGrant>>,
    comment: Option<String>,
}

fn grants(raw: Option<Vec<RawGrant>>) -> Vec<Grant> {
    raw.unwrap_or_default()
        .into_iter()
        .map(|(grantee, privilege, grantable)| Grant { grantee, privilege, grantable })
        .collect()
}

/// `pg_trigger.tgenabled`.
fn trigger_mode(code: &str) -> TriggerMode {
    match code {
        "D" => TriggerMode::Disabled,
        "R" => TriggerMode::Replica,
        "A" => TriggerMode::Always,
        _ => TriggerMode::Origin,
    }
}

/// `pg_attribute.attstorage`.
fn storage(code: &str) -> Option<String> {
    let word = match code {
        "p" => "PLAIN",
        "e" => "EXTERNAL",
        "m" => "MAIN",
        "x" => "EXTENDED",
        _ => return None,
    };
    Some(word.to_string())
}

/// `pg_attribute.attcompression` (PostgreSQL 14; empty: the default).
fn compression(code: &str) -> Option<String> {
    match code {
        "p" => Some("pglz".to_string()),
        "l" => Some("lz4".to_string()),
        _ => None,
    }
}

/// `pg_policy.polcmd` (`*`: every command).
fn policy_command(code: &str) -> Option<String> {
    let word = match code {
        "r" => "SELECT",
        "a" => "INSERT",
        "w" => "UPDATE",
        "d" => "DELETE",
        _ => return None,
    };
    Some(word.to_string())
}

/// The document a statement built, as the model.
fn parse(json: &str) -> Result<DdlSource, DbError> {
    let raw: Raw = serde_json::from_str(json).map_err(|e| DbError::Server(format!("DDL: {e}")))?;
    if raw.locked {
        return Err(DbError::Locked);
    }
    if let Some(i) = raw.index {
        // The comment of what it is: the constraint it backs, or the index.
        let comment = if i.constraint.is_some() { i.constraint_comment } else { i.comment };
        return Ok(DdlSource::Index(IndexDdl {
            schema: i.schema,
            table: i.table,
            name: i.name,
            definition: i.definition,
            constraint: i.constraint,
            comment,
        }));
    }
    if let Some(t) = raw.trigger {
        return Ok(DdlSource::Trigger(TriggerDdl {
            schema: t.schema,
            table: t.table,
            name: t.name,
            definition: t.definition,
            mode: trigger_mode(&t.enabled),
            comment: t.comment,
        }));
    }
    if let Some(f) = raw.function {
        return Ok(DdlSource::Function(FunctionDdl {
            schema: f.schema,
            name: f.name,
            arguments: f.arguments,
            procedure: f.procedure,
            owner: f.owner,
            definition: f.definition,
            comment: f.comment,
            grants: (!f.default_acl).then(|| grants(f.grants)),
        }));
    }
    let (Some(structure), Some(d)) = (raw.relation, raw.ddl) else { return Err(DbError::NoResult) };
    let structure = model(structure)?;
    Ok(DdlSource::Relation(Box::new(relation(structure, d))))
}

fn relation(structure: datarig_core::driver::structure::TableStructure, d: RawRelation) -> RelationDdl {
    let mut r = RelationDdl::new(&d.schema, &d.name, &d.owner, structure);
    r.unlogged = d.persistence == "u";
    r.access_method = d.am.filter(|a| a != "heap");
    r.options = d.options.unwrap_or_default();
    r.toast_options = d.toast_options.unwrap_or_default();
    r.tablespace = d.tablespace;
    r.partition_key = d.partition_key;
    r.of_type = d.of_type;
    r.partition_of = d.partition_of.map(|(schema, name, bound)| PartitionOf { schema, name, bound });
    r.inherits = d.inherits.unwrap_or_default();
    r.view_definition = d.view;
    r.foreign = d.server.map(|server| ForeignTable { server, options: d.server_options.unwrap_or_default() });
    let not_null = |name: &str| r.structure.column(name).is_some_and(|c| c.not_null);
    r.columns = d
        .columns
        .unwrap_or_default()
        .into_iter()
        .map(|c| ColumnDdl {
            // A default or `NOT NULL` that its parent does not have is its own.
            own_default: c.parent.as_ref().is_some_and(|p| p.default_tree != c.default_tree),
            own_not_null: c.parent.as_ref().is_some_and(|p| !p.not_null) && not_null(&c.name),
            fdw_options: c.fdw_options.unwrap_or_default(),
            name: c.name,
            local: c.local,
            collation: c.collation,
            storage: c.storage.as_deref().and_then(storage),
            compression: c.compression.as_deref().and_then(compression),
            // `-1` before PostgreSQL 17, `NULL` since: the default.
            statistics: c.statistics.filter(|n| *n >= 0),
            options: c.options.unwrap_or_default(),
            identity: c.identity.map(SequenceDdl::from),
            comment: c.comment,
            grants: grants(c.grants),
        })
        .collect();
    r.sequences = d
        .sequences
        .unwrap_or_default()
        .into_iter()
        .map(|o| OwnedSequence { column: o.column, sequence: o.sequence.into() })
        .collect();
    for c in d.constraints.unwrap_or_default() {
        if c.inherited {
            r.inherited_constraints.push(c.name.clone());
        }
        // A `NOT NULL` constraint (PostgreSQL 18) is said as one only when it is more than the
        // column's `NOT NULL`: a name of its own, or `NO INHERIT`.
        if let (true, Some(column), Some(def)) = (c.kind == "n", &c.column, &c.definition) {
            let default_name = format!("{}_{column}_not_null", d.name);
            if !c.inherited && (c.name != default_name || c.no_inherit) {
                r.not_null_constraints.push((c.name.clone(), column.clone(), def.clone()));
            }
        }
        if let (true, Some(def)) = (c.kind == "x", c.definition) {
            r.exclusions.push((c.name.clone(), def));
        }
        if let Some(text) = c.comment {
            r.constraint_comments.push(Comment { name: c.name, text });
        }
    }
    let constraint_index = |name: &str| r.structure.indexes.iter().any(|i| i.name == name && i.constraint);
    let mut replica = None;
    let mut index_comments = Vec::new();
    for i in d.indexes.unwrap_or_default() {
        if i.replica {
            replica = Some(i.name.clone());
        }
        if i.inherited {
            r.inherited_indexes.push(i.name.clone());
        }
        // A constraint's index has the constraint's comment.
        if let (Some(text), false, false) = (i.comment, i.inherited, constraint_index(&i.name)) {
            index_comments.push(Comment { name: i.name, text });
        }
    }
    r.index_comments = index_comments;
    // Only a table has one (a view says `n`).
    let table = matches!(r.structure.kind, RelationKind::Table | RelationKind::PartitionedTable);
    r.replica_identity = match (d.replica_identity.as_str(), replica) {
        _ if !table => ReplicaIdentity::Default,
        ("n", _) => ReplicaIdentity::Nothing,
        ("f", _) => ReplicaIdentity::Full,
        ("i", Some(name)) => ReplicaIdentity::Index(name),
        // Its index was dropped: the server then acts as with `NOTHING` (as documented).
        ("i", None) => ReplicaIdentity::Nothing,
        _ => ReplicaIdentity::Default,
    };
    r.triggers = d
        .triggers
        .unwrap_or_default()
        .into_iter()
        .map(|t| TriggerExtra {
            name: t.name,
            mode: trigger_mode(&t.enabled),
            inherited: t.inherited,
            comment: t.comment,
        })
        .collect();
    r.row_security = d.row_security;
    r.force_row_security = d.force_row_security;
    r.policies = d
        .policies
        .unwrap_or_default()
        .into_iter()
        .map(|p| Policy {
            name: p.name,
            permissive: p.permissive,
            command: policy_command(&p.command),
            roles: p.roles.unwrap_or_default(),
            using: p.using,
            check: p.check,
            comment: p.comment,
        })
        .collect();
    r.grants = grants(d.grants);
    r.comment = d.comment;
    r
}

#[cfg(test)]
mod tests;
