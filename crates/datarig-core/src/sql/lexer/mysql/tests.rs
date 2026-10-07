use super::super::{LexState, Tok, lex_from, lex_in};
use crate::sql::dialect::{Dialect, MySqlMode};

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false });
const ANSI: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: false });
const NBE: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: true });

fn kinds_in(src: &str, d: Dialect) -> Vec<(Tok, &str)> {
    lex_in(src, d).into_iter().filter(|t| !t.is_trivia()).map(|t| (t.kind, t.text(src))).collect()
}

fn kinds(src: &str) -> Vec<(Tok, &str)> {
    kinds_in(src, MY)
}

/// Comments kept.
fn tokens(src: &str) -> Vec<(Tok, &str)> {
    lex_in(src, MY).into_iter().filter(|t| t.kind != Tok::Whitespace).map(|t| (t.kind, t.text(src))).collect()
}

/// Every byte of `src` is in exactly one token, in order.
fn covers(src: &str, d: Dialect) {
    let toks = lex_in(src, d);
    let mut at = 0;
    for t in &toks {
        assert_eq!(t.start, at, "{src:?}: {toks:?}");
        assert!(t.end > t.start, "{src:?}: {toks:?}");
        at = t.end;
    }
    assert_eq!(at, src.len(), "{src:?}");
}

#[test]
fn strings_names_and_numbers() {
    assert_eq!(
        kinds("SELECT `a``b`, \"it\\\"s\", 'it''s', 1.5e3, .5, 0x1F, 0b01, x'41', B'1', N'n', _utf8mb4'u' FROM t"),
        vec![
            (Tok::Keyword, "SELECT"),
            (Tok::QuotedIdent, "`a``b`"),
            (Tok::Comma, ","),
            (Tok::Str, "\"it\\\"s\""),
            (Tok::Comma, ","),
            (Tok::Str, "'it''s'"),
            (Tok::Comma, ","),
            (Tok::Number, "1.5e3"),
            (Tok::Comma, ","),
            (Tok::Number, ".5"),
            (Tok::Comma, ","),
            (Tok::Number, "0x1F"),
            (Tok::Comma, ","),
            (Tok::Number, "0b01"),
            (Tok::Comma, ","),
            (Tok::Str, "x'41'"),
            (Tok::Comma, ","),
            (Tok::Str, "B'1'"),
            (Tok::Comma, ","),
            (Tok::Str, "N'n'"),
            (Tok::Comma, ","),
            (Tok::Ident, "_utf8mb4"),
            (Tok::Str, "'u'"),
            (Tok::Keyword, "FROM"),
            (Tok::Ident, "t"),
        ]
    );
}

/// Names that start with a digit, `$` in names, and what follows `name.` (the server's
/// `MY_LEX_NUMBER_IDENT` and `MY_LEX_IDENT_SEP`).
#[test]
fn names_that_look_like_numbers() {
    assert_eq!(kinds("1col"), vec![(Tok::Ident, "1col")]);
    assert_eq!(kinds("0x"), vec![(Tok::Ident, "0x")]);
    assert_eq!(kinds("0xZZ"), vec![(Tok::Ident, "0xZZ")]);
    assert_eq!(kinds("0b12"), vec![(Tok::Ident, "0b12")]);
    assert_eq!(kinds("1e5x"), vec![(Tok::Number, "1e5"), (Tok::Ident, "x")]);
    assert_eq!(kinds("1ex"), vec![(Tok::Ident, "1ex")]);
    assert_eq!(kinds("a$b $c"), vec![(Tok::Ident, "a$b"), (Tok::Ident, "$c")]);
    assert_eq!(kinds("t.1e5"), vec![(Tok::Ident, "t"), (Tok::Dot, "."), (Tok::Ident, "1e5")]);
    assert_eq!(kinds("t.select"), vec![(Tok::Ident, "t"), (Tok::Dot, "."), (Tok::Ident, "select")]);
    assert_eq!(kinds("`t`.12"), vec![(Tok::QuotedIdent, "`t`"), (Tok::Dot, "."), (Tok::Ident, "12")]);
    // Apart, a `.` and digits are a number.
    assert_eq!(kinds("t .5"), vec![(Tok::Ident, "t"), (Tok::Number, ".5")]);
    assert_eq!(kinds("caf\u{e9} \u{6f22}"), vec![(Tok::Ident, "caf\u{e9}"), (Tok::Ident, "\u{6f22}")]);
}

