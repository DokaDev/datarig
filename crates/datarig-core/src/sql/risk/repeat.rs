//! Which statements the app may run again, or count, on the user's behalf:
//! the **plain-`SELECT` allowlist**.
//!
//! Paging is explicit: a result's portal may be closed before the user has seen every row (never
//! held, the policy's `paging = "no_hold"`; the policy's `paging_idle_timeout`; another statement
//! run in the tab). The app then fetches the
//! next page only by running the same statement again and skipping the rows it has, and it
//! counts a result's rows only when the user asks, with `SELECT count(*) FROM (<statement>)`.
//! Both run the user's statement again, so both are allowed only for a statement that cannot
//! change anything when it runs, and whose rows do not depend on when it runs beyond the data
//! itself:
//!
//! * exactly one statement, a query at the top (`SELECT`, `VALUES`, `TABLE`): not `EXPLAIN`,
//!   `SHOW`, `FETCH`, `EXECUTE`, DML or anything else;
//! * a plain read by the classifier ([`super::classify`]): no `INTO`, no row lock, no
//!   data-modifying `WITH`, nothing dangerous, nothing asking for read-write;
//! * no code the text does not show ([`super::Risk::runs_code`]): every function it calls is a
//!   known built-in;
//! * no call of a **volatile** built-in, by any name the call is written with (a function with
//!   a side effect is volatile: `nextval`, `setval`, `pg_advisory_lock`, `pg_notify`,
//!   `set_config`; and so are those whose value changes on every call, which
//!   would make the rows skipped differ from the rows seen: `random`, `clock_timestamp`). The
//!   list ([`VOLATILE`]) is generated from the server; an integration test pins it to its
//!   catalog. A name that is volatile in any of its overloads counts as volatile;
//! * every operator it names is a built-in one ([`OPERATORS`]: `a + b`, `a OPERATOR(pg_catalog.+)
//!   b`; an operator of another schema, `OPERATOR(public.+)`, or a name no built-in operator
//!   has refuses it), and every type it names is a built-in one ([`TYPES`]: casts, `::`,
//!   `CAST`, typed literals, the column definitions of `json_to_record(…) AS (c t)`; a user's
//!   type or domain refuses it, since a domain's `CHECK` and a user type's input function are
//!   code). Both lists are generated from the server and pinned to it by integration tests;
//! * no `TABLESAMPLE` (its rows are chosen at random on every run);
//! * not over the classifier's caps, and a text the parser rejects is refused.
//!
//! **What only the server can tell** is asked right before the app runs the statement again or
//! counts it, in one query on the same session ([`check_query`], from the [`Names`] the text
//! shows): every relation it reads (and every table that inherits from one, or is a partition)
//! is a table, a partitioned table or a materialized view without row-level security (a view's
//! query, a foreign table's wrapper and a policy are code the text does not show); no function
//! of the user's has the name of a function it calls (a user's overload of a built-in, such as
//! `public.abs(text)` for `abs('x')`), or, with one argument, of a name it may call in attribute
//! notation (`t.f`, `(t).f`); no operator of the user's that takes a built-in type has the name
//! of an operator it uses; no type of the user's shadows a type it names (a schema ahead of
//! `pg_catalog` in `search_path`); and no column of a table it reads has a type of the user's
//! that functions, operators or casts of the user's take (or whose input or output function is
//! the user's): an operator, a sort or a comparison on such a column could run them.
//!
//! This is an allowlist: anything it does not recognise is refused, with the reason. What is
//! still not seen: a cast of the user's from a built-in type to a type of the user's that the
//! server applies on its own (an implicit cast, to call an operator of the user's), the fields
//! of a composite column whose types are the user's, and anything the text reaches only through
//! a built-in that reads a table by name (`table_to_xml` of a view). A re-run is a read, and a
//! read-only policy still makes the server refuse any write. Skipping by reading evaluates the
//! skipped rows again too (every expression of the select list, for every row skipped).

use super::{Class, Danger, Explain, MAX_BYTES, MAX_DEPTH, STACK, THREAD, calls, depth, has, node, parse, walk};
use crate::sql::lexer::{Tok, lex};
use serde_json::Value;

