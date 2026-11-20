//! How each engine writes names and strings, and where it cuts a text into statements.

use crate::Engine::{MariaDb, MySql, MySqlAnsiQuotes, MySqlNoBackslashEscapes, Pg};
use crate::{Case, Engine};
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, lex_in};
use datarig_core::sql::split::split_in;

/// `quote_ident`, `force_quote_ident` and `quote_literal` of `s` on `e`.
pub fn quoting(e: Engine, s: &&'static str) -> String {
    quoting_in(e.dialect(), s)
}

/// [`quoting`] in dialect `d`.
pub fn quoting_in(d: Dialect, s: &str) -> String {
    format!("{} | {} | {}", d.quote_ident(s), d.force_quote_ident(s), d.quote_literal(s))
}

/// The statements of `src` on `e`, then the strings, quoted names, dollar bodies, comments
/// and client commands its lexer finds, each with its kind (a `"…"` is a string or a name by
/// the sql mode).
pub fn split(e: Engine, src: &&'static str) -> String {
    split_in_dialect(e.dialect(), src)
}

/// [`split`] in dialect `d`.
pub fn split_in_dialect(d: Dialect, src: &str) -> String {
    let bodies: Vec<&str> = split_in(src, d).iter().map(|s| s.body(src)).collect();
    let marks: Vec<String> = lex_in(src, d)
        .iter()
        .filter(|t| {
            matches!(
                t.kind,
                Tok::Str
                    | Tok::QuotedIdent
                    | Tok::Dollar
                    | Tok::LineComment
                    | Tok::BlockComment
                    | Tok::ExecComment
                    | Tok::Directive
            )
        })
        .map(|t| format!("{:?} {:?}", t.kind, t.text(src)))
        .collect();
    format!("{} / {}", bodies.join(" | "), marks.join(", "))
}

pub fn quoting_cases() -> Vec<Case<&'static str>> {
    vec![
        Case {
            name: "plain name",
            input: "id",
            expect: &[(Pg, "id | \"id\" | 'id'"), (MySql, "id | `id` | 'id'"), (MariaDb, "id | `id` | 'id'")],
        },
        Case {
            name: "upper case",
            input: "Id",
            expect: &[(Pg, "\"Id\" | \"Id\" | 'Id'"), (MySql, "Id | `Id` | 'Id'"), (MariaDb, "Id | `Id` | 'Id'")],
        },
        Case {
            name: "space",
            input: "Mixed Col",
            expect: &[
                (Pg, "\"Mixed Col\" | \"Mixed Col\" | 'Mixed Col'"),
                (MySql, "`Mixed Col` | `Mixed Col` | 'Mixed Col'"),
                (MariaDb, "`Mixed Col` | `Mixed Col` | 'Mixed Col'"),
            ],
        },
        Case {
            name: "keyword",
            input: "select",
            expect: &[
                (Pg, "\"select\" | \"select\" | 'select'"),
                (MySql, "`select` | `select` | 'select'"),
                (MariaDb, "`select` | `select` | 'select'"),
            ],
        },
        Case {
            name: "double quote",
            input: "a\"b",
            expect: &[
                (Pg, "\"a\"\"b\" | \"a\"\"b\" | 'a\"b'"),
                (MySql, "`a\"b` | `a\"b` | 'a\"b'"),
                (MariaDb, "`a\"b` | `a\"b` | 'a\"b'"),
            ],
        },
        Case {
            name: "backtick",
            input: "a`b",
            expect: &[
                (Pg, "\"a`b\" | \"a`b\" | 'a`b'"),
                (MySql, "`a``b` | `a``b` | 'a`b'"),
                (MariaDb, "`a``b` | `a``b` | 'a`b'"),
                (MySqlAnsiQuotes, "`a``b` | `a``b` | 'a`b'"),
            ],
        },
        Case {
            name: "single quote",
            input: "it's",
            expect: &[
                (Pg, "\"it's\" | \"it's\" | 'it''s'"),
                (MySql, "`it's` | `it's` | 'it''s'"),
                (MariaDb, "`it's` | `it's` | 'it''s'"),
            ],
        },
        Case {
            name: "backslash",
            input: "back\\slash",
            expect: &[
                (Pg, "\"back\\slash\" | \"back\\slash\" | E'back\\\\slash'"),
                (MySql, "`back\\slash` | `back\\slash` | 'back\\\\slash'"),
                (MariaDb, "`back\\slash` | `back\\slash` | 'back\\\\slash'"),
                (MySqlNoBackslashEscapes, "`back\\slash` | `back\\slash` | 'back\\slash'"),
            ],
        },
        Case {
            name: "nul",
            input: "a\0b",
            expect: &[
                (Pg, "\"a\0b\" | \"a\0b\" | 'a\0b'"),
                (MySql, "`a\0b` | `a\0b` | 'a\\0b'"),
                (MariaDb, "`a\0b` | `a\0b` | 'a\\0b'"),
                (MySqlNoBackslashEscapes, "`a\0b` | `a\0b` | 'a\0b'"),
            ],
        },
        Case {
            name: "non ascii",
            input: "caf\u{e9}",
            expect: &[
                (Pg, "\"café\" | \"café\" | 'café'"),
                (MySql, "`café` | `café` | 'café'"),
                (MariaDb, "`café` | `café` | 'café'"),
            ],
        },
        Case {
            name: "leading digit",
            input: "1a",
            expect: &[(Pg, "\"1a\" | \"1a\" | '1a'"), (MySql, "`1a` | `1a` | '1a'"), (MariaDb, "`1a` | `1a` | '1a'")],
        },
        Case {
            name: "dollar",
            input: "a$b",
            expect: &[
                (Pg, "\"a$b\" | \"a$b\" | 'a$b'"),
                (MySql, "a$b | `a$b` | 'a$b'"),
                (MariaDb, "a$b | `a$b` | 'a$b'"),
            ],
        },
    ]
}

