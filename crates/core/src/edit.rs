//! Editing a result's rows: which table the rows are, which of its columns a change may set, and
//! how a row is found again when the change is applied ([`EditTarget`]).
//!
//! Everything here is the same for every database. What differs (which ways of finding a row a
//! server offers, which values compare exactly, the SQL text) is a format's, behind [`EditSql`]:
//! one writer per [`EditFormat`](crate::driver::EditFormat), chosen by the driver's
//! `Capabilities::edit`.
//!
//! A result is resolved in two steps, so the cheap one can run when the result arrives and the
//! table's structure is read only when the user starts to edit:
//!
//! 1. [`source`]: from the statement and the result's column origins, the one table the rows
//!    are and the table column of each result column (none for a computed one). An allowlist:
//!    anything but a projection of one table's rows is read-only.
//! 2. [`EditSource::target`]: with the table's structure, what each column allows and how a row
//!    is found ([`Identity`]), in the order the format prefers.
//!
//! The user's changes collect in an [`EditBatch`] on top of the rows as read; [`plan`] turns
//! them into the statements of one Apply, each with the count it must give.

mod batch;
mod plan;
mod sql;
mod target;

pub use batch::{Counts, Draft, EditBatch, RowEdit};
pub use plan::{Choice, Expect, Facts, Origin, Plan, PlanError, Step, Where, count_statements, plan};
pub use sql::{ColumnInfo, EditSql, Exactness, Inexact, Offer, Scope, Value};
pub use target::{
    ColumnReason, Duplicates, EditSource, EditTarget, Identity, ReadOnly, SourceColumn, TableColumn, TableRef, Write,
    source,
};

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