/// Why the app may not run a statement again or count its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotRepeatable {
    /// Not exactly one statement (none, or several).
    NotOne,
    /// Not a query at the top: `EXPLAIN`, `SHOW`, `FETCH`, `EXECUTE`, DML, DDL, …
    NotSelect,
    /// A query that writes, locks rows, asks for read-write or is dangerous (`INTO`,
    /// `FOR UPDATE`, a data-modifying `WITH`, a server built-in, …).
    Writes,
    /// It calls a function that is not a known built-in, or runs code the text does not show.
    UserFunction,
    /// It calls a volatile built-in (or samples a table): the name.
    Volatile(String),
    /// It uses an operator that is not a built-in one (another schema's, or a name no built-in
    /// operator has): the name as written.
    UserOperator(String),
    /// It names a type that is not a built-in one (a user's type or domain): the name as
    /// written.
    UserType(String),
    /// The server says it reads a relation that is not a table (a view, a foreign table, …) or
    /// that it cannot find: its name.
    NotATable(String),
    /// The server says a relation it reads has row-level security: its name.
    RowSecurity(String),
    /// The server has a function or an operator of the user's with a name it calls or uses (an
    /// overload of a built-in's name, or a function it may call in attribute notation).
    Shadowed(String),
    /// The server says a column it reads has a type of the user's that functions, operators or
    /// casts of the user's take: the type.
    UserColumnType(String),
    /// The parser rejects it, or it is too long or too deep to read.
    Unreadable,
}

impl NotRepeatable {
    /// The reason a row of [`check_query`] gives (`kind`, `name`); `None` for a kind it does not
    /// send.
    pub fn from_check(kind: &str, name: &str) -> Option<Self> {
        let name = name.to_string();
        Some(match kind {
            "relation" => NotRepeatable::NotATable(name),
            "policy" => NotRepeatable::RowSecurity(name),
            "shadowed" => NotRepeatable::Shadowed(name),
            "type" => NotRepeatable::UserType(name),
            "column_type" => NotRepeatable::UserColumnType(name),
            _ => return None,
        })
    }
}

/// The names of PostgreSQL 17's volatile built-in functions (`provolatile = 'v'`, `oid <
/// 10000`, in `pg_catalog`), one per line in byte order. Generated with `SELECT DISTINCT
/// proname::text COLLATE "C" FROM pg_proc WHERE pronamespace = 'pg_catalog'::regnamespace AND
/// oid < 10000 AND provolatile = 'v' ORDER BY 1`.
pub const VOLATILE: &str = include_str!("volatile.txt");

/// Whether `name` (as PostgreSQL folds it) is a volatile built-in function in [`VOLATILE`].
pub fn volatile(name: &str) -> bool {
    static SORTED: std::sync::OnceLock<Vec<&str>> = std::sync::OnceLock::new();
    SORTED.get_or_init(|| VOLATILE.lines().collect()).binary_search(&name).is_ok()
}

/// The names of PostgreSQL 17's built-in operators (`pg_operator` rows of `pg_catalog` that
/// initdb creates, `oid < 10000`), one per line in byte order. Generated with `SELECT DISTINCT
/// oprname::text COLLATE "C" FROM pg_operator WHERE oprnamespace = 'pg_catalog'::regnamespace
/// AND oid < 10000 ORDER BY 1`.
pub const OPERATORS: &str = include_str!("operators.txt");

/// The names of PostgreSQL 17's built-in types (`pg_type` rows of `pg_catalog` that initdb
/// creates, `oid < 10000`), one per line in byte order. Generated with `SELECT DISTINCT
/// typname::text COLLATE "C" FROM pg_type WHERE typnamespace = 'pg_catalog'::regnamespace AND
/// oid < 10000 ORDER BY 1`.
pub const TYPES: &str = include_str!("types.txt");

/// Whether `name` is a built-in operator's name in [`OPERATORS`].
pub fn builtin_operator(name: &str) -> bool {
    static SORTED: std::sync::OnceLock<Vec<&str>> = std::sync::OnceLock::new();
    SORTED.get_or_init(|| OPERATORS.lines().collect()).binary_search(&name).is_ok()
}

