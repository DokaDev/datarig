//! What the filter and sort dialog edits: the conditions of a table's `WHERE` text ([`Where`]),
//! its order keys and the text of its `LIMIT` field ([`Draft`]), and the steps back ([`Undo`]).
//!
//! The `WHERE` text is kept as it was given ([`Where::from_text`]) and read as the terms of its
//! top-level conjunction where that is proven ([`query::split_terms`]); text that is not proven
//! to be one is one term. Each term is a chip: a condition of one column that the dialog edits
//! ([`Row`]) when the term reads as exactly one of the forms the dialog writes, else an
//! expression kept as written. While no term is added, changed or taken out, the text is the
//! one given, byte for byte. After that it is written again from the terms
//! ([`query::join_terms`]): a term not touched as it was written, a changed one as
//! [`query::render`] writes its condition. A condition still waiting for its value is no chip
//! and is never part of the text.
//!
//! A value typed into the dialog becomes a number only when it is a plain number literal typed
//! for a number column ([`parse_input`]); anything else is text, which is written as a string
//! literal: what is typed is never written into the SQL as it is.

use std::ops::Range;

use datarig_core::driver::keys::CatalogColumn;
use datarig_core::driver::{ColumnMeta, ValueKind};
use datarig_core::query::{
    self, CaseMode, CmpOp, ColName, ColumnKind, Compares, Cond, Dir, Filter, KeyOrigin, LikeKind, NoEquality, OrderKey,
    Query, RenderError, Target, Value,
};
use datarig_core::sql::dialect::Dialect;

use crate::app::table_query::{self, LimitProblem};

/// The most steps [`Undo`] keeps; the oldest goes first.
pub(crate) const UNDO_STEPS: usize = 100;

/// The escape character of the patterns [`query::like_contains`] and
/// [`query::like_starts_with`] write.
const LIKE_ESCAPE: char = '!';

/// An operator of a condition row, in the order the operator picker offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Op {
    Eq,
    /// Between two values, both included (numbers, dates and times only).
    Between,
    Ne,
    /// Not equal, rows whose value is NULL kept.
    NeOrNull,
    Lt,
    Le,
    Gt,
    Ge,
    /// Text that contains the value, whatever the case.
    Contains,
    /// Text that starts with the value (the column's collation decides about case).
    StartsWith,
    InList,
    NotInList,
    IsNull,
    IsNotNull,
}

/// How many values an operator takes ([`Op::arity`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arity {
    Zero,
    One,
    Two,
    /// One or more.
    Many,
}

impl Op {
    /// Every operator, in the order the picker offers them.
    pub(crate) const ALL: [Op; 14] = [
        Op::Eq,
        Op::Between,
        Op::Ne,
        Op::NeOrNull,
        Op::Lt,
        Op::Le,
        Op::Gt,
        Op::Ge,
        Op::Contains,
        Op::StartsWith,
        Op::InList,
        Op::NotInList,
        Op::IsNull,
        Op::IsNotNull,
    ];

    /// How many values the operator takes.
    pub(crate) fn arity(self) -> Arity {
        match self {
            Op::IsNull | Op::IsNotNull => Arity::Zero,
            Op::Between => Arity::Two,
            Op::InList | Op::NotInList => Arity::Many,
            _ => Arity::One,
        }
    }

