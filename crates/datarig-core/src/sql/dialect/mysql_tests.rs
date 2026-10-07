//! MySQL's dialect, and the PostgreSQL assumptions that would silently misbehave on MySQL text,
//! each with its own test (`danger_NN_…`): MySQL text is read as MySQL reads it, and the same
//! text in PostgreSQL is read as before.

use super::*;
use crate::driver::keys::{FromTable, single_table};
use crate::export::{Column, Kind, Target, UpdateTarget, sql_insert, sql_update};
use crate::sql::complete::{Catalog, ColumnInfo, Relation, complete_in_dialect};
use crate::sql::format::{KeywordCase, Options, Refused, format_in};
use crate::sql::lexer::{Tok, changes_schema_in, lex_in};
use crate::sql::risk::{Class, Classifier, Danger};
use crate::sql::split::{split, split_in};

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });
const ANSI: Dialect =
    Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: false, dollar_quotes: false });
const NBE: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: true, dollar_quotes: false });
const PG: Dialect = Dialect::Postgres;

fn kinds(src: &str, d: Dialect) -> Vec<(Tok, &str)> {
    lex_in(src, d).into_iter().filter(|t| !t.is_trivia()).map(|t| (t.kind, t.text(src))).collect()
}

fn bodies(src: &str, d: Dialect) -> Vec<&str> {
    split_in(src, d).iter().map(|s| s.body(src)).collect()
}

fn catalog() -> Catalog {
    let col = |name: &str| ColumnInfo { name: name.to_string(), type_name: "int".to_string() };
    Catalog {
        schemas: vec!["shop".to_string(), "public".to_string()],
        relations: vec![
            Relation {
                schema: "shop".to_string(),
                name: "Users".to_string(),
                is_view: false,
                columns: vec![col("Id"), col("Status"), col("order"), col("plain")],
            },
            Relation {
                schema: "shop".to_string(),
                name: "users".to_string(),
                is_view: false,
                columns: vec![col("lower_only")],
            },
            Relation {
                schema: "public".to_string(),
                name: "Users".to_string(),
                is_view: false,
                columns: vec![col("pg_col")],
            },
        ],
    }
}

fn labels(src: &str, d: Dialect, path: &[String]) -> Vec<String> {
    let cursor = src.find('|').expect("a cursor");
    let text = src.replace('|', "");
    complete_in_dialect(&text, cursor, &catalog(), true, path, d)
        .map(|c| c.items.into_iter().map(|i| i.label).collect())
        .unwrap_or_default()
}

#[test]
fn mysql_quotes_names_and_strings() {
    let d = MY;
    assert_eq!(d.ident_quote(), '`');
    assert_eq!(d.ident_quotes(), &['`']);
    assert_eq!(ANSI.ident_quotes(), &['`', '"']);
    for (name, sql) in [
        ("users", "users"),
        ("Users", "Users"),
        ("status", "status"),
        ("order", "`order`"),
        ("Select", "`Select`"),
        ("rank", "`rank`"),
        ("a b", "`a b`"),
        ("a`b", "`a``b`"),
        ("1col", "`1col`"),
        ("", "``"),
        ("caf\u{e9}", "`caf\u{e9}`"),
        ("_binary", "`_binary`"),
        ("_UTF8MB4", "`_UTF8MB4`"),
        ("_utf8", "`_utf8`"),
        ("_utf8mb4x", "_utf8mb4x"),
        ("_id", "_id"),
    ] {
        assert_eq!(d.quote_ident(name), sql, "{name:?}");
        assert_eq!(d.unquote(&d.force_quote_ident(name)).as_deref(), Some(name), "{name:?}");
    }
    assert_eq!(d.fold("Shop"), "Shop");
    assert_eq!(d.quote_literal("plain"), "'plain'");
    assert_eq!(NBE.quote_literal("a\\b'c"), "'a\\b''c'");
}

/// A name being typed loses its opening quote at either end, never another quote it holds.
#[test]
fn unquoting_takes_only_the_quote_it_opened_with() {
    assert_eq!(MY.unquote_lenient("`open"), "open");
    assert_eq!(MY.unquote_lenient("`a``b`"), "a`b");
    assert_eq!(ANSI.unquote_lenient("`say \"hi\""), "say \"hi\"");
    assert_eq!(ANSI.unquote_lenient("\"say `hi`"), "say `hi`");
    assert_eq!(ANSI.unquote("\"a\"\"b\"").as_deref(), Some("a\"b"));
    // PostgreSQL as before.
    assert_eq!(PG.unquote_lenient("\"a\"\"\""), "a");
}