/// Whether `name` (as PostgreSQL folds it) is a built-in type's name in [`TYPES`].
pub fn builtin_type(name: &str) -> bool {
    static SORTED: std::sync::OnceLock<Vec<&str>> = std::sync::OnceLock::new();
    SORTED.get_or_init(|| TYPES.lines().collect()).binary_search(&name).is_ok()
}

/// The names the parser gives a `BETWEEN` (it becomes `>=` and `<=` of the operands' types).
const BETWEEN: [&str; 4] = ["BETWEEN", "NOT BETWEEN", "BETWEEN SYMMETRIC", "NOT BETWEEN SYMMETRIC"];

/// The objects a statement names, which the server is asked about before the app runs it again
/// or counts it ([`check_query`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    /// The relations it reads as written: schema (empty when not written) and name.
    pub relations: Vec<(String, String)>,
    /// The names of its `WITH` queries (a relation without a schema may be one of them).
    pub ctes: Vec<String>,
    /// The functions it calls (the last name of each call).
    pub functions: Vec<String>,
    /// The names it may call in attribute notation (every name of a column reference but the
    /// first, every name of an indirection).
    pub attributes: Vec<String>,
    /// The operators it uses (the implicit ones of `BETWEEN` too).
    pub operators: Vec<String>,
    /// The types it names without a schema.
    pub types: Vec<String>,
}

impl Names {
    fn add(list: &mut Vec<String>, name: &str) {
        if !list.iter().any(|n| n == name) {
            list.push(name.to_string());
        }
    }
}

/// What the parse tree of a text says for the allowlist.
struct Shape {
    statements: usize,
    select: bool,
    /// The first volatile call (or `TABLESAMPLE`), if any.
    volatile: Option<String>,
    order_by: bool,
    /// The first operator that is not a built-in one, as written.
    user_operator: Option<String>,
    /// The first type that is not a built-in one, as written.
    user_type: Option<String>,
    /// The first relation named with its database (`db.schema.table`).
    other_database: Option<String>,
    names: Names,
}

/// A qualified name as written (`a.b.c`).
fn dotted(names: &[&str]) -> String {
    names.join(".")
}

/// The names of a list of `String` nodes; `None` when one is not a name.
fn name_list(v: &Value) -> Option<Vec<&str>> {
    v.as_array()?.iter().map(super::text).collect()
}

/// Read `sql`'s parse tree on the classifier's thread (a large stack: see the module docs of
/// [`super`]); `None` when the parser rejects it or reading it failed.
fn shape(sql: &str) -> Option<Shape> {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().name(THREAD.to_string()).stack_size(STACK);
        worker.spawn_scoped(scope, || shape_here(sql)).ok().and_then(|h| h.join().ok()).flatten()
    })
}