    /// The SQL shown beside an operator in words (`BETWEEN a AND b`); `None` for one shown as
    /// a symbol.
    pub(crate) fn sql_hint(self, d: Dialect) -> Option<&'static str> {
        let pg = d == Dialect::Postgres;
        Some(match self {
            Op::Between => "BETWEEN a AND b",
            Op::NeOrNull if pg => "IS DISTINCT FROM",
            Op::NeOrNull => "NOT (… <=> …)",
            Op::Contains if pg => "ILIKE '%…%'",
            Op::Contains => "LIKE '%…%'",
            Op::StartsWith => "LIKE '…%'",
            Op::InList => "IN (a, b, …)",
            Op::NotInList => "NOT IN (…)",
            Op::IsNull => "IS NULL",
            Op::IsNotNull => "IS NOT NULL",
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => return None,
        })
    }

    /// The operator compares by order (`<`, `BETWEEN`).
    fn ordered(self) -> bool {
        matches!(self, Op::Between | Op::Lt | Op::Le | Op::Gt | Op::Ge)
    }

    /// The operator is a pattern of text.
    fn pattern(self) -> bool {
        matches!(self, Op::Contains | Op::StartsWith)
    }

    /// The comparison operator of a plain comparison.
    fn cmp(self) -> Option<CmpOp> {
        Some(match self {
            Op::Eq => CmpOp::Eq,
            Op::Ne => CmpOp::Ne,
            Op::Lt => CmpOp::Lt,
            Op::Le => CmpOp::Le,
            Op::Gt => CmpOp::Gt,
            Op::Ge => CmpOp::Ge,
            _ => return None,
        })
    }

    fn of_cmp(op: CmpOp) -> Op {
        match op {
            CmpOp::Eq => Op::Eq,
            CmpOp::Ne => Op::Ne,
            CmpOp::Lt => Op::Lt,
            CmpOp::Le => Op::Le,
            CmpOp::Gt => Op::Gt,
            CmpOp::Ge => Op::Ge,
        }
    }
}

/// Why an operator is offered dimmed for a column ([`ops_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpOff {
    /// The column's type has no equality a literal reaches.
    NoEquality,
    /// The column's type compares for equality only.
    NoOrder,
    /// The column does not hold text.
    NotText,
    /// The column's type is not known yet (no rows held), and the operator needs text.
    TypeUnknown,
    /// PostgreSQL refuses `ILIKE` on the column's collation (not deterministic).
    CaseInsensitiveRefused,
    /// The column's collation is not known to be one PostgreSQL takes `ILIKE` on.
    CollationUnknown,
    /// The server refuses `LIKE` on the column's collation (not deterministic, before 18).
    LikeRefused,
    /// Neither the column's collation nor the server is known to take `LIKE` here.
    LikeUnknown,
}

/// What the dialog knows of a column of the table.
#[derive(Clone, Debug)]
pub(crate) struct ColumnInfo {
    pub(crate) name: ColName,
    /// Its type as the catalogue or the result names it.
    pub(crate) type_name: String,
    /// What literals are written for it as.
    pub(crate) kind: ColumnKind,
    /// The column of the rows held, when they are held: what it compares by is known only
    /// then.
    pub(crate) meta: Option<ColumnMeta>,
    /// Its type is known to be a PostgreSQL enum.
    pub(crate) enum_known: bool,
    pub(crate) primary_key: bool,
    pub(crate) foreign_key: bool,
    pub(crate) not_null: bool,
    /// MySQL: left out of `SELECT *`.
    pub(crate) invisible: bool,
}

impl ColumnInfo {
    /// A column as the catalogue of dialect `d` describes it, no rows held.
    pub(crate) fn from_catalogue(d: Dialect, c: &CatalogColumn) -> Self {
        Self {
            name: c.name.clone(),
            type_name: c.type_name.clone(),
            kind: kind_of_type(d, &c.type_name),
            meta: None,
            enum_known: false,
            primary_key: c.primary_key,
            foreign_key: c.foreign_key,
            not_null: c.not_null,
            invisible: c.invisible,
        }
    }

    /// A column of the rows held, nothing else known of it.
    pub(crate) fn from_meta(m: &ColumnMeta) -> Self {
        Self {
            name: m.name.clone(),
            type_name: m.type_name.clone(),
            kind: table_query::column_kind(m),
            meta: Some(m.clone()),
            enum_known: false,
            primary_key: false,
            foreign_key: false,
            not_null: false,
            invisible: false,
        }
    }

    /// The same column with the rows' column `m` known: its kind is the result's.
    pub(crate) fn with_meta(self, m: &ColumnMeta) -> Self {
        Self { kind: table_query::column_kind(m), meta: Some(m.clone()), ..self }
    }

    /// Its values are numbers, dates or times: `BETWEEN` is offered.
    fn ranged(&self) -> bool {
        match &self.meta {
            Some(m) => matches!(
                m.kind,
                ValueKind::Integer
                    | ValueKind::Decimal
                    | ValueKind::Float
                    | ValueKind::Date
                    | ValueKind::Time
                    | ValueKind::Timestamp
                    | ValueKind::TimestampTz
                    | ValueKind::Interval
            ),
            None => self.kind == ColumnKind::Number || temporal_type(&self.type_name),
        }
    }
}

