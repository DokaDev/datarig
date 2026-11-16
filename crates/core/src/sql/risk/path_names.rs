//! The names in a PostgreSQL statement that the session's `search_path` decides.
//!
//! PostgreSQL looks up an operator, a function, a type or a collation written without a schema
//! in every schema of the path and takes the best match of the argument types among all of
//! them: listing `pg_catalog` first only settles a tie of identical signatures. A statement the
//! app writes itself names each of them with its schema, so that what it does never depends on
//! the path of the session it runs in. This reads PostgreSQL's own parse tree of a statement and
//! lists what is still left to the path; the app's statement constants are checked against it.

use serde_json::Value;

use super::{parse, text, walk};

/// Every operator, function, type and collation of `sql` whose resolution depends on the
/// search path, as `<kind> <name>` (`operator =`, `function count`, `type text`,
/// `collation C`), in the order of the parse tree; `None` when the parser rejects `sql`.
///
/// Forms that use an operator with no way to name its schema are listed too: `IN (…)` and
/// `NOT IN (…)`, `BETWEEN`, `IS [NOT] DISTINCT FROM`, `NULLIF`, `LIKE`/`ILIKE`/`SIMILAR TO` as
/// keywords, `= ANY (subquery)` written as `IN (subquery)`, a `CASE x WHEN v` (an `=` of its
/// own), `ORDER BY … USING` and the column comparisons of `JOIN … USING` and `NATURAL JOIN`. A
/// function called in attribute notation (`t.f`, read as a column first) is not seen. Relations are not listed: a relation is the first of its name on the path, so a schema
/// listed after `pg_catalog` cannot take its place.
pub fn path_names(sql: &str) -> Option<Vec<String>> {
    let mut found = Vec::new();
    for stmt in parse(sql)? {
        let mut visit = |o: &serde_json::Map<String, Value>| visit(o, &mut found);
        if let Value::Object(o) = &stmt {
            visit(o);
        }
        walk(&stmt, &mut visit);
    }
    Some(found)
}

fn visit(o: &serde_json::Map<String, Value>, found: &mut Vec<String>) {
    // A cast's or a column's type is a field of its own (`type_name`), not a node.
    if let Some(t) = o.get("type_name") {
        one_part("type", &t["names"], found);
    }
    let Some((kind, body)) = o.iter().next().filter(|_| o.len() == 1) else { return };
    let (label, names) = match kind.as_str() {
        "AExpr" => ("operator", &body["name"]),
        // `x IN (SELECT …)` is an `= ANY` whose `=` the parser looks up by name.
        "SubLink" if is_any(&body["sub_link_type"]) && body["oper_name"].as_array().is_none_or(Vec::is_empty) => {
            found.push("operator = (IN subquery)".to_string());
            return;
        }
        "SubLink" => ("operator", &body["oper_name"]),
        "FuncCall" => ("function", &body["funcname"]),
        "TypeName" => ("type", &body["names"]),
        "CollateClause" => ("collation", &body["collname"]),
        // `ORDER BY x USING <`.
        "SortBy" => ("operator", &body["use_op"]),
        // `JOIN … USING (c)` and `NATURAL JOIN` compare the columns with an `=` found by name.
        "JoinExpr"
            if body["is_natural"].as_bool() == Some(true)
                || body["using_clause"].as_array().is_some_and(|u| !u.is_empty()) =>
        {
            found.push("operator = (JOIN USING)".to_string());
            return;
        }
        "CaseExpr" if !body["arg"].is_null() => {
            found.push("operator = (CASE x WHEN)".to_string());
            return;
        }
        _ => return,
    };
    one_part(label, names, found);
}

/// Whether a `SubLink`'s type is `ANY_SUBLINK` (as the parse tree's number or name).
fn is_any(kind: &Value) -> bool {
    kind.as_i64() == Some(pg_query::protobuf::SubLinkType::AnySublink as i64) || kind.as_str() == Some("AnySublink")
}

/// Lists `names` when it is a name of one part (the path decides its schema).
fn one_part(label: &str, names: &Value, found: &mut Vec<String>) {
    let parts: Vec<&str> = names.as_array().into_iter().flatten().filter_map(text).collect();
    if parts.len() == 1 {
        found.push(format!("{label} {}", parts[0]));
    }
}

#[cfg(test)]
mod tests;
