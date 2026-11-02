//! An edit format: the ways it finds a row again, which values it compares exactly, and the text
//! of its statements. The planner (`plan`) is the only caller of the statement builders, and it
//! checks every statement they write before it keeps it.

use super::plan::Where;
use super::target::{TableColumn, TableRef};
use crate::driver::ValueKind;
use crate::driver::structure::StructureColumn;
use crate::sql::dialect::Language;

/// A way of finding a row of a table again ([`EditSql::offers`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// By its primary key, when the result holds every column of it.
    Primary,
    /// By a unique key whose columns are all `NOT NULL`, when the result holds every column of
    /// it.
    Unique,
    /// By every value of it the result shows that compares exactly.
    Values,
}

/// Whether a value read from a result finds its row again when compared with the column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exactness {
    /// The value as read compares equal to the stored one, and only to it.
    Exact,
    /// It may not: the column is left out when a row is found by its values.
    Inexact(Inexact),
}

/// Why a column's values do not compare exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inexact {
    /// A floating-point number: its text may round.
    Approximate,
    /// A document (JSON, XML): its text may be another spelling of the stored value.
    Document,
    /// A geometric or spatial value.
    Geometry,
    /// A type the format does not know compares exactly.
    Unknown,
}

/// A table column a result column comes from, as a format judges it.
#[derive(Clone, Copy, Debug)]
pub struct ColumnInfo<'a> {
    /// What the result read its values as.
    pub kind: ValueKind,
    /// The result's type name for it, as the server named it.
    pub result_type: &'a str,
    /// The table's column.
    pub column: &'a StructureColumn,
}

/// What a cell of a change is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// This value, as text in the form the result shows the column's values.
    Text(String),
    Null,
    /// The column's default (its identity, auto-increment or default expression).
    Default,
}

/// Which of the rows a statement's condition matches it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every one.
    All,
    /// One of them (they are the same in every column).
    One,
}

/// The SQL of one edit format. Its answers depend on the format alone (and the column asked
/// about), never on the session. The statement builders write one statement each and use the
/// condition they are given as it is: the planner checks both.
pub trait EditSql {
    /// The ways it finds a row again, in the order it prefers them.
    fn offers(&self) -> &'static [Offer];

    /// Whether values of column `c` compare exactly: the format's statements find a row by
    /// comparing such a value byte for byte (as stored), never by the column's collation, so a
    /// value matches only the rows that hold that very value, at least as finely as any unique
    /// index compares. A type it does not know is [`Inexact::Unknown`]: an allowlist.
    fn exactness(&self, c: &ColumnInfo) -> Exactness;

    /// The language its statements are written in (the planner reads them back in it).
    fn language(&self) -> Language;

    /// Table column `name` as an identifier.
    fn column(&self, name: &str) -> String;

    /// Value `v` of column `c` as SQL: a literal of the column's type, `NULL` or `DEFAULT`.
    fn value(&self, c: &TableColumn, v: &Value) -> String;

    /// A condition true exactly for the rows whose column `c` holds `v` (`None`: NULL), the
    /// value as the result read it: NULL-safe, and byte for byte for an exact column.
    fn eq_exact(&self, c: &TableColumn, v: Option<&str>) -> String;

    /// How many rows of `t` match `w`: a plain read, which locks no row.
    fn count(&self, t: &TableRef, w: &Where) -> String;

    /// A read of the rows of `t` that match `w`, one row each, that locks them, failing at once
    /// when another session holds one.
    fn check(&self, t: &TableRef, w: &Where) -> String;

    /// Set `set` (column, SQL value) in the rows of `t` that match `w`, or in one of them; `None`
    /// when the format cannot change just one.
    fn update(&self, t: &TableRef, set: &[(&TableColumn, String)], w: &Where, scope: Scope) -> Option<String>;

    /// Delete the rows of `t` that match `w`, or one of them; `None` when the format cannot
    /// delete just one.
    fn delete(&self, t: &TableRef, w: &Where, scope: Scope) -> Option<String>;

    /// What an `UPDATE` or a `DELETE` of its may hold after the condition it was given:
    /// nothing, or its own end (`LIMIT 1`, the close of the subquery that picks one row,
    /// `RETURNING *`), each exactly as it writes it.
    fn tails(&self) -> &'static [&'static str];

    /// Insert one row into `t` with `cells` (column, SQL value); the other columns take their
    /// defaults (none given: every column does).
    fn insert(&self, t: &TableRef, cells: &[(&TableColumn, String)]) -> String;
}
