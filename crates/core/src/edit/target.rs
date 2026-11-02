//! The table a result's rows are, and how each of its rows is found again.

use super::sql::{ColumnInfo, EditSql, Exactness, Offer};
use crate::driver::keys::{NotInsertable, single_table};
use crate::driver::structure::{ColumnFill, RelationKind, StructureColumn, TableStructure};
use crate::driver::{ColumnMeta, KeyCatalog, ValueKind};
use crate::sql::dialect::Dialect;

/// A table by schema and name, as the catalog has them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

/// Why a result's rows cannot be edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    /// The connection's driver (or its server) takes no edits.
    NoFormat,
    /// The rows are not provably rows of one table: a join, a subquery, grouping, a view, or
    /// a table the key cache does not know (the reason, as the `INSERT` copy refuses them).
    Source(NotInsertable),
    /// No column of the result is a column of the table (each is computed).
    NoTableColumn,
    /// The relation is not a plain table (a view, a materialized view, a foreign table).
    NotATable(RelationKind),
    /// A column of the result is not in the table's structure as read: the table changed.
    TableChanged,
    /// The table offers no way of finding a row the format takes.
    NoIdentity,
    /// Rows would be found by their values, but none of the result's columns compares
    /// exactly.
    NoExactColumn,
}

/// One result column of a result that may be editable: the table column it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceColumn {
    /// The table column's name, as the catalog has it (not the result's alias).
    pub name: String,
    /// The table column is shown more than once in the result.
    pub repeated: bool,
    /// The kind the result reads its values as.
    pub kind: ValueKind,
    /// The result's type name for it, as the server named it.
    pub result_type: String,
}

/// The table a result's rows are, from its statement and column origins
/// ([`source`]), before the table's structure is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditSource {
    pub table: TableRef,
    /// Per result column, the table column it is; `None` for a computed one.
    pub columns: Vec<Option<SourceColumn>>,
}

/// The table rows of a result are, when that is sure: `sql` (the statement that produced the
/// result) must be one `SELECT` of exactly one plain table ([`single_table`]), every result
/// column with an origin must be a column of that table, and the table must not be a view.
/// Unlike the `INSERT` copy, a computed column is allowed: it is read-only, the others are not.
pub fn source(
    d: Dialect,
    catalog: Option<&KeyCatalog>,
    cols: &[ColumnMeta],
    sql: &str,
) -> Result<EditSource, ReadOnly> {
    use NotInsertable::*;
    let from = single_table(d, sql).map_err(ReadOnly::Source)?;
    let catalog = catalog.ok_or(ReadOnly::Source(NoCatalog))?;
    let located = cols
        .iter()
        .map(|c| c.origin.as_ref().map(|o| catalog.locate(o).ok_or(ReadOnly::Source(UnknownTable))).transpose())
        .collect::<Result<Vec<Option<(u32, i16)>>, _>>()?;
    let mut tables = located.iter().flatten().map(|(t, _)| *t);
    let id = tables.next().ok_or(ReadOnly::NoTableColumn)?;
    if tables.any(|t| t != id) {
        return Err(ReadOnly::Source(SeveralTables));
    }
    let table = catalog.table(id).ok_or(ReadOnly::Source(UnknownTable))?;
    if !from.names(table, catalog.names()) {
        return Err(ReadOnly::Source(OtherTable));
    }
    if table.view {
        return Err(ReadOnly::Source(View));
    }
    let columns = located
        .iter()
        .zip(cols)
        .map(|(l, meta)| {
            let Some((_, n)) = l else { return Ok(None) };
            let c = table.columns.get(n).ok_or(ReadOnly::Source(UnknownTable))?;
            let repeated = located.iter().flatten().filter(|(_, m)| m == n).count() > 1;
            let (kind, result_type) = (meta.kind, meta.type_name.clone());
            Ok(Some(SourceColumn { name: c.name.clone(), repeated, kind, result_type }))
        })
        .collect::<Result<_, ReadOnly>>()?;
    Ok(EditSource { table: TableRef { schema: table.schema.clone(), name: table.name.clone() }, columns })
}

/// What a change does to a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Set it in an existing row.
    Update,
    /// Give it in a new row.
    Insert,
}

/// Why a column of an editable result cannot be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnReason {
    /// It is computed in the statement, not a column of the table.
    Computed,
    /// The database computes it (`GENERATED ALWAYS AS (…)`).
    Generated,
    /// `GENERATED ALWAYS AS IDENTITY`: an existing row's value is not changed (a new row may
    /// be given one).
    IdentityAlways,
    /// The same table column is shown more than once: one copy would contradict the other.
    Repeated,
}

/// A result column that is a column of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableColumn {
    /// The table's column (its name as the catalog has it).
    pub column: StructureColumn,
    /// The kind the result reads its values as.
    pub kind: ValueKind,
    pub exact: Exactness,
    pub repeated: bool,
}

impl TableColumn {
    /// Whether `w` may set it, and why not.
    pub fn allows(&self, w: Write) -> Result<(), ColumnReason> {
        if self.repeated {
            return Err(ColumnReason::Repeated);
        }
        match (&self.column.fill, w) {
            (ColumnFill::Stored(_) | ColumnFill::Virtual(_), _) => Err(ColumnReason::Generated),
            (ColumnFill::IdentityAlways, Write::Update) => Err(ColumnReason::IdentityAlways),
            _ => Ok(()),
        }
    }
}