/// A type name lower case, without its arguments (`(20)`, `('a','b')`), MySQL's `unsigned`
/// and `zerofill`, and with single spaces: `character varying(20)` is `character varying`.
fn base_type(type_name: &str) -> String {
    let lower = type_name.to_ascii_lowercase();
    let rest = match (lower.find('('), lower.rfind(')')) {
        (Some(open), Some(close)) if open < close => format!("{} {}", &lower[..open], &lower[close + 1..]),
        _ => lower,
    };
    rest.split_whitespace().filter(|w| !matches!(*w, "unsigned" | "zerofill")).collect::<Vec<_>>().join(" ")
}

/// What literals are written as for a column whose type the catalogue of dialect `d` names
/// `type_name` (PostgreSQL `format_type`, MySQL `COLUMN_TYPE`), as the held result would say:
/// numbers, text and bytes of the built-in types; anything else (a domain, an array, dates, a
/// type not named here) is [`ColumnKind::Other`], whose values are written as strings.
pub(crate) fn kind_of_type(d: Dialect, type_name: &str) -> ColumnKind {
    let base = base_type(type_name);
    if base.ends_with("[]") {
        return ColumnKind::Other;
    }
    match d {
        Dialect::Postgres => match base.as_str() {
            "smallint" | "integer" | "int" | "bigint" | "int2" | "int4" | "int8" | "oid" | "real" | "float4"
            | "double precision" | "float8" | "numeric" | "decimal" => ColumnKind::Number,
            "text" | "character varying" | "varchar" | "character" | "char" | "bpchar" | "name" => ColumnKind::Text,
            "bytea" => ColumnKind::Bytes,
            _ => ColumnKind::Other,
        },
        Dialect::MySql(_) => match base.as_str() {
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "year" | "decimal" | "numeric"
            | "dec" | "fixed" | "float" | "double" | "double precision" | "real" => ColumnKind::Number,
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum" | "set" => ColumnKind::Text,
            "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => ColumnKind::Bytes,
            _ => ColumnKind::Other,
        },
    }
}

/// A type of dates or times, by its name.
fn temporal_type(type_name: &str) -> bool {
    let base = base_type(type_name);
    let first = base.split(' ').next().unwrap_or_default();
    matches!(first, "date" | "time" | "timetz" | "timestamp" | "timestamptz" | "datetime" | "interval")
}

/// The operators the picker offers for column `c` of table `target`, in order, each with why it
/// is dimmed. `BETWEEN` is offered for numbers, dates and times only. Until rows are held what
/// the column compares by is not known, and nothing is dimmed for that (the server decides);
/// a pattern needs a column known to hold text, and one the collation rules let it be written
/// for.
pub(crate) fn ops_for(c: &ColumnInfo, target: &Target) -> Vec<(Op, Result<(), OpOff>)> {
    let compares = c.meta.as_ref().map(|m| query::compares(target.dialect, m, c.enum_known));
    Op::ALL
        .into_iter()
        .filter(|op| *op != Op::Between || c.ranged())
        .map(|op| (op, op_allowed(op, c, compares, target)))
        .collect()
}

fn op_allowed(
    op: Op,
    c: &ColumnInfo,
    compares: Option<Result<Compares, NoEquality>>,
    target: &Target,
) -> Result<(), OpOff> {
    if op.arity() == Arity::Zero {
        return Ok(());
    }
    match compares {
        Some(Err(NoEquality)) => return Err(OpOff::NoEquality),
        Some(Ok(Compares::Equality)) if op.ordered() => return Err(OpOff::NoOrder),
        _ => {}
    }
    if !op.pattern() {
        return Ok(());
    }
    match c.kind {
        ColumnKind::Text => {}
        ColumnKind::Other if c.meta.is_none() => return Err(OpOff::TypeUnknown),
        _ => return Err(OpOff::NotText),
    }
    // The collation rules are the renderer's: what it refuses to write is dimmed.
    let row = Row { col: c.name.clone(), op, values: vec![Value::Text("x".into())], keep_nulls: false };
    let Some(cond) = row.cond() else { return Ok(()) };
    query::render(&cond, target).map(|_| ()).map_err(|e| match e {
        RenderError::CaseInsensitiveRefused(_) => OpOff::CaseInsensitiveRefused,
        RenderError::CollationUnknown(_) => OpOff::CollationUnknown,
        RenderError::LikeRefused(_) => OpOff::LikeRefused,
        RenderError::LikeUnknown(_) | RenderError::NullCompare(_) | RenderError::EmptyList(_) => OpOff::LikeUnknown,
    })
}

