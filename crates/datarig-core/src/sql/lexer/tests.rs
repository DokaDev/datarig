use super::*;

fn kinds(src: &str) -> Vec<(Tok, &str)> {
    lex(src).into_iter().filter(|t| !t.is_trivia()).map(|t| (t.kind, t.text(src))).collect()
}

#[test]
fn basic_tokens() {
    let k = kinds("SELECT u.id, 'it''s' FROM \"Sh\".t WHERE x >= 1.5e3 -- c");
    assert_eq!(
        k,
        vec![
            (Tok::Keyword, "SELECT"),
            (Tok::Ident, "u"),
            (Tok::Dot, "."),
            (Tok::Ident, "id"),
            (Tok::Comma, ","),
            (Tok::Str, "'it''s'"),
            (Tok::Keyword, "FROM"),
            (Tok::QuotedIdent, "\"Sh\""),
            (Tok::Dot, "."),
            (Tok::Ident, "t"),
            (Tok::Keyword, "WHERE"),
            (Tok::Ident, "x"),
            (Tok::Op, ">"),
            (Tok::Op, "="),
            (Tok::Number, "1.5e3"),
        ]
    );
}

#[test]
fn dollar_quotes_and_comments() {
    let src = "SELECT $$a;b$$, $fn$ x $$ y $fn$, $1 /* a /* nested; */ b */";
    let k = kinds(src);
    assert_eq!(k[1], (Tok::Dollar, "$$a;b$$"));
    assert_eq!(k[3], (Tok::Dollar, "$fn$ x $$ y $fn$"));
    assert_eq!(k[5], (Tok::Param, "$1"));
    assert_eq!(lex(src).last().unwrap().kind, Tok::BlockComment);
}

#[test]
fn unterminated_runs_to_end() {
    let src = "SELECT 'abc";
    assert_eq!(lex(src).last().unwrap().kind, Tok::Str);
    assert_eq!(lex(src).last().unwrap().end, src.len());
    let src = "SELECT $$ body";
    assert_eq!(lex(src).last().unwrap().kind, Tok::Dollar);
}

#[test]
fn escape_string_and_unicode_ident() {
    let k = kinds("SELECT E'a\\'b', 漢字列 FROM t");
    assert_eq!(k[1], (Tok::Str, "E'a\\'b'"));
    assert_eq!(k[3], (Tok::Ident, "漢字列"));
}

#[test]
fn schema_changes_are_create_alter_drop_and_do_or_call_bodies() {
    for sql in [
        "CREATE TABLE t (id int PRIMARY KEY)",
        "  -- new\n/* x */ alter table t add unique (id)",
        "Drop Table t",
        "create unique index on t (a)",
        "DO $$ BEGIN EXECUTE 'ALTER TABLE t ADD PRIMARY KEY (id)'; END $$",
        "do language plpgsql $body$ BEGIN CREATE TABLE u (id int); END $body$",
        "CALL make_tables()",
    ] {
        assert!(changes_schema(sql), "{sql}");
    }
    for sql in [
        "SELECT 'CREATE'",
        "INSERT INTO t VALUES (1)",
        "-- DROP TABLE t\nSELECT 1",
        "\"create\"",
        "\"do\"",
        "done",
        "SELECT do_it()",
        "",
    ] {
        assert!(!changes_schema(sql), "{sql}");
    }
}

#[test]
fn backslash_strings_end_where_a_non_conforming_server_ends_them() {
    let src = r"SELECT 'x\''; DROP TABLE t; --'";
    let semis = |toks: Vec<Token>| toks.iter().filter(|t| t.kind == Tok::Semi).count();
    // Standard: `''` is a quote inside the string, which runs to the last `'`.
    assert_eq!(semis(lex(src)), 0);
    // standard_conforming_strings = off: `\'` is the quote, the next `'` ends the string and
    // the rest is SQL.
    assert_eq!(semis(lex_backslash_strings(src)), 2);
    // E'' strings escape the same way in both.
    assert_eq!(lex(r"E'\''").len(), lex_backslash_strings(r"E'\''").len());
}