#[test]
fn comments_as_mysql_reads_them() {
    // `--` needs a blank or a control character after it.
    assert_eq!(kinds("a--1"), vec![(Tok::Ident, "a"), (Tok::Op, "-"), (Tok::Op, "-"), (Tok::Number, "1")]);
    assert_eq!(kinds("a --;"), vec![(Tok::Ident, "a"), (Tok::Op, "-"), (Tok::Op, "-"), (Tok::Semi, ";")]);
    for c in ["-- x;", "--\tx;", "--", "--\u{1}x;", "# x;", "#x;"] {
        let src = format!("a {c}\nb");
        assert_eq!(tokens(&src), vec![(Tok::Ident, "a"), (Tok::LineComment, c), (Tok::Ident, "b")], "{src:?}");
    }
    // A line comment ends at `\n` only.
    assert_eq!(tokens("a # x\r+1\nb"), vec![(Tok::Ident, "a"), (Tok::LineComment, "# x\r+1"), (Tok::Ident, "b")]);
    // Block comments do not nest; optimizer hints are comments.
    assert_eq!(
        tokens("/* a /* b */ c */"),
        vec![(Tok::BlockComment, "/* a /* b */"), (Tok::Ident, "c"), (Tok::Op, "*"), (Tok::Op, "/")]
    );
    assert_eq!(kinds("SELECT /*+ BKA(t) ; */ 1"), vec![(Tok::Keyword, "SELECT"), (Tok::Number, "1")]);
}

#[test]
fn executable_comments_are_code() {
    assert_eq!(
        kinds("SELECT 1 /*!80000 + 1 */, 2 /*! , 3 */"),
        vec![
            (Tok::Keyword, "SELECT"),
            (Tok::Number, "1"),
            (Tok::ExecComment, "/*!80000"),
            (Tok::Op, "+"),
            (Tok::Number, "1"),
            (Tok::ExecComment, "*/"),
            (Tok::Comma, ","),
            (Tok::Number, "2"),
            (Tok::ExecComment, "/*!"),
            (Tok::Comma, ","),
            (Tok::Number, "3"),
            (Tok::ExecComment, "*/"),
        ]
    );
    // A six-digit version (MySQL 8.4 reads `/*!080400` as 8.4.0); four digits are code.
    assert_eq!(kinds("/*!080400 x */")[0], (Tok::ExecComment, "/*!080400"));
    assert_eq!(kinds("/*!1234 x */")[..2], [(Tok::ExecComment, "/*!"), (Tok::Number, "1234")]);
    // A comment inside one is a comment, and the `*/` after it closes the executable one.
    assert_eq!(
        kinds("/*! a /* b */ c */ d"),
        vec![
            (Tok::ExecComment, "/*!"),
            (Tok::Ident, "a"),
            (Tok::Ident, "c"),
            (Tok::ExecComment, "*/"),
            (Tok::Ident, "d")
        ]
    );
    // Outside one, `*/` is two operators; MariaDB's `/*M!` is a comment to MySQL.
    assert_eq!(kinds("a */"), vec![(Tok::Ident, "a"), (Tok::Op, "*"), (Tok::Op, "/")]);
    assert_eq!(kinds("/*M! x */ 1"), vec![(Tok::Number, "1")]);
    // Strings and comments inside it are read as such: `*/` in a string does not close it.
    assert_eq!(kinds("/*! '*/' */").last(), Some(&(Tok::ExecComment, "*/")));
}

#[test]
fn variables_and_parameters() {
    assert_eq!(
        kinds("SET @a = @'b c', @`d` := @@session.sql_mode, @@x; SELECT ?, @, user@host"),
        vec![
            (Tok::Keyword, "SET"),
            (Tok::Variable, "@a"),
            (Tok::Op, "="),
            (Tok::Variable, "@'b c'"),
            (Tok::Comma, ","),
            (Tok::Variable, "@`d`"),
            (Tok::Op, ":"),
            (Tok::Op, "="),
            (Tok::Variable, "@@session.sql_mode"),
            (Tok::Comma, ","),
            (Tok::Variable, "@@x"),
            (Tok::Semi, ";"),
            (Tok::Keyword, "SELECT"),
            (Tok::Param, "?"),
            (Tok::Comma, ","),
            (Tok::Op, "@"),
            (Tok::Comma, ","),
            (Tok::Ident, "user"),
            (Tok::Variable, "@host"),
        ]
    );
    // A `;` in a quoted variable name ends nothing.
    assert_eq!(kinds("@'a;b'"), vec![(Tok::Variable, "@'a;b'")]);
}

#[test]
fn sql_modes_change_quotes_and_backslashes() {
    assert_eq!(kinds_in("\"a\"", MY), vec![(Tok::Str, "\"a\"")]);
    assert_eq!(kinds_in("\"a\\\"", ANSI), vec![(Tok::QuotedIdent, "\"a\\\"")]);
    assert_eq!(kinds_in("'a\\'; b'", MY), vec![(Tok::Str, "'a\\'; b'")]);
    assert_eq!(kinds_in("'a\\'; b'", NBE), vec![(Tok::Str, "'a\\'"), (Tok::Semi, ";"), (Tok::Str, "b'")]);
    assert_eq!(kinds_in("'\\\\'; x", MY), vec![(Tok::Str, "'\\\\'"), (Tok::Semi, ";"), (Tok::Ident, "x")]);
    // No dollar quotes.
    assert_eq!(kinds("$$a;b$$"), vec![(Tok::Ident, "$$a"), (Tok::Semi, ";"), (Tok::Ident, "b$$")]);
}