/// One condition of one column, as a row of the dialog edits it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) col: ColName,
    pub(crate) op: Op,
    /// The values given so far ([`Op::arity`]); a NULL among them is a ticked NULL.
    pub(crate) values: Vec<Value>,
    /// [`Op::NotInList`]: rows whose value is NULL are kept.
    pub(crate) keep_nulls: bool,
}

impl Row {
    /// A row of column `col` with operator `op` and no value yet.
    pub(crate) fn new(col: impl Into<ColName>, op: Op) -> Self {
        Self { col: col.into(), op, values: Vec::new(), keep_nulls: false }
    }

    /// The condition the row stands for; `None` while values are missing.
    ///
    /// A NULL value of `=` is `IS NULL`, of `≠` and `≠ or NULL` `IS NOT NULL`. A NULL among the
    /// values of a list is tested on its own as [`query::render`] writes it. `not in list` that
    /// keeps NULL rows is `(c NOT IN (…) OR c IS NULL)`, unless NULL itself is among the values.
    pub(crate) fn cond(&self) -> Option<Cond> {
        let col = self.col.clone();
        let one = || match self.values.as_slice() {
            [v] => Some(v.clone()),
            _ => None,
        };
        let pattern = |make: fn(&str) -> String| match one()? {
            Value::Text(s) | Value::Number(s) => Some(make(&s)),
            _ => None,
        };
        Some(match self.op {
            Op::IsNull | Op::IsNotNull => Cond::IsNull { col, negated: self.op == Op::IsNotNull },
            Op::Eq | Op::Ne | Op::NeOrNull if one()? == Value::Null => Cond::IsNull { col, negated: self.op != Op::Eq },
            Op::NeOrNull => Cond::NotDistinct { col, value: one()?, negated: true },
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => Cond::Cmp { col, op: self.op.cmp()?, value: one()? },
            Op::Between => match self.values.as_slice() {
                [lo, hi] => Cond::Between { col, lo: lo.clone(), hi: hi.clone(), negated: false },
                _ => return None,
            },
            Op::Contains => {
                Cond::Like { col, pattern: pattern(query::like_contains)?, negated: false, case: CaseMode::Insensitive }
            }
            Op::StartsWith => {
                Cond::Like { col, pattern: pattern(query::like_starts_with)?, negated: false, case: CaseMode::Server }
            }
            Op::InList | Op::NotInList if self.values.is_empty() => return None,
            Op::InList => Cond::In { col, values: self.values.clone(), negated: false },
            Op::NotInList if self.keep_nulls && !self.values.contains(&Value::Null) => Cond::Any(vec![
                Cond::In { col: col.clone(), values: self.values.clone(), negated: true },
                Cond::IsNull { col, negated: false },
            ]),
            Op::NotInList => Cond::In { col, values: self.values.clone(), negated: true },
        })
    }

