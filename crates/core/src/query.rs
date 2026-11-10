//! A query as data: the rows of one table, a filter, an order and a cap ([`Query`]), compiled to
//! SQL of a dialect ([`compile`]).
//!
//! The filter is text the user typed (or an app action wrote into the bar): it is sent as typed,
//! inside one pair of parentheses, never rewritten. What the app understands of it
//! ([`recognise`]) is kept beside it ([`Filter::recognised`]) for marks and history; `None`
//! there means "not understood", not "no filter". An app action that filters by a cell's value
//! writes the text of a condition ([`render`]) for the user to see before it runs. The order is
//! keys, read from the text of an `ORDER BY` field ([`parse_order`]) and written back as such
//! ([`order_text`]).
//!
//! Before any statement built from a filter runs, [`check_filter`] checks the text: one boolean
//! expression and nothing else (no `;`, no comment, no parenthesis that leaves ours), a plain
//! read of our one table by the risk classifier, and no call of a function that is not known to
//! be free of side effects without the user's say (refused outright on a read-only profile).
//!
//! Every name and string written here goes through the dialect ([`Dialect`]): nothing is
//! PostgreSQL's or MySQL's but what the dialect says.

mod check;
mod compile;
mod order;
mod recognise;
mod render;

pub use check::{
    BarRefusal, Confirm, FilterVerdict, LexProblem, MYSQL_FILTER_CALLS, MYSQL_REFUSED_CALLS, PG_FILTER_CALLS,
    PG_REFUSED_CALLS, ShapeProblem, check_filter,
};
pub use compile::{Compiled, capped, compile, compile_capped};
pub use order::{OrderError, OrderProblem, SortStep, order_text, parse_order, sort_column};
pub use recognise::recognise;
pub use render::{RenderError, and_filter, like_contains, like_starts_with, render};

use crate::sql::dialect::Dialect;
use serde::{Deserialize, Serialize};

/// A column's name, exactly as the catalog has it (any case, never quoted).
pub type ColName = String;

/// The rows a query reads, filtered, ordered and capped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub source: Source,
    pub filter: Option<Filter>,
    /// The sort keys, in order. Tie-breaker keys ([`KeyOrigin::TieBreak`]) kept here are dropped
    /// and worked out again by [`compile`].
    pub order: Vec<OrderKey>,
    /// A cap on the total number of rows. The grid run leaves it out of the SQL (its pager stops
    /// there); [`compile_capped`] writes it.
    pub limit: Option<u64>,
    /// Rows to skip before the first. Like `limit`, only [`compile_capped`] writes it.
    pub offset: Option<u64>,
}

impl Query {
    /// Every row of table `t`, unordered.
    pub fn table(t: TableName) -> Self {
        Self { source: Source::Table(t), filter: None, order: Vec::new(), limit: None, offset: None }
    }
}

/// Where a query's rows come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Table(TableName),
}

/// A table by its names as the catalog has them. `database` is the database a session must be
/// connected to (PostgreSQL) or is `None` for the session's own; it is not written into the SQL.
/// On MySQL the schema is the database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableName {
    pub database: Option<String>,
    pub schema: String,
    pub name: String,
}

/// The `WHERE` part of a query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// What is sent, verbatim, inside one pair of parentheses.
    pub text: String,
    /// What the app understood of `text` ([`recognise`]); `None` when it understood nothing,
    /// which does not mean the text filters nothing.
    pub recognised: Option<Cond>,
}

/// A comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    /// The operator as SQL (`<>` for not equal).
    pub fn sql(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "<>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// A condition on a table's rows, as far as the app models one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cond {
    /// Every condition holds.
    All(Vec<Cond>),
    /// At least one condition holds.
    Any(Vec<Cond>),
    Not(Box<Cond>),
    /// `col op value` (never with a NULL value: see [`Cond::IsNull`] and [`Cond::NotDistinct`]).
    Cmp {
        col: ColName,
        op: CmpOp,
        value: Value,
    },
    /// NULL-safe equality: NULL equals NULL. `negated`: NULL-safe "not equal", which keeps the
    /// rows whose value is NULL.
    NotDistinct {
        col: ColName,
        value: Value,
        negated: bool,
    },
    IsNull {
        col: ColName,
        negated: bool,
    },
    /// `col IN (values)`. A NULL among the values is written as its own test of NULL.
    In {
        col: ColName,
        values: Vec<Value>,
        negated: bool,
    },
    /// `col LIKE pattern`, the pattern escaped with `!` ([`like_starts_with`],
    /// [`like_contains`]).
    Like {
        col: ColName,
        pattern: String,
        negated: bool,
        case: CaseMode,
    },
    Between {
        col: ColName,
        lo: Value,
        hi: Value,
        negated: bool,
    },
    /// Any expression the app does not model, as written.
    Raw(String),
}

/// A literal value. A number keeps the text the server showed (no round trip through a float).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
    Bytes(Vec<u8>),
}

/// A sort key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderKey {
    pub col: ColName,
    pub dir: Dir,
    /// Where NULLs go, when the user asked; `None` leaves it to the server (PostgreSQL sorts
    /// them last going up, MySQL first).
    pub nulls: Option<Nulls>,
    pub origin: KeyOrigin,
}

