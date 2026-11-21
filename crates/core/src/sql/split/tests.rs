use super::*;

const EXAMPLE: &str = "-- datarig 🐘  Ctrl+E: カーソル位置の文を実行
SELECT * FROM shop.users WHERE id <= 8;

SELECT u.name, o.status, o.total_amount
FROM shop.orders o
JOIN shop.users u ON u.id = o.user_id
WHERE o.status = 'paid';

SELECT * FROM analytics.events;

SELECT * FROM analytics.events ORDER BY created_at DESC;

SELECT analytics.slow(30);

SELECT $$セミコロン; 入りの本文$$ AS dollar, 'it''s; fine' AS quoted;
";

fn at(src: &str, needle: &str, delta: isize) -> usize {
    (src.find(needle).expect(needle) as isize + delta) as usize
}

fn body_at(cursor: usize) -> Option<&'static str> {
    let st = split(EXAMPLE);
    statement_at(&st, cursor).map(|i| st[i].body(EXAMPLE))
}

#[test]
fn splits_example_into_six() {
    let st = split(EXAMPLE);
    assert_eq!(st.len(), 6);
    assert_eq!(st[0].body(EXAMPLE), "SELECT * FROM shop.users WHERE id <= 8");
    assert_eq!(st[5].body(EXAMPLE), "SELECT $$セミコロン; 入りの本文$$ AS dollar, 'it''s; fine' AS quoted");
}

#[test]
fn cursor_positions() {
    let join = "SELECT u.name";
    let q2 = "SELECT u.name, o.status, o.total_amount\nFROM shop.orders o\nJOIN shop.users u ON u.id = o.user_id\nWHERE o.status = 'paid'";
    // first line of statement
    assert_eq!(body_at(at(EXAMPLE, join, 0)), Some(q2));
    // middle line
    assert_eq!(body_at(at(EXAMPLE, "JOIN shop", 3)), Some(q2));
    // right after ';'
    assert_eq!(body_at(at(EXAMPLE, "'paid';", 7)), Some(q2));
    // blank line between statements -> previous statement
    assert_eq!(body_at(at(EXAMPLE, "'paid';", 8)), Some(q2));
    // on the ';'
    assert_eq!(body_at(at(EXAMPLE, "'paid';", 6)), Some(q2));
    // trailing newline at end of buffer -> last statement
    assert!(body_at(EXAMPLE.len()).unwrap().starts_with("SELECT $$"));
    // leading comment before the first statement: nothing before it
    assert_eq!(body_at(3), None);
}

#[test]
fn comment_only_is_not_a_statement() {
    assert!(split("-- just a comment\n/* block; */\n  ;  ;").is_empty());
    assert_eq!(split("SELECT 1; -- tail").len(), 1);
}

#[test]
fn missing_final_semicolon() {
    let src = "SELECT 1;\nSELECT 2\n\n";
    let st = split(src);
    assert_eq!(st.len(), 2);
    assert_eq!(st[1].body(src), "SELECT 2");
    assert_eq!(statement_at(&st, src.len()), Some(1));
}

#[test]
fn visual_selection_of_two_statements() {
    let sel = "SELECT 1;\n\nSELECT 'a;b';";
    let bodies: Vec<_> = split(sel).iter().map(|s| s.body(sel)).collect();
    assert_eq!(bodies, vec!["SELECT 1", "SELECT 'a;b'"]);
}

#[test]
fn segments() {
    let src = "SELECT 1; SELECT u. FROM x u";
    let (s, e) = segment_at(src, src.len());
    assert_eq!(&src[s..e], " SELECT u. FROM x u");
    let (s, e) = segment_at(src, 3);
    assert_eq!(&src[s..e], "SELECT 1");
}