#[test]
fn mysql_tools() {
    assert_eq!(MY.default_path(Some("shop")), ["shop"]);
    assert!(MY.default_path(None).is_empty());
    assert_eq!(MY.comment_marker(), "--");
    for (text, mark) in [("-- x", Some("--")), ("--", Some("--")), ("--\tx", Some("--")), ("# x", Some("#"))] {
        assert_eq!(MY.line_comment_at(text), mark, "{text:?}");
    }
    assert_eq!(MY.line_comment_at("--x"), None, "minus minus x");
    assert_eq!(MY.line_comment_at("x -- y"), None);
    assert!(matches!(MY.sqlformat_dialect(), sqlformat::Dialect::Generic));
    assert_eq!(MY.explain_sql("SELECT 1", false), None, "the plan view does not read MySQL's plans yet");
    assert!(MY.keywords().windows(2).all(|w| w[0] < w[1]), "sorted");
    for w in ["SELECT", "LIMIT", "STRAIGHT_JOIN", "DUPLICATE", "REGEXP", "AUTO_INCREMENT"] {
        assert!(MY.is_keyword(w), "{w}");
    }
    for w in ["ILIKE", "RETURNING", "status", "date", "text", "DELIMITER"] {
        assert!(!MY.is_keyword(w), "{w}");
    }
    assert_eq!(Language::Sql(MY).dialect(), MY);
}

/// 1. `"x"` is a string in MySQL: never written for a name, and read as a string. Completion
///    writes a name that needs quotes in backticks, so `WHERE "Id" = 0` cannot happen.
#[test]
fn danger_01_double_quotes_are_strings_and_names_get_backticks() {
    assert_eq!(kinds("UPDATE t SET x = 1 WHERE \"Id\" = 0", MY)[7], (Tok::Str, "\"Id\""));
    assert_eq!(kinds("UPDATE t SET x = 1 WHERE \"Id\" = 0", ANSI)[7], (Tok::QuotedIdent, "\"Id\""));
    assert_eq!(kinds("UPDATE t SET x = 1 WHERE \"Id\" = 0", PG)[7], (Tok::QuotedIdent, "\"Id\""));
    let path = MY.default_path(Some("shop"));
    assert_eq!(labels("SELECT u.| FROM Users u", MY, &path), ["Id", "Status", "`order`", "plain"]);
    assert_eq!(labels("SELECT u.or| FROM Users u", MY, &path), ["`order`"]);
    // After an opening backtick every name is quoted, and the quote is the one typed.
    assert_eq!(labels("SELECT u.`St| FROM Users u", MY, &path), ["`Status`"]);
    // PostgreSQL quotes the mixed-case names in double quotes, as before.
    assert_eq!(
        labels("SELECT u.| FROM shop.\"Users\" u", PG, &PG.default_path(Some("shop"))),
        ["\"Id\"", "\"Status\"", "\"order\"", "plain"]
    );
}

/// 2. `/*! … */` and `/*!NNNNN … */` are code: their content is tokens, a statement that is only
///    one is a statement (never dropped as a comment), and nothing reads it as harmless.
#[test]
fn danger_02_executable_comments_are_code() {
    let src = "DELETE FROM t WHERE id = 1 /*! OR 1=1 */";
    let k = kinds(src, MY);
    assert!(k.contains(&(Tok::ExecComment, "/*!")) && k.contains(&(Tok::Keyword, "OR")), "{k:?}");
    assert_eq!(
        bodies("/*! DROP TABLE t */;\n/*!50001 DROP VIEW v */;", MY),
        ["/*! DROP TABLE t */", "/*!50001 DROP VIEW v */"]
    );
    assert!(changes_schema_in("/*!50001 DROP VIEW v */", MY));
    // The classifier of MySQL text (until it reads MySQL) asks about everything, and a
    // read-only policy refuses it.
    let risk = Classifier::new(Language::Sql(MY)).classify(src);
    assert_eq!((risk.class, risk.danger), (Class::Unknown, Some(Danger::Unparsed)));
    assert!(risk.read_only().is_err());
    // PostgreSQL: a comment, as before.
    assert_eq!(bodies("/*! DROP TABLE t */;", PG), Vec::<&str>::new());
}