pub fn split_cases() -> Vec<Case<&'static str>> {
    vec![
        Case {
            name: "two statements",
            input: "SELECT 1; SELECT 2",
            expect: &[
                (Pg, "SELECT 1 | SELECT 2 / "),
                (MySql, "SELECT 1 | SELECT 2 / "),
                (MariaDb, "SELECT 1 | SELECT 2 / "),
            ],
        },
        Case {
            name: "semicolon in a string",
            input: "SELECT 'a;b'; SELECT 2",
            expect: &[
                (Pg, "SELECT 'a;b' | SELECT 2 / Str \"'a;b'\""),
                (MySql, "SELECT 'a;b' | SELECT 2 / Str \"'a;b'\""),
                (MariaDb, "SELECT 'a;b' | SELECT 2 / Str \"'a;b'\""),
            ],
        },
        Case {
            name: "semicolon in double quotes",
            input: "SELECT \"a;b\"; SELECT 2",
            expect: &[
                (Pg, "SELECT \"a;b\" | SELECT 2 / QuotedIdent \"\\\"a;b\\\"\""),
                (MySql, "SELECT \"a;b\" | SELECT 2 / Str \"\\\"a;b\\\"\""),
                (MariaDb, "SELECT \"a;b\" | SELECT 2 / Str \"\\\"a;b\\\"\""),
                (MySqlAnsiQuotes, "SELECT \"a;b\" | SELECT 2 / QuotedIdent \"\\\"a;b\\\"\""),
            ],
        },
        Case {
            name: "backtick name",
            input: "SELECT `a;b` FROM t; SELECT 2",
            expect: &[
                (Pg, "SELECT `a | b` FROM t | SELECT 2 / "),
                (MySql, "SELECT `a;b` FROM t | SELECT 2 / QuotedIdent \"`a;b`\""),
                (MariaDb, "SELECT `a;b` FROM t | SELECT 2 / QuotedIdent \"`a;b`\""),
                (MySqlAnsiQuotes, "SELECT `a;b` FROM t | SELECT 2 / QuotedIdent \"`a;b`\""),
            ],
        },
        Case {
            name: "backslash before a quote",
            input: "SELECT 'a\\'; SELECT 2; -- '",
            expect: &[
                (Pg, "SELECT 'a\\' | SELECT 2 / Str \"'a\\\\'\", LineComment \"-- '\""),
                (MySql, "SELECT 'a\\'; SELECT 2; -- ' / Str \"'a\\\\'; SELECT 2; -- '\""),
                (MariaDb, "SELECT 'a\\'; SELECT 2; -- ' / Str \"'a\\\\'; SELECT 2; -- '\""),
                (MySqlNoBackslashEscapes, "SELECT 'a\\' | SELECT 2 / Str \"'a\\\\'\", LineComment \"-- '\""),
            ],
        },
        Case {
            name: "dollar quotes",
            input: "SELECT $$a;b$$; SELECT 2",
            expect: &[
                (Pg, "SELECT $$a;b$$ | SELECT 2 / Dollar \"$$a;b$$\""),
                (MySql, "SELECT $$a | b$$ | SELECT 2 / "),
                (MariaDb, "SELECT $$a | b$$ | SELECT 2 / "),
            ],
        },
        Case {
            name: "delimiter",
            input: "DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; END//\nDELIMITER ;\nSELECT 2;",
            expect: &[
                (Pg, "DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1 | END//\nDELIMITER | SELECT 2 / "),
                (
                    MySql,
                    "CREATE PROCEDURE p() BEGIN SELECT 1; END | SELECT 2 / Directive \"DELIMITER //\", Directive \"DELIMITER ;\"",
                ),
                (
                    MariaDb,
                    "CREATE PROCEDURE p() BEGIN SELECT 1; END | SELECT 2 / Directive \"DELIMITER //\", Directive \"DELIMITER ;\"",
                ),
            ],
        },
        Case {
            name: "executable comment",
            input: "SELECT 1 /*!50000 ; SELECT 2 */; SELECT 3",
            expect: &[
                (Pg, "SELECT 1 | SELECT 3 / BlockComment \"/*!50000 ; SELECT 2 */\""),
                (MySql, "SELECT 1 /*!50000 | SELECT 2 */ | SELECT 3 / ExecComment \"/*!50000\""),
                (MariaDb, "SELECT 1 /*!50000 | SELECT 2 */ | SELECT 3 / ExecComment \"/*!50000\""),
            ],
        },
        Case {
            name: "mariadb executable comment",
            input: "SELECT 1 /*M!100000 ; SELECT 2 */; SELECT 3",
            expect: &[
                (Pg, "SELECT 1 | SELECT 3 / BlockComment \"/*M!100000 ; SELECT 2 */\""),
                (MySql, "SELECT 1 | SELECT 3 / BlockComment \"/*M!100000 ; SELECT 2 */\""),
                (MariaDb, "SELECT 1 | SELECT 3 / BlockComment \"/*M!100000 ; SELECT 2 */\""),
            ],
        },
        Case {
            name: "hash comment",
            input: "SELECT 1 # c;\nFROM t; SELECT 2",
            expect: &[
                (Pg, "SELECT 1 # c | FROM t | SELECT 2 / "),
                (MySql, "SELECT 1 # c;\nFROM t | SELECT 2 / LineComment \"# c;\""),
                (MariaDb, "SELECT 1 # c;\nFROM t | SELECT 2 / LineComment \"# c;\""),
            ],
        },
        Case {
            name: "dash comment without a space",
            input: "SELECT 1 --c;\n; SELECT 2",
            expect: &[
                (Pg, "SELECT 1 | SELECT 2 / LineComment \"--c;\""),
                (MySql, "SELECT 1 --c | SELECT 2 / "),
                (MariaDb, "SELECT 1 --c | SELECT 2 / "),
            ],
        },
        Case {
            name: "nested block comment",
            input: "SELECT 1 /* a /* b */ ; */; SELECT 2",
            expect: &[
                (Pg, "SELECT 1 | SELECT 2 / BlockComment \"/* a /* b */ ; */\""),
                (MySql, "SELECT 1 | */ | SELECT 2 / BlockComment \"/* a /* b */\""),
                (MariaDb, "SELECT 1 | */ | SELECT 2 / BlockComment \"/* a /* b */\""),
            ],
        },
    ]
}