    /// The row a condition of table `target` is, when it is exactly one of the forms
    /// [`Row::cond`] gives; `None` for anything else (an expression chip): a comparison with
    /// NULL, a negated `BETWEEN` or pattern, a pattern of another shape, a NULL-safe equality.
    /// A MySQL pattern with the server's case on a column whose collation ignores case (`_ci`)
    /// is the case-insensitive one, as the renderer writes it there.
    pub(crate) fn from_cond(c: &Cond, target: &Target) -> Option<Row> {
        let row =
            |col: &str, op: Op, values: Vec<Value>| Some(Row { col: col.to_string(), op, values, keep_nulls: false });
        match c {
            Cond::Cmp { col, op, value } if *value != Value::Null => row(col, Op::of_cmp(*op), vec![value.clone()]),
            Cond::NotDistinct { col, value, negated: true } if *value != Value::Null => {
                row(col, Op::NeOrNull, vec![value.clone()])
            }
            Cond::IsNull { col, negated } => row(col, if *negated { Op::IsNotNull } else { Op::IsNull }, Vec::new()),
            Cond::In { col, values, negated } if values.iter().any(|v| *v != Value::Null) => {
                row(col, if *negated { Op::NotInList } else { Op::InList }, values.clone())
            }
            Cond::Between { col, lo, hi, negated: false } if *lo != Value::Null && *hi != Value::Null => {
                row(col, Op::Between, vec![lo.clone(), hi.clone()])
            }
            Cond::Like { col, pattern, negated: false, case } => {
                let op = match (query::like_kind(pattern, LIKE_ESCAPE)?, case) {
                    (LikeKind::Contains(s), CaseMode::Insensitive) => (Op::Contains, s),
                    (LikeKind::Contains(s), CaseMode::Server) if ignores_case(target, col) => (Op::Contains, s),
                    (LikeKind::StartsWith(s), CaseMode::Server) => (Op::StartsWith, s),
                    _ => return None,
                };
                row(col, op.0, vec![Value::Text(op.1)])
            }
            Cond::Any(_) => {
                let col = c.column_condition()?;
                let Cond::Any(parts) = c else { return None };
                let Some(Cond::In { values, .. }) = parts.first() else { return None };
                Some(Row { col: col.to_string(), op: Op::NotInList, values: values.clone(), keep_nulls: true })
            }
            _ => None,
        }
    }

    /// The row has every value its operator takes.
    pub(crate) fn complete(&self) -> bool {
        self.cond().is_some()
    }

    /// Each value as the value field shows it on dialect `d` ([`value_text`]).
    pub(crate) fn value_texts(&self, d: Dialect) -> Vec<String> {
        self.values.iter().map(|v| value_text(v, d)).collect()
    }

    /// The values as one value field shows them on dialect `d`: one as it is, several joined
    /// with `, ` (a list).
    pub(crate) fn input_text(&self, d: Dialect) -> String {
        self.value_texts(d).join(", ")
    }
}

/// A MySQL column whose collation compares without case (`utf8mb4_0900_ai_ci`).
fn ignores_case(target: &Target, col: &str) -> bool {
    matches!(target.dialect, Dialect::MySql(_))
        && target
            .column(col)
            .and_then(|f| f.collation.as_deref())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with("_ci"))
}

/// Value `v` as a value field shows it on dialect `d`: text and numbers as they are, bytes in
/// hexadecimal as the grid shows them (PostgreSQL `\x…`, MySQL `0x…`), NULL as `NULL`.
pub(crate) fn value_text(v: &Value, d: Dialect) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(s) | Value::Text(s) => s.clone(),
        Value::Bytes(b) => match d {
            Dialect::Postgres => format!("\\x{}", b.iter().map(|b| format!("{b:02x}")).collect::<String>()),
            Dialect::MySql(_) => format!("0x{}", b.iter().map(|b| format!("{b:02X}")).collect::<String>()),
        },
    }
}

/// The values typed for a row of operator `op` on column `c`, one text per value field
/// (`BETWEEN` has two; a list has one, its values separated by commas).
///
/// A field left empty gives no value (the row waits for it); the values of a `BETWEEN` stop at
/// the first empty one. A list's values are trimmed and empty ones left out: a value holding a
/// comma is ticked in the values, not typed. On a number column a value is a number only when
/// it is a plain number literal (an optional `-`, digits with at most one point, an optional
/// exponent); bytes are typed in hexadecimal after `\x` or `0x`. Anything else is text, written
/// as a string literal the server converts: `NULL` typed is the text `NULL` (a NULL row is
/// `is NULL`). A pattern's value is always text.
pub(crate) fn parse_input(op: Op, fields: &[&str], c: &ColumnInfo) -> Vec<Value> {
    let typed = |s: &str| if op.pattern() { Value::Text(s.to_string()) } else { typed_value(s, c) };
    match op.arity() {
        Arity::Zero => Vec::new(),
        Arity::One => fields.first().filter(|s| !s.is_empty()).map(|s| vec![typed(s)]).unwrap_or_default(),
        Arity::Two => fields.iter().take(2).take_while(|s| !s.is_empty()).map(|s| typed(s)).collect(),
        Arity::Many => fields
            .first()
            .map(|s| s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(typed).collect())
            .unwrap_or_default(),
    }
}