impl OrderKey {
    /// A key the user asked for, NULLs where the server puts them.
    pub fn user(col: impl Into<ColName>, dir: Dir) -> Self {
        Self { col: col.into(), dir, nulls: None, origin: KeyOrigin::User }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Nulls {
    First,
    Last,
}

/// Who put a sort key there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyOrigin {
    User,
    /// A column of the primary key (or, without one, of a unique key) the app appended so that
    /// pages follow one order.
    TieBreak,
}

/// How a pattern compares letters of different case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaseMode {
    /// As the column's collation compares them.
    Server,
    /// Case does not matter.
    Insensitive,
}

/// What the app knows of a table and the session that reads it, for writing its SQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub dialect: Dialect,
    pub columns: Vec<ColumnFacts>,
    /// The primary key's columns, in key order; empty when it has none or it is not known.
    pub primary_key: Vec<ColName>,
    /// The other unique keys whose columns are all `NOT NULL`, each in key order.
    pub unique_keys: Vec<Vec<ColName>>,
    /// Append the primary key's missing columns to a user's order ([`KeyOrigin::TieBreak`]),
    /// or those of the shortest unique key of [`Target::unique_keys`] when it has none.
    pub tie_break: bool,
    /// MySQL: the session's character set is known to be `utf8mb4`, so a plain string literal
    /// is read as written. When unknown, text is written as its UTF-8 bytes.
    pub utf8mb4: bool,
    /// PostgreSQL: whether the server takes `LIKE` on a nondeterministic collation (version 18
    /// and later; `ILIKE` never). `None`: not known.
    pub like_nondeterministic: Option<bool>,
}

impl Target {
    /// A table of dialect `d` with these columns and nothing else known: no key, a tie-breaker
    /// wanted, the session's character set and what `LIKE` takes unknown.
    pub fn new(dialect: Dialect, columns: Vec<ColumnFacts>) -> Self {
        Self {
            dialect,
            columns,
            primary_key: Vec::new(),
            unique_keys: Vec::new(),
            tie_break: true,
            utf8mb4: false,
            like_nondeterministic: None,
        }
    }

    /// The facts of column `name`, if known.
    pub fn column(&self, name: &str) -> Option<&ColumnFacts> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The names of the columns.
    pub fn names(&self) -> Vec<ColName> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }
}

/// What the app knows of one column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnFacts {
    pub name: ColName,
    pub kind: ColumnKind,
    /// MySQL `TIMESTAMP`: an instant, shown and compared in the session's time zone. A value
    /// written with the offset of the zone it was read in (`2026-11-10 09:30:00+00:00`, as a
    /// copy as SQL takes it) is written as that instant in the session's zone when the
    /// statement runs, whatever the zone is then ([`render`]).
    pub zoned: bool,
    /// Its collation's name, when it has one and the server said it.
    pub collation: Option<String>,
    /// Equal strings are equal byte for byte under its collation (PostgreSQL: a
    /// nondeterministic ICU collation is not; it refuses `ILIKE`). `None`: not known.
    pub deterministic: Option<bool>,
}

impl ColumnFacts {
    /// A column of kind `kind`, its collation not known.
    pub fn new(name: impl Into<ColName>, kind: ColumnKind) -> Self {
        Self { name: name.into(), kind, zoned: false, collation: None, deterministic: None }
    }
}

/// What a column holds, as far as literals are written for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnKind {
    /// A number type: a number is written bare.
    Number,
    /// A binary floating-point number of single precision (PostgreSQL `real`, MySQL `FLOAT`),
    /// whose values are shown as the shortest decimal that reads back as them, not as they are
    /// held: a bare decimal compared with one is read as another type and widened to `double
    /// precision`, where the column's value is another number (`0.1` is not the `real` 0.1).
    /// PostgreSQL: a number is written as a string, which the server reads as the column's
    /// type, so that it compares exactly. MySQL: a number is written bare (it has no literal of
    /// that type), and it is not compared for equality.
    Real,
    /// Characters: patterns apply.
    Text,
    /// Binary data.
    Bytes,
    /// Anything else (dates, JSON, …): values are written as strings the server converts.
    Other,
}

impl Cond {
    /// The columns the condition compares, each once, in the order they first come. A part
    /// kept as written ([`Cond::Raw`]) names none here, though it may use columns: a column
    /// left out is not said to be unused.
    pub fn columns(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let mut stack = vec![self];
        while let Some(c) = stack.pop() {
            let col = match c {
                Cond::All(parts) | Cond::Any(parts) => {
                    stack.extend(parts.iter().rev());
                    continue;
                }
                Cond::Not(inner) => {
                    stack.push(inner);
                    continue;
                }
                Cond::Raw(_) => continue,
                Cond::Cmp { col, .. }
                | Cond::NotDistinct { col, .. }
                | Cond::IsNull { col, .. }
                | Cond::In { col, .. }
                | Cond::Like { col, .. }
                | Cond::Between { col, .. } => col.as_str(),
            };
            if !out.contains(&col) {
                out.push(col);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
