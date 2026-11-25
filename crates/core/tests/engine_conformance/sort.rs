//! How each engine orders and pages a table's rows: where NULLs sort when a key does not say,
//! how a key that says is written, whether an order may say it, how an unquoted name finds its
//! column, and how rows are skipped without a cap. Which bare names the server may read as a
//! call rather than as the column of that name ([`bare_call`], [`bare_call_cases`]).

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};
use datarig_core::query::{
    ColumnFacts, ColumnKind, Cond, Dir, Nulls, OrderKey, Query, TableName, Target, compile, compile_capped,
    parse_order, recognise,
};

/// What a sort case asks.
pub enum Input {
    /// Whether a key of this direction that does not say where its NULLs go has them first.
    DefaultNulls(Dir),
    /// The `ORDER BY` clause written for a key on `id` of this direction with its NULLs placed.
    NullsKey(Dir, Nulls),
    /// The keys an `ORDER BY` text reads as, or why it is refused.
    Order(&'static str),
    /// A capped read with this limit and offset.
    Capped(Option<u64>, Option<u64>),
}

/// The table every case reads: a number key and two text columns whose names differ only in
/// case.
fn target(e: Engine) -> Target {
    let columns = vec![
        ColumnFacts::new("id", ColumnKind::Number),
        ColumnFacts::new("aB", ColumnKind::Text),
        ColumnFacts::new("Ab", ColumnKind::Text),
    ];
    Target { tie_break: false, ..Target::new(e.dialect(), columns) }
}

fn query(order: Vec<OrderKey>, limit: Option<u64>, offset: Option<u64>) -> Query {
    let mut q = Query::table(TableName { database: None, schema: "s".to_string(), name: "t".to_string() });
    q.order = order;
    q.limit = limit;
    q.offset = offset;
    q
}

/// The answer of engine `e` to `input`.
pub fn sort(e: Engine, input: &Input) -> String {
    let t = target(e);
    match input {
        Input::DefaultNulls(dir) => {
            let first = OrderKey::user("id", *dir).nulls_first(e.dialect());
            (if first { "first" } else { "last" }).to_string()
        }
        Input::NullsKey(dir, nulls) => {
            let key = OrderKey { nulls: Some(*nulls), ..OrderKey::user("id", *dir) };
            let sql = compile(&query(vec![key], None, None), &t).sql;
            sql.split_once(" ORDER BY ").map_or(sql.clone(), |(_, keys)| keys.to_string())
        }
        Input::Order(text) => match parse_order(text, &t) {
            Ok(keys) => {
                keys.iter().map(|k| format!("{} {:?} {:?}", k.col, k.dir, k.nulls)).collect::<Vec<_>>().join(", ")
            }
            Err(e) => format!("{e:?}"),
        },
        Input::Capped(limit, offset) => compile_capped(&query(Vec::new(), *limit, *offset), &t).sql,
    }
}

pub fn sort_cases() -> Vec<Case<Input>> {
    vec![
        Case {
            name: "default nulls ascending",
            input: Input::DefaultNulls(Dir::Asc),
            expect: &[(Pg, "last"), (MySql, "first"), (MariaDb, "first")],
        },
        Case {
            name: "default nulls descending",
            input: Input::DefaultNulls(Dir::Desc),
            expect: &[(Pg, "first"), (MySql, "last"), (MariaDb, "last")],
        },
        Case {
            name: "nulls first ascending",
            input: Input::NullsKey(Dir::Asc, Nulls::First),
            expect: &[
                (Pg, "\"id\" ASC NULLS FIRST"),
                (MySql, "`id` IS NULL DESC, `id` ASC"),
                (MariaDb, "`id` IS NULL DESC, `id` ASC"),
            ],
        },
        Case {
            name: "nulls last ascending",
            input: Input::NullsKey(Dir::Asc, Nulls::Last),
            expect: &[
                (Pg, "\"id\" ASC NULLS LAST"),
                (MySql, "`id` IS NULL ASC, `id` ASC"),
                (MariaDb, "`id` IS NULL ASC, `id` ASC"),
            ],
        },
        Case {
            name: "nulls first descending",
            input: Input::NullsKey(Dir::Desc, Nulls::First),
            expect: &[
                (Pg, "\"id\" DESC NULLS FIRST"),
                (MySql, "`id` IS NULL DESC, `id` DESC"),
                (MariaDb, "`id` IS NULL DESC, `id` DESC"),
            ],
        },
        Case {
            name: "nulls last descending",
            input: Input::NullsKey(Dir::Desc, Nulls::Last),
            expect: &[
                (Pg, "\"id\" DESC NULLS LAST"),
                (MySql, "`id` IS NULL ASC, `id` DESC"),
                (MariaDb, "`id` IS NULL ASC, `id` DESC"),
            ],
        },
        Case {
            name: "typed nulls first",
            input: Input::Order("id DESC NULLS FIRST"),
            expect: &[
                (Pg, "id Desc Some(First)"),
                (MySql, "OrderError { range: 8..19, kind: NullsUnsupported }"),
                (MariaDb, "OrderError { range: 8..19, kind: NullsUnsupported }"),
            ],
        },
        Case {
            name: "typed nulls last",
            input: Input::Order("id NULLS LAST"),
            expect: &[
                (Pg, "id Asc Some(Last)"),
                (MySql, "OrderError { range: 3..13, kind: NullsUnsupported }"),
                (MariaDb, "OrderError { range: 3..13, kind: NullsUnsupported }"),
            ],
        },
        Case {
            name: "a name as written",
            input: Input::Order("Ab"),
            expect: &[(Pg, "Ab Asc None"), (MySql, "Ab Asc None"), (MariaDb, "Ab Asc None")],
        },
        Case {
            name: "a name two columns match without case",
            input: Input::Order("ab"),
            expect: &[
                (Pg, "OrderError { range: 0..2, kind: UnknownColumn(\"ab\") }"),
                (MySql, "OrderError { range: 0..2, kind: AmbiguousColumn(\"ab\") }"),
                (MariaDb, "OrderError { range: 0..2, kind: AmbiguousColumn(\"ab\") }"),
            ],
        },
        Case {
            name: "a name one column matches without case",
            input: Input::Order("ID DESC"),
            expect: &[(Pg, "id Desc None"), (MySql, "id Desc None"), (MariaDb, "id Desc None")],
        },
        Case {
            name: "offset alone",
            input: Input::Capped(None, Some(20)),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" OFFSET 20"),
                (MySql, "SELECT * FROM `s`.`t` LIMIT 18446744073709551615 OFFSET 20"),
                (MariaDb, "SELECT * FROM `s`.`t` LIMIT 18446744073709551615 OFFSET 20"),
            ],
        },
        Case {
            name: "limit and offset",
            input: Input::Capped(Some(10), Some(20)),
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" LIMIT 10 OFFSET 20"),
                (MySql, "SELECT * FROM `s`.`t` LIMIT 10 OFFSET 20"),
                (MariaDb, "SELECT * FROM `s`.`t` LIMIT 10 OFFSET 20"),
            ],
        },
    ]
}