/// Text `s` typed as a value of column `c` (see [`parse_input`]).
fn typed_value(s: &str, c: &ColumnInfo) -> Value {
    let t = s.trim();
    match c.kind {
        ColumnKind::Number if plain_number(t) => Value::Number(t.to_string()),
        ColumnKind::Bytes => match hex_bytes(t) {
            Some(b) => Value::Bytes(b),
            None => Value::Text(s.to_string()),
        },
        _ => Value::Text(s.to_string()),
    }
}

/// A number literal written bare: an optional `-`, digits with at most one point (digits on
/// one side at least), an optional exponent; not a negative zero (which the renderer writes as
/// a string).
fn plain_number(s: &str) -> bool {
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = match unsigned.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e.strip_prefix(['+', '-']).unwrap_or(e))),
        None => (unsigned, None),
    };
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mantissa_ok = digits(int) && digits(frac) && !(int.is_empty() && frac.is_empty());
    let exp_ok = exp.is_none_or(|e| !e.is_empty() && digits(e));
    let zero = s.starts_with('-') && mantissa.bytes().all(|b| matches!(b, b'0' | b'.'));
    mantissa_ok && exp_ok && !zero
}

/// Bytes typed in hexadecimal after `\x` or `0x`, two digits each.
fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let digits = s.strip_prefix("\\x").or_else(|| s.strip_prefix("0x"))?.as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }
    digits.chunks(2).map(|p| std::str::from_utf8(p).ok().and_then(|s| u8::from_str_radix(s, 16).ok())).collect()
}

/// Where a term's text comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// This byte range of the text given ([`Where::original`]), untouched.
    Original(Range<usize>),
    /// Written by the dialog from its row.
    Edited,
}

/// One term of the `WHERE` text: a chip once it has text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Term {
    /// What is sent for it; `None` while its row waits for values or cannot be written
    /// ([`Term::problem`]): no chip, never sent.
    pub(crate) text: Option<String>,
    pub(crate) origin: Origin,
    /// The condition of one column it is; `None` for an expression kept as written.
    pub(crate) row: Option<Row>,
    /// Why its row is not written ([`query::render`]).
    pub(crate) problem: Option<RenderError>,
}

impl Term {
    /// The term of row `row` of table `target`, written as [`query::render`] writes it.
    fn edited(row: Row, target: &Target) -> Self {
        let (text, problem) = match row.cond().map(|c| query::render(&c, target)) {
            None => (None, None),
            Some(Ok(text)) => (Some(text), None),
            Some(Err(e)) => (None, Some(e)),
        };
        Self { text, origin: Origin::Edited, row: Some(row), problem }
    }

    /// It has text: a chip, sent.
    pub(crate) fn complete(&self) -> bool {
        self.text.is_some()
    }

    /// An expression the dialog does not edit (no row).
    pub(crate) fn is_expr(&self) -> bool {
        self.row.is_none()
    }
}

/// A complete term as a chip shows it ([`Where::chips`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chip<'a> {
    /// The term's place among all terms.
    pub(crate) index: usize,
    pub(crate) text: &'a str,
    pub(crate) expr: bool,
}

/// The `WHERE` text of a draft, as the text given and its terms (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Where {
    original: String,
    terms: Vec<Term>,
    /// A term was added, changed or taken out: the text is written from the terms.
    rebuilt: bool,
    dialect: Dialect,
}

impl Where {
    /// Filter text `text` of table `target`, as its terms: none for a blank text; the terms of
    /// a proven top-level conjunction, else one expression over the whole text (blanks around
    /// it left out of its range). Each term is read on its own: a row when it reads as one.
    pub(crate) fn from_text(text: &str, target: &Target) -> Self {
        let d = target.dialect;
        let terms = if text.trim().is_empty() {
            Vec::new()
        } else {
            match query::split_terms(text, d) {
                Some(ranges) => ranges
                    .into_iter()
                    .map(|r| {
                        let t = &text[r.clone()];
                        let row = query::recognise(t, target).and_then(|c| Row::from_cond(&c, target));
                        Term { text: Some(t.to_string()), origin: Origin::Original(r), row, problem: None }
                    })
                    .collect(),
                None => {
                    let start = text.len() - text.trim_start().len();
                    let r = start..text.trim_end().len();
                    vec![Term {
                        text: Some(text[r.clone()].to_string()),
                        origin: Origin::Original(r),
                        row: None,
                        problem: None,
                    }]
                }
            }
        };
        Self { original: text.to_string(), terms, rebuilt: false, dialect: d }
    }

