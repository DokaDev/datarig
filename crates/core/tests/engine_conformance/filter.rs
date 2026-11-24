//! A table's filter and order as each engine's SQL, which column types a filter compares, and
//! the statement that counts the rows of each value of a column.

use crate::Engine::{MariaDb, MySql, MySqlAnsiQuotes, MySqlDollarNotKnown, MySqlNoBackslashEscapes, Pg};
use crate::{Case, Engine};
use datarig_core::driver::{ArrayElement, ColumnMeta, ValueKind};
use datarig_core::query::{
    CaseMode, CmpOp, ColumnFacts, ColumnKind, Cond, Dir, GroupCount, Nulls, OrderKey, Query, TableName, Target, Value,
    compile_capped,
};
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::sql::risk::Classifier;

/// What a filter case writes: one condition (with what the session's character set is known
/// to be), a pattern on the `name` column with what is known of its collation and of the
/// server, or a whole query read at once.
pub enum Input {
    Cond { cond: Cond, utf8mb4: bool },
    Like(Like),
    Query(Query),
}

/// A pattern `name LIKE 'a%'` in a case mode, on a column whose
/// collation is (`Some(true)`), is not (`Some(false)`) or may not be (`None`) deterministic,
/// named `collation`, on a server that takes (`Some(true)`), refuses (`Some(false)`) or may
/// refuse (`None`) `LIKE` on a nondeterministic collation.
pub struct Like {
    pub case: CaseMode,
    pub deterministic: Option<bool>,
    pub like_nondeterministic: Option<bool>,
    pub collation: Option<&'static str>,
}

/// The table every case reads: a number key, text (of a deterministic collation), a float,
/// bytes and a date.
fn target(d: Dialect, utf8mb4: bool) -> Target {
    let mut name = ColumnFacts::new("name", ColumnKind::Text);
    name.deterministic = Some(true);
    let mut t = Target::new(
        d,
        vec![
            ColumnFacts::new("id", ColumnKind::Number),
            name,
            ColumnFacts::new("price", ColumnKind::Real),
            ColumnFacts::new("data", ColumnKind::Bytes),
            ColumnFacts::new("born", ColumnKind::Other),
        ],
    );
    t.primary_key = vec!["id".to_string()];
    t.utf8mb4 = utf8mb4;
    t
}

/// The SQL of a case on `e`, or the `Debug` of why it is not written.
pub fn render(e: Engine, input: &Input) -> String {
    render_in(e.dialect(), input)
}

/// [`render`] in dialect `d`.
pub fn render_in(d: Dialect, input: &Input) -> String {
    match input {
        Input::Cond { cond, utf8mb4 } => match datarig_core::query::render(cond, &target(d, *utf8mb4)) {
            Ok(sql) => sql,
            Err(why) => format!("{why:?}"),
        },
        Input::Like(like) => {
            let mut t = target(d, true);
            let name = t.columns.iter_mut().find(|c| c.name == "name").expect("the name column");
            name.deterministic = like.deterministic;
            name.collation = like.collation.map(str::to_string);
            t.like_nondeterministic = like.like_nondeterministic;
            let cond = Cond::Like { col: "name".into(), pattern: "a%".into(), negated: false, case: like.case };
            match datarig_core::query::render(&cond, &t) {
                Ok(sql) => sql,
                Err(why) => format!("{why:?}"),
            }
        }
        Input::Query(q) => compile_capped(q, &target(d, false)).sql,
    }
}

fn like(case: CaseMode, deterministic: Option<bool>, like_nondeterministic: Option<bool>) -> Input {
    Input::Like(Like { case, deterministic, like_nondeterministic, collation: None })
}