/// How filter text `<name> IS NULL` reads on `e`, on a table with a column of that name (in
/// lower case): `column` when it is that column's condition, `as written` when the bare name
/// may be a call and the text is kept as typed.
pub fn bare_call(e: Engine, name: &&'static str) -> String {
    let t = Target::new(e.dialect(), vec![ColumnFacts::new(name.to_lowercase(), ColumnKind::Number)]);
    match recognise(&format!("{name} IS NULL"), &t) {
        Some(Cond::IsNull { .. }) => "column".to_string(),
        Some(Cond::Raw(_)) => "as written".to_string(),
        other => format!("{other:?}"),
    }
}

const CALL: &[(Engine, &str)] = &[(Pg, "column"), (MySql, "as written"), (MariaDb, "as written")];
const COLUMN: &[(Engine, &str)] = &[(Pg, "column"), (MySql, "column"), (MariaDb, "column")];

/// The words MySQL or MariaDB may read bare as a call (each on every session of either: one
/// set for both), and words that stay column names.
pub fn bare_call_cases() -> Vec<Case<&'static str>> {
    let calls = [
        "current_date",
        "current_time",
        "current_timestamp",
        "current_user",
        "localtime",
        "localtimestamp",
        "utc_date",
        "utc_time",
        "utc_timestamp",
        "current_role",
        "CURRENT_ROLE",
    ];
    let columns = ["date", "char", "user", "now", "sysdate", "database", "session_user", "id"];
    calls
        .into_iter()
        .map(|name| Case { name, input: name, expect: CALL })
        .chain(columns.into_iter().map(|name| Case { name, input: name, expect: COLUMN }))
        .collect()
}