#[test]
fn delimiter_lines_change_the_terminator() {
    let src = "DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; END//\ndelimiter ;\nSELECT 2;";
    let k = kinds(src);
    assert_eq!(k[0], (Tok::Directive, "DELIMITER //"));
    assert!(k.contains(&(Tok::Op, ";")), "`;` is not the terminator inside: {k:?}");
    assert!(k.contains(&(Tok::Semi, "//")), "{k:?}");
    assert!(k.contains(&(Tok::Directive, "delimiter ;")), "{k:?}");
    assert_eq!(k.last(), Some(&(Tok::Semi, ";")));
    // In a name (as the client finds it, anywhere outside strings, names in quotes and
    // comments), case as written.
    assert_eq!(kinds("DELIMITER $$\nSELECT a$$")[2..], [(Tok::Ident, "a"), (Tok::Semi, "$$")]);
    assert_eq!(kinds("DELIMITER XX\nSELECT 1xx")[2..], [(Tok::Ident, "1xx")]);
    let k = kinds("DELIMITER $$\nSELECT '$$', `$$`, /* $$ */ 1$$");
    assert_eq!(k.iter().filter(|t| t.0 == Tok::Semi).collect::<Vec<_>>(), [&(Tok::Semi, "$$")], "{k:?}");
    assert_eq!(k.last(), Some(&(Tok::Semi, "$$")));
}

/// What counts as a `DELIMITER` command: what the client takes (probed with mysql 8.4's).
#[test]
fn delimiter_lines_as_the_client_reads_them() {
    let delim = |src: &str| {
        let mut s = LexState::default();
        for t in lex_in(src, MY) {
            s = s.after(&t, src, MY);
        }
        s.delimiter.as_str().to_string()
    };
    assert_eq!(delim("DELIMITER //"), "//");
    assert_eq!(delim("  dElImItEr\t$$  and the rest"), "$$");
    assert_eq!(delim("delimiter 'a b'"), "a b");
    assert_eq!(delim("delimiter 1234567890123456789"), "123456789012345", "15 bytes kept");
    assert_eq!(delim("delimiter \\x"), ";", "a backslash is refused");
    assert_eq!(delim("delimiter"), ";", "no word");
    assert_eq!(delim("/* c */ delimiter //"), "//", "after a comment");
    assert_eq!(delim("# c\ndelimiter //"), "//", "after a comment line");
    assert_eq!(delim("SELECT 1\ndelimiter //"), ";", "in a statement");
    assert_eq!(delim("SELECT 1; delimiter //"), ";", "not at a line's start");
    assert_eq!(delim("delimiter(x)"), ";", "not followed by a blank");
    assert_eq!(delim("delimiter //\nselect 1 // delimiter ;"), "//");
    assert_eq!(delim("delimiter //\ndelimiter ;"), ";");
    // `SELECT 1 AS` then `delimiter` on the next line: a name.
    let src = "SELECT 1 AS\ndelimiter;";
    assert_eq!(kinds(src).last(), Some(&(Tok::Semi, ";")));
    assert_eq!(kinds(src)[3], (Tok::Ident, "delimiter"));
}

/// A text lexed from the middle starts from the state there: the terminator, whether a
/// statement has begun, an executable comment open.
#[test]
fn lexing_from_a_state() {
    let src = "DELIMITER //\nSELECT 1; /*! x;\ny */ //\ndelimiter ;\nz";
    let toks = lex_in(src, MY);
    let mut state = LexState::default();
    for t in &toks {
        // From every token start, the rest lexes to the same tokens.
        let rest: Vec<_> = lex_from(&src[t.start..], MY, state)
            .into_iter()
            .map(|u| (u.kind, u.start + t.start, u.end + t.start))
            .collect();
        let whole: Vec<_> = toks.iter().filter(|u| u.start >= t.start).map(|u| (u.kind, u.start, u.end)).collect();
        assert_eq!(rest, whole, "from {}", t.start);
        state = state.after(t, src, MY);
    }
    // PostgreSQL has no state.
    let pg = LexState::default();
    for t in super::super::lex(src) {
        assert_eq!(pg.after(&t, src, Dialect::Postgres), pg);
    }
}

#[test]
fn every_byte_is_in_one_token() {
    for src in [
        "",
        "SELECT 'unterminated",
        "/* open",
        "/*! open",
        "`open",
        "@'open",
        "x'open",
        "DELIMITER //\nSELECT 1//\n",
        "a--1 #c\n-- d\n/*!8 x*/ 0x 0xg 1e 1e+ 1.e5 .e @ @@ ?",
        "\u{1F418} \u{a0}\u{0}\r\n\t;;",
        "delimiter \u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\nx\u{e9}\u{e9}",
    ] {
        for d in [MY, ANSI, NBE] {
            covers(src, d);
        }
    }
}

/// A terminator longer than 15 bytes is cut at a character's boundary.
#[test]
fn a_long_terminator_keeps_whole_characters() {
    let src = "delimiter \u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\nx";
    let t = lex_in(src, MY)[0];
    let s = LexState::default().after(&t, src, MY);
    assert_eq!(s.delimiter.as_str(), "\u{e9}".repeat(7));
}