fn shape_here(sql: &str) -> Option<Shape> {
    let statements = parse(sql)?;
    let top = statements.first().and_then(node);
    let select = matches!(top, Some(("SelectStmt", _)));
    let order_by = top.is_some_and(|(_, b)| b["sort_clause"].as_array().is_some_and(|s| !s.is_empty()));
    let mut volatile = None;
    let (mut user_operator, mut user_type, mut other_database) = (None, None, None);
    let mut names = Names::default();
    for s in &statements {
        if has(s, "RangeTableSample") {
            volatile = volatile.or(Some("TABLESAMPLE".to_string()));
        }
        walk(s, &mut |o: &serde_json::Map<String, Value>| {
            for call in calls(o) {
                let name = match call {
                    super::Call::Func(written, _) => {
                        let f = written.last().copied();
                        if let Some(f) = f {
                            Names::add(&mut names.functions, f);
                        }
                        f
                    }
                    super::Call::Attribute(f, _) => {
                        Names::add(&mut names.attributes, f);
                        Some(f)
                    }
                };
                if let Some(f) = name.filter(|f| self::volatile(f)) {
                    volatile.get_or_insert_with(|| f.to_string());
                }
            }
            // Operators: of an expression, of `x op ANY (SELECT …)`, of `ORDER BY x USING op`.
            let operator = [("AExpr", "name"), ("SubLink", "oper_name"), ("SortBy", "use_op")]
                .iter()
                .find_map(|(kind, key)| o.get(*kind).map(|b| &b[*key]))
                .filter(|n| n.as_array().is_some_and(|a| !a.is_empty()));
            if let Some(written) = operator {
                match name_list(written).as_deref() {
                    Some([op]) if BETWEEN.contains(op) => {
                        Names::add(&mut names.operators, ">=");
                        Names::add(&mut names.operators, "<=");
                    }
                    Some([op] | ["pg_catalog", op]) if builtin_operator(op) => Names::add(&mut names.operators, op),
                    Some(other) => {
                        user_operator.get_or_insert_with(|| dotted(other));
                    }
                    None => {
                        user_operator.get_or_insert_with(String::new);
                    }
                }
            }
            // Types: of a cast (`::`, `CAST`, a typed literal), a column definition list, the
            // `RETURNING` of a JSON function, an `XMLSERIALIZE`, …: every type name of the tree.
            let written = o.get("type_name").or_else(|| o.get("TypeName")).filter(|t| t.get("names").is_some());
            if let Some(t) = written {
                let pct = t["pct_type"].as_bool().unwrap_or(false);
                match name_list(&t["names"]).as_deref() {
                    Some([ty]) if !pct && builtin_type(ty) => Names::add(&mut names.types, ty),
                    Some(["pg_catalog", ty]) if !pct && builtin_type(ty) => {}
                    Some(other) => {
                        user_type.get_or_insert_with(|| dotted(other));
                    }
                    None => {
                        user_type.get_or_insert_with(String::new);
                    }
                }
            }
            if let Some(r) = o.get("RangeVar") {
                let (catalog, schema, rel) = (
                    super::text(&r["catalogname"]).unwrap_or_default(),
                    super::text(&r["schemaname"]).unwrap_or_default(),
                    super::text(&r["relname"]).unwrap_or_default(),
                );
                if catalog.is_empty() {
                    let pair = (schema.to_string(), rel.to_string());
                    if !names.relations.contains(&pair) {
                        names.relations.push(pair);
                    }
                } else {
                    other_database.get_or_insert_with(|| dotted(&[catalog, schema, rel]));
                }
            }
            if let Some(cte) = o.get("CommonTableExpr").and_then(|c| super::text(&c["ctename"])) {
                Names::add(&mut names.ctes, cte);
            }
        });
    }
    Some(Shape {
        statements: statements.len(),
        select,
        volatile,
        order_by,
        user_operator,
        user_type,
        other_database,
        names,
    })
}

/// Whether the app may run `sql` again (to fetch past a closed portal) or count its rows, as
/// far as its text tells: the plain-`SELECT` allowlist (see the module docs). The reason when it
/// may not. What only the server can tell is asked with [`check_query`] of [`names`] right
/// before it runs.
pub fn repeatable(sql: &str) -> Result<(), NotRepeatable> {
    names(sql).map(|_| ())
}

/// [`repeatable`], with the objects the statement names when it is on the allowlist.
pub fn names(sql: &str) -> Result<Names, NotRepeatable> {
    if sql.len() > MAX_BYTES || depth(sql) > MAX_DEPTH {
        return Err(NotRepeatable::Unreadable);
    }
    let shape = shape(sql).ok_or(NotRepeatable::Unreadable)?;
    if shape.statements != 1 {
        return Err(NotRepeatable::NotOne);
    }
    if !shape.select {
        return Err(NotRepeatable::NotSelect);
    }
    let risk = super::classify(sql);
    match risk.danger {
        Some(Danger::Unparsed | Danger::TooComplex) => return Err(NotRepeatable::Unreadable),
        Some(_) => return Err(NotRepeatable::Writes),
        None => {}
    }
    if risk.class != Class::Read || risk.writes || risk.read_write || risk.stdio || risk.explain != Explain::No {
        return Err(NotRepeatable::Writes);
    }
    if risk.runs_code {
        return Err(NotRepeatable::UserFunction);
    }
    if let Some(f) = shape.volatile {
        return Err(NotRepeatable::Volatile(f));
    }
    if let Some(op) = shape.user_operator {
        return Err(NotRepeatable::UserOperator(op));
    }
    if let Some(ty) = shape.user_type {
        return Err(NotRepeatable::UserType(ty));
    }
    if let Some(rel) = shape.other_database {
        return Err(NotRepeatable::NotATable(rel));
    }
    Ok(shape.names)
}

