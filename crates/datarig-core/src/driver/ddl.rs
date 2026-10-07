//! What a DDL view shows: an object's definition as the server's catalog has it, read by a
//! driver with `Capabilities::ddl` (`DbCommand::LoadDdl`) and written out as SQL by
//! [`crate::sql::ddl`].
//!
//! The model is the catalog's: a relation's [`TableStructure`] and what a `CREATE` statement
//! needs beyond it (its owner, options, partitioning, policies, grants, comments, the sequences
//! its columns own), an index's, a trigger's or a function's definition as the server prints
//! it. Names are as the catalog has them (unquoted); expressions, types and definitions are as
//! the server prints them (quoted and qualified where SQL needs it).

use super::structure::TableStructure;

/// The object whose DDL is asked for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DdlObject {
    /// A table, a view, a materialized view or a foreign table.
    Relation { schema: String, name: String },
    /// An index (of a table in the same schema).
    Index { schema: String, name: String },
    /// Trigger `name` of table (or view) `schema.table`.
    Trigger { schema: String, table: String, name: String },
    /// The function trigger `trigger` of `schema.table` calls.
    TriggerFunction { schema: String, table: String, trigger: String },
    /// What `name` names, written as SQL writes a name (`shop.users`, `"Mixed Case"`,
    /// `area(int)`): a relation, an index or a function, looked up in `schema` (the session's
    /// search path when `None`).
    Named { name: String, schema: Option<String> },
}

impl DdlObject {
    /// The object as the UI names it (a tab's title): `schema.name`, a trigger's
    /// `schema.table.trigger`, the function of one with `()`; a named one as it was typed.
    pub fn label(&self) -> String {
        match self {
            DdlObject::Relation { schema, name } | DdlObject::Index { schema, name } => format!("{schema}.{name}"),
            DdlObject::Trigger { schema, table, name } => format!("{schema}.{table}.{name}"),
            DdlObject::TriggerFunction { schema, table, trigger } => format!("{schema}.{table}.{trigger}()"),
            DdlObject::Named { name, .. } => name.clone(),
        }
    }
}

/// An object's DDL as the catalog has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdlSource {
    Relation(Box<RelationDdl>),
    Index(IndexDdl),
    Trigger(TriggerDdl),
    Function(FunctionDdl),
    /// DDL text the server writes itself (MySQL's `SHOW CREATE …`), shown as it is: `name` is
    /// the object's name as the server gives it. PostgreSQL never sends one: its DDL is
    /// rebuilt from the catalog.
    Verbatim {
        name: String,
        text: String,
    },
}

impl DdlSource {
    /// The object's own `schema.name` (a trigger's `schema.table.trigger`, a function's
    /// `schema.name(arguments)`), as the catalog has the names.
    pub fn label(&self) -> String {
        match self {
            DdlSource::Relation(r) => format!("{}.{}", r.schema, r.name),
            DdlSource::Index(i) => format!("{}.{}", i.schema, i.name),
            DdlSource::Trigger(t) => format!("{}.{}.{}", t.schema, t.table, t.name),
            DdlSource::Function(f) => format!("{}.{}({})", f.schema, f.name, f.arguments),
            DdlSource::Verbatim { name, .. } => name.clone(),
        }
    }
}

/// A privilege granted on an object (a column's, for a column).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// The role it is granted to; `None`: `PUBLIC`.
    pub grantee: Option<String>,
    /// As SQL writes it (`SELECT`, `UPDATE`).
    pub privilege: String,
    /// `WITH GRANT OPTION`.
    pub grantable: bool,
}

/// A sequence: one a column owns (`serial`), or an identity column's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceDdl {
    pub schema: String,
    pub name: String,
    /// Its type (`smallint`, `integer`, `bigint`).
    pub type_name: String,
    pub start: i64,
    pub increment: i64,
    pub min: i64,
    pub max: i64,
    pub cache: i64,
    pub cycle: bool,
    /// `UNLOGGED` (PostgreSQL 15; a sequence an unlogged table owns).
    pub unlogged: bool,
}

