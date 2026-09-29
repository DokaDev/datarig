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
