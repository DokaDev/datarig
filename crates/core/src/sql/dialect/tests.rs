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
    // A backslash reads the same whatever standard_conforming_strings is only in `E'…'`.
    assert_eq!(d.quote_literal("a\\b"), "E'a\\\\b'");
    assert_eq!(d.quote_literal("it's \\"), "E'it''s \\\\'");
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
    // A kept quirk: every trailing quote is dropped, the escaped one too (`a"` would be right).
    assert_eq!(d.unquote_lenient("\"a\"\"\""), "a");
    // Not checked: the last quote of `"a""` is escaped, yet it reads as closed.
    assert_eq!(d.unquote("\"a\"\"").as_deref(), Some("a\""));
    assert_eq!(d.unescape_ident('"', "a\"\"b\"\""), "a\"b\"");
    assert_eq!(d.ident_quotes(), &['"']);
    assert_eq!((d.opening_quote("\"x\""), d.opening_quote("x"), d.opening_quote("")), (Some('"'), None, None));
}

#[test]
fn postgres_tools() {
    let d = Dialect::Postgres;
    assert_eq!(d.default_path(None), ["public"]);
    assert_eq!(d.default_path(Some("public")), ["public"]);
    assert_eq!(d.default_path(Some("shop")), ["shop", "public"]);
    assert_eq!(d.comment_marker(), "--");
    assert_eq!(d.line_comment_at("-- x"), Some("--"));
    assert_eq!(d.line_comment_at("--x"), Some("--"));
    assert_eq!(d.line_comment_at("# x"), None);
    assert!(matches!(d.sqlformat_dialect(), sqlformat::Dialect::PostgreSql));
}

/// How a string literal opens: its prefix, its quote and whether a backslash escapes, in each
/// dialect; strings that hold no plain value open none.
#[test]
fn literals_open_by_dialect() {
    let pg = Dialect::Postgres;
    let open = |prefix, quote, backslash| Some(LiteralOpen { prefix, quote, backslash });
    assert_eq!(pg.literal_open("", "'a'"), open(0, '\'', false));
    assert_eq!(pg.literal_open("", "E'a'"), open(1, '\'', true));
    assert_eq!(pg.literal_open("", "e'a"), open(1, '\'', true));
    assert_eq!(pg.literal_open("", "N'a'"), open(1, '\'', false));
    for token in ["B'01'", "b'01'", "X'ff'", "x'ff'", "$$a$$", "\"a\"", "a", ""] {
        assert_eq!(pg.literal_open("", token), None, "{token}");
    }
    assert_eq!(pg.literal_open("x = U&", "'a'"), None, "a Unicode-escape string");
    assert_eq!(pg.literal_open("x = u&", "'a'"), None);
    assert_eq!(pg.literal_open("x = menu&", "'a'"), open(0, '\'', false), "`menu & 'a'` is an operator");
    assert_eq!(pg.literal_open("x = U& ", "'a'"), open(0, '\'', false));
    assert_eq!(pg.string_quotes(), &['\'']);
    let my = |ansi_quotes, no_backslash_escapes| {
        Dialect::MySql(MySqlMode {
            ansi_quotes,
            no_backslash_escapes,
            dollar_quotes: DollarQuotes::Read,
            ignore_space: false,
        })
    };
    assert_eq!(my(false, false).literal_open("", "'a'"), open(0, '\'', true));
    assert_eq!(my(false, false).literal_open("", "\"a\""), open(0, '"', true));
    assert_eq!(my(false, true).literal_open("", "\"a\""), open(0, '"', false));
    assert_eq!(my(false, true).literal_open("", "N'a'"), open(1, '\'', false));
    assert_eq!(my(true, false).literal_open("", "\"a\""), None, "a name under ANSI_QUOTES");
    for token in ["X'ff'", "B'01'", "E'a'", "$$a$$", "$t$a$t$", "`a`"] {
        assert_eq!(my(false, false).literal_open("", token), None, "{token}");
    }
    assert_eq!(my(false, false).literal_open("x = U&", "'a'"), open(0, '\'', true), "PostgreSQL's only");
    assert_eq!(my(false, false).string_quotes(), &['\'', '"']);
    assert_eq!(my(true, false).string_quotes(), &['\'']);
}