/// 3. Copying rows as SQL escapes backslashes (and NULs) for MySQL: `C:\new` stays `C:\new`.
#[test]
fn danger_03_copied_strings_keep_their_backslashes() {
    assert_eq!(MY.quote_literal("C:\\new"), "'C:\\\\new'");
    assert_eq!(MY.quote_literal("{\"a\": \"q\\\"\"}"), "'{\"a\": \"q\\\\\"\"}'");
    assert_eq!(MY.quote_literal("it's\0"), "'it''s\\0'");
    let cols = [Column { name: "path", kind: Kind::Text }, Column { name: "Order", kind: Kind::Number }];
    let target = Target::Table { schema: "shop", name: "files", columns: vec!["path", "Order"], overriding: true };
    assert_eq!(
        sql_insert(MY, &target, &cols, &[vec![Some("C:\\new"), Some("1")]]),
        "INSERT INTO `shop`.`files` (`path`, `Order`) VALUES ('C:\\\\new', 1);"
    );
    let update = UpdateTarget { schema: "shop", name: "files", set: vec![(0, "path")], keys: vec![(1, "Order")] };
    assert_eq!(
        sql_update(MY, &update, &cols, &[vec![Some("a\\b"), Some("2")]]),
        "UPDATE `shop`.`files` SET `path` = 'a\\\\b' WHERE `Order` = 2;"
    );
    // The lexer reads the written literal back as one string: the text it was made from.
    for s in ["C:\\new", "\\", "'\\'", "a\0b", "\\\\'"] {
        let lit = MY.quote_literal(s);
        assert_eq!(kinds(&lit, MY), [(Tok::Str, lit.as_str())], "{s:?}");
    }
    // PostgreSQL: a backslash is an ordinary character there, as before.
    assert_eq!(PG.quote_literal("C:\\new"), "'C:\\new'");
}

/// 4. `#` comments and backslash-escaped quotes split statements where MySQL does (its client,
///    which reads a backslash as an escape in `"…"` under `ANSI_QUOTES` too).
#[test]
fn danger_04_hash_comments_and_backslash_quotes_split_as_mysql_does() {
    assert_eq!(bodies("SELECT \"a\\\"; b\"; SELECT 2", ANSI), ["SELECT \"a\\\"; b\"", "SELECT 2"]);
    assert_eq!(bodies("SELECT 1; # don't run this; DELETE FROM t\nSELECT 2;", MY), ["SELECT 1", "SELECT 2"]);
    assert_eq!(bodies("SELECT 'it\\'s; fine'; SELECT 2", MY), ["SELECT 'it\\'s; fine'", "SELECT 2"]);
    assert_eq!(bodies("SELECT \"a\\\"; b\"; SELECT 2", MY), ["SELECT \"a\\\"; b\"", "SELECT 2"]);
    assert_eq!(bodies("SELECT 1 -- x;\n; SELECT 1--1;", MY), ["SELECT 1", "SELECT 1--1"]);
    assert_eq!(bodies("SELECT 'it\\'s; fine'; SELECT 2", NBE), ["SELECT 'it\\'s", "fine'; SELECT 2"]);
    // PostgreSQL, as before: `#` is an operator, a backslash an ordinary character.
    assert_eq!(bodies("SELECT 'it\\'s; fine'; SELECT 2", PG), ["SELECT 'it\\'s", "fine'; SELECT 2"]);
}

/// 5. `DELIMITER` sets what ends a statement, the line itself is never sent, and `$$` is no
///    dollar quote.
#[test]
fn danger_05_delimiter_blocks_split_as_the_client_does() {
    let src =
        "DELIMITER $$\nCREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND$$\nDELIMITER ;\nCALL p();\nSELECT 3";
    assert_eq!(bodies(src, MY), ["CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND", "CALL p()", "SELECT 3"]);
    let k = kinds(src, MY);
    assert_eq!(k[0], (Tok::Directive, "DELIMITER $$"));
    assert!(!bodies(src, MY).iter().any(|b| b.to_ascii_uppercase().contains("DELIMITER")));
    // In PostgreSQL `$$` opens a dollar quote and the `DELIMITER` line is sent, as before.
    assert!(split(src)[0].body(src).starts_with("DELIMITER $$\nCREATE"));
}

