use super::*;

fn cat() -> Catalog {
    let col = |n: &str, t: &str| ColumnInfo { name: n.into(), type_name: t.into() };
    let rel = |s: &str, n: &str, v: bool, cols: Vec<ColumnInfo>| Relation {
        schema: s.into(),
        name: n.into(),
        is_view: v,
        columns: cols,
    };
    Catalog {
        schemas: vec!["analytics".into(), "public".into(), "shop".into()],
        relations: vec![
            rel("analytics", "events", false, vec![col("id", "bigint"), col("event_type", "text")]),
            rel("shop", "audit_log", false, vec![col("actor", "text")]),
            rel("shop", "order_items", false, vec![col("order_id", "bigint")]),
            rel("shop", "order_summary", true, vec![col("order_id", "bigint")]),
            rel(
                "shop",
                "orders",
                false,
                vec![
                    col("id", "bigint"),
                    col("user_id", "bigint"),
                    col("status", "text"),
                    col("total_amount", "numeric(14,2)"),
                ],
            ),
            rel("shop", "products", false, vec![col("id", "bigint")]),
            rel("shop", "reviews", false, vec![col("id", "bigint")]),
            rel(
                "shop",
                "users",
                false,
                vec![col("id", "bigint"), col("email", "text"), col("name", "text"), col("nickname", "text")],
            ),
        ],
    }
}

fn run(src_with_bar: &str) -> Vec<(String, Kind)> {
    let cursor = src_with_bar.find('|').unwrap();
    let src = src_with_bar.replace('|', "");
    complete(&src, cursor, &cat(), false)
        .map(|c| c.items.into_iter().map(|i| (i.label, i.kind)).collect())
        .unwrap_or_default()
}

fn labels(v: &[(String, Kind)]) -> Vec<&str> {
    v.iter().map(|(l, _)| l.as_str()).collect()
}

#[test]
fn from_prefix_suggests_schema_and_tables() {
    let r = run("SELECT * FROM sh|");
    assert_eq!(r[0], ("shop".to_string(), Kind::Schema));
    assert!(labels(&r).contains(&"shop.users"));
    assert!(labels(&r).contains(&"shop.order_summary"));
    assert!(!labels(&r).contains(&"analytics.events"));
}

#[test]
fn schema_dot_lists_tables_and_views() {
    let r = run("SELECT * FROM shop.|");
    assert_eq!(labels(&r), vec!["audit_log", "order_items", "order_summary", "orders", "products", "reviews", "users"]);
    assert_eq!(r[2].1, Kind::View);
}

#[test]
fn alias_dot_in_incomplete_sql() {
    let r = run("SELECT * FROM shop.users u WHERE u.|");
    assert_eq!(labels(&r), vec!["id", "email", "name", "nickname"]);
    let c = complete("SELECT * FROM shop.users u WHERE u.", 35, &cat(), false).unwrap();
    assert_eq!(c.items[0].detail.as_deref(), Some("bigint"));
}

#[test]
fn alias_defined_after_cursor() {
    let r = run("SELECT o.| FROM shop.orders o");
    assert_eq!(labels(&r), vec!["id", "user_id", "status", "total_amount"]);
    let r = run("SELECT o.st| FROM shop.orders AS o");
    assert_eq!(labels(&r), vec!["status"]);
}

#[test]
fn join_alias_and_other_statements_ignored() {
    let src = "SELECT 1 FROM analytics.events e;\nSELECT u.| FROM shop.orders o JOIN shop.users u ON u.id = o.user_id";
    let r = run(src);
    assert_eq!(labels(&r), vec!["id", "email", "name", "nickname"]);
}

#[test]
fn general_position_columns_then_keywords() {
    let r = run("SELECT * FROM shop.orders o WHERE st|");
    assert_eq!(r[0], ("status".to_string(), Kind::Column));
    assert!(r.iter().skip(1).all(|(_, k)| *k == Kind::Keyword));
    let r = run("sel|");
    assert_eq!(labels(&r), vec!["select"]);
}

#[test]
fn nothing_inside_strings_or_comments() {
    assert!(run("SELECT 'sh|").is_empty());
    assert!(run("-- FROM sh|").is_empty());
    assert!(run("SELECT 1 |").is_empty());
}