/// A value written into a literal ([`Dialect::literal_body`]) reads back as itself
/// ([`Dialect::unescape_literal_body`], [`Dialect::unquote_literal`]) in every dialect, mode
/// and opening; `quote_literal` is the plain literal's (PostgreSQL's `E'…'` one for a backslash).
#[test]
fn literal_bodies_round_trip() {
    let values = [
        "",
        "plain",
        "it's",
        "''",
        "back\\slash",
        "\\",
        "\\'",
        "a\\nb",
        "say \"hi\"",
        "50%_off\\%",
        "NUL\0x",
        "line\nbreak",
        "tab\there",
        "\u{d55c}\u{ae00} \u{1f418}",
        "$$",
        "E'x'",
    ];
    let mut dialects = vec![Dialect::Postgres];
    for bits in 0..8u8 {
        let m = MySqlMode {
            ansi_quotes: bits & 1 != 0,
            no_backslash_escapes: bits & 2 != 0,
            dollar_quotes: if bits & 4 != 0 { DollarQuotes::Read } else { DollarQuotes::NotRead },
            ignore_space: false,
        };
        dialects.push(Dialect::MySql(m));
    }
    for d in dialects {
        for prefix in ["", "E", "N"] {
            for quote in ['\'', '"'] {
                let opener = format!("{prefix}{quote}");
                let Some(open) = d.literal_open("", &opener) else { continue };
                for v in values {
                    // PostgreSQL text holds no NUL.
                    if d == Dialect::Postgres && v.contains('\0') {
                        continue;
                    }
                    let body = d.literal_body(v, open);
                    assert_eq!(d.unescape_literal_body(&body, open), v, "{d:?} {opener} {v:?}");
                    let token = format!("{opener}{body}{quote}");
                    assert_eq!(d.unquote_literal(&token).as_deref(), Some(v), "{d:?} {token:?}");
                    // The lexer reads it as one string, ending where it ends.
                    let lexed = crate::sql::lexer::lex_in(&format!("{token} x"), d);
                    assert_eq!(
                        (lexed[0].kind, lexed[0].end),
                        (crate::sql::lexer::Tok::Str, token.len()),
                        "{d:?} {token:?}"
                    );
                    // PostgreSQL writes a value with a backslash as `E'…'`.
                    let e = d == Dialect::Postgres && v.contains('\\');
                    if quote == '\'' && prefix == if e { "E" } else { "" } {
                        assert_eq!(d.quote_literal(v), token, "{d:?} {v:?}");
                    }
                }
            }
        }
    }
}

/// Reading what is between the quotes: escapes of each dialect, and text cut short while it is
/// typed.
#[test]
fn literal_bodies_unescape() {
    let e = LiteralOpen { prefix: 1, quote: '\'', backslash: true };
    let pg = Dialect::Postgres;
    assert_eq!(pg.unescape_literal_body("a\\tb\\n\\\\\\'", e), "a\tb\n\\'");
    assert_eq!(pg.unescape_literal_body("\\101\\x41\\u0041\\U00000041\\q", e), "AAAAq");
    assert_eq!(pg.unescape_literal_body("\\x", e), "x");
    assert_eq!(pg.unescape_literal_body("cut\\", e), "cut\\", "an escape cut short stays");
    assert_eq!(pg.unescape_literal_body("it''s", LiteralOpen { backslash: false, ..e }), "it's");
    assert_eq!(pg.unescape_literal_body("a\\tb", LiteralOpen { backslash: false, ..e }), "a\\tb");
    let my = Dialect::MySql(MySqlMode::default());
    let plain = LiteralOpen { prefix: 0, quote: '\'', backslash: true };
    assert_eq!(my.unescape_literal_body("\\0\\Z\\%\\_\\q\\\"", plain), "\0\x1a\\%\\_q\"");
    assert_eq!(my.unescape_literal_body("it\\'s it''s", plain), "it's it's");
    assert_eq!(pg.unquote_literal("'open"), None);
    assert_eq!(pg.unquote_literal("'it''"), None, "the last quote is one half of a doubled one");
    assert_eq!(pg.unquote_literal("E'a\\'"), None, "the last quote is escaped");
    assert_eq!(pg.unquote_literal("'a'"), Some("a".into()));
}

/// Every sql mode.
fn every_mode() -> Vec<MySqlMode> {
    (0..16u8)
        .map(|b| MySqlMode {
            ansi_quotes: b & 1 != 0,
            no_backslash_escapes: b & 2 != 0,
            dollar_quotes: if b & 4 != 0 { DollarQuotes::Read } else { DollarQuotes::NotRead },
            ignore_space: b & 8 != 0,
        })
        .collect()
}