/// When rows are found by their values, what to do when several rows match one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Duplicates {
    /// Refuse the change: the matching rows may differ in values the result does not show or
    /// that do not compare exactly, or the table has a key the result does not hold.
    Refuse,
    /// The rows are the same in every column (the table has no key, the result shows every
    /// column, and every one compares exactly): the user may change one of them or all.
    Choose,
}

/// How a row of the result is found in the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    /// By a key the result holds whole: the primary key, or a unique key of `NOT NULL`
    /// columns. The result columns of the key, in the key's order.
    Key { primary: bool, columns: Vec<usize> },
    /// By every value the result shows that compares exactly: these result columns (each table
    /// column once).
    Values { columns: Vec<usize>, duplicates: Duplicates },
}

/// An editable result: its table, what each column allows, and how a row is found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditTarget {
    pub table: TableRef,
    /// Per result column, the table column it is; `None` for a computed one.
    pub columns: Vec<Option<TableColumn>>,
    pub identity: Identity,
    /// Every column of the table is in the result.
    pub all_columns_shown: bool,
}

impl EditTarget {
    /// Whether `w` may set result column `i`, and why not.
    pub fn allows(&self, i: usize, w: Write) -> Result<&TableColumn, ColumnReason> {
        let c = self.columns.get(i).and_then(Option::as_ref).ok_or(ColumnReason::Computed)?;
        c.allows(w).map(|()| c)
    }
}

impl EditSource {
    /// With the table's structure `s`: what each result column allows, and how a row is found,
    /// the first of the format's offers the table and result allow.
    ///
    /// A table others inherit from (legacy inheritance) is read with their rows, and its keys
    /// do not hold across them: its rows are found by their values only, and never as
    /// duplicates of each other.
    pub fn target(&self, s: &TableStructure, f: &dyn EditSql) -> Result<EditTarget, ReadOnly> {
        if !matches!(s.kind, RelationKind::Table | RelationKind::PartitionedTable) {
            return Err(ReadOnly::NotATable(s.kind));
        }
        let columns = self
            .columns
            .iter()
            .map(|src| {
                let Some(src) = src else { return Ok(None) };
                let column = s.column(&src.name).ok_or(ReadOnly::TableChanged)?.clone();
                let info = ColumnInfo { kind: src.kind, result_type: &src.result_type, column: &column };
                let exact = f.exactness(&info);
                Ok(Some(TableColumn { exact, kind: src.kind, repeated: src.repeated, column }))
            })
            .collect::<Result<Vec<Option<TableColumn>>, ReadOnly>>()?;
        let shown = |name: &str| columns.iter().flatten().any(|c| c.column.name == name);
        let all_columns_shown = s.columns.iter().all(|c| shown(&c.name));
        // The result column of table column `name`, when it compares exactly (its first copy).
        let exact = |name: &str| {
            columns
                .iter()
                .position(|c| c.as_ref().is_some_and(|c| c.column.name == name && c.exact == Exactness::Exact))
        };
        let whole = |key: &[String]| key.iter().map(|c| exact(c)).collect::<Option<Vec<usize>>>();
        let not_null = |key: &[String]| key.iter().all(|c| s.column(c).is_some_and(|c| c.not_null));
        // Unique keys of `NOT NULL` columns: constraints, then unique indexes on plain columns
        // that are not partial.
        let uniques: Vec<Vec<String>> = s
            .unique_constraints
            .iter()
            .map(|u| u.columns.clone())
            .chain(
                s.indexes
                    .iter()
                    .filter(|x| x.unique && !x.primary && x.predicate.is_none())
                    .filter_map(|x| x.key_columns.iter().cloned().collect::<Option<Vec<String>>>()),
            )
            .filter(|k| !k.is_empty() && not_null(k))
            .collect();
        let keyed = s.inherited || s.primary_key.is_some() || !uniques.is_empty();
        let identity = || {
            for offer in f.offers() {
                match offer {
                    Offer::Primary if !s.inherited => {
                        if let Some(columns) = s.primary_key.as_ref().and_then(|k| whole(&k.columns)) {
                            return Ok(Identity::Key { primary: true, columns });
                        }
                    }
                    Offer::Unique if !s.inherited => {
                        if let Some(columns) = uniques.iter().find_map(|k| whole(k)) {
                            return Ok(Identity::Key { primary: false, columns });
                        }
                    }
                    Offer::Primary | Offer::Unique => {}
                    Offer::Values => {
                        // Each exact table column once (its first copy).
                        let found: Vec<usize> = (0..columns.len())
                            .filter(|&i| columns[i].as_ref().is_some_and(|c| exact(&c.column.name) == Some(i)))
                            .collect();
                        if found.is_empty() {
                            return Err(ReadOnly::NoExactColumn);
                        }
                        let all_exact = columns.iter().flatten().all(|c| c.exact == Exactness::Exact);
                        let duplicates = if !keyed && all_columns_shown && all_exact {
                            Duplicates::Choose
                        } else {
                            Duplicates::Refuse
                        };
                        return Ok(Identity::Values { columns: found, duplicates });
                    }
                }
            }
            Err(ReadOnly::NoIdentity)
        };
        let identity = identity()?;
        Ok(EditTarget { table: self.table.clone(), columns, identity, all_columns_shown })
    }
}