/// `name` as a string constant the server reads the same way whatever
/// `standard_conforming_strings` says (dollar quoted); `None` when it holds the quote.
fn constant(name: &str) -> Option<String> {
    const TAG: &str = "$datarig$";
    (!name.contains(TAG)).then(|| format!("{TAG}{name}{TAG}"))
}

/// `ARRAY[…]::pg_catalog.name[]` of `names`.
fn name_array(names: &[String]) -> Option<String> {
    let items: Vec<String> = names.iter().map(|n| constant(n)).collect::<Option<_>>()?;
    Some(format!("ARRAY[{}]::pg_catalog.name[]", items.join(", ")))
}

/// The query the app sends right before it runs a statement again or counts it, on the same
/// session: what only the server can tell for the allowlist (see the module docs), from the
/// objects the statement names. It answers no row when the statement may run, else one row
/// (`kind`, `name`) for [`NotRepeatable::from_check`]. It reads the catalog only, names every
/// object and operator with its schema (a `search_path` of the user's changes nothing in it),
/// and cannot fail but for the transaction's state or a cancel. `None` when a name cannot be
/// written as a constant.
pub fn check_query(n: &Names) -> Option<String> {
    let mut named = Vec::new();
    for (schema, rel) in &n.relations {
        let cte = schema.is_empty() && n.ctes.contains(rel);
        named.push(format!("({}, {}, {cte})", constant(schema)?, constant(rel)?));
    }
    let named = if named.is_empty() {
        "SELECT NULL::pg_catalog.text, NULL::pg_catalog.text, true WHERE false".to_string()
    } else {
        format!("VALUES {}", named.join(", "))
    };
    let (functions, attributes) = (name_array(&n.functions)?, name_array(&n.attributes)?);
    let (operators, types) = (name_array(&n.operators)?, name_array(&n.types)?);
    Some(format!(
        "WITH RECURSIVE named(sch, rel, cte) AS ({named}),
top AS (SELECT n.sch, n.rel, n.cte, pg_catalog.to_regclass(CASE WHEN n.sch OPERATOR(pg_catalog.=) '' \
THEN pg_catalog.quote_ident(n.rel) ELSE pg_catalog.concat(pg_catalog.quote_ident(n.sch), '.', \
pg_catalog.quote_ident(n.rel)) END)::pg_catalog.oid AS oid FROM named n),
rels(oid) AS (SELECT t.oid FROM top t WHERE t.oid IS NOT NULL UNION SELECT i.inhrelid FROM \
pg_catalog.pg_inherits i JOIN rels r ON i.inhparent OPERATOR(pg_catalog.=) r.oid),
usertypes(oid) AS (SELECT DISTINCT y.oid FROM rels r JOIN pg_catalog.pg_attribute a ON a.attrelid \
OPERATOR(pg_catalog.=) r.oid JOIN pg_catalog.pg_type t ON t.oid OPERATOR(pg_catalog.=) a.atttypid, \
LATERAL (VALUES (t.oid), (t.typelem), (t.typbasetype)) y(oid) WHERE a.attnum OPERATOR(pg_catalog.>) 0 \
AND NOT a.attisdropped AND y.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid)
SELECT w.kind, w.name FROM (
SELECT 1, 'relation', CASE WHEN t.sch OPERATOR(pg_catalog.=) '' THEN t.rel ELSE \
pg_catalog.concat(t.sch, '.', t.rel) END FROM top t WHERE t.oid IS NULL AND NOT t.cte
UNION ALL SELECT 2, 'relation', c.oid::pg_catalog.regclass::pg_catalog.text FROM rels r JOIN \
pg_catalog.pg_class c ON c.oid OPERATOR(pg_catalog.=) r.oid WHERE c.relkind OPERATOR(pg_catalog.<>) \
ALL (ARRAY['r', 'p', 'm']::pg_catalog.\"char\"[])
UNION ALL SELECT 3, 'policy', c.oid::pg_catalog.regclass::pg_catalog.text FROM rels r JOIN \
pg_catalog.pg_class c ON c.oid OPERATOR(pg_catalog.=) r.oid WHERE c.relrowsecurity
UNION ALL SELECT 4, 'shadowed', p.proname::pg_catalog.text FROM pg_catalog.pg_proc p WHERE p.oid \
OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid AND (p.proname OPERATOR(pg_catalog.=) ANY ({functions}) \
OR p.pronargs OPERATOR(pg_catalog.=) 1 AND p.proname OPERATOR(pg_catalog.=) ANY ({attributes}))
UNION ALL SELECT 5, 'shadowed', o.oprname::pg_catalog.text FROM pg_catalog.pg_operator o WHERE o.oid \
OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid AND o.oprname OPERATOR(pg_catalog.=) ANY ({operators}) \
AND (o.oprleft OPERATOR(pg_catalog.<) 10000::pg_catalog.oid OR o.oprright OPERATOR(pg_catalog.<) \
10000::pg_catalog.oid)
UNION ALL SELECT 6, 'type', t.typname::pg_catalog.text FROM pg_catalog.pg_type t JOIN \
pg_catalog.pg_namespace s ON s.oid OPERATOR(pg_catalog.=) t.typnamespace WHERE t.oid \
OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid AND t.typname OPERATOR(pg_catalog.=) ANY ({types}) AND \
pg_catalog.array_position(pg_catalog.current_schemas(true), s.nspname) OPERATOR(pg_catalog.<) \
pg_catalog.array_position(pg_catalog.current_schemas(true), 'pg_catalog'::pg_catalog.name)
UNION ALL SELECT 7, 'column_type', pg_catalog.format_type(u.oid, NULL) FROM usertypes u JOIN \
pg_catalog.pg_type t ON t.oid OPERATOR(pg_catalog.=) u.oid WHERE EXISTS (SELECT FROM \
pg_catalog.pg_operator o WHERE o.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid AND (o.oprleft \
OPERATOR(pg_catalog.=) u.oid OR o.oprright OPERATOR(pg_catalog.=) u.oid)) OR EXISTS (SELECT FROM \
pg_catalog.pg_cast k WHERE k.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid AND (k.castsource \
OPERATOR(pg_catalog.=) u.oid OR k.casttarget OPERATOR(pg_catalog.=) u.oid)) OR EXISTS (SELECT FROM \
pg_catalog.pg_proc p WHERE p.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid AND u.oid \
OPERATOR(pg_catalog.=) ANY (p.proargtypes)) OR t.typinput::pg_catalog.oid OPERATOR(pg_catalog.>=) \
10000::pg_catalog.oid OR t.typoutput::pg_catalog.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid \
OR EXISTS (SELECT FROM pg_catalog.pg_range g WHERE g.rngtypid OPERATOR(pg_catalog.=) u.oid AND \
(g.rngcanonical::pg_catalog.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid OR \
g.rngsubdiff::pg_catalog.oid OPERATOR(pg_catalog.>=) 10000::pg_catalog.oid))
) w(o, kind, name) ORDER BY w.o LIMIT 1"
    ))
}

/// Whether the query `sql` sorts its rows at the top (`ORDER BY`): without it the order of a
/// run again is not guaranteed to be the same, which the app says when it re-runs one.
pub fn ordered(sql: &str) -> bool {
    shape(sql).is_some_and(|s| s.order_by)
}

/// `sql` without the `;` (and the blanks and comments around it) that end it.
fn without_semicolons(sql: &str) -> &str {
    let mut end = sql.len();
    loop {
        let body = &sql[..end];
        let last = lex(body).into_iter().rfind(|t| !t.is_trivia());
        match last {
            Some(t) if t.kind == Tok::Semi => end = t.start,
            _ => return body.trim_end(),
        }
    }
}

/// The query that counts the rows of `sql` when the user asks: `SELECT count(*) FROM (<sql>)
/// AS datarig_count`, for a statement on the allowlist ([`repeatable`]). The statement goes
/// on lines of its own, so a trailing `--` comment ends before the closing parenthesis, and a
/// trailing `;` is left out. The text is read again and must itself be one plain query on the
/// allowlist.
pub fn count_query(sql: &str) -> Result<String, NotRepeatable> {
    repeatable(sql)?;
    let text = format!("SELECT count(*) FROM (\n{}\n) AS datarig_count", without_semicolons(sql));
    repeatable(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests;