/// Tricky texts on which the splitter must agree with PostgreSQL's own parser (libpg_query):
/// comments ended by `\r`, `$` inside identifiers, non-ASCII identifiers and whitespace,
/// nested comments, dollar quotes with tags, escape strings, Unicode strings and parameters.
const CORPUS: &[&str] = &[
    "SELECT 1; SELECT 2",
    "SELECT 1;;SELECT 2;",
    "UPDATE t SET a = 0 WHERE id = 5 --\r OR true\nRETURNING id; SELECT 1",
    "SELECT 1 --a;\r, 2; SELECT 3 --x;\r\n;SELECT 4",
    "WITH a AS (SELECT 1 AS x\u{a0}$$), d AS (DELETE FROM t RETURNING 1) SELECT 1 AS y\u{a0}$$; SELECT 2",
    "SELECT 1 AS \u{1f418}$$; SELECT $$;$$; SELECT 1 AS a$b$; SELECT 'a;'",
    "SELECT $漢$ ; $漢$; SELECT $_x1$ ; $_x1$; SELECT $a$ $b$ ; $b$ $a$",
    "SELECT $1; SELECT $12 ;SELECT 3",
    "SELECT /* a /* nested ; */ still ; */ 1; SELECT 2",
    "SELECT E'\\';'; SELECT 'x'';y'; SELECT U&'d\\0061t;a'",
    "SELECT \"a;b\", \"x\"\"y;\" FROM t; SELECT 2",
    "SELECT a\u{3000}; SELECT b\u{a0}; SELECT 3",
    "SELECT 1\x0b;\x0cSELECT 2",
    "SELECT 1+--;\n2; SELECT 3",
    "SELECT 1 */* ; */ 2; SELECT 3",
    "SELECT 漢字列 FROM 注文; SELECT 'ok'",
    "CREATE FUNCTION f() RETURNS int AS $body$ SELECT 1; $body$ LANGUAGE sql; SELECT f()",
    "DO $$ BEGIN RAISE NOTICE ';'; END $$; SELECT 1",
    "PREPARE p AS DELETE FROM t; EXECUTE p",
    "SELECT 1 -- only a comment at the end",
    "  \n-- c1\n/* c2 */ SELECT 1 ; -- trailing\n",
];

