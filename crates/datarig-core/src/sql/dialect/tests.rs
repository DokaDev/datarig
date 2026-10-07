use super::*;

#[test]
fn defaults_are_postgres() {
    assert_eq!(Dialect::default(), Dialect::Postgres);
    assert_eq!(Language::default(), Language::Sql(Dialect::Postgres));
}

#[test]
fn language_names_its_dialect() {
    assert_eq!(Language::Sql(Dialect::Postgres).dialect(), Dialect::Postgres);
    assert_eq!(Language::default().dialect(), Dialect::default());
}

#[test]
fn postgres_quotes_names_and_strings() {
    let d = Dialect::Postgres;
    assert_eq!(d.ident_quote(), '"');
    for name in ["users", "user_id", "Mixed Col", "select", "user", "say \"hi\"", "", "1st", "caf\u{e9}"] {
        assert_eq!(d.quote_ident(name), crate::sql::ident::sql_ident(name), "{name:?}");
        assert_eq!(d.needs_quotes(name), crate::sql::ident::needs_quotes(name), "{name:?}");
        assert_eq!(d.unquote(&d.force_quote_ident(name)).as_deref(), Some(name), "{name:?}");
    }
    assert_eq!(d.quote_ident("users"), "users");
    assert_eq!(d.force_quote_ident("users"), "\"users\"");
    assert_eq!(d.force_quote_ident("Order \"Items\""), "\"Order \"\"Items\"\"\"");
    assert_eq!(d.quote_literal("it's"), "'it''s'");
    assert_eq!(d.quote_literal("back\\slash"), "'back\\slash'", "standard conforming strings");
    assert_eq!(d.fold("Shop_\u{c9}"), "shop_\u{c9}", "ASCII letters only");
}

#[test]
fn unquoting_reads_a_quoted_name_back() {
    let d = Dialect::Postgres;
    assert_eq!(d.unquote("\"Mixed \"\"Q\"\"\"").as_deref(), Some("Mixed \"Q\""));
    assert_eq!(d.unquote("\"\"").as_deref(), Some(""));
    assert_eq!(d.unquote("\"open"), None, "not closed");
    assert_eq!(d.unquote("\""), None);
    assert_eq!(d.unquote("bare"), None);
    // The lenient form takes a name being typed, and drops every quote at either end.
    assert_eq!(d.unquote_lenient("\"open"), "open");
    assert_eq!(d.unquote_lenient("\"a\"\"b\""), "a\"b");
    assert_eq!(d.unquote_lenient("\"a\"\"\""), "a");
    assert_eq!(d.unescape_ident("a\"\"b\"\""), "a\"b\"");
}
