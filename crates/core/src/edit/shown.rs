//! What a new row's cell left to its column's default is shown as, before it is applied: the
//! value the server will put there when it is a constant, `<generated>` for a number it draws
//! or a column it computes, NULL, the default's expression, or that the cell needs a value
//! ([`needs_value`]). Only shown: the row is still sent with the column left out, so the
//! server applies its own default.

use crate::driver::structure::{ColumnFill, StructureColumn};
use crate::sql::dialect::{Constant, Dialect};

/// What a new row's cell left to its default shows ([`default_shown`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultShown {
    /// `NOT NULL` without a default: the server would refuse the row ([`needs_value`]).
    Needed,
    /// A number the table draws (an identity, `AUTO_INCREMENT`, a sequence) or a generated
    /// column.
    Generated,
    /// NULL: a column that takes NULL without a default, or a default of NULL.
    Null,
    /// A constant default: the value (a string unquoted, its casts gone).
    Value(String),
    /// Any other default (`now()`, `CURRENT_TIMESTAMP`), without parentheses around it.
    Expression(String),
}

/// Whether a new row must give column `c` a value: `NOT NULL`, no default expression, not
/// numbered nor computed by the table.
pub fn needs_value(c: &StructureColumn) -> bool {
    c.not_null && c.default.is_none() && c.fill == ColumnFill::Default
}

/// What a new row's cell of column `c` left to its default shows, its default read as SQL of
/// dialect `d`: the session's, in which the driver writes a literal default (an expression
/// default as its server prints it).
pub fn default_shown(c: &StructureColumn, d: Dialect) -> DefaultShown {
    if c.fill != ColumnFill::Default {
        return DefaultShown::Generated;
    }
    if needs_value(c) {
        return DefaultShown::Needed;
    }
    let Some(sql) = c.default.as_deref() else { return DefaultShown::Null };
    let sql = d.insert_default(sql);
    // MySQL: an expression default (the driver puts it in parentheses) is the server's own
    // text, in its default mode; a literal one the driver wrote in the session's mode.
    let d = match d {
        Dialect::MySql(_) if sql.trim_start().starts_with('(') => d.printed(),
        _ => d,
    };
    if d.draws_number(sql) {
        return DefaultShown::Generated;
    }
    match d.constant(sql) {
        Some(Constant::Null) => DefaultShown::Null,
        Some(Constant::Value(v)) => DefaultShown::Value(v),
        None => DefaultShown::Expression(d.unparenthesized(sql).to_string()),
    }
}

#[cfg(test)]
mod tests;