/// 6. MySQL text is never classified by PostgreSQL's parser: until MySQL's classifier exists,
///    every statement of it asks, a read-only policy refuses it, and nothing is run again.
#[test]
fn danger_06_mysql_text_is_never_read_by_the_postgres_classifier() {
    let mut c = Classifier::new(Language::Sql(MY));
    assert_eq!(c.language(), Language::Sql(MY));
    for sql in ["SELECT `a` FROM t LIMIT 1, 2", "SHOW FULL PROCESSLIST", "SET @x = 1", "SELECT 1"] {
        let r = c.classify(sql);
        assert_eq!((r.class, r.danger), (Class::Unknown, Some(Danger::Unparsed)), "{sql}");
        assert!(r.confirm(false).is_some() && r.read_only().is_err(), "{sql}");
        assert!(c.repeatable(sql).is_err() && c.count_query(sql).is_err(), "{sql}");
    }
    // PostgreSQL's classifier, as before.
    assert_eq!(Classifier::new(Language::Sql(PG)).classify("SELECT 1").class, Class::Read);
}

/// 7. MySQL settings are not judged by PostgreSQL's names: `SET sql_log_bin = 0` asks.
#[test]
fn danger_07_mysql_settings_are_not_judged_by_postgres_names() {
    for sql in ["SET sql_log_bin = 0", "SET foreign_key_checks = 0", "SET autocommit = 0", "SET a.b = 1"] {
        let r = Classifier::classify_once(Language::Sql(MY), sql);
        assert!(!r.safe_setting && r.danger == Some(Danger::Unparsed), "{sql}");
    }
    // PostgreSQL reads a dotted name as a custom setting, as before.
    assert!(Classifier::classify_once(Language::Sql(PG), "SET a.b = 1").safe_setting);
}

/// 8. Unqualified names resolve in the session's database, never in `public`.
#[test]
fn danger_08_unqualified_names_resolve_in_the_current_database() {
    let shop = MY.default_path(Some("shop"));
    assert_eq!(labels("SELECT | FROM Users", MY, &shop)[..4], ["Id", "Status", "`order`", "plain"]);
    // Without a database, `public` is not looked in first.
    assert!(!MY.default_path(None).contains(&"public".to_string()));
    // PostgreSQL's search path, as before.
    assert_eq!(labels("SELECT pg| FROM \"Users\"", PG, &PG.default_path(None)), ["pg_col"]);
}

/// 9. Names keep their case: `Users` and `users` are different tables on a server with
///    `lower_case_table_names = 0`.
#[test]
fn danger_09_names_keep_their_case() {
    let shop = MY.default_path(Some("shop"));
    assert_eq!(single_table(MY, "SELECT * FROM Users"), Ok(FromTable { schema: None, name: "Users".into() }));
    assert_eq!(
        single_table(MY, "SELECT * FROM `shop`.`Users` u WHERE u.Id = 1"),
        Ok(FromTable { schema: Some("shop".into()), name: "Users".into() })
    );
    assert_eq!(labels("SELECT lower| FROM users", MY, &shop), ["lower_only"]);
    // PostgreSQL folds a bare name, as before.
    assert_eq!(single_table(PG, "SELECT * FROM Users"), Ok(FromTable { schema: None, name: "users".into() }));
}

/// 10. The table a copy targets is read from backtick names (and `"…"` is a string, no table).
#[test]
fn danger_10_copy_targets_read_backtick_names() {
    assert_eq!(
        single_table(MY, "SELECT `a``b`.x FROM `my db`.`a``b` WHERE 1;"),
        Ok(FromTable { schema: Some("my db".into()), name: "a`b".into() })
    );
    assert!(single_table(MY, "SELECT * FROM \"t\"").is_err());
    assert_eq!(single_table(ANSI, "SELECT * FROM \"T\""), Ok(FromTable { schema: None, name: "T".into() }));
    assert_eq!(single_table(PG, "SELECT * FROM \"T\""), Ok(FromTable { schema: None, name: "T".into() }));
}

