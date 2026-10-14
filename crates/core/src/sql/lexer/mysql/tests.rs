use super::super::{LexState, Tok, lex_from, lex_in};
use crate::sql::dialect::{Dialect, MySqlMode};

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });
const ANSI: Dialect =
    Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: false, dollar_quotes: false });
const NBE: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: true, dollar_quotes: false });

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
    for c in ["-- x;", "--\tx;", "--", "--\u{b}x;", "# x;", "#x;"] {
        let src = format!("a {c}\nb");
        assert_eq!(tokens(&src), vec![(Tok::Ident, "a"), (Tok::LineComment, c), (Tok::Ident, "b")], "{src:?}");
    }
    // `--` and a control character that is no blank: code to the client, which ends the
    // statement at the `;` after it.
    assert_eq!(kinds("a --\u{1}x;")[..3], [(Tok::Ident, "a"), (Tok::Op, "-"), (Tok::Op, "-")]);
    assert_eq!(kinds("a --\u{1}x;").last(), Some(&(Tok::Semi, ";")));
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
    assert_eq!(delim("delimiter \\x"), "x", "a backslash escapes the next character");
    assert_eq!(delim("delimiter a\\ b c"), "a b", "a space too");
    assert_eq!(delim("delimiter \\\\x"), ";", "a backslash left is refused");
    assert_eq!(delim("delimiter '\\x'"), ";", "in quotes too");
    assert_eq!(delim("delimiter $$\t-- x"), "$$\t--", "only a space ends the word");
    assert_eq!(delim("delimiter //\r"), "//", "the client reads lines without their carriage return");
    assert_eq!(delim("delimiter\r\n"), ";", "a command without its word");
    assert_eq!(delim("/* a\nb */ delimiter //"), ";", "a line that starts in a comment");
    assert_eq!(delim("'a\nb' delimiter //"), ";", "a line that starts in a string");
    assert_eq!(delim("/*! x */\ndelimiter //"), ";", "after an executable comment");
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

/// The terminator is found at any ASCII character outside strings and comments, as the client
/// looks for it: in hex and bit numbers, exponents and blanks too; never at a character outside
/// ASCII, so one that starts with such a character never ends a statement.
#[test]
fn the_terminator_is_found_where_the_client_looks() {
    let semis = |src: &str| {
        lex_in(src, MY).iter().filter(|t| t.kind == Tok::Semi).map(|t| t.text(src).to_string()).collect::<Vec<_>>()
    };
    assert_eq!(semis("delimiter ab\nselect 0xab"), ["ab"]);
    assert_eq!(semis("delimiter e5\nselect 1e5"), ["e5"]);
    assert_eq!(semis("delimiter 'x '\nselect 1x  y"), ["x "]);
    assert_eq!(semis("delimiter \u{e9}\nselect 1\u{e9} select 2\u{e9}"), Vec::<String>::new());
    assert_eq!(semis("delimiter a\u{e9}\nselect 1 a\u{e9}"), ["a\u{e9}"]);
}

/// `\g` and `\G` end a statement; a backslash outside strings takes the next character with
/// it, so `\'` opens no string.
#[test]
fn client_backslash_commands() {
    let src = "select 1\\G select 2\\g select 3 \\'; select 4;";
    let k = kinds(src);
    assert_eq!(k.iter().filter(|t| t.0 == Tok::Semi).map(|t| t.1).collect::<Vec<_>>(), ["\\G", "\\g", ";", ";"]);
    assert!(k.contains(&(Tok::Op, "\\'")), "{k:?}");
    // In a string a backslash is an escape, as before.
    assert_eq!(kinds("'\\g'"), [(Tok::Str, "'\\g'")]);
}

/// The client reads a backslash as an escape in every string it tracks: `"…"` under
/// `ANSI_QUOTES` and hex and bit strings too.
#[test]
fn the_client_reads_backslashes_in_every_string() {
    assert_eq!(kinds_in("select \"a\\\"b\"; select 1", ANSI)[1], (Tok::QuotedIdent, "\"a\\\"b\""));
    assert_eq!(kinds("select x'4\\'; x'")[1], (Tok::Str, "x'4\\'; x'"));
    assert_eq!(kinds_in("select x'4\\'; x'", NBE)[1], (Tok::Str, "x'4\\'"));
}

/// Dollar quotes, on a server whose client reads them: `$$…$$` and `$tag$…$tag$` are strings
/// (a backslash escaping the next character, as the client reads them); a `$` after a name
/// character goes on the name. Without them, `$` is a name character.
#[test]
fn dollar_quotes_where_the_server_reads_them() {
    let dq = Dialect::MySql(MySqlMode { dollar_quotes: true, ..MySqlMode::default() });
    assert_eq!(
        kinds_in("select $$a;b$$, $t$ $$ ; $t$", dq)[..4],
        [(Tok::Keyword, "select"), (Tok::Str, "$$a;b$$"), (Tok::Comma, ","), (Tok::Str, "$t$ $$ ; $t$")]
    );
    assert_eq!(kinds_in("select $$a\\$$ b$$", dq)[1], (Tok::Str, "$$a\\$$ b$$"));
    assert_eq!(kinds_in("select a$$ from t", dq)[1], (Tok::Ident, "a$$"));
    assert_eq!(kinds_in("select $x y$ from t", dq)[1], (Tok::Ident, "$x"), "a tag of name characters");
    assert_eq!(kinds_in("select $$open", dq)[1], (Tok::Str, "$$open"));
    // `DELIMITER $$` comes first.
    assert_eq!(kinds_in("delimiter $$\nselect 1$$", dq).last(), Some(&(Tok::Semi, "$$")));
    assert_eq!(kinds("select $$a;b$$")[1], (Tok::Ident, "$$a"));
}

/// Executable comment versions: six digits, or five (the server reads five of seven).
#[test]
fn executable_comment_versions() {
    assert_eq!(kinds("/*!8000001 x */")[..2], [(Tok::ExecComment, "/*!80000"), (Tok::Number, "01")]);
    assert_eq!(kinds("/*!080400 x */")[0], (Tok::ExecComment, "/*!080400"));
}

/// A script with `\r\n` line ends splits as one with `\n` ends: the client reads its lines
/// without the carriage returns.
#[test]
fn crlf_scripts_split_as_the_client_does() {
    let src = "delimiter $$\r\nselect 1 $$ -- one\r\nselect 2$$\r\ndelimiter ;\r\nselect 3; -- three\r\nselect 4;\r\n";
    let bodies: Vec<&str> = crate::sql::split::split_in(src, MY).iter().map(|s| s.body(src)).collect();
    assert_eq!(bodies, ["select 1", "select 2", "select 3", "select 4"]);
}

/// A backslash at the end of a line is dropped by the client: a `DELIMITER` on the next line is
/// still a command.
#[test]
fn a_backslash_at_a_line_end_is_dropped() {
    let src = "\\\ndelimiter $$\nselect 6$$";
    assert_eq!(
        kinds(src),
        [(Tok::Directive, "delimiter $$"), (Tok::Keyword, "select"), (Tok::Number, "6"), (Tok::Semi, "$$")]
    );
    assert_eq!(kinds("select 1 \\"), [(Tok::Keyword, "select"), (Tok::Number, "1")]);
    let crlf = "\\\r\ndelimiter $$\r\nselect 6$$";
    assert_eq!(kinds(crlf)[0], (Tok::Directive, "delimiter $$\r"));
}