    /// The text sent: the text given, byte for byte, until a term is added, changed or taken
    /// out; then the complete terms' texts joined ([`query::join_terms`]).
    pub(crate) fn text(&self) -> String {
        if !self.rebuilt {
            return self.original.clone();
        }
        let texts: Vec<&str> = self.terms.iter().filter_map(|t| t.text.as_deref()).collect();
        query::join_terms(&texts, self.dialect)
    }

    /// The text given.
    pub(crate) fn original(&self) -> &str {
        &self.original
    }

    /// A term was added, changed or taken out since the text was given.
    pub(crate) fn rebuilt(&self) -> bool {
        self.rebuilt
    }

    pub(crate) fn terms(&self) -> &[Term] {
        &self.terms
    }

    /// Adds row `row` (complete or not) after the other terms; its index.
    pub(crate) fn add(&mut self, row: Row, target: &Target) -> usize {
        self.terms.push(Term::edited(row, target));
        self.rebuilt = true;
        self.terms.len() - 1
    }

    /// Term `index` becomes row `row`, written again in its place. `false` when there is no
    /// such term.
    pub(crate) fn set(&mut self, index: usize, row: Row, target: &Target) -> bool {
        let Some(t) = self.terms.get_mut(index) else { return false };
        *t = Term::edited(row, target);
        self.rebuilt = true;
        true
    }

    /// Takes term `index` out.
    pub(crate) fn remove(&mut self, index: usize) -> Option<Term> {
        (index < self.terms.len()).then(|| {
            self.rebuilt = true;
            self.terms.remove(index)
        })
    }

    /// Row `row` becomes the only condition of its column: the column's own rows are taken
    /// out (expressions and other columns' rows stay as they are) and `row` takes the place of
    /// the first of them, else goes after the others. Its index.
    pub(crate) fn replace_column(&mut self, row: Row, target: &Target) -> usize {
        let own = |t: &Term| t.row.as_ref().is_some_and(|r| r.col == row.col);
        let at = self.terms.iter().position(own);
        self.terms.retain(|t| !own(t));
        let term = Term::edited(row, target);
        self.rebuilt = true;
        match at {
            Some(i) => {
                self.terms.insert(i, term);
                i
            }
            None => {
                self.terms.push(term);
                self.terms.len() - 1
            }
        }
    }

    /// Takes every term out.
    pub(crate) fn clear(&mut self) {
        self.terms.clear();
        self.rebuilt = true;
    }

    /// The text of the `WHERE` field read back: nothing changes when it is the text sent
    /// ([`Where::text`]); else the terms are those of `text`, given anew.
    pub(crate) fn set_text(&mut self, text: &str, target: &Target) {
        if text != self.text() {
            *self = Self::from_text(text, target);
        }
    }

    /// The complete terms, in order.
    pub(crate) fn chips(&self) -> Vec<Chip<'_>> {
        self.terms
            .iter()
            .enumerate()
            .filter_map(|(index, t)| Some(Chip { index, text: t.text.as_deref()?, expr: t.is_expr() }))
            .collect()
    }

    /// The indices of the rows of column `col`, complete or not.
    pub(crate) fn rows_of(&self, col: &str) -> Vec<usize> {
        let of = |t: &Term| t.row.as_ref().is_some_and(|r| r.col == col);
        self.terms.iter().enumerate().filter(|(_, t)| of(t)).map(|(i, _)| i).collect()
    }

    /// A row waits for values or cannot be written.
    pub(crate) fn has_incomplete(&self) -> bool {
        self.terms.iter().any(|t| !t.complete())
    }

    /// The text sent, `None` when blank.
    fn sent(&self) -> Option<String> {
        Some(self.text()).filter(|t| !t.trim().is_empty())
    }
}

/// What the dialog edits: the `WHERE` text, the user's order keys and the `LIMIT` field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    pub(crate) where_: Where,
    /// The user's keys (tie-breakers are the compiler's).
    pub(crate) order: Vec<OrderKey>,
    /// The `LIMIT` field's text (blank: no cap).
    pub(crate) limit: String,
}