/// A lone carriage return ends a `--` comment, as in PostgreSQL's scanner: what follows it is
/// SQL (a saved file with `WHERE id=5 --\r OR true`).
#[test]
fn a_carriage_return_ends_a_line_comment() {
    let src = "UPDATE t SET a = 0 WHERE id = 5 --\r OR true\nRETURNING id";
    let k = kinds(src);
    assert!(k.contains(&(Tok::Keyword, "OR")), "{k:?}");
    assert!(k.contains(&(Tok::Keyword, "true")), "{k:?}");
    let comment = lex(src).into_iter().find(|t| t.kind == Tok::LineComment).unwrap();
    assert_eq!(comment.text(src), "--");
    assert_eq!(kinds("SELECT 1 --a\r\nSELECT 2").len(), 4);
}

/// Every character outside ASCII continues (and starts) an identifier, and so does `$`: a `$`
/// right after one belongs to it and starts no dollar quote (`x<NBSP>$$` and
/// emoji cases). Only ASCII whitespace separates tokens.
#[test]
fn identifiers_take_non_ascii_characters_and_dollars() {
    for (src, ident) in [
        ("SELECT 1 AS x\u{a0}$$, 2", "x\u{a0}$$"),
        ("SELECT 1 AS \u{1f418}$$, 2", "\u{1f418}$$"),
        ("SELECT 1 AS a$b$, 2", "a$b$"),
        ("SELECT 1 AS \u{a0}, 2", "\u{a0}"),
        ("SELECT 1 AS 漢字$$, 2", "漢字$$"),
    ] {
        let k = kinds(src);
        assert_eq!(k[3], (Tok::Ident, ident), "{src}: {k:?}");
        assert_eq!(k[4], (Tok::Comma, ","), "{src}: {k:?}");
    }
    // A dollar quote still starts after whitespace or an operator, also with a non-ASCII tag.
    assert_eq!(kinds("SELECT $漢$ a; b $漢$")[1], (Tok::Dollar, "$漢$ a; b $漢$"));
    assert_eq!(kinds("SELECT 1+$$x$$")[3], (Tok::Dollar, "$$x$$"));
    // A tag cannot start with a digit: `$1` is a parameter.
    assert_eq!(kinds("SELECT $1$")[1], (Tok::Param, "$1"));
    // Non-ASCII whitespace is not whitespace to PostgreSQL.
    assert!(!lex("a\u{3000}b").iter().any(|t| t.kind == Tok::Whitespace));
    assert_eq!(kinds("a\u{3000}b"), [(Tok::Ident, "a\u{3000}b")]);
    assert_eq!(kinds("a\x0bb").len(), 2, "vertical tab is whitespace");
}

/// The lexer in PostgreSQL is the one [`lex`] and [`changes_schema`] use, and its keywords are
/// [`KEYWORDS`].
#[test]
fn lex_in_postgres_is_lex() {
    let texts = [
        "SELECT u.id, 'it''s' FROM \"Sh\".t WHERE x >= 1.5e3 -- c",
        "SELECT $$a;b$$, $t$x$t$, E'\\'', /* a /* b */ c */ 1; DO $$ x $$",
        "create table t (a int); CALL p(); drop view v; select 'unterminated",
        "  -- c\n ALTER TABLE x; SELECT x$$ FROM \u{d14c}\u{c774}\u{be14}\r--y\rSELECT $1",
    ];
    for src in texts {
        assert_eq!(lex_in(src, Dialect::Postgres), lex(src), "{src:?}");
        assert_eq!(changes_schema_in(src, Dialect::Postgres), changes_schema(src), "{src:?}");
    }
    assert_eq!(Dialect::Postgres.keywords(), KEYWORDS);
    assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]), "sorted, for the binary search");
    for w in ["select", "SELECT", "Select", "users", "x$"] {
        assert_eq!(Dialect::Postgres.is_keyword(w), is_keyword(w), "{w}");
    }
}