/// 11. `RENAME TABLE` and `TRUNCATE` change the schema in MySQL (the cached keys are read
///     again); its `DO` evaluates expressions.
#[test]
fn danger_11_mysql_schema_changes() {
    for sql in ["RENAME TABLE a TO b", "truncate t", "/*!40101 ALTER TABLE t ADD c INT */", "CALL p()", "DROP TABLE t"]
    {
        assert!(changes_schema_in(sql, MY), "{sql}");
    }
    for sql in ["DO SLEEP(1)", "SELECT 1", "-- CREATE\nSELECT 1"] {
        assert!(!changes_schema_in(sql, MY), "{sql}");
    }
    // PostgreSQL, as before.
    assert!(changes_schema_in("DO $$ BEGIN END $$", PG) && !changes_schema_in("RENAME TABLE a TO b", PG));
}

/// 12. The formatter's guard holds on MySQL text: a `#` comment holding a quote, backticks,
///     executable comments and backslash strings come back as they were (only the layout
///     changes) or the text is refused; a function's name stays next to its `(` (`count (*)`
///     is read otherwise); a `DELIMITER` line is always refused.
#[test]
fn danger_12_the_formatter_keeps_mysql_tokens() {
    let opts = Options { case: KeywordCase::Upper, indent: 4 };
    // Each text, and whether it is laid out (else refused).
    for (src, laid_out) in [
        ("select a # it's a comment\n, b from t", true),
        ("select `a b`, 'it\\'s', \"dq\" from `t` where x = 1", true),
        ("select * from t where a--1 > 0", true),
        ("select first, status from t", true),
        ("select 1 /*!80000 + 1 */, @x, @@session.sql_mode, ? from dual", false),
        ("select x'41', _utf8mb4'u', 0x1F, t.1e5 from t", false),
        // `sqlformat` writes `group_concat (a)`: refused.
        ("select group_concat(a), count(*), substring(b, 1) from t group by c", false),
        // A blank after other words before `(` is fine (`IN (`, `IF (`, `VALUES (`).
        ("select a from t where a in(1,2) and if(a,1,2) = 1", true),
        ("insert into t(a) values(1)", true),
        ("select count(*), max(a) from t", true),
    ] {
        let formatted = format_in(src, opts, "", MY);
        assert_eq!(formatted.is_ok(), laid_out, "{src:?} => {formatted:?}");
        match formatted {
            Ok((span, out)) => {
                let a: Vec<(Tok, String)> = lex_in(&src[span], MY)
                    .into_iter()
                    .filter(|t| t.kind != Tok::Whitespace)
                    .map(|t| (t.kind, t.text(src).to_ascii_uppercase()))
                    .collect();
                let b: Vec<(Tok, String)> = lex_in(&out, MY)
                    .into_iter()
                    .filter(|t| t.kind != Tok::Whitespace)
                    .map(|t| (t.kind, t.text(&out).to_ascii_uppercase()))
                    .collect();
                assert_eq!(a, b, "{src:?} => {out:?}");
                // Non-reserved keywords may be names: their case is kept.
                if src.contains("first") {
                    assert!(out.contains("first") && out.contains("status") && out.contains("SELECT"), "{out}");
                }
            }
            Err(Refused::Changed { .. }) => {}
        }
    }
    let src = "DELIMITER //\nselect 1//";
    assert_eq!(format_in(src, opts, "", MY), Err(Refused::Changed { at: 0 }));
}