fn cond(cond: Cond) -> Input {
    Input::Cond { cond, utf8mb4: false }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn query(order: Vec<OrderKey>, limit: Option<u64>, offset: Option<u64>) -> Input {
    let mut q = Query::table(TableName { database: None, schema: "s".to_string(), name: "t".to_string() });
    q.order = order;
    q.limit = limit;
    q.offset = offset;
    Input::Query(q)
}

fn nulls(col: &str, dir: Dir, nulls: Nulls) -> OrderKey {
    OrderKey { nulls: Some(nulls), ..OrderKey::user(col, dir) }
}

pub fn render_cases() -> Vec<Case<Input>> {
    vec![
        Case {
            name: "equal text",
            input: cond(Cond::Cmp { col: "name".into(), op: CmpOp::Eq, value: text("x") }),
            expect: &[(Pg, "name = 'x'"), (MySql, "name = _utf8mb4 X'78'"), (MariaDb, "name = _utf8mb4 X'78'")],
        },
        Case {
            name: "null-safe equal",
            input: cond(Cond::NotDistinct { col: "name".into(), value: text("x"), negated: false }),
            expect: &[
                (Pg, "name IS NOT DISTINCT FROM 'x'"),
                (MySql, "name <=> _utf8mb4 X'78'"),
                (MariaDb, "name <=> _utf8mb4 X'78'"),
            ],
        },
        Case {
            name: "null-safe not equal",
            input: cond(Cond::NotDistinct { col: "name".into(), value: text("x"), negated: true }),
            expect: &[
                (Pg, "name IS DISTINCT FROM 'x'"),
                (MySql, "NOT (name <=> _utf8mb4 X'78')"),
                (MariaDb, "NOT (name <=> _utf8mb4 X'78')"),
            ],
        },
        Case {
            name: "is null",
            input: cond(Cond::IsNull { col: "name".into(), negated: false }),
            expect: &[(Pg, "name IS NULL"), (MySql, "name IS NULL"), (MariaDb, "name IS NULL")],
        },
        Case {
            name: "in with null",
            input: cond(Cond::In {
                col: "id".into(),
                values: vec![Value::Number("1".into()), Value::Null],
                negated: false,
            }),
            expect: &[
                (Pg, "(id IN (1) OR id IS NULL)"),
                (MySql, "(id IN (1) OR id IS NULL)"),
                (MariaDb, "(id IN (1) OR id IS NULL)"),
            ],
        },
        Case {
            name: "not in",
            input: cond(Cond::In {
                col: "id".into(),
                values: vec![Value::Number("1".into()), Value::Number("2".into())],
                negated: true,
            }),
            expect: &[(Pg, "id NOT IN (1, 2)"), (MySql, "id NOT IN (1, 2)"), (MariaDb, "id NOT IN (1, 2)")],
        },
        Case {
            name: "like",
            input: cond(Cond::Like {
                col: "name".into(),
                pattern: "a!%%".into(),
                negated: false,
                case: CaseMode::Server,
            }),
            expect: &[
                (Pg, "name LIKE 'a!%%' ESCAPE '!'"),
                (MySql, "name LIKE _utf8mb4 X'61212525' ESCAPE '!'"),
                (MariaDb, "name LIKE _utf8mb4 X'61212525' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like ignoring case",
            input: cond(Cond::Like {
                col: "name".into(),
                pattern: "a%".into(),
                negated: true,
                case: CaseMode::Insensitive,
            }),
            expect: &[
                (Pg, "name NOT ILIKE 'a%' ESCAPE '!'"),
                (MySql, "LOWER(name) NOT LIKE LOWER(_utf8mb4 X'6125') ESCAPE '!'"),
                (MariaDb, "LOWER(name) NOT LIKE LOWER(_utf8mb4 X'6125') ESCAPE '!'"),
            ],
        },
        Case {
            name: "between",
            input: cond(Cond::Between {
                col: "born".into(),
                lo: text("2026-01-01"),
                hi: text("2026-12-31"),
                negated: false,
            }),
            expect: &[
                (Pg, "born BETWEEN '2026-01-01' AND '2026-12-31'"),
                (MySql, "born BETWEEN _utf8mb4 X'323032362D30312D3031' AND _utf8mb4 X'323032362D31322D3331'"),
                (MariaDb, "born BETWEEN _utf8mb4 X'323032362D30312D3031' AND _utf8mb4 X'323032362D31322D3331'"),
            ],
        },
        Case {
            name: "float",
            input: cond(Cond::Cmp { col: "price".into(), op: CmpOp::Ge, value: Value::Number("0.1".into()) }),
            expect: &[(Pg, "price >= '0.1'"), (MySql, "price >= 0.1"), (MariaDb, "price >= 0.1")],
        },
        Case {
            name: "bytes",
            input: cond(Cond::Cmp { col: "data".into(), op: CmpOp::Eq, value: Value::Bytes(vec![0, 0xab]) }),
            expect: &[
                (Pg, "data = E'\\\\x00ab'::pg_catalog.bytea"),
                (MySql, "data = X'00AB'"),
                (MariaDb, "data = X'00AB'"),
            ],
        },
        Case {
            name: "bool",
            input: cond(Cond::Cmp { col: "id".into(), op: CmpOp::Ne, value: Value::Bool(true) }),
            expect: &[(Pg, "id <> TRUE"), (MySql, "id <> TRUE"), (MariaDb, "id <> TRUE")],
        },
        Case {
            name: "text with a backslash",
            input: cond(Cond::Cmp { col: "name".into(), op: CmpOp::Eq, value: text("a\\b'c") }),
            expect: &[
                (Pg, "name = E'a\\\\b''c'"),
                (MySql, "name = _utf8mb4 X'615C622763'"),
                (MariaDb, "name = _utf8mb4 X'615C622763'"),
            ],
        },
        Case {
            name: "text literal, utf8mb4 not known",
            input: Input::Cond {
                cond: Cond::Cmp { col: "name".into(), op: CmpOp::Eq, value: text("caf\u{e9}") },
                utf8mb4: false,
            },
            expect: &[
                (Pg, "name = 'café'"),
                (MySql, "name = _utf8mb4 X'636166C3A9'"),
                (MariaDb, "name = _utf8mb4 X'636166C3A9'"),
            ],
        },
        Case {
            name: "text literal, utf8mb4",
            input: Input::Cond {
                cond: Cond::Cmp { col: "name".into(), op: CmpOp::Eq, value: text("caf\u{e9}") },
                utf8mb4: true,
            },
            expect: &[(Pg, "name = 'café'"), (MySql, "name = 'café'"), (MariaDb, "name = 'café'")],
        },
        Case {
            name: "ascii text literal, utf8mb4 not known",
            input: Input::Cond {
                cond: Cond::Cmp { col: "name".into(), op: CmpOp::Eq, value: text("plain") },
                utf8mb4: false,
            },
            expect: &[
                (Pg, "name = 'plain'"),
                (MySql, "name = _utf8mb4 X'706C61696E'"),
                (MariaDb, "name = _utf8mb4 X'706C61696E'"),
            ],
        },
        Case {
            name: "unknown column",
            input: cond(Cond::Cmp { col: "nope".into(), op: CmpOp::Eq, value: text("x") }),
            expect: &[(Pg, "nope = 'x'"), (MySql, "nope = _utf8mb4 X'78'"), (MariaDb, "nope = _utf8mb4 X'78'")],
        },
        Case {
            name: "all and any",
            input: cond(Cond::All(vec![
                Cond::Any(vec![Cond::IsNull { col: "id".into(), negated: true }, Cond::Raw("price > 1".into())]),
                Cond::Not(Box::new(Cond::IsNull { col: "name".into(), negated: false })),
            ])),
            expect: &[
                (Pg, "(id IS NOT NULL OR (price > 1)) AND NOT (name IS NULL)"),
                (MySql, "(id IS NOT NULL OR (price > 1)) AND NOT (name IS NULL)"),
                (MariaDb, "(id IS NOT NULL OR (price > 1)) AND NOT (name IS NULL)"),
            ],
        },
        Case {
            name: "like, deterministic yes, server takes nondeterministic yes",
            input: like(CaseMode::Server, Some(true), Some(true)),
            expect: &[
                (Pg, "name LIKE 'a%' ESCAPE '!'"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic yes, server takes nondeterministic no",
            input: like(CaseMode::Server, Some(true), Some(false)),
            expect: &[
                (Pg, "name LIKE 'a%' ESCAPE '!'"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic yes, server takes nondeterministic unknown",
            input: like(CaseMode::Server, Some(true), None),
            expect: &[
                (Pg, "name LIKE 'a%' ESCAPE '!'"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic no, server takes nondeterministic yes",
            input: like(CaseMode::Server, Some(false), Some(true)),
            expect: &[
                (Pg, "name LIKE 'a%' ESCAPE '!'"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic no, server takes nondeterministic no",
            input: like(CaseMode::Server, Some(false), Some(false)),
            expect: &[
                (Pg, "LikeRefused(\"name\")"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic no, server takes nondeterministic unknown",
            input: like(CaseMode::Server, Some(false), None),
            expect: &[
                (Pg, "LikeUnknown(\"name\")"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic unknown, server takes nondeterministic yes",
            input: like(CaseMode::Server, None, Some(true)),
            expect: &[
                (Pg, "name LIKE 'a%' ESCAPE '!'"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic unknown, server takes nondeterministic no",
            input: like(CaseMode::Server, None, Some(false)),
            expect: &[
                (Pg, "LikeUnknown(\"name\")"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "like, deterministic unknown, server takes nondeterministic unknown",
            input: like(CaseMode::Server, None, None),
            expect: &[
                (Pg, "LikeUnknown(\"name\")"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic yes, server takes nondeterministic yes",
            input: like(CaseMode::Insensitive, Some(true), Some(true)),
            expect: &[
                (Pg, "name ILIKE 'a%' ESCAPE '!'"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic yes, server takes nondeterministic no",
            input: like(CaseMode::Insensitive, Some(true), Some(false)),
            expect: &[
                (Pg, "name ILIKE 'a%' ESCAPE '!'"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic yes, server takes nondeterministic unknown",
            input: like(CaseMode::Insensitive, Some(true), None),
            expect: &[
                (Pg, "name ILIKE 'a%' ESCAPE '!'"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic no, server takes nondeterministic yes",
            input: like(CaseMode::Insensitive, Some(false), Some(true)),
            expect: &[
                (Pg, "CaseInsensitiveRefused(\"name\")"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic no, server takes nondeterministic no",
            input: like(CaseMode::Insensitive, Some(false), Some(false)),
            expect: &[
                (Pg, "CaseInsensitiveRefused(\"name\")"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic no, server takes nondeterministic unknown",
            input: like(CaseMode::Insensitive, Some(false), None),
            expect: &[
                (Pg, "CaseInsensitiveRefused(\"name\")"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic unknown, server takes nondeterministic yes",
            input: like(CaseMode::Insensitive, None, Some(true)),
            expect: &[
                (Pg, "CollationUnknown(\"name\")"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic unknown, server takes nondeterministic no",
            input: like(CaseMode::Insensitive, None, Some(false)),
            expect: &[
                (Pg, "CollationUnknown(\"name\")"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, deterministic unknown, server takes nondeterministic unknown",
            input: like(CaseMode::Insensitive, None, None),
            expect: &[
                (Pg, "CollationUnknown(\"name\")"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, collation utf8mb4_0900_ai_ci",
            input: Input::Like(Like {
                case: CaseMode::Insensitive,
                deterministic: Some(true),
                like_nondeterministic: None,
                collation: Some("utf8mb4_0900_ai_ci"),
            }),
            expect: &[
                (Pg, "name ILIKE 'a%' ESCAPE '!'"),
                (MySql, "name LIKE 'a%' ESCAPE '!'"),
                (MariaDb, "name LIKE 'a%' ESCAPE '!'"),
            ],
        },
        Case {
            name: "case-insensitive like, collation utf8mb4_bin",
            input: Input::Like(Like {
                case: CaseMode::Insensitive,
                deterministic: Some(true),
                like_nondeterministic: None,
                collation: Some("utf8mb4_bin"),
            }),
            expect: &[
                (Pg, "name ILIKE 'a%' ESCAPE '!'"),
                (MySql, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
                (MariaDb, "LOWER(name) LIKE LOWER('a%') ESCAPE '!'"),
            ],
        },
        Case {
            name: "nulls first going up",
            input: query(vec![nulls("name", Dir::Asc, Nulls::First)], None, None),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" ORDER BY \"name\" ASC NULLS FIRST, \"id\" ASC"),
                (MySql, "SELECT * FROM `s`.`t` ORDER BY `name` IS NULL DESC, `name` ASC, `id` ASC"),
                (MariaDb, "SELECT * FROM `s`.`t` ORDER BY `name` IS NULL DESC, `name` ASC, `id` ASC"),
            ],
        },
        Case {
            name: "nulls last going up",
            input: query(vec![nulls("name", Dir::Asc, Nulls::Last)], None, None),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" ORDER BY \"name\" ASC NULLS LAST, \"id\" ASC"),
                (MySql, "SELECT * FROM `s`.`t` ORDER BY `name` IS NULL ASC, `name` ASC, `id` ASC"),
                (MariaDb, "SELECT * FROM `s`.`t` ORDER BY `name` IS NULL ASC, `name` ASC, `id` ASC"),
            ],
        },
        Case {
            name: "nulls first going down",
            input: query(vec![nulls("name", Dir::Desc, Nulls::First)], None, None),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" ORDER BY \"name\" DESC NULLS FIRST, \"id\" ASC"),
                (MySql, "SELECT * FROM `s`.`t` ORDER BY `name` IS NULL DESC, `name` DESC, `id` ASC"),
                (MariaDb, "SELECT * FROM `s`.`t` ORDER BY `name` IS NULL DESC, `name` DESC, `id` ASC"),
            ],
        },
        Case {
            name: "server's nulls",
            input: query(vec![OrderKey::user("name", Dir::Desc)], None, None),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" ORDER BY \"name\" DESC, \"id\" ASC"),
                (MySql, "SELECT * FROM `s`.`t` ORDER BY `name` DESC, `id` ASC"),
                (MariaDb, "SELECT * FROM `s`.`t` ORDER BY `name` DESC, `id` ASC"),
            ],
        },
        Case {
            name: "limit",
            input: query(Vec::new(), Some(10), None),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" LIMIT 10"),
                (MySql, "SELECT * FROM `s`.`t` LIMIT 10"),
                (MariaDb, "SELECT * FROM `s`.`t` LIMIT 10"),
            ],
        },
        Case {
            name: "limit and offset",
            input: query(vec![OrderKey::user("id", Dir::Asc)], Some(10), Some(20)),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" ORDER BY \"id\" ASC LIMIT 10 OFFSET 20"),
                (MySql, "SELECT * FROM `s`.`t` ORDER BY `id` ASC LIMIT 10 OFFSET 20"),
                (MariaDb, "SELECT * FROM `s`.`t` ORDER BY `id` ASC LIMIT 10 OFFSET 20"),
            ],
        },
        Case {
            name: "offset alone",
            input: query(Vec::new(), None, Some(20)),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" OFFSET 20"),
                (MySql, "SELECT * FROM `s`.`t` LIMIT 18446744073709551615 OFFSET 20"),
                (MariaDb, "SELECT * FROM `s`.`t` LIMIT 18446744073709551615 OFFSET 20"),
            ],
        },
    ]
}

/// A result column of a type, as a driver describes it, and whether its type is known to be a
/// PostgreSQL enum.
pub struct Column {
    pub type_name: &'static str,
    pub kind: ValueKind,
    pub enum_known: bool,
}

/// How a filter compares the values of the column on `e`.
pub fn compares(e: Engine, c: &Column) -> String {
    compares_in(e.dialect(), c)
}

/// [`compares`] in dialect `d`.
pub fn compares_in(d: Dialect, c: &Column) -> String {
    let meta = ColumnMeta::new("c".to_string(), c.type_name.to_string(), c.kind, None);
    match datarig_core::query::compares(d, &meta, c.enum_known) {
        Ok(how) => format!("{how:?}"),
        Err(why) => format!("{why:?}"),
    }
}

fn col(type_name: &'static str, kind: ValueKind) -> Column {
    Column { type_name, kind, enum_known: false }
}

pub fn compares_cases() -> Vec<Case<Column>> {
    vec![
        Case {
            name: "integer",
            input: col("int4", ValueKind::Integer),
            expect: &[(Pg, "Order"), (MySql, "Order"), (MariaDb, "Order")],
        },
        Case {
            name: "text",
            input: col("text", ValueKind::Text),
            expect: &[(Pg, "Order"), (MySql, "Order"), (MariaDb, "Order")],
        },
        Case {
            name: "json",
            input: col("json", ValueKind::Json),
            expect: &[(Pg, "NoEquality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "jsonb",
            input: col("jsonb", ValueKind::Json),
            expect: &[(Pg, "Equality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "enum known",
            input: Column { type_name: "mood", kind: ValueKind::Other, enum_known: true },
            expect: &[(Pg, "Order"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "enum not known",
            input: col("mood", ValueKind::Other),
            expect: &[(Pg, "NoEquality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "uuid",
            input: col("uuid", ValueKind::Other),
            expect: &[(Pg, "Order"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "geometry",
            input: col("geometry", ValueKind::Bytes),
            expect: &[(Pg, "Order"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "vector",
            input: col("vector", ValueKind::Bytes),
            expect: &[(Pg, "Order"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "bytes",
            input: col("bytea", ValueKind::Bytes),
            expect: &[(Pg, "Order"), (MySql, "Order"), (MariaDb, "Order")],
        },
        Case {
            name: "point",
            input: col("point", ValueKind::Other),
            expect: &[(Pg, "NoEquality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "bits",
            input: col("bit", ValueKind::Bit),
            expect: &[(Pg, "Order"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "array of numbers",
            input: col("int4[]", ValueKind::Array(ArrayElement::Number)),
            expect: &[(Pg, "Equality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "array of text",
            input: col("text[]", ValueKind::Array(ArrayElement::Text)),
            expect: &[(Pg, "Equality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "array of json",
            input: col("json[]", ValueKind::Array(ArrayElement::Json)),
            expect: &[(Pg, "NoEquality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "array of points",
            input: col("point[]", ValueKind::Array(ArrayElement::Text)),
            expect: &[(Pg, "NoEquality"), (MySql, "NoEquality"), (MariaDb, "NoEquality")],
        },
        Case {
            name: "interval",
            input: col("interval", ValueKind::Interval),
            expect: &[(Pg, "Order"), (MySql, "Order"), (MariaDb, "Order")],
        },
    ]
}

/// A column a group count reads: one of the table's (by name), as a driver describes it.
pub struct Grouped {
    pub name: &'static str,
    pub type_name: &'static str,
    pub kind: ValueKind,
}

/// The statement that counts the rows of each value of the column on `e`, or the `Debug` of why
/// it is not written.
pub fn group_count(e: Engine, c: &Grouped) -> String {
    group_count_in(e.dialect(), c)
}

/// [`group_count`] in dialect `d`.
pub fn group_count_in(d: Dialect, c: &Grouped) -> String {
    let table = TableName { database: None, schema: "s".to_string(), name: "t".to_string() };
    let column = ColumnMeta::new(c.name.to_string(), c.type_name.to_string(), c.kind, None);
    let classifier = Classifier::new(Language::Sql(d));
    let g = GroupCount {
        table: &table,
        target: &target(d, true),
        column: &column,
        enum_known: false,
        other: None,
        classifier: &classifier,
        read_only: false,
    };
    match datarig_core::query::group_count(&g) {
        Ok(sql) => sql,
        Err(why) => format!("{why:?}"),
    }
}

fn grouped(name: &'static str, type_name: &'static str, kind: ValueKind) -> Grouped {
    Grouped { name, type_name, kind }
}

/// The statement of a family text column: the same in every sql mode.
const FAMILY_TEXT: &str = "SELECT IF(OCTET_LENGTH(`name`) <= 4096, `name`, NULL), OCTET_LENGTH(`name`), COUNT(*) \
     FROM `s`.`t` GROUP BY `name` ORDER BY COUNT(*) DESC LIMIT 201";

pub fn group_count_cases() -> Vec<Case<Grouped>> {
    vec![
        Case {
            name: "text",
            input: grouped("name", "text", ValueKind::Text),
            expect: &[
                (
                    Pg,
                    "SELECT CASE WHEN pg_catalog.octet_length(\"name\") OPERATOR(pg_catalog.<=) 4096 THEN \"name\" END, \
                     pg_catalog.octet_length(\"name\"), pg_catalog.count(*) FROM \"s\".\"t\" GROUP BY \"name\" \
                     ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (MySql, FAMILY_TEXT),
                (MariaDb, FAMILY_TEXT),
                (MySqlAnsiQuotes, FAMILY_TEXT),
                (MySqlNoBackslashEscapes, FAMILY_TEXT),
            ],
        },
        Case {
            name: "bytes",
            input: grouped("data", "bytea", ValueKind::Bytes),
            expect: &[
                (
                    Pg,
                    "SELECT CASE WHEN pg_catalog.octet_length(\"data\") OPERATOR(pg_catalog.<=) 4096 THEN \"data\" END, \
                     pg_catalog.octet_length(\"data\"), pg_catalog.count(*) FROM \"s\".\"t\" GROUP BY \"data\" \
                     ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (
                    MySql,
                    "SELECT IF(OCTET_LENGTH(`data`) <= 4096, `data`, NULL), OCTET_LENGTH(`data`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `data` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
                (
                    MariaDb,
                    "SELECT IF(OCTET_LENGTH(`data`) <= 4096, `data`, NULL), OCTET_LENGTH(`data`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `data` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
            ],
        },
        Case {
            name: "number",
            input: grouped("id", "int4", ValueKind::Integer),
            expect: &[
                (
                    Pg,
                    "SELECT \"id\", NULL, pg_catalog.count(*) FROM \"s\".\"t\" GROUP BY \"id\" \
                     ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (
                    MySql,
                    "SELECT IF(OCTET_LENGTH(`id`) <= 4096, `id`, NULL), OCTET_LENGTH(`id`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `id` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
                (
                    MariaDb,
                    "SELECT IF(OCTET_LENGTH(`id`) <= 4096, `id`, NULL), OCTET_LENGTH(`id`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `id` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
            ],
        },
        Case {
            name: "jsonb",
            input: grouped("born", "jsonb", ValueKind::Json),
            expect: &[
                (
                    Pg,
                    "SELECT CASE WHEN pg_catalog.octet_length(pg_catalog.textin(pg_catalog.jsonb_out(\"born\"))) \
                     OPERATOR(pg_catalog.<=) 4096 THEN \"born\" END, \
                     pg_catalog.octet_length(pg_catalog.textin(pg_catalog.jsonb_out(\"born\"))), pg_catalog.count(*) \
                     FROM \"s\".\"t\" GROUP BY \"born\" ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (MySql, "NoEquality"),
                (MariaDb, "NoEquality"),
            ],
        },
        Case {
            name: "citext",
            input: grouped("born", "citext", ValueKind::Other),
            expect: &[
                (
                    Pg,
                    "SELECT CASE WHEN pg_catalog.octet_length(\"born\") OPERATOR(pg_catalog.<=) 4096 THEN \"born\" END, \
                     pg_catalog.octet_length(\"born\"), pg_catalog.count(*) FROM \"s\".\"t\" GROUP BY \"born\" \
                     ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (MySql, "NoEquality"),
                (MariaDb, "NoEquality"),
            ],
        },
        Case {
            name: "numeric",
            input: grouped("price", "numeric", ValueKind::Decimal),
            expect: &[
                (
                    Pg,
                    "SELECT CASE WHEN pg_catalog.octet_length(pg_catalog.textin(pg_catalog.numeric_out(\"price\"))) \
                     OPERATOR(pg_catalog.<=) 4096 THEN \"price\" END, \
                     pg_catalog.octet_length(pg_catalog.textin(pg_catalog.numeric_out(\"price\"))), pg_catalog.count(*) \
                     FROM \"s\".\"t\" GROUP BY \"price\" ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (
                    MySql,
                    "SELECT IF(OCTET_LENGTH(`price`) <= 4096, `price`, NULL), OCTET_LENGTH(`price`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `price` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
                (
                    MariaDb,
                    "SELECT IF(OCTET_LENGTH(`price`) <= 4096, `price`, NULL), OCTET_LENGTH(`price`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `price` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
            ],
        },
        Case {
            name: "money",
            input: grouped("price", "money", ValueKind::Decimal),
            expect: &[
                (
                    Pg,
                    "SELECT \"price\", NULL, pg_catalog.count(*) FROM \"s\".\"t\" GROUP BY \"price\" \
                     ORDER BY pg_catalog.count(*) DESC LIMIT 201",
                ),
                (
                    MySql,
                    "SELECT IF(OCTET_LENGTH(`price`) <= 4096, `price`, NULL), OCTET_LENGTH(`price`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `price` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
                (
                    MariaDb,
                    "SELECT IF(OCTET_LENGTH(`price`) <= 4096, `price`, NULL), OCTET_LENGTH(`price`), COUNT(*) \
                     FROM `s`.`t` GROUP BY `price` ORDER BY COUNT(*) DESC LIMIT 201",
                ),
            ],
        },
    ]
}

/// Filter text read back into a condition: the bar text and the table it is read for (the
/// table of [`read_target`], or with `mixed` one with the columns `aB` and `Ab` only).
pub struct Read {
    pub text: &'static str,
    pub mixed: bool,
}

/// The table filter text is read for: numbers `id`, `x` and `y`, text `name`, a float `price`,
/// bytes `data`, a date `d` and an instant `at` shown in the session's zone; with `mixed`, the
/// columns `aB` and `Ab` (in that order).
fn read_target(d: Dialect, mixed: bool) -> Target {
    if mixed {
        return Target::new(
            d,
            vec![ColumnFacts::new("aB", ColumnKind::Number), ColumnFacts::new("Ab", ColumnKind::Number)],
        );
    }
    let mut at = ColumnFacts::new("at", ColumnKind::Other);
    at.zoned = true;
    Target::new(
        d,
        vec![
            ColumnFacts::new("id", ColumnKind::Number),
            ColumnFacts::new("name", ColumnKind::Text),
            ColumnFacts::new("price", ColumnKind::Real),
            ColumnFacts::new("data", ColumnKind::Bytes),
            ColumnFacts::new("x", ColumnKind::Number),
            ColumnFacts::new("y", ColumnKind::Number),
            ColumnFacts::new("d", ColumnKind::Other),
            at,
        ],
    )
}

/// What filter text reads as on `e`: the condition the reader finds and the terms of its
/// conjunction, each as its `Debug`.
pub fn read(e: Engine, r: &Read) -> String {
    let d = e.dialect();
    let cond = datarig_core::query::recognise(r.text, &read_target(d, r.mixed));
    let terms = datarig_core::query::split_terms(r.text, d);
    format!("{cond:?} | terms {terms:?}")
}

fn read_case(text: &'static str) -> Read {
    Read { text, mixed: false }
}

pub fn read_cases() -> Vec<Case<Read>> {
    vec![
        Case {
            name: "null-safe equality",
            input: read_case("id <=> 1"),
            expect: &[
                (Pg, "Some(Raw(\"id <=> 1\")) | terms Some([0..8])"),
                (MySql, "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..8])"),
                (
                    MariaDb,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..8])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..8])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..8])",
                ),
            ],
        },
        Case {
            name: "negated null-safe equality",
            input: read_case("NOT (id <=> 1)"),
            expect: &[
                (Pg, "Some(Not(Raw(\"id <=> 1\"))) | terms Some([0..14])"),
                (MySql, "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: true }) | terms Some([0..14])"),
                (
                    MariaDb,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: true }) | terms Some([0..14])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: true }) | terms Some([0..14])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: true }) | terms Some([0..14])",
                ),
            ],
        },
        Case {
            name: "is not distinct from",
            input: read_case("id IS NOT DISTINCT FROM 1"),
            expect: &[
                (Pg, "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..25])"),
                (
                    MySql,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..25])",
                ),
                (
                    MariaDb,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..25])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..25])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(NotDistinct { col: \"id\", value: Number(\"1\"), negated: false }) | terms Some([0..25])",
                ),
            ],
        },
        Case {
            name: "ilike",
            input: read_case("name ILIKE 'a%'"),
            expect: &[
                (
                    Pg,
                    "Some(Like { col: \"name\", pattern: \"a%\", negated: false, case: Insensitive }) | terms Some([0..15])",
                ),
                (MySql, "Some(Raw(\"name ILIKE 'a%'\")) | terms Some([0..15])"),
                (MariaDb, "Some(Raw(\"name ILIKE 'a%'\")) | terms Some([0..15])"),
                (MySqlAnsiQuotes, "Some(Raw(\"name ILIKE 'a%'\")) | terms Some([0..15])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"name ILIKE 'a%'\")) | terms Some([0..15])"),
            ],
        },
        Case {
            name: "lower like lower",
            input: read_case("LOWER(name) LIKE LOWER('a%')"),
            expect: &[
                (
                    Pg,
                    "Some(Like { col: \"name\", pattern: \"a%\", negated: false, case: Insensitive }) | terms Some([0..28])",
                ),
                (
                    MySql,
                    "Some(Like { col: \"name\", pattern: \"a%\", negated: false, case: Insensitive }) | terms Some([0..28])",
                ),
                (
                    MariaDb,
                    "Some(Like { col: \"name\", pattern: \"a%\", negated: false, case: Insensitive }) | terms Some([0..28])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(Like { col: \"name\", pattern: \"a%\", negated: false, case: Insensitive }) | terms Some([0..28])",
                ),
                (MySqlNoBackslashEscapes, "Some(Raw(\"LOWER(name) LIKE LOWER('a%')\")) | terms Some([0..28])"),
            ],
        },
        Case {
            name: "xor",
            input: read_case("id = 1 XOR y = 2"),
            expect: &[
                (Pg, "Some(Raw(\"id = 1 XOR y = 2\")) | terms None"),
                (MySql, "Some(Raw(\"id = 1 XOR y = 2\")) | terms None"),
                (MariaDb, "Some(Raw(\"id = 1 XOR y = 2\")) | terms None"),
                (MySqlAnsiQuotes, "Some(Raw(\"id = 1 XOR y = 2\")) | terms None"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"id = 1 XOR y = 2\")) | terms None"),
            ],
        },
        Case {
            name: "bitwise or",
            input: read_case("id | 1"),
            expect: &[
                (Pg, "Some(Raw(\"id | 1\")) | terms Some([0..6])"),
                (MySql, "Some(Raw(\"id | 1\")) | terms None"),
                (MariaDb, "Some(Raw(\"id | 1\")) | terms None"),
                (MySqlAnsiQuotes, "Some(Raw(\"id | 1\")) | terms None"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"id | 1\")) | terms None"),
            ],
        },
        Case {
            name: "bitwise and in a conjunction",
            input: read_case("id = 1 AND y & 2"),
            expect: &[
                (
                    Pg,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Raw(\"y & 2\")])) | terms Some([0..6, 11..16])",
                ),
                (MySql, "Some(Raw(\"id = 1 AND y & 2\")) | terms None"),
                (MariaDb, "Some(Raw(\"id = 1 AND y & 2\")) | terms None"),
                (MySqlAnsiQuotes, "Some(Raw(\"id = 1 AND y & 2\")) | terms None"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"id = 1 AND y & 2\")) | terms None"),
            ],
        },
        Case {
            name: "bang in a conjunction",
            input: read_case("id = 1 AND ! y"),
            expect: &[
                (
                    Pg,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Raw(\"! y\")])) | terms Some([0..6, 11..14])",
                ),
                (MySql, "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Raw(\"! y\")])) | terms None"),
                (MariaDb, "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Raw(\"! y\")])) | terms None"),
                (
                    MySqlAnsiQuotes,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Raw(\"! y\")])) | terms None",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Raw(\"! y\")])) | terms None",
                ),
            ],
        },
        Case {
            name: "not equal in a conjunction",
            input: read_case("id = 1 AND y != 2"),
            expect: &[
                (
                    Pg,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Cmp { col: \"y\", op: Ne, value: Number(\"2\") }])) | terms Some([0..6, 11..17])",
                ),
                (
                    MySql,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Cmp { col: \"y\", op: Ne, value: Number(\"2\") }])) | terms Some([0..6, 11..17])",
                ),
                (
                    MariaDb,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Cmp { col: \"y\", op: Ne, value: Number(\"2\") }])) | terms Some([0..6, 11..17])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Cmp { col: \"y\", op: Ne, value: Number(\"2\") }])) | terms Some([0..6, 11..17])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(All([Cmp { col: \"id\", op: Eq, value: Number(\"1\") }, Cmp { col: \"y\", op: Ne, value: Number(\"2\") }])) | terms Some([0..6, 11..17])",
                ),
            ],
        },
        Case {
            name: "not before between",
            input: read_case("NOT id BETWEEN 1 AND 2"),
            expect: &[
                (
                    Pg,
                    "Some(Not(Between { col: \"id\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false })) | terms Some([0..22])",
                ),
                (MySql, "Some(Raw(\"NOT id BETWEEN 1 AND 2\")) | terms Some([0..22])"),
                (MariaDb, "Some(Raw(\"NOT id BETWEEN 1 AND 2\")) | terms Some([0..22])"),
                (MySqlAnsiQuotes, "Some(Raw(\"NOT id BETWEEN 1 AND 2\")) | terms Some([0..22])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"NOT id BETWEEN 1 AND 2\")) | terms Some([0..22])"),
            ],
        },
        Case {
            name: "not over a group",
            input: read_case("NOT (id BETWEEN 1 AND 2)"),
            expect: &[
                (
                    Pg,
                    "Some(Not(Between { col: \"id\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false })) | terms Some([0..24])",
                ),
                (
                    MySql,
                    "Some(Not(Between { col: \"id\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false })) | terms Some([0..24])",
                ),
                (
                    MariaDb,
                    "Some(Not(Between { col: \"id\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false })) | terms Some([0..24])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(Not(Between { col: \"id\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false })) | terms Some([0..24])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Not(Between { col: \"id\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false })) | terms Some([0..24])",
                ),
            ],
        },
        Case {
            name: "between in a conjunction",
            input: read_case("x BETWEEN 1 AND 2 AND y = 1"),
            expect: &[
                (
                    Pg,
                    "Some(All([Between { col: \"x\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false }, Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms Some([0..17, 22..27])",
                ),
                (
                    MySql,
                    "Some(All([Between { col: \"x\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false }, Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms Some([0..17, 22..27])",
                ),
                (
                    MariaDb,
                    "Some(All([Between { col: \"x\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false }, Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms Some([0..17, 22..27])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(All([Between { col: \"x\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false }, Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms Some([0..17, 22..27])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(All([Between { col: \"x\", lo: Number(\"1\"), hi: Number(\"2\"), negated: false }, Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms Some([0..17, 22..27])",
                ),
            ],
        },
        Case {
            name: "interval in a conjunction",
            input: read_case("d > NOW() - INTERVAL 1 DAY AND y = 1"),
            expect: &[
                (
                    Pg,
                    "Some(All([Raw(\"d > NOW() - INTERVAL 1 DAY\"), Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms Some([0..26, 31..36])",
                ),
                (
                    MySql,
                    "Some(All([Raw(\"d > NOW() - INTERVAL 1 DAY\"), Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms None",
                ),
                (
                    MariaDb,
                    "Some(All([Raw(\"d > NOW() - INTERVAL 1 DAY\"), Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms None",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(All([Raw(\"d > NOW() - INTERVAL 1 DAY\"), Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms None",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(All([Raw(\"d > NOW() - INTERVAL 1 DAY\"), Cmp { col: \"y\", op: Eq, value: Number(\"1\") }])) | terms None",
                ),
            ],
        },
        Case {
            name: "utf8mb4 hex text",
            input: read_case("name = _utf8mb4 X'78'"),
            expect: &[
                (Pg, "Some(Raw(\"name = _utf8mb4 X'78'\")) | terms Some([0..21])"),
                (MySql, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..21])"),
                (MariaDb, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..21])"),
                (MySqlAnsiQuotes, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..21])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..21])",
                ),
            ],
        },
        Case {
            name: "hex bytes",
            input: read_case("data = X'00ab'"),
            expect: &[
                (Pg, "Some(Raw(\"data = X'00ab'\")) | terms Some([0..14])"),
                (MySql, "Some(Cmp { col: \"data\", op: Eq, value: Bytes([0, 171]) }) | terms Some([0..14])"),
                (MariaDb, "Some(Cmp { col: \"data\", op: Eq, value: Bytes([0, 171]) }) | terms Some([0..14])"),
                (MySqlAnsiQuotes, "Some(Cmp { col: \"data\", op: Eq, value: Bytes([0, 171]) }) | terms Some([0..14])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"data\", op: Eq, value: Bytes([0, 171]) }) | terms Some([0..14])",
                ),
            ],
        },
        Case {
            name: "bytea cast",
            input: read_case(r#"data = '\x00ab'::bytea"#),
            expect: &[
                (Pg, "Some(Raw(\"data = '\\\\x00ab'::bytea\")) | terms Some([0..22])"),
                (MySql, "Some(Raw(\"data = '\\\\x00ab'::bytea\")) | terms Some([0..22])"),
                (MariaDb, "Some(Raw(\"data = '\\\\x00ab'::bytea\")) | terms Some([0..22])"),
                (MySqlAnsiQuotes, "Some(Raw(\"data = '\\\\x00ab'::bytea\")) | terms Some([0..22])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"data = '\\\\x00ab'::bytea\")) | terms Some([0..22])"),
            ],
        },
        Case {
            name: "bytea cast with its schema",
            input: read_case(r#"data = '\x00ab'::pg_catalog.bytea"#),
            expect: &[
                (Pg, "Some(Raw(\"data = '\\\\x00ab'::pg_catalog.bytea\")) | terms Some([0..33])"),
                (MySql, "Some(Raw(\"data = '\\\\x00ab'::pg_catalog.bytea\")) | terms Some([0..33])"),
                (MariaDb, "Some(Raw(\"data = '\\\\x00ab'::pg_catalog.bytea\")) | terms Some([0..33])"),
                (MySqlAnsiQuotes, "Some(Raw(\"data = '\\\\x00ab'::pg_catalog.bytea\")) | terms Some([0..33])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"data = '\\\\x00ab'::pg_catalog.bytea\")) | terms Some([0..33])"),
            ],
        },
        Case {
            name: "plain string with a backslash",
            input: read_case(r#"name = 'a\b'"#),
            expect: &[
                (Pg, "Some(Raw(\"name = 'a\\\\b'\")) | terms Some([0..12])"),
                (MySql, "Some(Raw(\"name = 'a\\\\b'\")) | terms Some([0..12])"),
                (MariaDb, "Some(Raw(\"name = 'a\\\\b'\")) | terms Some([0..12])"),
                (MySqlAnsiQuotes, "Some(Raw(\"name = 'a\\\\b'\")) | terms Some([0..12])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"name\", op: Eq, value: Text(\"a\\\\b\") }) | terms Some([0..12])",
                ),
            ],
        },
        Case {
            name: "plain string with an escaped backslash",
            input: read_case(r#"name = 'a\\b'"#),
            expect: &[
                (Pg, "Some(Raw(\"name = 'a\\\\\\\\b'\")) | terms Some([0..13])"),
                (MySql, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"a\\\\b\") }) | terms Some([0..13])"),
                (MariaDb, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"a\\\\b\") }) | terms Some([0..13])"),
                (MySqlAnsiQuotes, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"a\\\\b\") }) | terms Some([0..13])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"name\", op: Eq, value: Text(\"a\\\\\\\\b\") }) | terms Some([0..13])",
                ),
            ],
        },
        Case {
            name: "escape string",
            input: read_case(r#"E'a\\b' = name"#),
            expect: &[
                (Pg, "Some(Raw(\"E'a\\\\\\\\b' = name\")) | terms Some([0..14])"),
                (MySql, "Some(Raw(\"E'a\\\\\\\\b' = name\")) | terms Some([0..14])"),
                (MariaDb, "Some(Raw(\"E'a\\\\\\\\b' = name\")) | terms Some([0..14])"),
                (MySqlAnsiQuotes, "Some(Raw(\"E'a\\\\\\\\b' = name\")) | terms Some([0..14])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"E'a\\\\\\\\b' = name\")) | terms Some([0..14])"),
            ],
        },
        Case {
            name: "escape string compared",
            input: read_case(r#"name = E'a\\b'"#),
            expect: &[
                (Pg, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"a\\\\b\") }) | terms Some([0..14])"),
                (MySql, "Some(Raw(\"name = E'a\\\\\\\\b'\")) | terms Some([0..14])"),
                (MariaDb, "Some(Raw(\"name = E'a\\\\\\\\b'\")) | terms Some([0..14])"),
                (MySqlAnsiQuotes, "Some(Raw(\"name = E'a\\\\\\\\b'\")) | terms Some([0..14])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"name = E'a\\\\\\\\b'\")) | terms Some([0..14])"),
            ],
        },
        Case {
            name: "double-quoted",
            input: read_case(r#"name = "x""#),
            expect: &[
                (Pg, "Some(Raw(\"name = \\\"x\\\"\")) | terms Some([0..10])"),
                (MySql, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])"),
                (MariaDb, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])"),
                (MySqlAnsiQuotes, "Some(Raw(\"name = \\\"x\\\"\")) | terms Some([0..10])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])",
                ),
            ],
        },
        Case {
            name: "like without escape",
            input: read_case(r#"name LIKE 'a\%'"#),
            expect: &[
                (Pg, "Some(Raw(\"name LIKE 'a\\\\%'\")) | terms Some([0..15])"),
                (MySql, "Some(Raw(\"name LIKE 'a\\\\%'\")) | terms Some([0..15])"),
                (MariaDb, "Some(Raw(\"name LIKE 'a\\\\%'\")) | terms Some([0..15])"),
                (MySqlAnsiQuotes, "Some(Raw(\"name LIKE 'a\\\\%'\")) | terms Some([0..15])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"name LIKE 'a\\\\%'\")) | terms Some([0..15])"),
            ],
        },
        Case {
            name: "like with escape",
            input: read_case("name LIKE 'a!%' ESCAPE '!'"),
            expect: &[
                (
                    Pg,
                    "Some(Like { col: \"name\", pattern: \"a!%\", negated: false, case: Server }) | terms Some([0..26])",
                ),
                (
                    MySql,
                    "Some(Like { col: \"name\", pattern: \"a!%\", negated: false, case: Server }) | terms Some([0..26])",
                ),
                (
                    MariaDb,
                    "Some(Like { col: \"name\", pattern: \"a!%\", negated: false, case: Server }) | terms Some([0..26])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(Like { col: \"name\", pattern: \"a!%\", negated: false, case: Server }) | terms Some([0..26])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Like { col: \"name\", pattern: \"a!%\", negated: false, case: Server }) | terms Some([0..26])",
                ),
            ],
        },
        Case {
            name: "instant",
            input: read_case("at = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)"),
            expect: &[
                (
                    Pg,
                    "Some(Raw(\"at = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\")) | terms Some([0..69])",
                ),
                (
                    MySql,
                    "Some(Cmp { col: \"at\", op: Eq, value: Text(\"2026-11-10 09:30:00+00:00\") }) | terms Some([0..69])",
                ),
                (
                    MariaDb,
                    "Some(Cmp { col: \"at\", op: Eq, value: Text(\"2026-11-10 09:30:00+00:00\") }) | terms Some([0..69])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(Cmp { col: \"at\", op: Eq, value: Text(\"2026-11-10 09:30:00+00:00\") }) | terms Some([0..69])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"at\", op: Eq, value: Text(\"2026-11-10 09:30:00+00:00\") }) | terms Some([0..69])",
                ),
            ],
        },
        Case {
            name: "instant on a column not zoned",
            input: read_case("d = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)"),
            expect: &[
                (
                    Pg,
                    "Some(Raw(\"d = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\")) | terms Some([0..68])",
                ),
                (
                    MySql,
                    "Some(Raw(\"d = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\")) | terms Some([0..68])",
                ),
                (
                    MariaDb,
                    "Some(Raw(\"d = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\")) | terms Some([0..68])",
                ),
                (
                    MySqlAnsiQuotes,
                    "Some(Raw(\"d = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\")) | terms Some([0..68])",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Raw(\"d = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\")) | terms Some([0..68])",
                ),
            ],
        },
        Case {
            name: "mixed-case column",
            input: read_case("NAME = 'x'"),
            expect: &[
                (Pg, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])"),
                (MySql, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])"),
                (MariaDb, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])"),
                (MySqlAnsiQuotes, "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"name\", op: Eq, value: Text(\"x\") }) | terms Some([0..10])",
                ),
            ],
        },
        Case {
            name: "dollar string",
            input: read_case("name = $$a$$"),
            expect: &[
                (Pg, "Some(Raw(\"name = $$a$$\")) | terms Some([0..12])"),
                (MySql, "Some(Raw(\"name = $$a$$\")) | terms Some([0..12])"),
                (MariaDb, "Some(Raw(\"name = $$a$$\")) | terms Some([0..12])"),
                (MySqlAnsiQuotes, "Some(Raw(\"name = $$a$$\")) | terms Some([0..12])"),
                (MySqlNoBackslashEscapes, "Some(Raw(\"name = $$a$$\")) | terms Some([0..12])"),
                (MySqlDollarNotKnown, "Some(Raw(\"name = $$a$$\")) | terms Some([0..12])"),
            ],
        },
        Case {
            name: "name as written",
            input: Read { text: "Ab = 1", mixed: true },
            expect: &[
                (Pg, "Some(Raw(\"Ab = 1\")) | terms Some([0..6])"),
                (MySql, "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])"),
                (MariaDb, "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])"),
                (MySqlAnsiQuotes, "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])",
                ),
            ],
        },
        Case {
            name: "name in another case",
            input: Read { text: "ab = 1", mixed: true },
            expect: &[
                (Pg, "Some(Raw(\"ab = 1\")) | terms Some([0..6])"),
                (MySql, "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])"),
                (MariaDb, "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])"),
                (MySqlAnsiQuotes, "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])"),
                (
                    MySqlNoBackslashEscapes,
                    "Some(Cmp { col: \"aB\", op: Eq, value: Number(\"1\") }) | terms Some([0..6])",
                ),
            ],
        },
    ]
}

/// Filter text checked before it runs, on a session judged on its own engine or, with
/// `sibling`, on both engines of the MySQL family (PostgreSQL's is always its own).
pub struct Check {
    pub text: &'static str,
    pub sibling: bool,
}

/// The check of filter text on `e` (its verdict or refusal, whole), whether it may be planned
/// for an estimate, and whether it may hold a subquery.
pub fn check(e: Engine, c: &Check) -> String {
    use datarig_core::query::{check_filter, estimate_allowed, has_subquery};
    use datarig_core::sql::risk::mysql::Judged;
    let d = e.dialect();
    let table = TableName { database: None, schema: "s".to_string(), name: "t".to_string() };
    let classifier = match Classifier::new(Language::Sql(d)) {
        Classifier::MySql(mode, _) if c.sibling => Classifier::MySql(mode, Judged::WithSibling),
        Classifier::MariaDb(mode, _) if c.sibling => Classifier::MariaDb(mode, Judged::WithSibling),
        own => own,
    };
    let verdict = check_filter(c.text, &table, &read_target(d, false), &classifier, false);
    let estimate = match &verdict {
        Ok(v) => format!("{:?}", estimate_allowed(v, c.text, d)),
        Err(_) => "-".to_string(),
    };
    format!("{verdict:?} | estimate {estimate} | subquery {}", has_subquery(c.text, d))
}

fn check_case(text: &'static str) -> Check {
    Check { text, sibling: false }
}

pub fn check_cases() -> Vec<Case<Check>> {
    vec![
        Case {
            name: "plain",
            input: check_case("id = 1"),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Ok(()) | subquery false"),
            ],
        },
        Case {
            name: "subquery",
            input: check_case("id IN (SELECT 1)"),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery true"),
                (MySql, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
                (MariaDb, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
            ],
        },
        Case {
            name: "subquery in another case",
            input: check_case("id IN ( select 1 )"),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery true"),
                (MySql, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
                (MariaDb, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Err(Subquery) | subquery true"),
            ],
        },
        Case {
            name: "mariadb executable comment",
            input: check_case("id = 1 /*M! OR 1 = 1 */"),
            expect: &[
                (Pg, "Err(Lexical { problem: Comment, at: 7..23 }) | estimate - | subquery true"),
                (MySql, "Err(Lexical { problem: ExecutableComment, at: 7..23 }) | estimate - | subquery true"),
                (MariaDb, "Err(Lexical { problem: ExecutableComment, at: 7..23 }) | estimate - | subquery true"),
                (
                    MySqlAnsiQuotes,
                    "Err(Lexical { problem: ExecutableComment, at: 7..23 }) | estimate - | subquery true",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Lexical { problem: ExecutableComment, at: 7..23 }) | estimate - | subquery true",
                ),
            ],
        },
        Case {
            name: "executable comment",
            input: check_case("id = 1 /*! OR 1 = 1 */"),
            expect: &[
                (Pg, "Err(Lexical { problem: Comment, at: 7..22 }) | estimate - | subquery true"),
                (MySql, "Err(Lexical { problem: ExecutableComment, at: 7..10 }) | estimate - | subquery true"),
                (MariaDb, "Err(Lexical { problem: ExecutableComment, at: 7..10 }) | estimate - | subquery true"),
                (
                    MySqlAnsiQuotes,
                    "Err(Lexical { problem: ExecutableComment, at: 7..10 }) | estimate - | subquery true",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Lexical { problem: ExecutableComment, at: 7..10 }) | estimate - | subquery true",
                ),
            ],
        },
        Case {
            name: "comment",
            input: check_case("id = 1 /* x */"),
            expect: &[
                (Pg, "Err(Lexical { problem: Comment, at: 7..14 }) | estimate - | subquery true"),
                (MySql, "Err(Lexical { problem: Comment, at: 7..14 }) | estimate - | subquery true"),
                (MariaDb, "Err(Lexical { problem: Comment, at: 7..14 }) | estimate - | subquery true"),
                (MySqlAnsiQuotes, "Err(Lexical { problem: Comment, at: 7..14 }) | estimate - | subquery true"),
                (MySqlNoBackslashEscapes, "Err(Lexical { problem: Comment, at: 7..14 }) | estimate - | subquery true"),
            ],
        },
        Case {
            name: "hash comment",
            input: check_case("id = 1 # x"),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Err(Lexical { problem: Comment, at: 7..10 }) | estimate - | subquery true"),
                (MariaDb, "Err(Lexical { problem: Comment, at: 7..10 }) | estimate - | subquery true"),
                (MySqlAnsiQuotes, "Err(Lexical { problem: Comment, at: 7..10 }) | estimate - | subquery true"),
                (MySqlNoBackslashEscapes, "Err(Lexical { problem: Comment, at: 7..10 }) | estimate - | subquery true"),
            ],
        },
        Case {
            name: "semicolon",
            input: check_case("id = 1; DROP TABLE t"),
            expect: &[
                (Pg, "Err(Lexical { problem: Semicolon, at: 6..7 }) | estimate - | subquery true"),
                (MySql, "Err(Lexical { problem: Semicolon, at: 6..7 }) | estimate - | subquery true"),
                (MariaDb, "Err(Lexical { problem: Semicolon, at: 6..7 }) | estimate - | subquery true"),
                (MySqlAnsiQuotes, "Err(Lexical { problem: Semicolon, at: 6..7 }) | estimate - | subquery true"),
                (MySqlNoBackslashEscapes, "Err(Lexical { problem: Semicolon, at: 6..7 }) | estimate - | subquery true"),
            ],
        },
        Case {
            name: "backslash in a plain string",
            input: check_case(r#"name = 'a\b'"#),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Ok(()) | subquery false"),
            ],
        },
        Case {
            name: "backslash before a quote",
            input: check_case(r#"name = 'a\' OR id = 1 OR name = '"#),
            expect: &[
                (Pg, "Err(Lexical { problem: Unterminated, at: 32..33 }) | estimate - | subquery true"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Lexical { problem: Unterminated, at: 32..33 }) | estimate - | subquery true",
                ),
            ],
        },
        Case {
            name: "double quote",
            input: check_case(r#"name = "a""#),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Ok(()) | subquery false"),
            ],
        },
        Case {
            name: "double quote read as a name",
            input: check_case(r#""name" = 'a' "#),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Ok(()) | subquery false"),
            ],
        },
        Case {
            name: "dollar",
            input: check_case("name = $$a$$"),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlDollarNotKnown, "Ok(Clear) | estimate Ok(()) | subquery false"),
            ],
        },
        Case {
            name: "for update",
            input: check_case("id = 1 FOR UPDATE"),
            expect: &[
                (Pg, "Err(Shape { problem: Locking, at: Some(7..17) }) | estimate - | subquery false"),
                (MySql, "Err(Shape { problem: Locking, at: Some(7..10) }) | estimate - | subquery false"),
                (MariaDb, "Err(Shape { problem: Locking, at: Some(7..10) }) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(Shape { problem: Locking, at: Some(7..10) }) | estimate - | subquery false"),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Shape { problem: Locking, at: Some(7..10) }) | estimate - | subquery false",
                ),
            ],
        },
        Case {
            name: "lock in share mode",
            input: check_case("id = 1 LOCK IN SHARE MODE"),
            expect: &[
                (Pg, "Err(NotRead { class: Unknown, danger: Some(Unparsed) }) | estimate - | subquery false"),
                (MySql, "Err(Shape { problem: Locking, at: Some(7..11) }) | estimate - | subquery false"),
                (MariaDb, "Err(Shape { problem: Locking, at: Some(7..11) }) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(Shape { problem: Locking, at: Some(7..11) }) | estimate - | subquery false"),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Shape { problem: Locking, at: Some(7..11) }) | estimate - | subquery false",
                ),
            ],
        },
        Case {
            name: "union",
            input: check_case("id = 1 UNION SELECT 1"),
            expect: &[
                (Pg, "Err(NotRead { class: Unknown, danger: Some(Unparsed) }) | estimate - | subquery false"),
                (MySql, "Err(Shape { problem: SetOperation, at: Some(7..12) }) | estimate - | subquery false"),
                (MariaDb, "Err(Shape { problem: SetOperation, at: Some(7..12) }) | estimate - | subquery false"),
                (
                    MySqlAnsiQuotes,
                    "Err(Shape { problem: SetOperation, at: Some(7..12) }) | estimate - | subquery false",
                ),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Shape { problem: SetOperation, at: Some(7..12) }) | estimate - | subquery false",
                ),
            ],
        },
        Case {
            name: "next value for",
            input: check_case("id = NEXT VALUE FOR s"),
            expect: &[
                (Pg, "Err(NotRead { class: Unknown, danger: Some(Unparsed) }) | estimate - | subquery false"),
                (MySql, "Err(Shape { problem: Sequence, at: Some(5..9) }) | estimate - | subquery false"),
                (MariaDb, "Err(Shape { problem: Sequence, at: Some(5..9) }) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(Shape { problem: Sequence, at: Some(5..9) }) | estimate - | subquery false"),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Shape { problem: Sequence, at: Some(5..9) }) | estimate - | subquery false",
                ),
            ],
        },
        Case {
            name: "nextval",
            input: check_case("NEXTVAL(s) > 0"),
            expect: &[
                (Pg, "Err(RefusedCalls([\"nextval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "setval",
            input: check_case("SETVAL(s, 1) > 0"),
            expect: &[
                (Pg, "Err(RefusedCalls([\"setval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "nextval beside an unknown call",
            input: check_case("NEXTVAL(s) > 0 AND foo(id) = 1"),
            expect: &[
                (Pg, "Err(RefusedCalls([\"nextval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "setval beside an unknown call",
            input: check_case("SETVAL(s, 1) > 0 AND foo(id) = 1"),
            expect: &[
                (Pg, "Err(RefusedCalls([\"setval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "nextval judged with the sibling",
            input: Check { text: "NEXTVAL(s) > 0", sibling: true },
            expect: &[
                (Pg, "Err(RefusedCalls([\"nextval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "setval judged with the sibling",
            input: Check { text: "SETVAL(s, 1) > 0", sibling: true },
            expect: &[
                (Pg, "Err(RefusedCalls([\"setval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "nextval beside an unknown call judged with the sibling",
            input: Check { text: "NEXTVAL(s) > 0 AND foo(id) = 1", sibling: true },
            expect: &[
                (Pg, "Err(RefusedCalls([\"nextval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"NEXTVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "setval beside an unknown call judged with the sibling",
            input: Check { text: "SETVAL(s, 1) > 0 AND foo(id) = 1", sibling: true },
            expect: &[
                (Pg, "Err(RefusedCalls([\"setval\"])) | estimate - | subquery false"),
                (MySql, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"SETVAL\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "unknown call",
            input: check_case("foo(id) = 1"),
            expect: &[
                (Pg, "Ok(NeedsConfirm([Calls([\"foo\"])])) | estimate Err(Calls) | subquery false"),
                (MySql, "Ok(NeedsConfirm([Calls([\"FOO\"])])) | estimate Err(Calls) | subquery false"),
                (MariaDb, "Ok(NeedsConfirm([Calls([\"FOO\"])])) | estimate Err(Calls) | subquery false"),
                (MySqlAnsiQuotes, "Ok(NeedsConfirm([Calls([\"FOO\"])])) | estimate Err(Calls) | subquery false"),
                (
                    MySqlNoBackslashEscapes,
                    "Ok(NeedsConfirm([Calls([\"FOO\"])])) | estimate Err(Calls) | subquery false",
                ),
            ],
        },
        Case {
            name: "allowed call",
            input: check_case("LOWER(name) = 'a'"),
            expect: &[
                (Pg, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySql, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MariaDb, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlAnsiQuotes, "Ok(Clear) | estimate Ok(()) | subquery false"),
                (MySqlNoBackslashEscapes, "Ok(Clear) | estimate Ok(()) | subquery false"),
            ],
        },
        Case {
            name: "refused call",
            input: check_case("SLEEP(1) = 0"),
            expect: &[
                (Pg, "Ok(NeedsConfirm([Calls([\"sleep\"])])) | estimate Err(Calls) | subquery false"),
                (MySql, "Err(RefusedCalls([\"SLEEP\"])) | estimate - | subquery false"),
                (MariaDb, "Err(RefusedCalls([\"SLEEP\"])) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(RefusedCalls([\"SLEEP\"])) | estimate - | subquery false"),
                (MySqlNoBackslashEscapes, "Err(RefusedCalls([\"SLEEP\"])) | estimate - | subquery false"),
            ],
        },
        Case {
            name: "assignment",
            input: check_case("(@a := 1) = 1"),
            expect: &[
                (Pg, "Err(NotRead { class: Unknown, danger: Some(Unparsed) }) | estimate - | subquery false"),
                (MySql, "Err(Shape { problem: Assignment, at: Some(4..5) }) | estimate - | subquery false"),
                (MariaDb, "Err(Shape { problem: Assignment, at: Some(4..5) }) | estimate - | subquery false"),
                (MySqlAnsiQuotes, "Err(Shape { problem: Assignment, at: Some(4..5) }) | estimate - | subquery false"),
                (
                    MySqlNoBackslashEscapes,
                    "Err(Shape { problem: Assignment, at: Some(4..5) }) | estimate - | subquery false",
                ),
            ],
        },
    ]
}