/// What a column has beyond the structure's (the same order as the structure's columns).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnDdl {
    pub name: String,
    /// Defined by the table itself (an inherited-only column is not: `INHERITS` brings it).
    pub local: bool,
    /// Its collation when it is not its type's: schema and name.
    pub collation: Option<(String, String)>,
    /// Its storage when it is not its type's (`PLAIN`, `EXTERNAL`, `MAIN`, `EXTENDED`).
    pub storage: Option<String>,
    /// Its compression method when one is set (`pglz`, `lz4`).
    pub compression: Option<String>,
    /// Its statistics target when one is set.
    pub statistics: Option<i32>,
    /// Its attribute options (`n_distinct=100`).
    pub options: Vec<String>,
    /// A foreign table column's options (`column_name=id`).
    pub fdw_options: Vec<String>,
    /// A column that comes from a parent (a partition's, an inheriting table's) with a default
    /// of its own: the structure's default (none: it dropped the parent's).
    pub own_default: bool,
    /// Such a column is `NOT NULL` where its parent's is not.
    pub own_not_null: bool,
    /// An identity column's sequence.
    pub identity: Option<SequenceDdl>,
    pub comment: Option<String>,
    /// Privileges granted on the column alone.
    pub grants: Vec<Grant>,
}

/// A sequence a column owns (`ALTER SEQUENCE … OWNED BY`), as `serial` makes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedSequence {
    pub column: String,
    pub sequence: SequenceDdl,
}

/// The parent a partition is a partition of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionOf {
    pub schema: String,
    pub name: String,
    /// Its bound (`FOR VALUES FROM (0) TO (10)`, `DEFAULT`).
    pub bound: String,
}

/// A foreign table's server and options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignTable {
    pub server: String,
    /// `name=value`, as the catalog keeps them.
    pub options: Vec<String>,
}

/// A row-level security policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub name: String,
    /// `AS PERMISSIVE` (else `AS RESTRICTIVE`).
    pub permissive: bool,
    /// The command it applies to (`SELECT`, `INSERT`, `UPDATE`, `DELETE`), `None`: `ALL`.
    pub command: Option<String>,
    /// The roles it applies to; `None` among them: `PUBLIC`.
    pub roles: Vec<Option<String>>,
    pub using: Option<String>,
    pub check: Option<String>,
    pub comment: Option<String>,
}

/// How a table's changes identify their rows for logical replication.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReplicaIdentity {
    /// The primary key.
    #[default]
    Default,
    Nothing,
    Full,
    /// That unique index.
    Index(String),
}

/// When a trigger fires (`session_replication_role`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TriggerMode {
    /// Enabled (in the `origin` and `local` roles).
    #[default]
    Origin,
    Disabled,
    /// Only in the `replica` role.
    Replica,
    Always,
}

/// What a trigger of a relation has beyond the structure's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerExtra {
    pub name: String,
    pub mode: TriggerMode,
    /// A partition's copy of its parent's trigger (the parent's DDL creates it).
    pub inherited: bool,
    pub comment: Option<String>,
}

/// A named object of a relation and its comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub name: String,
    pub text: String,
}

