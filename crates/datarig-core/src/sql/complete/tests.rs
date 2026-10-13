use super::*;

fn cat() -> Catalog {
    let col = |n: &str, t: &str| ColumnInfo::new(n, t);
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
    let col = |n: &str| ColumnInfo::new(n, "text");
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
    let col = |n: &str| ColumnInfo::new(n, "text");
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

/// Completion in PostgreSQL is the one [`complete_in`] does, with the same search path.
#[test]
fn complete_in_postgres_is_complete_in() {
    let texts = [
        "SELECT * FROM |",
        "SELECT * FROM shop.|",
        "SELECT o.| FROM shop.orders o",
        "SELECT \"Mi| FROM t",
        "WITH w AS (SELECT 1 AS n) SELECT | FROM w",
        "sel|",
        "SELECT 'x|'",
    ];
    for c in [cat(), mixed()] {
        for path in [default_path(), schema_path("shop")] {
            for src in texts {
                let cursor = src.find('|').unwrap();
                let text = src.replacen('|', "", 1);
                for force in [false, true] {
                    let a = complete_in_dialect(&text, cursor, &c, force, &path, Dialect::Postgres);
                    let b = complete_in(&text, cursor, &c, force, &path);
                    let key = |c: Option<Completion>| c.map(|c| (c.items, c.replace_start, c.trail));
                    assert_eq!(key(a), key(b), "{src:?} {path:?}");
                }
            }
        }
    }
}

/// A string whose last quote is one half of a doubled quote, or escaped (`E'…\'`), is not
/// closed: the cursor at its end is inside it, where nothing is offered, not even on demand.
#[test]
fn an_unclosed_string_ending_in_a_quote_is_inside() {
    for src in ["SELECT * FROM shop.orders WHERE 'it''", "SELECT * FROM shop.orders WHERE E'it\\'"] {
        assert!(complete(src, src.len(), &cat(), true).is_none(), "{src}");
    }
    // A closed one is left: after it the statement goes on.
    let src = "SELECT * FROM shop.orders WHERE 'it''s'";
    assert!(complete(src, src.len(), &cat(), true).is_some(), "{src}");
}

// ── values inside string literals ───────────────────────────────────────────

/// [`cat`] with enumerated columns: `shop.orders.status` (`order_status`), and `shop.notes.kind`
/// whose values hold a quote, a backslash and a double quote.
fn enums() -> Catalog {
    let mut c = cat();
    let values = |v: &[&str]| Some(v.iter().map(|s| s.to_string()).collect::<Vec<_>>().into());
    let orders = c.relations.iter_mut().find(|r| r.name == "orders").unwrap();
    let status = orders.columns.iter_mut().find(|c| c.name == "status").unwrap();
    status.type_name = "order_status".into();
    status.values = values(&["pending", "paid", "shipped", "cancelled"]);
    let mut kind = ColumnInfo::new("kind", "note_kind");
    kind.values = values(&["it's", "back\\slash", "say \"hi\"", "plain"]);
    c.relations.push(Relation {
        schema: "shop".into(),
        name: "notes".into(),
        is_view: false,
        columns: vec![ColumnInfo::new("id", "bigint"), kind, ColumnInfo::new("body", "text")],
    });
    c
}

/// Completion at `|` in dialect `d` with the checks of `shop.orders` given (`None`: unknown).
fn values_in(d: Dialect, src_with_bar: &str, checks: Option<&[CheckValues]>) -> Outcome {
    let cursor = src_with_bar.find('|').unwrap();
    let src = src_with_bar.replacen('|', "", 1);
    let lookup = |s: &str, t: &str| if (s, t) == ("shop", "orders") { checks } else { Some(&[][..]) };
    complete_with(&src, cursor, &enums(), false, &default_path(), d, LexState::default(), &lookup)
}

fn value_labels(d: Dialect, src_with_bar: &str) -> Vec<String> {
    let o = values_in(d, src_with_bar, Some(&[]));
    o.completion.map(|c| c.items.into_iter().map(|i| i.label).collect()).unwrap_or_default()
}

fn pg_values(src_with_bar: &str) -> Vec<String> {
    value_labels(Dialect::Postgres, src_with_bar)
}

const ALL: [&str; 4] = ["pending", "paid", "shipped", "cancelled"];

/// In a literal compared with an enumerated column its values are offered, in the type's
/// order, whatever the comparison: `=`, `<>`, `!=`, an `IN` / `NOT IN` list (also after other
/// items), `UPDATE … SET`.
#[test]
fn values_of_the_column_a_literal_is_compared_with() {
    for src in [
        "SELECT * FROM shop.orders WHERE status = '|'",
        "SELECT * FROM shop.orders WHERE status = '|",
        "SELECT * FROM shop.orders WHERE status <> '|'",
        "SELECT * FROM shop.orders WHERE status != '|'",
        "SELECT * FROM shop.orders WHERE status IN ('|')",
        "SELECT * FROM shop.orders WHERE status IN ('paid', 3, NULL, '|')",
        "SELECT * FROM shop.orders WHERE status NOT IN ('paid', '|",
        "UPDATE shop.orders SET status = '|' WHERE id = 1",
        "SELECT * FROM shop.orders o WHERE o.status = '|'",
        "SELECT * FROM shop.orders WHERE shop.orders.status = '|'",
        "SELECT * FROM shop.orders WHERE orders.status = '|'",
        "SELECT * FROM shop.orders WHERE (status) = '|'",
        "SELECT * FROM shop.users u JOIN shop.orders o ON o.user_id = u.id WHERE status = '|'",
        "SELECT * FROM shop.orders WHERE status /* c */ = -- d\n '|'",
        "SELECT *\n  FROM shop.orders\n WHERE status =\n   '|'",
        "SELECT * FROM shop.orders WHERE STATUS = '|'",
    ] {
        assert_eq!(pg_values(src), ALL, "{src:?}");
    }
    // The alias defined after the literal; another statement before it.
    assert_eq!(pg_values("SELECT 1; SELECT o.id FROM shop.orders AS o WHERE o.status = 'pa|'"), ["paid"]);
    let o = values_in(Dialect::Postgres, "SELECT * FROM shop.orders WHERE status = '|'", Some(&[]));
    let c = o.completion.unwrap();
    assert_eq!((c.items[0].kind, c.items[0].detail.as_deref()), (Kind::Value, Some("order_status")));
}

/// No values: not compared with a column, a column without values or unknown, a `WITH`
/// query's column, the cursor outside the literal's value, strings that hold no plain value.
#[test]
fn no_values_elsewhere() {
    for src in [
        "SELECT '|'",
        "SELECT 'sh|",
        "SELECT * FROM shop.orders WHERE status = 'paid'|",
        "SELECT * FROM shop.orders WHERE status = |'paid'",
        "SELECT * FROM shop.orders WHERE status = E|'paid'",
        "SELECT * FROM shop.orders WHERE status LIKE '|'",
        "SELECT * FROM shop.orders WHERE status > '|'",
        "SELECT * FROM shop.orders WHERE nope = '|'",
        "SELECT * FROM shop.orders WHERE x.status = '|'",
        "SELECT * FROM shop.orders WHERE status = B'|'",
        "SELECT * FROM shop.orders WHERE status = X'|'",
        "SELECT * FROM shop.orders WHERE status = U&'|'",
        "SELECT * FROM shop.orders WHERE status = $$|$$",
        "SELECT * FROM shop.orders WHERE status IN (lower('a'), '|')",
        "SELECT * FROM shop.orders WHERE status = 'a' || '|'",
        "WITH w AS (SELECT status FROM shop.orders) SELECT * FROM w WHERE w.status = '|'",
        "SELECT * FROM shop.orders WHERE status = -- '|\n 1",
    ] {
        assert_eq!(pg_values(src), Vec::<String>::new(), "{src:?}");
        assert_eq!(values_in(Dialect::Postgres, src, None).wants_checks, None, "{src:?}");
    }
    // A column without values of its type: none, its checks asked for.
    let src = "SELECT * FROM shop.orders WHERE id = '|'";
    assert_eq!(pg_values(src), Vec::<String>::new());
    assert!(values_in(Dialect::Postgres, src, None).wants_checks.is_some());
}

/// What the literal holds before the cursor filters them, as names are: a prefix first, then
/// letters in order. Accepting one replaces the literal's value, the rest after the cursor too,
/// never the closing quote; an open literal gets its closing quote with it.
#[test]
fn values_are_filtered_and_replace_the_literal() {
    assert_eq!(pg_values("SELECT * FROM shop.orders WHERE status = 'p|'"), ["pending", "paid", "shipped"]);
    assert_eq!(pg_values("SELECT * FROM shop.orders WHERE status = 'PA|'"), ["paid"]);
    assert_eq!(pg_values("SELECT * FROM shop.orders WHERE status = 'pd|'"), ["pending", "paid", "shipped"]);
    assert_eq!(pg_values("SELECT * FROM shop.orders WHERE status = 'sd|'"), ["shipped"]);
    assert_eq!(pg_values("SELECT * FROM shop.orders WHERE status = 'zz|'"), Vec::<String>::new());
    let at = |src: &str| {
        let c = values_in(Dialect::Postgres, src, Some(&[])).completion.expect(src);
        (c.replace_start, c.trail, c.items[0].text().to_string())
    };
    let closed = "SELECT * FROM shop.orders WHERE status = 'pa|id'";
    let start = closed.find('\'').unwrap() + 1;
    assert_eq!(at(closed), (start, 2, "paid".to_string()), "the rest of the value goes, the quote stays");
    assert_eq!(at("SELECT * FROM shop.orders WHERE status = 'pa|"), (start, 0, "paid'".to_string()));
    // A quote typed before other text that pairs with a quote on another line, or right before
    // a name: open at the cursor.
    let far = "SELECT * FROM shop.orders WHERE status = 'pa|\n  AND note = 'x'";
    assert_eq!(at(far), (start, 0, "paid'".to_string()));
    let glued = "SELECT * FROM shop.orders WHERE status = 'pa| AND user_id = 'x'";
    assert_eq!(at(glued), (start, 0, "paid'".to_string()));
    // Across segments: offsets are the whole text's.
    let two = "SELECT 1;\nSELECT * FROM shop.orders WHERE status = '|'";
    assert_eq!(at(two).0, two.find("'|").unwrap() + 1);
}

/// A table's checks add their values after the type's, once each; unknown checks are asked
/// for, with where the literal's value starts.
#[test]
fn check_values_follow_the_type_values() {
    let checks = [CheckValues { column: "status".into(), values: vec!["paid".into(), "refunded".into()] }];
    let o = values_in(Dialect::Postgres, "SELECT * FROM shop.orders WHERE status = '|'", Some(&checks));
    let c = o.completion.unwrap();
    let got: Vec<_> = c.items.iter().map(|i| (i.label.as_str(), i.detail.as_deref())).collect();
    assert_eq!(
        got,
        [
            ("pending", Some("order_status")),
            ("paid", Some("order_status")),
            ("shipped", Some("order_status")),
            ("cancelled", Some("order_status")),
            ("refunded", Some("CHECK"))
        ]
    );
    assert_eq!(o.wants_checks, None);
    // A column with check values only.
    let checks = [CheckValues { column: "user_id".into(), values: vec!["7".into()] }];
    let o = values_in(Dialect::Postgres, "SELECT * FROM shop.orders WHERE user_id = '|'", Some(&checks));
    assert_eq!(o.completion.unwrap().items[0].label, "7");
    // Unknown checks: asked for; the type's values are offered meanwhile.
    let src = "SELECT 1;\nSELECT * FROM shop.orders o WHERE o.status = 'p|'";
    let o = values_in(Dialect::Postgres, src, None);
    let literal = src.find("'p").unwrap() + 1;
    assert_eq!(o.wants_checks, Some(WantsChecks { schema: "shop".into(), table: "orders".into(), literal }));
    assert_eq!(o.completion.unwrap().items.len(), 3);
    let o = values_in(Dialect::Postgres, "SELECT * FROM shop.orders WHERE user_id = '|'", None);
    assert!(o.completion.is_none() && o.wants_checks.is_some(), "nothing known yet, asked");
    // `complete_from` knows no checks: the type's values only.
    let src = "SELECT * FROM shop.orders WHERE status = '";
    let c = complete_from(src, src.len(), &enums(), false, &default_path(), Dialect::Postgres, LexState::default());
    assert_eq!(c.unwrap().items.len(), 4);
}

/// A value is written as its literal reads it: PostgreSQL `'…'` doubles a quote only, `E'…'`
/// a backslash too; MySQL doubles a backslash unless `NO_BACKSLASH_ESCAPES`, and in a `"…"`
/// string doubles `"`. What is typed is read back the same way to filter.
#[test]
fn values_are_escaped_for_their_literal() {
    use crate::sql::dialect::MySqlMode;
    let my = |nbe: bool, ansi: bool| {
        Dialect::MySql(MySqlMode { ansi_quotes: ansi, no_backslash_escapes: nbe, dollar_quotes: false })
    };
    let inserts = |d: Dialect, src: &str| -> Vec<String> {
        values_in(d, src, Some(&[]))
            .completion
            .map(|c| c.items.iter().map(|i| i.text().to_string()).collect())
            .unwrap_or_default()
    };
    let q = "SELECT * FROM shop.notes WHERE kind = ";
    let pg = Dialect::Postgres;
    assert_eq!(inserts(pg, &format!("{q}'|'")), ["it''s", "back\\slash", "say \"hi\"", "plain"]);
    assert_eq!(inserts(pg, &format!("{q}E'|'")), ["it''s", "back\\\\slash", "say \"hi\"", "plain"]);
    assert_eq!(inserts(pg, &format!("{q}e'|")), ["it''s'", "back\\\\slash'", "say \"hi\"'", "plain'"]);
    assert_eq!(inserts(pg, &format!("{q}N'|'")), ["it''s", "back\\slash", "say \"hi\"", "plain"]);
    assert_eq!(inserts(my(false, false), &format!("{q}'|'")), ["it''s", "back\\\\slash", "say \"hi\"", "plain"]);
    assert_eq!(inserts(my(true, false), &format!("{q}'|'")), ["it''s", "back\\slash", "say \"hi\"", "plain"]);
    assert_eq!(inserts(my(false, false), &format!("{q}\"|\"")), ["it's", "back\\\\slash", "say \"\"hi\"\"", "plain"]);
    assert_eq!(
        inserts(my(true, false), &format!("{q}\"|")),
        ["it's\"", "back\\slash\"", "say \"\"hi\"\"\"", "plain\""]
    );
    // Under ANSI_QUOTES `"…"` is a name: names are offered there, not values.
    let names = values_in(my(false, true), &format!("{q}\"|\""), Some(&[])).completion.unwrap();
    assert!(names.items.iter().all(|i| i.kind == Kind::Column), "{:?}", names.items);
    // Typed escapes are read back: a doubled quote, a backslash escape.
    assert_eq!(value_labels(pg, &format!("{q}'it''|'")), ["it's"]);
    assert_eq!(value_labels(pg, &format!("{q}E'it\\'|'")), ["it's"]);
    assert_eq!(value_labels(pg, &format!("{q}E'back\\\\|'")), ["back\\slash"]);
    assert_eq!(value_labels(pg, &format!("{q}'back\\|'")), ["back\\slash"], "a backslash is a character");
    assert_eq!(value_labels(my(false, false), &format!("{q}'it\\'|'")), ["it's"]);
    assert_eq!(value_labels(my(false, false), &format!("{q}'back\\\\|'")), ["back\\slash"]);
    assert_eq!(value_labels(my(true, false), &format!("{q}'back\\|'")), ["back\\slash"]);
    // MySQL: a charset introducer in the list; a string glued to one.
    assert_eq!(value_labels(my(false, false), &format!("{q}_utf8mb4'pl|'")), ["plain"]);
    let list = "SELECT * FROM shop.notes WHERE kind IN (_utf8mb4'plain', '|')";
    assert_eq!(value_labels(my(false, false), list).len(), 4);
    // MySQL: `$$` is a string when the server reads dollar quotes, but no value's.
    let dollar = Dialect::MySql(MySqlMode { dollar_quotes: true, ..MySqlMode::default() });
    assert_eq!(value_labels(dollar, &format!("{q}$$|$$")), Vec::<String>::new());
    assert_eq!(value_labels(my(false, false), &format!("{q}X'|'")), Vec::<String>::new());
}