/// Completion in MySQL text: none inside its strings (`"…"`, an escaped quote does not close
/// one), variables or a terminator; the segment is the one the terminator in effect makes.
#[test]
fn mysql_completion_reads_mysql_text() {
    let shop = MY.default_path(Some("shop"));
    assert!(labels("SELECT \"St| FROM Users", MY, &shop).is_empty());
    assert!(labels("SELECT 'a\\' St| FROM Users", MY, &shop).is_empty());
    assert!(labels("SELECT @St| FROM Users", MY, &shop).is_empty());
    assert!(labels("SELECT 1 # St|\nFROM Users", MY, &shop).is_empty());
    assert_eq!(labels("SELECT 'a\\'' AS x, St| FROM Users", MY, &shop), ["Status", "STRAIGHT_JOIN"]);
    // `STRAIGHT_JOIN` introduces a table.
    assert_eq!(labels("SELECT * FROM Users a STRAIGHT_JOIN us|", MY, &shop)[..2], ["Users", "users"]);
    // From a state with `//` as the terminator: the `;` inside a body does not end the segment.
    let mut state = crate::sql::lexer::LexState::default();
    let head = "DELIMITER //\n";
    for t in lex_in(head, MY) {
        state = state.after(&t, head, MY);
    }
    let text = "SELECT u.Id; SELECT St FROM Users u//SELECT 1//";
    let cursor = text.find("St ").expect("St") + 2;
    let c = crate::sql::complete::complete_from(text, cursor, &catalog(), false, &shop, MY, state).expect("items");
    assert_eq!(c.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Status", "STRAIGHT_JOIN"]);
    // Between the characters of a terminator, nothing.
    let at = text.find("//").expect("//") + 1;
    assert!(crate::sql::complete::complete_from(text, at, &catalog(), true, &shop, MY, state).is_none());
}

/// `EXPLAIN` and its MySQL synonyms; no JSON rewrite of MySQL's plans yet.
#[test]
fn mysql_explain_statements() {
    use crate::sql::plan::{explain, is_explain, is_explain_in};
    for sql in ["EXPLAIN SELECT 1", "describe t", "DESC SELECT 1", "/*!80000 EXPLAIN */ SELECT 1", "# c\nexplain x"] {
        assert!(is_explain_in(sql, MY), "{sql}");
    }
    assert!(!is_explain_in("SELECT 'EXPLAIN'", MY));
    assert!(!is_explain("DESC t") && is_explain("EXPLAIN SELECT 1"), "PostgreSQL as before");
    assert_eq!(explain::json_text_in("EXPLAIN SELECT 1", MY), Err(explain::NotJson::Unreadable));
    assert_eq!(explain::json_in("EXPLAIN SELECT 1", MY), Err(explain::NotJson::Unreadable));
    assert_eq!(explain::json_text_in("EXPLAIN SELECT 1", PG), explain::json_text("EXPLAIN SELECT 1"));
}

/// A name in MySQL is looked for first in the session's database, in its own case, then in any
/// case; only then in other databases.
#[test]
fn mysql_names_resolve_in_the_database_first() {
    let col = |n: &str| ColumnInfo { name: n.into(), type_name: String::new() };
    let rel = |schema: &str, name: &str, c: &str| Relation {
        schema: schema.into(),
        name: name.into(),
        is_view: false,
        columns: vec![col(c)],
    };
    let cat = Catalog {
        schemas: vec!["other".into(), "shop".into()],
        relations: vec![rel("other", "Users", "other_col"), rel("shop", "users", "shop_col")],
    };
    let shop = MY.default_path(Some("shop"));
    let got = complete_in_dialect("SELECT  FROM Users", 7, &cat, true, &shop, MY).expect("items");
    assert_eq!(got.items[0].label, "shop_col");
    let got = complete_in_dialect("SELECT  FROM Users", 7, &cat, true, &[], MY).expect("items");
    assert_eq!(got.items[0].label, "other_col", "no database: the name as written");
}

/// The segment around a cursor inside a terminator of more than one character, or a
/// `DELIMITER` line, is the one before it.
#[test]
fn a_cursor_inside_a_terminator_is_in_the_segment_before_it() {
    use crate::sql::split::segment_at_in;
    let src = "DELIMITER $$\nselect 1$$ select 2$$";
    let at = src.find("$$ ").expect("$$") + 1;
    let (a, b) = segment_at_in(src, at, MY);
    assert!(a <= at && &src[a..b] == "\nselect 1", "{:?}", &src[a..b]);
    let (a, b) = segment_at_in(src, 3, MY);
    assert_eq!((a, b), (0, 0));
}

/// A backslash at a line's end (dropped by the client) is never dropped by the formatter
/// without a word: the text is refused.
#[test]
fn the_formatter_refuses_a_backslash_at_a_line_end() {
    let opts = Options { case: KeywordCase::Upper, indent: 2 };
    assert!(format_in("select 1 \\\nfrom t", opts, "", MY).is_err());
    assert!(format_in("select 1\nfrom t", opts, "", MY).is_ok());
}