#[test]
fn comma_in_from_list() {
    let r = run("SELECT * FROM shop.users u, an|");
    assert_eq!(r[0], ("analytics".to_string(), Kind::Schema));
}

#[test]
fn max_ten_items() {
    let r = complete("SELECT ", 7, &cat(), true).unwrap();
    assert!(r.items.len() <= MAX_ITEMS);
}

/// A tab's schema is its search path: unqualified names resolve there (columns of
/// `users` are `shop.users`'s), and a table position offers that schema's tables by their bare
/// name first, then everything qualified.
#[test]
fn a_search_path_resolves_unqualified_names() {
    let path = vec!["shop".to_string()];
    let at = |s: &str| {
        let cursor = s.find('|').unwrap();
        let src = s.replace('|', "");
        complete_in(&src, cursor, &cat(), false, &path)
            .map(|c| c.items.into_iter().map(|i| i.label).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let r = at("SELECT * FROM pro|");
    assert_eq!(r.first().map(String::as_str), Some("products"), "{r:?}");
    assert!(r.contains(&"shop.products".to_string()), "qualified too: {r:?}");
    assert_eq!(at("SELECT em| FROM users"), ["email"], "columns of shop.users");
    assert_eq!(at("SELECT u.| FROM users u")[..2], ["id".to_string(), "email".to_string()]);
    // Without a path that reaches `shop`, `users` resolves nowhere on the path: any schema's.
    let default = complete("SELECT * FROM pro|".replace('|', "").as_str(), 17, &cat(), false).unwrap();
    assert_eq!(default.items[0].label, "shop.products", "the default path is public");
}

/// `public` follows the chosen schema. A name resolves in the first schema of the
/// path that has it (`shop` before `public`), a name only `public` has resolves there, and the
/// table position offers both schemas' tables by their bare name.
#[test]
fn public_follows_the_chosen_schema() {
    assert_eq!(schema_path("shop"), ["shop", "public"]);
    assert_eq!(schema_path("public"), ["public"]);
    let mut c = cat();
    let col = |n: &str| ColumnInfo { name: n.into(), type_name: "text".into() };
    let rel = |s: &str, n: &str, cols: Vec<ColumnInfo>| Relation {
        schema: s.into(),
        name: n.into(),
        is_view: false,
        columns: cols,
    };
    c.relations.push(rel("public", "users", vec![col("public_only")]));
    c.relations.push(rel("public", "zz_ext", vec![col("ext_col")]));
    let path = schema_path("shop");
    let at = |s: &str| {
        let cursor = s.find('|').unwrap();
        let src = s.replace('|', "");
        complete_in(&src, cursor, &c, false, &path)
            .map(|c| c.items.into_iter().map(|i| i.label).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    assert_eq!(at("SELECT em| FROM users"), ["email"], "shop.users wins over public.users");
    let r = at("SELECT ext| FROM zz_ext");
    assert_eq!(r.first().map(String::as_str), Some("ext_col"), "a name only public has resolves there: {r:?}");
    let r = at("SELECT * FROM zz_|");
    assert_eq!(r.first().map(String::as_str), Some("zz_ext"), "public's tables by their bare name: {r:?}");
    let r = at("SELECT * FROM use|");
    assert_eq!(r.iter().filter(|l| *l == "users").count(), 1, "one bare name, the first schema's: {r:?}");
}

/// A catalog with names PostgreSQL reads back only when they are quoted.
fn mixed() -> Catalog {
    let mut c = cat();
    let col = |n: &str| ColumnInfo { name: n.into(), type_name: "text".into() };
    c.schemas.push("Sales".into());
    c.relations.push(Relation {
        schema: "Sales".into(),
        name: "Order Lines".into(),
        is_view: false,
        columns: vec![col("Mixed Col"), col("MixedCase"), col("plain"), col("select"), col("say \"hi\"")],
    });
    c.relations.push(Relation {
        schema: "public".into(),
        name: "Accounts".into(),
        is_view: false,
        columns: vec![col("Id"), col("owner")],
    });
    c
}

fn run_in(c: &Catalog, src_with_bar: &str) -> Option<Completion> {
    let cursor = src_with_bar.find('|').unwrap();
    let src = src_with_bar.replacen('|', "", 1);
    complete(&src, cursor, c, false)
}

fn labels_in(c: &Catalog, src_with_bar: &str) -> Vec<String> {
    run_in(c, src_with_bar).map(|c| c.items.into_iter().map(|i| i.label).collect()).unwrap_or_default()
}

/// Accepting a candidate puts SQL in the text: names that are not plain lower-case words (or
/// are reserved keywords) come quoted, with `"` doubled. They match what was typed without
/// quotes, case-insensitively.
#[test]
fn names_that_need_quotes_are_inserted_quoted() {
    let c = mixed();
    let src = "SELECT mi| FROM \"Sales\".\"Order Lines\"";
    assert_eq!(labels_in(&c, src), ["\"Mixed Col\"", "\"MixedCase\""]);
    assert_eq!(labels_in(&c, "SELECT sel| FROM \"Sales\".\"Order Lines\""), ["\"select\"", "select"]);
    assert_eq!(labels_in(&c, "SELECT sa| FROM \"Sales\".\"Order Lines\""), ["\"say \"\"hi\"\"\""]);
    assert_eq!(labels_in(&c, "SELECT pl| FROM \"Sales\".\"Order Lines\""), ["plain"]);
    assert_eq!(labels_in(&c, "SELECT o.| FROM \"Sales\".\"Order Lines\" o")[..2], ["\"Mixed Col\"", "\"MixedCase\""]);
    // Tables and schemas.
    let r = labels_in(&c, "SELECT * FROM acc|");
    assert_eq!(r, ["\"Accounts\"", "public.\"Accounts\""]);
    let r = labels_in(&c, "SELECT * FROM sal|");
    assert_eq!(r[..2], ["\"Sales\"", "\"Sales\".\"Order Lines\""]);
    assert_eq!(labels_in(&c, "SELECT * FROM \"Sales\".|"), ["\"Order Lines\""]);
    assert_eq!(labels_in(&c, "SELECT a.| FROM \"Accounts\" a"), ["\"Id\"", "owner"]);
}

/// After an opening `"` the rest is matched (case-insensitively) and every name is quoted; the
/// accepted name replaces the quote too, and the closing `"` an editor put right after the
/// cursor. Keywords are not offered there. The statement's other tables are still read.
#[test]
fn an_opening_quote_completes_quoted_names() {
    let c = mixed();
    let src = "SELECT \"Mi| FROM \"Sales\".\"Order Lines\"";
    let done = run_in(&c, src).expect("completes after a quote");
    let names: Vec<_> = done.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(names, ["\"Mixed Col\"", "\"MixedCase\""]);
    assert_eq!((done.replace_start, done.trail), (7, 0));
    // With the closing quote already there.
    let done = run_in(&c, "SELECT \"pl|\" FROM \"Sales\".\"Order Lines\"").expect("before a closing quote");
    assert_eq!(done.items[0].label, "\"plain\"");
    assert_eq!((done.replace_start, done.trail), (7, 1));
    // An empty quote offers every column; no keywords.
    let r = labels_in(&c, "SELECT \"| FROM \"Accounts\"");
    assert_eq!(r, ["\"Id\"", "\"owner\""]);
    let r = labels_in(&c, "SELECT * FROM \"acc|");
    assert_eq!(r, ["\"Accounts\"", "\"public\".\"Accounts\""]);
    // Inside a finished quoted name (not at its end) there is nothing, as before.
    assert!(labels_in(&c, "SELECT \"Mi|xed\" FROM \"Accounts\"").is_empty());
    assert!(labels_in(&c, "SELECT \"Id\"| FROM \"Accounts\"").is_empty());
}

/// `WITH` queries are tables of the statement: offered first in a table position (hiding a
/// catalog table of the same name), with the columns of their written list or of their select
/// list.
#[test]
fn with_queries_are_tables_with_their_columns() {
    let src = "WITH recent(oid, who) AS (SELECT id, user_id FROM shop.orders),\n\
               totals AS MATERIALIZED (SELECT o.user_id, sum(o.total_amount) AS spent, count(*) n, \
               o.status::text, 1 + 2, * FROM shop.orders o GROUP BY 1),\n\
               users AS (SELECT DISTINCT \"Who Am I\" FROM shop.users)\n";
    let at = |tail: &str| {
        let s = format!("{src}{tail}");
        let cursor = s.find('|').unwrap();
        complete(&s.replace('|', ""), cursor, &cat(), true)
            .map(|c| c.items.into_iter().map(|i| (i.label, i.kind)).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let r = at("SELECT * FROM |");
    assert_eq!(labels(&r)[..3], ["recent", "totals", "users"], "{r:?}");
    assert_eq!(r[0].1, Kind::Table);
    assert_eq!(labels(&r).iter().filter(|l| **l == "users").count(), 1, "the WITH query hides shop.users: {r:?}");
    let r = at("SELECT * FROM t|");
    assert_eq!(labels(&r)[0], "totals");
    assert_eq!(labels(&at("SELECT r.| FROM recent r")), ["oid", "who"]);
    assert_eq!(labels(&at("SELECT totals.| FROM totals")), ["user_id", "spent", "n"]);
    assert_eq!(labels(&at("SELECT sp| FROM totals")), ["spent"]);
    assert_eq!(labels(&at("SELECT u.| FROM users u")), ["\"Who Am I\""]);
    // Columns of a WITH query have no type.
    let s = format!("{src}SELECT sp FROM totals");
    let c = complete(&s, s.len() - " FROM totals".len(), &cat(), false).unwrap();
    assert_eq!(c.items[0].detail, None);
    // `WITH RECURSIVE`, and a WITH query still being written.
    let r = run("WITH RECURSIVE t(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM t) SELECT t.| FROM t");
    assert_eq!(labels(&r), ["n"]);
    let r = run("WITH a AS (SELECT id, email FROM shop.users) SELECT a.|");
    assert_eq!(labels(&r), ["id", "email"]);
    let r = run("WITH a AS (SELECT id FROM shop.users), b AS (SELECT a.| FROM a");
    assert_eq!(labels(&r), ["id"], "a later WITH query still being written reads an earlier one");
    // Not WITH queries: `WITH TIME ZONE`, `WITH (options)`.
    let r = run("CREATE TABLE zz (t timestamp WITH TIME ZONE) WITH (fillfactor = 70); SELECT * FROM ti|");
    assert!(!labels(&r).contains(&"time"), "{r:?}");
}

/// Names that start with what was typed come first; then names the typed letters appear in,
/// in order (`oi` → `order_items`). Keywords match by their start only.
#[test]
fn prefix_matches_first_then_subsequences() {
    let r = run("SELECT * FROM shop.oi|");
    assert_eq!(labels(&r), ["order_items"]);
    let r = run("SELECT * FROM shop.us|");
    assert_eq!(labels(&r), ["users", "products"], "prefix first, then a subsequence");
    let r = run("SELECT * FROM shop.users WHERE nm|");
    assert_eq!(labels(&r), ["name", "nickname"], "no keyword by its letters (NUMERIC)");
    let r = run("SELECT * FROM shop.orders WHERE ta|");
    assert_eq!(labels(&r), ["table", "status", "total_amount"], "a keyword's prefix match comes first");
}

/// A `WITH` query's bare names are folded to lower case, as the server reads them: a name
/// written `Recent` is the relation `recent`, and is offered (and inserted) as such.
#[test]
fn with_query_names_fold_to_lower_case() {
    let r = run("WITH Recent AS (SELECT id FROM shop.users) SELECT * FROM Rec|");
    assert_eq!(labels(&r)[0], "recent");
    let r = run("with r as (select Amount, t.Total, x AS Big, \"Kept\" from t) select r.|");
    assert_eq!(labels(&r), ["amount", "total", "big", "\"Kept\""]);
    let r = run("with r(A, \"B\") as (select 1, 2) select r.|");
    assert_eq!(labels(&r), ["a", "\"B\""]);
}

/// Only a select-list item that really ends in its name gives a column: not an expression
/// ending in a keyword's operand (`a and b`, `not c`) or a collation.
#[test]
fn with_query_columns_are_only_names_written_as_such() {
    let r = run("with r as (select a and b, not c, x collate \"C\", count(*) n, 1 one, y z from t) select r.|");
    assert_eq!(labels(&r), ["n", "one", "z"]);
}