/// The byte offsets of the `;` that end the statements of `src`, as PostgreSQL's parser splits
/// it.
fn pg_terminators(src: &str) -> Vec<usize> {
    let pieces = pg_query::split_with_parser(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
    pieces
        .iter()
        .map(|p| p.as_ptr() as usize - src.as_ptr() as usize + p.len())
        .filter(|&end| src[end..].starts_with(';'))
        .collect()
}

#[test]
fn statement_boundaries_agree_with_postgresql() {
    for src in CORPUS {
        let ours = split(src);
        let pg = pg_query::split_with_parser(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
        assert_eq!(ours.len(), pg.len(), "statements of {src:?}: ours {ours:?}, PostgreSQL {pg:?}");
        let terminated: Vec<usize> = ours.iter().filter(|s| src[..s.end].ends_with(';')).map(|s| s.end - 1).collect();
        assert_eq!(terminated, pg_terminators(src), "the ; of {src:?}");
        // Each of our statements lies in PostgreSQL's (which starts after the previous `;`, with
        // the comments and whitespace before the statement).
        for (s, p) in ours.iter().zip(&pg) {
            let p_start = p.as_ptr() as usize - src.as_ptr() as usize;
            assert!(s.start >= p_start && s.body_end <= p_start + p.len(), "{src:?}: {:?} in {p:?}", s.body(src));
        }
    }
}

/// The same with libpg_query's scanner-based split (it skips statements that do not start with
/// a keyword, so only texts that parse are compared).
#[test]
fn statement_boundaries_agree_with_postgresqls_scanner() {
    for src in CORPUS {
        let ours: Vec<&str> = split(src).iter().map(|s| s.body(src)).collect();
        let pg: Vec<&str> = pg_query::split_with_scanner(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
        // PostgreSQL's whitespace is ASCII only.
        let pg: Vec<&str> = pg
            .into_iter()
            .map(|p| p.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b'))
            .filter(|p| !p.is_empty())
            .collect();
        // The scanner split keeps comments around a statement; compare where each one ends.
        assert_eq!(ours.len(), pg.len(), "{src:?}: ours {ours:?}, PostgreSQL {pg:?}");
        for (o, p) in ours.iter().zip(&pg) {
            assert!(p.ends_with(o) || p.contains(o), "{src:?}: {o:?} vs {p:?}");
        }
    }
}

/// The splitter in PostgreSQL is the one [`split`] and [`segment_at`] use: the same statements
/// and segments, at every cursor, on every text here.
#[test]
fn split_in_postgres_is_split() {
    for src in CORPUS.iter().chain([&EXAMPLE]) {
        assert_eq!(split_in(src, Dialect::Postgres), split(src), "{src:?}");
        for cursor in (0..=src.len()).filter(|&i| src.is_char_boundary(i)) {
            assert_eq!(segment_at_in(src, cursor, Dialect::Postgres), segment_at(src, cursor), "{src:?} {cursor}");
        }
    }
}

fn bodies_in(src: &str, d: Dialect) -> Vec<&str> {
    split_in(src, d).iter().map(|s| s.body(src)).collect()
}

/// An executable comment is code to the server that runs it: MySQL's `/*! … */` and
/// `/*!NNNNN … */`, and MariaDB's `/*M! … */` and `/*M!NNNNNN … */` (MariaDB runs both; MySQL
/// reads `/*M!` as a plain comment). A statement keeps every one of them, at its start, in it
/// and at its end, and one that holds nothing else is a statement too. In every dialect: a
/// MySQL reading (the driver's before a session says which server it is on) may be a MariaDB
/// server's, and text with no database known yet is read as PostgreSQL's.
#[test]
fn executable_comments_are_kept_when_splitting() {
    use crate::sql::dialect::MySqlMode;
    let mode = MySqlMode::default();
    let ansi = MySqlMode { ansi_quotes: true, ..MySqlMode::default() };
    for d in [Dialect::MySql(mode), Dialect::MariaDb(mode), Dialect::MariaDb(ansi), Dialect::Postgres] {
        for (src, want) in [
            ("UPDATE t SET v=0 /*M! , w=1 */;", vec!["UPDATE t SET v=0 /*M! , w=1 */"]),
            ("SELECT a FROM t /*M! , b */", vec!["SELECT a FROM t /*M! , b */"]),
            ("SELECT a FROM t /*M!100500 , b */ ;", vec!["SELECT a FROM t /*M!100500 , b */"]),
            ("SELECT 1 /*M! , 2 */", vec!["SELECT 1 /*M! , 2 */"]),
            ("/*M! SET @a=1 */;", vec!["/*M! SET @a=1 */"]),
            ("/*M!100500 SET @a=1 */", vec!["/*M!100500 SET @a=1 */"]),
            ("/*M! SET @a=1 */ SELECT 1;", vec!["/*M! SET @a=1 */ SELECT 1"]),
            ("/*M!100500 SET @a=1 */ SELECT 1", vec!["/*M!100500 SET @a=1 */ SELECT 1"]),
            ("UPDATE t SET v=0 /*! , w=1 */;", vec!["UPDATE t SET v=0 /*! , w=1 */"]),
            ("SELECT 1 /*!80000 , 2 */", vec!["SELECT 1 /*!80000 , 2 */"]),
            ("/*! SET @a=1 */;", vec!["/*! SET @a=1 */"]),
            ("/*!80000 SET @a=1 */", vec!["/*!80000 SET @a=1 */"]),
            ("/*! SET @a=1 */ SELECT 1;", vec!["/*! SET @a=1 */ SELECT 1"]),
            ("/*!80000 SET @a=1 */ SELECT 1", vec!["/*!80000 SET @a=1 */ SELECT 1"]),
            (
                "UPDATE t SET v=0 /* why */ /*M! , w=1 */ -- note\n; SELECT 2 /*M! , 3 */",
                vec!["UPDATE t SET v=0 /* why */ /*M! , w=1 */", "SELECT 2 /*M! , 3 */"],
            ),
            (" /* a */ /*M! SET @a=1 */ ; /*M! SET @b=2 */", vec!["/*M! SET @a=1 */", "/*M! SET @b=2 */"]),
            // Blanks and plain comments around a statement are trimmed as before.
            ("  /* a */ SELECT 1 /* b */ -- c\n;", vec!["SELECT 1"]),
            ("/* only */ ; -- line\n", vec![]),
            ("/*m! lower case is a plain comment */ SELECT 1 /*+ hint */", vec!["SELECT 1"]),
        ] {
            assert_eq!(bodies_in(src, d), want, "{d:?}: {src:?}");
        }
    }
}

/// A client command line (`DELIMITER`) after an executable comment that no `;` ended ends the
/// statement the comment is: the statements and their text are those of the same script with a
/// plain comment there, and the executable comments besides.
#[test]
fn executable_comments_before_a_client_command_keep_the_statements_apart() {
    use crate::sql::dialect::MySqlMode;
    let script = "/*M! SET @a = 1 */\nDELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; END //\n\
                  /*M!100500 SET @b = 2 */\nDELIMITER ;\nCALL p();\n/*M! SET @c = 3 */ DELIMITER $$\nSELECT 4$$";
    let plain = script.replace("/*M!", "/*");
    let mode = MySqlMode::default();
    for d in [Dialect::MySql(mode), Dialect::MariaDb(mode)] {
        assert_eq!(
            bodies_in(script, d),
            [
                "/*M! SET @a = 1 */",
                "CREATE PROCEDURE p() BEGIN SELECT 1; END",
                "/*M!100500 SET @b = 2 */",
                "CALL p()",
                "/*M! SET @c = 3 */",
                "SELECT 4",
            ],
            "{d:?}"
        );
        assert_eq!(bodies_in(&plain, d), ["CREATE PROCEDURE p() BEGIN SELECT 1; END", "CALL p()", "SELECT 4"]);
    }
}
