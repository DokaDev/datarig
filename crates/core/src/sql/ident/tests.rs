use super::*;
use pg_query::protobuf::KeywordKind;

/// What PostgreSQL's own scanner (libpg_query) says the bare word `w` is.
fn kind_of(w: &str) -> KeywordKind {
    let r = pg_query::scan(w).unwrap();
    assert_eq!(r.tokens.len(), 1, "{w:?} is one token");
    r.tokens[0].keyword_kind()
}

#[test]
fn the_list_is_sorted_and_matches_the_server() {
    assert!(QUOTED_KEYWORDS.windows(2).all(|w| w[0] < w[1]), "sorted for the binary search");
    for w in QUOTED_KEYWORDS {
        let k = kind_of(w);
        assert!(!matches!(k, KeywordKind::NoKeyword | KeywordKind::UnreservedKeyword), "{w}: {k:?}");
    }
}

/// Every word the highlighter knows, and some common column names that are keywords, are
/// quoted exactly when the server's scanner says they are not unreserved.
#[test]
fn keywords_follow_the_server() {
    let common = ["name", "key", "data", "type", "value", "year", "zone", "action", "comment", "position", "user"];
    let words = crate::sql::lexer::KEYWORDS.iter().map(|k| k.to_lowercase()).chain(common.map(String::from));
    for w in words {
        let k = kind_of(&w);
        let reserved = !matches!(k, KeywordKind::NoKeyword | KeywordKind::UnreservedKeyword);
        assert_eq!(needs_quotes(&w), reserved, "{w}: {k:?}");
    }
}

#[test]
fn quoting_rules() {
    for plain in ["users", "user_id", "_x", "t1", "order_items", "name"] {
        assert_eq!(sql_ident(plain), plain);
    }
    let cases = [
        ("Mixed Col", "\"Mixed Col\""),
        ("MixedCase", "\"MixedCase\""),
        ("ID", "\"ID\""),
        ("1st", "\"1st\""),
        ("a-b", "\"a-b\""),
        ("x$", "\"x$\""),
        ("select", "\"select\""),
        ("user", "\"user\""),
        ("say \"hi\"", "\"say \"\"hi\"\"\""),
        ("", "\"\""),
        ("caf\u{e9}", "\"caf\u{e9}\""),
        ("\u{c774}\u{b984}", "\"\u{c774}\u{b984}\""),
    ];
    for (name, sql) in cases {
        assert_eq!(sql_ident(name), sql, "{name:?}");
    }
}
