//! What an edit format says about a table: the ways it finds a row again, and which values it
//! compares exactly.

use crate::driver::ValueKind;
use crate::driver::structure::StructureColumn;

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

/// The SQL of one edit format. Its answers depend on the format alone (and the column asked
/// about), never on the session.
pub trait EditSql {
    /// The ways it finds a row again, in the order it prefers them.
    fn offers(&self) -> &'static [Offer];

    /// Whether values of column `c` compare exactly: the format's statements find a row by
    /// comparing such a value byte for byte (as stored), never by the column's collation, so a
    /// value matches only the rows that hold that very value, at least as finely as any unique
    /// index compares. A type it does not know is [`Inexact::Unknown`]: an allowlist.
    fn exactness(&self, c: &ColumnInfo) -> Exactness;
}