/// The MariaDB dialect reads and writes text as the MySQL one of the same mode, in every
/// method; the methods that give a dialect back keep the engine.
#[test]
fn every_method_answers_mariadb_as_the_mysql_dialect() {
    let names = ["users", "Mixed Col", "select", "user", "a`b", "say \"hi\"", "", "1st", "caf\u{e9}", "`", "\""];
    let values = ["", "it's", "a\\b", "a\0b", "say \"hi\"", "\\%_", "line\nbreak", "caf\u{e9}"];
    let tokens = [
        "'a'",
        "'it''s'",
        "'a\\'b'",
        "'a\\nb\\Z\\%'",
        "\"a\"",
        "\"a\"\"b\"",
        "N'a'",
        "n'a'",
        "E'a\\nb'",
        "X'41'",
        "B'01'",
        "_utf8mb4'x'",
        "$$a$$",
        "'open",
        "`a`",
        "`a``b`",
        "\"a\"",
        "`open",
        "x",
        "",
    ];
    let befores = ["", "U&", "u&", "xU&", " "];
    let texts = [
        "-- c",
        "--c",
        "--\tc",
        "# c",
        "#c",
        "/* c */",
        "x",
        "",
        "--",
        "'x'",
        "(('x'))",
        "_utf8mb4'x'",
        "_utf8mb4 'x'",
        "-1",
        "+2.5",
        "TRUE",
        "null",
        "NULL::text",
        "'now'::date",
        "(0)::numeric",
        "now()",
        "CURRENT_TIMESTAMP",
        "CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP",
        "'on update' ON UPDATE x",
        "nextval('s'::regclass)",
        "(a) + (b)",
        "\"x\"",
        "'a\\'b'",
    ];
    let words = ["select", "SELECT", "Lateral", "returning", "users", "", "sql_mode"];
    let schemas = [None, Some("public"), Some("shop")];
    for mode in every_mode() {
        let (my, ma) = (Dialect::MySql(mode), Dialect::MariaDb(mode));
        let at = format!("{mode:?}");
        assert_eq!(ma.ident_quote(), my.ident_quote(), "{at}");
        assert_eq!(ma.ident_quotes(), my.ident_quotes(), "{at}");
        assert_eq!(ma.string_quotes(), my.string_quotes(), "{at}");
        assert_eq!(ma.keywords(), my.keywords(), "{at}");
        assert_eq!(ma.comment_marker(), my.comment_marker(), "{at}");
        assert_eq!(ma.sqlformat_dialect(), my.sqlformat_dialect(), "{at}");
        // Kept engines: the printed dialect and another mode of the same engine.
        assert_eq!(ma.printed(), Dialect::MariaDb(MySqlMode::default()), "{at}");
        assert_eq!(my.printed(), Dialect::MySql(MySqlMode::default()), "{at}");
        for other in every_mode() {
            assert_eq!(ma.with_mode(other), Dialect::MariaDb(other), "{at}");
            assert_eq!(my.with_mode(other), Dialect::MySql(other), "{at}");
        }
        for s in names.iter().chain(&values) {
            let at = format!("{s:?} in {mode:?}");
            assert_eq!(ma.needs_quotes(s), my.needs_quotes(s), "{at}");
            assert_eq!(ma.quote_ident(s), my.quote_ident(s), "{at}");
            assert_eq!(ma.force_quote_ident(s), my.force_quote_ident(s), "{at}");
            assert_eq!(ma.quote_literal(s), my.quote_literal(s), "{at}");
            assert_eq!(ma.fold(s), my.fold(s), "{at}");
            for q in ['`', '"', '\''] {
                assert_eq!(ma.unescape_ident(q, s), my.unescape_ident(q, s), "{at}");
            }
            for open in [
                LiteralOpen { prefix: 0, quote: '\'', backslash: true },
                LiteralOpen { prefix: 0, quote: '"', backslash: false },
                LiteralOpen { prefix: 1, quote: '\'', backslash: true },
            ] {
                assert_eq!(ma.literal_body(s, open), my.literal_body(s, open), "{at}");
                assert_eq!(ma.unescape_literal_body(s, open), my.unescape_literal_body(s, open), "{at}");
            }
        }
        for t in tokens.iter().chain(&texts) {
            let at = format!("{t:?} in {mode:?}");
            assert_eq!(ma.opening_quote(t), my.opening_quote(t), "{at}");
            assert_eq!(ma.unquote(t), my.unquote(t), "{at}");
            assert_eq!(ma.unquote_lenient(t), my.unquote_lenient(t), "{at}");
            assert_eq!(ma.unquote_literal(t), my.unquote_literal(t), "{at}");
            for before in befores {
                assert_eq!(ma.literal_open(before, t), my.literal_open(before, t), "{before:?} {at}");
            }
            assert_eq!(ma.line_comment_at(t), my.line_comment_at(t), "{at}");
            assert_eq!(ma.unparenthesized(t), my.unparenthesized(t), "{at}");
            assert_eq!(ma.constant(t), my.constant(t), "{at}");
            assert_eq!(ma.draws_number(t), my.draws_number(t), "{at}");
            assert_eq!(ma.insert_default(t), my.insert_default(t), "{at}");
        }
        for w in words {
            assert_eq!(ma.is_keyword(w), my.is_keyword(w), "{w:?} in {mode:?}");
        }
        for s in schemas {
            assert_eq!(ma.default_path(s), my.default_path(s), "{s:?} in {mode:?}");
        }
        assert_eq!(Language::Sql(ma).dialect(), ma, "{at}");
    }
    assert_eq!(Dialect::Postgres.with_mode(MySqlMode::default()), Dialect::Postgres);
}