/// A table, a view, a materialized view or a foreign table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationDdl {
    pub schema: String,
    pub name: String,
    /// Its columns, keys, indexes, constraints and triggers (no estimates).
    pub structure: TableStructure,
    pub owner: String,
    /// `UNLOGGED`.
    pub unlogged: bool,
    /// Its table access method when it is not the default (`heap`).
    pub access_method: Option<String>,
    /// Its storage parameters (`fillfactor=70`; a view's `check_option=local`).
    pub options: Vec<String>,
    /// Its TOAST table's (written `toast.<name>`).
    pub toast_options: Vec<String>,
    pub tablespace: Option<String>,
    /// A partitioned table's key (`RANGE (created)`).
    pub partition_key: Option<String>,
    pub partition_of: Option<PartitionOf>,
    /// The tables it inherits from (not as a partition), in order: schema and name.
    pub inherits: Vec<(String, String)>,
    /// A view's or a materialized view's query, as the server prints it.
    pub view_definition: Option<String>,
    pub foreign: Option<ForeignTable>,
    pub columns: Vec<ColumnDdl>,
    /// Constraints that come from a parent (inherited, or a partition's copy): not its own.
    pub inherited_constraints: Vec<String>,
    /// Exclusion constraints (the structure has no group for them): name and definition.
    pub exclusions: Vec<(String, String)>,
    /// `NOT NULL` constraints with a name of their own or `NO INHERIT` (PostgreSQL 18): name,
    /// column and definition. Their columns are not marked `NOT NULL` inline.
    pub not_null_constraints: Vec<(String, String, String)>,
    /// A typed table's type (`OF type`): its columns are the type's.
    pub of_type: Option<String>,
    /// Indexes that are a partition of the parent's index (the parent's DDL creates them).
    pub inherited_indexes: Vec<String>,
    pub replica_identity: ReplicaIdentity,
    pub row_security: bool,
    pub force_row_security: bool,
    pub policies: Vec<Policy>,
    pub triggers: Vec<TriggerExtra>,
    /// Sequences its columns own (`serial`).
    pub sequences: Vec<OwnedSequence>,
    pub grants: Vec<Grant>,
    pub comment: Option<String>,
    /// Comments of its constraints and indexes.
    pub constraint_comments: Vec<Comment>,
    pub index_comments: Vec<Comment>,
}

impl RelationDdl {
    /// A relation `schema.name` of `structure`, owned by `owner`, with nothing else.
    pub fn new(schema: &str, name: &str, owner: &str, structure: TableStructure) -> Self {
        let columns = structure
            .columns
            .iter()
            .map(|c| ColumnDdl { name: c.name.clone(), local: true, ..ColumnDdl::default() })
            .collect();
        Self {
            schema: schema.to_string(),
            name: name.to_string(),
            structure,
            owner: owner.to_string(),
            unlogged: false,
            access_method: None,
            options: Vec::new(),
            toast_options: Vec::new(),
            tablespace: None,
            partition_key: None,
            partition_of: None,
            inherits: Vec::new(),
            view_definition: None,
            foreign: None,
            columns,
            inherited_constraints: Vec::new(),
            exclusions: Vec::new(),
            not_null_constraints: Vec::new(),
            of_type: None,
            inherited_indexes: Vec::new(),
            replica_identity: ReplicaIdentity::Default,
            row_security: false,
            force_row_security: false,
            policies: Vec::new(),
            triggers: Vec::new(),
            sequences: Vec::new(),
            grants: Vec::new(),
            comment: None,
            constraint_comments: Vec::new(),
            index_comments: Vec::new(),
        }
    }
}

/// An index, or the constraint it backs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDdl {
    pub schema: String,
    /// Its table.
    pub table: String,
    pub name: String,
    /// The server's `CREATE INDEX` statement.
    pub definition: String,
    /// The primary key, unique or exclusion constraint it backs: its name and definition.
    pub constraint: Option<(String, String)>,
    /// The index's comment (the constraint's, for one that backs a constraint).
    pub comment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerDdl {
    pub schema: String,
    pub table: String,
    pub name: String,
    /// The server's `CREATE TRIGGER` statement.
    pub definition: String,
    pub mode: TriggerMode,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionDdl {
    pub schema: String,
    pub name: String,
    /// Its arguments as its identity (`integer, text`).
    pub arguments: String,
    /// A procedure (else a function).
    pub procedure: bool,
    pub owner: String,
    /// The server's `CREATE OR REPLACE FUNCTION` statement.
    pub definition: String,
    pub comment: Option<String>,
    /// The privileges granted on it, when they are not the defaults (`EXECUTE` to `PUBLIC`):
    /// `None` for the defaults. The owner's are left out.
    pub grants: Option<Vec<Grant>>,
}