impl Draft {
    /// The draft of query `q` on table `target`.
    pub(crate) fn from_query(q: &Query, target: &Target) -> Self {
        let text = q.filter.as_ref().map_or("", |f| f.text.as_str());
        Self {
            where_: Where::from_text(text, target),
            order: q.order.iter().filter(|k| k.origin == KeyOrigin::User).cloned().collect(),
            limit: q.limit.map(|n| n.to_string()).unwrap_or_default(),
        }
    }

    /// The query of the draft, of `base`'s table and offset: its `WHERE` text as it is sent
    /// (none when blank) with what is recognised of it, its keys and its cap. A `LIMIT` field
    /// that is no cap is refused with where and why ([`table_query::parse_limit`]).
    pub(crate) fn to_query(&self, base: &Query, target: &Target) -> Result<Query, (Range<usize>, LimitProblem)> {
        let limit = table_query::parse_limit(&self.limit)?;
        let filter = self.where_.sent().map(|text| Filter { recognised: query::recognise(&text, target), text });
        Ok(Query { source: base.source.clone(), filter, order: self.order.clone(), limit, offset: base.offset })
    }

    /// Both drafts send the same query: the same `WHERE` text (blank as none), keys and cap.
    pub(crate) fn same_as(&self, other: &Draft) -> bool {
        let limit = match (table_query::parse_limit(&self.limit), table_query::parse_limit(&other.limit)) {
            (Ok(a), Ok(b)) => a == b,
            _ => self.limit == other.limit,
        };
        self.where_.sent() == other.where_.sent() && self.order == other.order && limit
    }

    /// Column `col`'s sort direction and its place among the keys (from 0).
    pub(crate) fn sort_of(&self, col: &str) -> Option<(Dir, usize)> {
        self.order.iter().position(|k| k.col == col).map(|i| (self.order[i].dir, i))
    }

    /// Column `col` sorts `dir` (a new key goes last, a key kept keeps its place), or not at
    /// all.
    pub(crate) fn set_sort(&mut self, col: &str, dir: Option<Dir>) {
        let at = self.order.iter().position(|k| k.col == col);
        match (dir, at) {
            (None, Some(i)) => {
                self.order.remove(i);
            }
            (None, None) => {}
            (Some(dir), Some(i)) => self.order[i].dir = dir,
            (Some(dir), None) => self.order.push(OrderKey::user(col, dir)),
        }
    }

    /// Moves column `col`'s key `delta` places (negative: earlier), within the keys. `false`
    /// when it did not move.
    pub(crate) fn move_key(&mut self, col: &str, delta: isize) -> bool {
        let Some(i) = self.order.iter().position(|k| k.col == col) else { return false };
        let to = i.saturating_add_signed(delta).min(self.order.len() - 1);
        if to == i {
            return false;
        }
        let key = self.order.remove(i);
        self.order.insert(to, key);
        true
    }
}

/// The drafts to go back to, newest last, at most [`UNDO_STEPS`]. Typing in one field is one
/// step: a step recorded for field `K` while the last one was for the same field is left out,
/// until the field is left ([`Undo::leave_field`]).
#[derive(Clone, Debug)]
pub(crate) struct Undo<K> {
    steps: Vec<Draft>,
    coalescing: Option<K>,
}

impl<K> Default for Undo<K> {
    fn default() -> Self {
        Self { steps: Vec::new(), coalescing: None }
    }
}

impl<K: PartialEq> Undo<K> {
    /// The draft `before` a change, made by typing in `field` (`None`: anything else).
    pub(crate) fn record(&mut self, before: &Draft, field: Option<K>) {
        if field.is_some() && field == self.coalescing {
            return;
        }
        self.steps.push(before.clone());
        if self.steps.len() > UNDO_STEPS {
            self.steps.remove(0);
        }
        self.coalescing = field;
    }

    /// The field typed in is left: the next typing is a step of its own.
    pub(crate) fn leave_field(&mut self) {
        self.coalescing = None;
    }

    /// The draft before the last step, taken off.
    pub(crate) fn undo(&mut self) -> Option<Draft> {
        self.coalescing = None;
        self.steps.pop()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.steps.len()
    }
}

#[cfg(test)]
mod tests;
