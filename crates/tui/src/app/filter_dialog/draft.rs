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

use std::collections::{HashMap, VecDeque};
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
    /// A MySQL `FLOAT`, held more precisely than any value written for it: no equality.
    InexactEquality,
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
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// What a count of its values reads it as when no rows describe it: a column `SELECT *`
    /// leaves out (MySQL `INVISIBLE`), as its catalogue type says ([`catalogue_meta`]).
    pub(crate) fn count_meta(&self, d: Dialect) -> Option<ColumnMeta> {
        self.invisible.then(|| catalogue_meta(d, &self.name, &self.type_name)).flatten()
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
            None => matches!(self.kind, ColumnKind::Number | ColumnKind::Real) || temporal_type(&self.type_name),
        }
    }
}

/// What a MySQL column whose catalogue type (`COLUMN_TYPE`) is `type_name` would be described
/// as in a result: its type named and its kind as the MySQL driver names them for a result
/// column of that type. For a column no result describes (an `INVISIBLE` one, which `SELECT *`
/// leaves out). `None` on PostgreSQL, which has no such column. A type not named here is of no
/// kind of its own ([`ValueKind::Other`]), whose values are not compared.
pub(crate) fn catalogue_meta(d: Dialect, name: &str, type_name: &str) -> Option<ColumnMeta> {
    if !matches!(d, Dialect::MySql(_)) {
        return None;
    }
    let lower = type_name.to_ascii_lowercase();
    let base = base_type(type_name);
    let unsigned = lower.split_whitespace().any(|w| w == "unsigned");
    let numeric = |n: &str| if unsigned { format!("{n} unsigned") } else { n.to_string() };
    let (ty, kind) = match base.as_str() {
        "tinyint" if lower.starts_with("tinyint(1)") => (numeric("tinyint(1)"), ValueKind::Integer),
        "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => (numeric(&base), ValueKind::Integer),
        "year" => (base.clone(), ValueKind::Integer),
        "decimal" => (numeric("decimal"), ValueKind::Decimal),
        "float" | "double" => (numeric(&base), ValueKind::Float),
        "bit" => (base.clone(), ValueKind::Bit),
        "date" => (base.clone(), ValueKind::Date),
        "time" => (base.clone(), ValueKind::Interval),
        "datetime" => (base.clone(), ValueKind::Timestamp),
        "timestamp" => (base.clone(), ValueKind::TimestampTz),
        "json" => (base.clone(), ValueKind::Json),
        "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum" | "set" => {
            (base.clone(), ValueKind::Text)
        }
        "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => (base.clone(), ValueKind::Bytes),
        "geometry" | "point" | "linestring" | "polygon" | "multipoint" | "multilinestring" | "multipolygon"
        | "geometrycollection" | "geomcollection" => ("geometry".to_string(), ValueKind::Bytes),
        "vector" => (base.clone(), ValueKind::Bytes),
        _ => (base.clone(), ValueKind::Other),
    };
    Some(ColumnMeta::new(name.to_string(), ty, kind, None))
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
            "real" | "float4" => ColumnKind::Real,
            "smallint" | "integer" | "int" | "bigint" | "int2" | "int4" | "int8" | "oid" | "double precision"
            | "float8" | "numeric" | "decimal" => ColumnKind::Number,
            "text" | "character varying" | "varchar" | "character" | "char" | "bpchar" | "name" => ColumnKind::Text,
            "bytea" => ColumnKind::Bytes,
            _ => ColumnKind::Other,
        },
        Dialect::MySql(_) => match base.as_str() {
            "float" => ColumnKind::Real,
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "year" | "decimal" | "numeric"
            | "dec" | "fixed" | "double" | "double precision" | "real" => ColumnKind::Number,
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
    let equality = matches!(op, Op::Eq | Op::Ne | Op::NeOrNull | Op::InList | Op::NotInList);
    if equality && c.kind == ColumnKind::Real && matches!(target.dialect, Dialect::MySql(_)) {
        return Err(OpOff::InexactEquality);
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
        RenderError::LikeUnknown(_)
        | RenderError::NullCompare(_)
        | RenderError::EmptyList(_)
        | RenderError::InexactEquality(_)
        | RenderError::NotNumber(_) => OpOff::LikeUnknown,
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn complete(&self) -> bool {
        self.cond().is_some()
    }

    /// Each value as the value field shows it on dialect `d` ([`value_text`]).
    pub(crate) fn value_texts(&self, d: Dialect) -> Vec<String> {
        self.values.iter().map(|v| value_text(v, d)).collect()
    }

    /// Each value as its own field shows it on dialect `d` for column `c` ([`field_text`]).
    pub(crate) fn field_texts(&self, d: Dialect, c: &ColumnInfo) -> Vec<String> {
        self.values.iter().map(|v| field_text(v, self.op, d, c)).collect()
    }

    /// The values as one value field shows them on dialect `d` for column `c`: one as
    /// [`field_text`] writes it, a list as [`list_text`] does (each read back to the same
    /// values).
    pub(crate) fn input_text(&self, d: Dialect, c: &ColumnInfo) -> String {
        match self.op.arity() {
            Arity::Many => list_text(&self.values, d, c),
            _ => self.field_texts(d, c).join(", "),
        }
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
/// the first empty one. A list is read by [`parse_list`]: values separated by commas, trimmed,
/// empty ones left out, a value in single quotes as it is and `NULL` bare SQL NULL. On a
/// number column a value is a number only when it is a plain number literal (an optional `-`, digits with at most one point, an optional
/// exponent); bytes are typed in hexadecimal after `\x` or `0x`. Anything else is text, written
/// as a string literal the server converts: `NULL` typed in a field of one value is the text
/// `NULL` (a NULL row is `is NULL`). A pattern's value is always text. A field of one value
/// written in single quotes is that text, as a list's value in quotes is ([`field_quoted`]):
/// the field holds one line, and shows a value with a line break so ([`field_text`]).
pub(crate) fn parse_input(op: Op, fields: &[&str], c: &ColumnInfo) -> Vec<Value> {
    parse_with(op, fields, c, &|s| field_value(op, s, c))
}

/// Values `texts` (each a value's text as it is, [`Row::value_texts`]) as values of a row of
/// operator `op` on column `c`, each read as typed ([`parse_input`] without the quotes of a
/// field of one value).
pub(crate) fn parse_texts(op: Op, texts: &[&str], c: &ColumnInfo) -> Vec<Value> {
    parse_with(op, texts, c, &|s| if op.pattern() { Value::Text(s.to_string()) } else { typed_value(s, c) })
}

/// [`parse_input`], each text of one value read by `one`.
fn parse_with(op: Op, fields: &[&str], c: &ColumnInfo, one: &dyn Fn(&str) -> Value) -> Vec<Value> {
    match op.arity() {
        Arity::Zero => Vec::new(),
        Arity::One => fields.first().filter(|s| !s.is_empty()).map(|s| vec![one(s)]).unwrap_or_default(),
        Arity::Two => fields.iter().take(2).take_while(|s| !s.is_empty()).map(|s| one(s)).collect(),
        Arity::Many => fields.first().map(|s| parse_list(s, c, &[])).unwrap_or_default(),
    }
}

/// The text after an opening single quote, read as a value in quotes: what is between the
/// quotes (a quote inside written twice; `\n` a line feed, `\r` a carriage return, `\\` a
/// backslash, a backslash before anything else itself), and where the closing quote ends,
/// counted from the opening one (`None`: left open, it runs to the end).
fn quoted_body(body: &str) -> (String, Option<usize>) {
    let mut inner = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\'' {
            if chars.next_if(|&(_, c)| c == '\'').is_some() {
                inner.push('\'');
                continue;
            }
            return (inner, Some(1 + i + 1));
        }
        if c == '\\' {
            match chars.next_if(|&(_, c)| matches!(c, 'n' | 'r' | '\\')) {
                Some((_, 'n')) => inner.push('\n'),
                Some((_, 'r')) => inner.push('\r'),
                _ => inner.push('\\'),
            }
            continue;
        }
        inner.push(c);
    }
    (inner, None)
}

/// Text `s` in single quotes on one line, as [`quoted_body`] reads it back.
fn quote(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('\'');
    for ch in s.chars() {
        match ch {
            '\'' => q.push_str("''"),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            '\r' => q.push_str("\\r"),
            ch => q.push(ch),
        }
    }
    q.push('\'');
    q
}

/// The value written in single quotes in the text of a field of one value, as a list reads
/// one ([`list_items`]): the whole text (blanks around it aside) is that value, or a quote left
/// open runs to its end. `None` when the text is not one value in quotes.
pub(crate) fn field_quoted(s: &str) -> Option<String> {
    let t = s.trim_start();
    let (inner, close) = quoted_body(t.strip_prefix('\'')?);
    match close {
        Some(at) if !t[at..].trim().is_empty() => None,
        _ => Some(inner),
    }
}

/// The text of a field of one value of operator `op` on column `c`, as its value: a value in
/// single quotes is that text ([`field_quoted`]), anything else as typed ([`parse_input`]).
fn field_value(op: Op, s: &str, c: &ColumnInfo) -> Value {
    match field_quoted(s) {
        Some(q) => Value::Text(q),
        None if op.pattern() => Value::Text(s.to_string()),
        None => typed_value(s, c),
    }
}

/// Value `v` as a field of one value of operator `op` on column `c` shows it on dialect `d`,
/// on one line: as it is ([`value_text`]) where that reads back as it, else text in single
/// quotes ([`field_value`] reads it back to the same value).
pub(crate) fn field_text(v: &Value, op: Op, d: Dialect, c: &ColumnInfo) -> String {
    let plain = value_text(v, d);
    match v {
        Value::Text(s) if plain.is_empty() || plain.contains(['\n', '\r']) || field_value(op, &plain, c) != *v => {
            quote(s)
        }
        _ => plain,
    }
}

/// One value of a list field as written: its text without the blanks around it, and what is
/// between the quotes when it is written in single quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListItem {
    pub(crate) written: String,
    pub(crate) quoted: Option<String>,
}

/// The values written in list field text `s`, separated by commas outside single quotes. A
/// value in single quotes is what is between them: a quote inside written twice, and, as the
/// field holds one line, `\n` a line feed, `\r` a carriage return and `\\` a backslash (a
/// backslash before anything else is itself). A quote left open runs to the end of the text;
/// text after the closing quote makes the value the text as written.
pub(crate) fn list_items(s: &str) -> Vec<ListItem> {
    let mut items = Vec::new();
    let mut rest = s;
    loop {
        let t = rest.trim_start();
        let unquoted = |t: &str| ListItem { written: t.trim_end().to_string(), quoted: None };
        let (item, next) = match t.strip_prefix('\'') {
            Some(body) => {
                let (inner, close) = quoted_body(body);
                match close {
                    None => (ListItem { written: t.trim_end().to_string(), quoted: Some(inner) }, None),
                    Some(at) => {
                        let (tail, next) = match t[at..].find(',') {
                            Some(k) => (&t[at..at + k], Some(&t[at + k + 1..])),
                            None => (&t[at..], None),
                        };
                        match tail.trim().is_empty() {
                            true => (ListItem { written: t[..at].to_string(), quoted: Some(inner) }, next),
                            false => (unquoted(&t[..at + tail.len()]), next),
                        }
                    }
                }
            }
            None => match t.find(',') {
                Some(k) => (unquoted(&t[..k]), Some(&t[k + 1..])),
                None => (unquoted(t), None),
            },
        };
        items.push(item);
        match next {
            Some(n) => rest = n,
            None => break,
        }
    }
    items
}

/// The values of list field text `s` for column `c` ([`list_items`]): a value in quotes is that
/// text; `NULL` written bare is SQL NULL; anything else as one typed value is
/// ([`parse_input`]); an empty one is left out. A value written as `kept` gives it (each the
/// text [`list_text`] wrote for a value the field held, with that value), so editing one value
/// of a list never changes the others.
pub(crate) fn parse_list(s: &str, c: &ColumnInfo, kept: &[(String, Value)]) -> Vec<Value> {
    // The values held by how they were written, in their order for one written alike twice.
    let mut by_text: HashMap<&str, VecDeque<&Value>> = HashMap::with_capacity(kept.len());
    for (w, v) in kept {
        by_text.entry(w.as_str()).or_default().push_back(v);
    }
    list_items(s)
        .into_iter()
        .filter_map(|it| {
            #[cfg(test)]
            LIST_PROBES.with(|p| p.set(p.get() + 1));
            if let Some(v) = by_text.get_mut(it.written.as_str()).and_then(VecDeque::pop_front) {
                return Some(v.clone());
            }
            match it.quoted {
                Some(q) => Some(Value::Text(q)),
                None if it.written.is_empty() => None,
                None => Some(list_value(&it.written, c)),
            }
        })
        .collect()
}

#[cfg(test)]
thread_local! {
    /// How many values the field held [`parse_list`] has looked at (tests).
    pub(crate) static LIST_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A list value written bare: `NULL` is SQL NULL, anything else as one typed value is.
fn list_value(written: &str, c: &ColumnInfo) -> Value {
    match written.eq_ignore_ascii_case("NULL") {
        true => Value::Null,
        false => typed_value(written, c),
    }
}

/// Value `v` of a list of column `c` as [`list_text`] writes it on dialect `d`: as the field
/// shows one value ([`value_text`]) where that reads back as it, else text in single quotes,
/// on one line ([`list_items`] reads the escapes back).
fn list_item_text(v: &Value, d: Dialect, c: &ColumnInfo) -> String {
    let plain = value_text(v, d);
    let reads_back = !plain.is_empty()
        && plain.trim() == plain
        && !plain.contains([',', '\n', '\r'])
        && !plain.starts_with('\'')
        && list_value(&plain, c) == *v;
    match v {
        Value::Text(s) if !reads_back => quote(s),
        _ => plain,
    }
}

/// Values `values` of column `c` as a list field shows them on dialect `d`: separated by `, `,
/// each as [`parse_list`] reads it back to the same value (SQL NULL bare, text that would read
/// otherwise in single quotes).
pub(crate) fn list_text(values: &[Value], d: Dialect, c: &ColumnInfo) -> String {
    values.iter().map(|v| list_item_text(v, d, c)).collect::<Vec<_>>().join(", ")
}

/// Values `values` each with the text [`list_text`] writes for it, for [`parse_list`].
pub(crate) fn list_kept(values: &[Value], d: Dialect, c: &ColumnInfo) -> Vec<(String, Value)> {
    values.iter().map(|v| (list_item_text(v, d, c), v.clone())).collect()
}

/// Text `s` typed as a value of column `c` (see [`parse_input`]).
fn typed_value(s: &str, c: &ColumnInfo) -> Value {
    let t = s.trim();
    match c.kind {
        ColumnKind::Number | ColumnKind::Real if plain_number(t) => Value::Number(t.to_string()),
        // MySQL's BOOLEAN (`tinyint(1)`) takes TRUE and FALSE (1 and 0); text that is no number
        // on any other number column is refused when written (the server would read it as 0).
        ColumnKind::Number if c.type_name.starts_with("tinyint(1)") && t.eq_ignore_ascii_case("true") => {
            Value::Bool(true)
        }
        ColumnKind::Number if c.type_name.starts_with("tinyint(1)") && t.eq_ignore_ascii_case("false") => {
            Value::Bool(false)
        }
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
    #[cfg_attr(not(test), allow(dead_code))]
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
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// The text sent without column `col`'s own conditions (its rows; an expression stays,
    /// whatever it names): the text sent as it is when the column has none, else the other
    /// complete terms' texts joined as [`Where::text`] joins them. `None` when nothing is left.
    pub(crate) fn text_without(&self, col: &str) -> Option<String> {
        let own = |t: &Term| t.row.as_ref().is_some_and(|r| r.col == col);
        if !self.terms.iter().any(own) {
            return self.sent();
        }
        let texts: Vec<&str> = self.terms.iter().filter(|t| !own(t)).filter_map(|t| t.text.as_deref()).collect();
        Some(query::join_terms(&texts, self.dialect)).filter(|t| !t.trim().is_empty())
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

/// Where a term went when the terms changed ([`follow`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Moved {
    /// It is term `.0` now (changed in its place, or moved by terms taken out or put in
    /// before it).
    To(usize),
    /// It is gone; the terms that followed it start at `.0` now.
    Gone(usize),
}

/// Where term `t` of `before` is among `after`: the terms both lists start and end with are
/// the same terms, the part between them is what changed. A term in that part keeps its place
/// when the part kept its length (changed where it is), else it is gone.
pub(crate) fn follow(before: &[Term], after: &[Term], t: usize) -> Moved {
    let head = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let room = before.len().min(after.len()) - head;
    let tail = before.iter().rev().zip(after.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
    if t < head {
        Moved::To(t)
    } else if t >= before.len() - tail {
        Moved::To(t + after.len() - before.len())
    } else if before.len() == after.len() {
        Moved::To(t)
    } else {
        Moved::Gone(head)
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

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.steps.len()
    }
}

#[cfg(test)]
mod tests;
