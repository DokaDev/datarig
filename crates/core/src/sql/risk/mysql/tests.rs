use super::*;
use crate::sql::risk::{Classifier, ReadOnlyBlock, TxStep};

include!("corpus.rs");

const MODE: MySqlMode = MySqlMode {
    ansi_quotes: false,
    no_backslash_escapes: false,
    dollar_quotes: DollarQuotes::NotRead,
    ignore_space: false,
};
/// MySQL's judge in the default mode.
const MYSQL: Judge = Judge::mysql(MODE);
/// MariaDB's judge in the default mode.
const MARIADB: Judge = Judge::mariadb(MODE);

fn risk(sql: &str) -> Risk {
    classify(sql, MYSQL)
}

/// A risk in the words of the corpus (see `corpus.rs`).
fn render(r: &Risk) -> String {
    let class = match r.class {
        Class::Read => "read",
        Class::Session => "session",
        Class::Tx => "tx",
        Class::Write => "write",
        Class::Ddl => "ddl",
        Class::Maintenance => "maint",
        Class::Procedural => "proc",
        Class::Unknown => "unknown",
    };
    let mut out = vec![class.to_string()];
    if let Some(d) = r.danger {
        let name = match d {
            Danger::Drop => "drop",
            Danger::Truncate => "truncate",
            Danger::DeleteAll => "delete-all",
            Danger::UpdateAll => "update-all",
            Danger::DropColumn => "drop-column",
            Danger::AlterColumnType => "alter-type",
            Danger::Merge => "merge",
            Danger::Procedural => "procedural",
            Danger::CopyProgram => "copy-program",
            Danger::CopyFile => "copy-file",
            Danger::ServerFile => "server-file",
            Danger::ServerAction => "server-action",
            Danger::RunsQueryText => "runs-query-text",
            Danger::UnknownPrepared => "unknown-prepared",
            Danger::Unparsed => "unparsed",
            Danger::TooComplex => "too-complex",
            Danger::ExecutableComment => "exec-comment",
            Danger::Unrecognized => "unrecognized",
            Danger::Locks => "locks",
            Danger::Privileges => "privileges",
            Danger::Rename => "rename",
            Danger::AnalyzeRunsCode => "analyze-code",
            Danger::Setting => "setting",
            Danger::DynamicSql => "dynamic-sql",
            Danger::ServerCommand => "server-command",
            Danger::FileAccess => "file-access",
        };
        let how = match r.no_where {
            Some(NoWhere::Missing) => ":missing",
            Some(NoWhere::AlwaysTrue) => ":always",
            Some(NoWhere::NoColumn) => ":nocol",
            Some(NoWhere::OtherTables) => ":others",
            None => "",
        };
        out.push(format!("{name}{how}"));
    }
    let flags = [
        (r.writes, "writes"),
        (r.explain == Explain::Plan, "plan"),
        (r.explain == Explain::Analyze, "analyze"),
        (r.read_write, "rw"),
        (r.implicit_commit, "commit"),
        (r.runs_code, "code"),
        (r.unchecked_call.is_some(), "udf"),
        (r.read_only().is_ok(), "ro"),
    ];
    out.extend(flags.iter().filter(|(on, _)| *on).map(|(_, f)| f.to_string()));
    out.join(" ")
}

#[test]
fn the_corpus_reads_as_expected() {
    assert!(CORPUS.len() >= 400, "{} statements", CORPUS.len());
    let wrong: Vec<String> = CORPUS
        .iter()
        .filter_map(|(sql, want)| {
            let want: Vec<&str> = want.split(' ').filter(|w| !matches!(*w, "norun" | "srvok")).collect();
            let want = want.join(" ");
            let got = render(&risk(sql));
            (got != want).then(|| format!("{sql:?}\n    want {want}\n    got  {got}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {}:\n{}", wrong.len(), CORPUS.len(), wrong.join("\n"));
}

#[test]
fn the_corpus_holds_no_statement_twice() {
    let mut seen = std::collections::HashSet::new();
    for (sql, _) in CORPUS {
        assert!(seen.insert(*sql), "{sql:?} twice");
    }
}

#[test]
fn the_classifier_is_the_one_of_mysql_sessions() {
    let lang = crate::sql::dialect::Language::Sql(Dialect::MySql(MODE));
    let mut c = Classifier::new(lang);
    assert_eq!(c, Classifier::MySql(MODE, Judged::Own));
    assert_eq!(c.language(), lang);
    for (sql, _) in CORPUS.iter().take(50) {
        assert_eq!(c.classify(sql), risk(sql), "{sql}");
        assert_eq!(Classifier::classify_once(lang, sql), risk(sql), "{sql}");
    }
    // Prepared statements are not remembered: an EXECUTE never inherits a PREPARE.
    c.classify("PREPARE s FROM 'SELECT 1'");
    assert!(!c.knows("s"));
    assert_eq!(c.classify("EXECUTE s").danger, Some(Danger::UnknownPrepared));
    c.forget("PREPARE s FROM 'SELECT 1'");
    assert_eq!(c, Classifier::MySql(MODE, Judged::Own));
}

#[test]
fn every_dangerous_statement_asks_and_read_only_refuses_what_is_not_a_read() {
    for (sql, _) in CORPUS {
        let r = risk(sql);
        if r.danger.is_some() {
            assert!(r.confirm(false).is_some(), "{sql}");
        }
        if r.read_only().is_ok() {
            assert!(matches!(r.class, Class::Read | Class::Session | Class::Tx), "{sql}");
            assert!(r.danger.is_none() && !r.read_write && r.unchecked_call.is_none(), "{sql}");
            assert!(r.class != Class::Session || r.safe_setting, "{sql}");
        }
    }
}

#[test]
fn read_only_says_why_it_refuses() {
    let why = |sql: &str| risk(sql).read_only().err();
    assert_eq!(why("SELECT /*! 1 */"), Some(ReadOnlyBlock::ExecutableComment));
    assert_eq!(why("SELEC 1"), Some(ReadOnlyBlock::Unrecognized));
    assert_eq!(why("SELECT sys_exec('x')"), Some(ReadOnlyBlock::UnknownFunction("sys_exec".into())));
    assert_eq!(why("SELECT shop.`Udf`(1)"), Some(ReadOnlyBlock::UnknownFunction("shop.`Udf`".into())));
    assert_eq!(why("PREPARE s FROM 'SELECT 1'"), Some(ReadOnlyBlock::DynamicSql));
    assert_eq!(why("KILL 1"), Some(ReadOnlyBlock::ServerCommand));
    assert_eq!(why("XA START 'x'"), Some(ReadOnlyBlock::ServerCommand));
    assert_eq!(why("SELECT LOAD_FILE('/x')"), Some(ReadOnlyBlock::ServerFile));
    assert_eq!(why("SELECT GET_LOCK('x', 1)"), Some(ReadOnlyBlock::ServerAction));
    assert_eq!(why("SET SESSION TRANSACTION READ WRITE"), Some(ReadOnlyBlock::ReadWrite));
    assert_eq!(why("SET timestamp = 1"), Some(ReadOnlyBlock::Setting));
    assert_eq!(why("CREATE TEMPORARY TABLE t (a INT)"), Some(ReadOnlyBlock::Class(Class::Ddl)));
    assert_eq!(why("INSERT INTO t VALUES (1)"), Some(ReadOnlyBlock::Class(Class::Write)));
    assert_eq!(why("SELECT * FROM t FOR SHARE"), Some(ReadOnlyBlock::Class(Class::Write)));
    assert_eq!(why("LOCK TABLES t READ"), Some(ReadOnlyBlock::Locks));
    assert_eq!(why("HANDLER t OPEN"), Some(ReadOnlyBlock::Locks));
    assert_eq!(why("SELECT * FROM t INTO OUTFILE '/x'"), Some(ReadOnlyBlock::FileAccess));
    assert_eq!(why("SET sql_log_bin = 0"), Some(ReadOnlyBlock::Setting));
    // Whatever the class, these dangers are refused by themselves.
    for d in [Danger::Locks, Danger::FileAccess, Danger::Setting, Danger::ServerCommand, Danger::DynamicSql] {
        for class in [Class::Read, Class::Session, Class::Tx] {
            let r = Risk { safe_setting: true, ..Risk::danger(class, d) };
            assert!(r.read_only().is_err(), "{d:?} {class:?}");
        }
    }
    // Several statements: the danger a read-only policy refuses by itself is kept.
    assert_eq!(risk("DELETE FROM t; SET GLOBAL x = 1").danger, Some(Danger::ServerCommand));
    assert_eq!(risk("DROP TABLE t; SELECT GET_LOCK('x', 1)").danger, Some(Danger::ServerAction));
    assert_eq!(risk("SET sql_log_bin = 0; SET GLOBAL x = 1").danger, Some(Danger::Setting));
    assert_eq!(why("CALL p()"), Some(ReadOnlyBlock::Class(Class::Procedural)));
    assert_eq!(why("EXECUTE s"), Some(ReadOnlyBlock::Class(Class::Unknown)));
}

#[test]
fn targets_are_the_names_as_written() {
    let target = |sql: &str| risk(sql).target;
    assert_eq!(target("DELETE FROM `shop`.`Users` WHERE id = 1").as_deref(), Some("`shop`.`Users`"));
    assert_eq!(target("DELETE t1 FROM t1 JOIN t2 ON t1.id = t2.id").as_deref(), Some("t1"));
    assert_eq!(target("UPDATE LOW_PRIORITY shop.users SET a = 1").as_deref(), Some("shop.users"));
    assert_eq!(target("INSERT IGNORE INTO t (a) VALUES (1)").as_deref(), Some("t"));
    assert_eq!(target("REPLACE t VALUES (1)").as_deref(), Some("t"));
    assert_eq!(target("DROP TABLE IF EXISTS s.t").as_deref(), Some("s.t"));
    assert_eq!(target("DROP TEMPORARY TABLE t").as_deref(), Some("t"));
    assert_eq!(target("TRUNCATE TABLE t").as_deref(), Some("t"));
    assert_eq!(target("RENAME TABLE a TO b").as_deref(), Some("a"));
    assert_eq!(target("ALTER TABLE s.t DROP COLUMN c").as_deref(), Some("s.t"));
    assert_eq!(target("CREATE TABLE IF NOT EXISTS t2 AS SELECT 1").as_deref(), Some("t2"));
    assert_eq!(target("LOAD DATA INFILE 'x' INTO TABLE t").as_deref(), Some("t"));
    assert_eq!(target("WITH c AS (SELECT 1) DELETE FROM t").as_deref(), Some("t"));
    assert_eq!(target("SELECT 1"), None);
}

#[test]
fn the_sql_mode_changes_what_is_a_string() {
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    let raw = MySqlMode { no_backslash_escapes: true, ..MODE };
    // A quoted name under ANSI_QUOTES, a string otherwise: both reads.
    assert_eq!(render(&classify("SELECT \"a\" FROM t", Judge::mysql(ansi))), "read ro");
    assert_eq!(render(&classify("SELECT \"a\" FROM t", MYSQL)), "read ro");
    // The server reads no escape in a quoted name: `"a\"` ends there.
    assert_eq!(render(&classify("SELECT \"a\\\" FROM t; DELETE FROM t\"", Judge::mysql(ansi))), "unknown unrecognized");
    // Without backslash escapes, `'\'` is a whole string and what follows is code. Whatever
    // mode the app was told, the text is also read in the other: the worse reading counts.
    let text = "SELECT '\\'; DELETE FROM t; -- '";
    assert_eq!(render(&classify(text, Judge::mysql(raw))), "write delete-all:missing writes");
    assert_eq!(render(&classify(text, MYSQL)), "write delete-all:missing writes");
    let text = "SELECT * FROM t WHERE b = 'x\\' OR 1 = 1; DELETE FROM t -- '";
    assert_eq!(classify(text, Judge::mysql(raw)).danger, Some(Danger::DeleteAll));
    assert_eq!(classify(text, MYSQL).danger, Some(Danger::DeleteAll));
    // Another reading the classifier does not know counts: only a syntax error does not.
    assert_eq!(render(&classify("SELECT 'a\\' , (DELETE FROM t) #'", MYSQL)), "unknown unrecognized");
    assert_eq!(render(&classify("SELECT 'a\\' , ? #'", MYSQL)), "unknown unrecognized");
    assert_eq!(repeatable("SELECT 'a\\' , (DELETE FROM t) #'", MYSQL), Err(NotRepeatable::Unreadable));
    // A quoted name with a backslash under ANSI_QUOTES: the lexer would end it elsewhere than
    // the server, so the reading is not known (and is not taken for a syntax error).
    assert_eq!(render(&classify("SELECT \"a\\\"", Judge::mysql(ansi))), "unknown unrecognized");
    assert_eq!(classify_as("SELECT \"a\\\"", Judge::mysql(ansi)).map(|r| r.danger), Some(Some(Danger::Unrecognized)));
    let r = classify("SELECT 1 FROM t WHERE \"x\\\" = 1; DELETE FROM t", Judge::mysql(ansi));
    assert!(r.danger == Some(Danger::Unrecognized) && r.writes && r.read_only().is_err());
    // A reading the server would refuse (an unterminated string) does not count.
    assert_eq!(render(&classify("SELECT 'it\\'s'", MYSQL)), "read ro");
    assert_eq!(render(&classify("SELECT \"it\"\"s\" FROM t", Judge::mysql(ansi))), "read ro");
    // Dollar quotes only where the server reads them, and where the driver may be wrong about
    // that, both readings count (a server that reads them refuses such a `SELECT` anyway).
    let dollar = MySqlMode { dollar_quotes: DollarQuotes::Read, ..MODE };
    assert_eq!(
        classify_as("SELECT $$a; DROP TABLE t$$", Judge::mysql(dollar)).map(|r| render(&r)),
        Some("read ro".into())
    );
    assert_eq!(render(&classify("SELECT $$a; DROP TABLE t$$", Judge::mysql(dollar))), "ddl drop commit");
    assert_eq!(render(&classify("SELECT $$a; DROP TABLE t$$", MYSQL)), "ddl drop commit");
}

#[test]
fn a_hint_with_a_semicolon_or_a_write_stays_a_comment() {
    assert_eq!(render(&risk("SELECT /*+ ; DELETE FROM t */ 1")), "read ro");
    // A hint that turns a check off or read-only off for its statement asks.
    assert_eq!(render(&risk("SELECT /*+ SET_VAR(transaction_read_only = OFF) */ 1")), "read setting");
    assert_eq!(
        render(&risk("DELETE /*+ SET_VAR(foreign_key_checks=0) */ FROM t WHERE id = 1")),
        "write setting writes"
    );
    assert_eq!(render(&risk("SELECT /*+ SET_VAR(sort_buffer_size = 1M) */ 1")), "read ro");
    // A quoted name is not read: it asks as a risky one does.
    for sql in [
        "SELECT /*+ SET_VAR(`sql_mode` = '') */ 1",
        "SELECT /*+ SET_VAR( `foreign_key_checks` = 0) */ 1",
        "SELECT /*+ SET_VAR(\"sql_mode\" = '') */ 1",
    ] {
        assert_eq!(render(&risk(sql)), "read setting", "{sql}");
    }
}

#[test]
fn compound_statements_are_one_statement() {
    let text = "CREATE PROCEDURE p()\nBEGIN\n  DELETE FROM t;\n  DROP TABLE u;\nEND";
    assert_eq!(render(&risk(text)), "ddl commit");
    // Any other text with a `;` is several statements.
    assert_eq!(render(&risk("CREATE TABLE t (a INT); DROP TABLE u")), "ddl drop commit");
}

#[test]
fn over_the_caps_is_too_complex_without_recursing() {
    let deep = format!("SELECT {}1{}", "(".repeat(100_000), ")".repeat(100_000));
    assert_eq!(risk(&deep).danger, Some(Danger::TooComplex));
    let ok = format!("SELECT {}1{}", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
    assert_eq!(render(&risk(&ok)), "read ro");
    let long = format!("SELECT '{}'", "x".repeat(MAX_BYTES));
    assert_eq!(risk(&long).danger, Some(Danger::TooComplex));
    // A deep condition is read without a deep recursion.
    let cond = format!("DELETE FROM t WHERE {}a = 1{}", "(a = 1 OR ".repeat(10_000), ")".repeat(10_000));
    assert_eq!(render(&risk(&cond)), "write writes");
    let cond = format!("UPDATE t SET a = 1 WHERE {}1 = 1{}", "(".repeat(10_000), ")".repeat(10_000));
    assert_eq!(risk(&cond).danger, Some(Danger::UpdateAll));
}

#[test]
fn classifying_is_fast() {
    // About 10 KB and 256 KB of ordinary statements.
    let unit = "SELECT a.id, b.name, COUNT(*) FROM orders a JOIN users b ON b.id = a.user_id WHERE a.created > NOW() - INTERVAL 1 DAY AND b.name LIKE 'x%' GROUP BY a.id, b.name ORDER BY 3 DESC;\n";
    for (bytes, budget_ms) in [(10 * 1024, 2.0 * 50.0), (MAX_BYTES - 1024, 20.0 * 50.0)] {
        let text = unit.repeat(bytes / unit.len());
        let start = std::time::Instant::now();
        let r = risk(&text);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(render(&r), "read ro");
        // Generous for a debug build; the release budget is checked by the bench.
        assert!(ms < budget_ms, "{bytes} bytes took {ms:.1} ms");
    }
}

#[test]
fn words_before_a_parenthesis_that_are_no_call_are_reserved() {
    for w in NOT_CALLS {
        assert!(crate::sql::ident::mysql::RESERVED.contains(&w.to_ascii_lowercase().as_str()), "{w}");
    }
    // A non-reserved word may name a loadable function: before `(` it is a call, unless its place
    // makes it syntax.
    for sql in ["SELECT text('x')", "SELECT hash(1)", "SELECT list(1)", "SELECT a FROM t WHERE signed(a) = 1"] {
        assert!(risk(sql).unchecked_call.is_some(), "{sql}");
    }
    for sql in [
        "SELECT * FROM t WHERE a = ANY (SELECT 1)",
        "SELECT * FROM t WHERE a > SOME (SELECT 1)",
        "SELECT MATCH (a) AGAINST ('x') FROM t",
        "SELECT * FROM JSON_TABLE('[1]', '$[*]' COLUMNS (x INT PATH '$')) AS j",
        "SELECT CAST(a AS SIGNED), CAST(a AS NCHAR(2)) FROM t",
    ] {
        assert!(risk(sql).unchecked_call.is_none(), "{sql}");
    }
}

#[test]
fn a_routine_ends_where_its_body_does() {
    let text = "CREATE PROCEDURE p() BEGIN\n  DECLARE x INT DEFAULT REPEAT('a', 2);\n  IF x THEN SET x = CASE WHEN 1 THEN 2 END; END IF;\n  l: LOOP LEAVE l; END LOOP l;\n  WHILE x DO SET x = 0; END WHILE;\n  REPEAT SET x = 1; UNTIL x END REPEAT;\n  CASE x WHEN 1 THEN SELECT 1; ELSE BEGIN END; END CASE;\nEND";
    assert_eq!(render(&risk(text)), "ddl commit");
    assert_eq!(render(&risk(&format!("{text}; DELETE FROM t"))), "ddl delete-all:missing writes commit");
    assert_eq!(render(&risk("CREATE PROCEDURE p() SELECT 1; DROP TABLE t")), "ddl drop commit");
    assert_eq!(
        render(&risk("CREATE TRIGGER tr BEFORE INSERT ON t FOR EACH ROW SET NEW.a = 1; SET GLOBAL x = 1")),
        "maint server-command commit"
    );
    assert_eq!(
        render(&risk("CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO DELETE FROM t; SELECT 1")),
        "ddl delete-all:missing commit"
    );
    // Blocks that cannot be followed: what follows is not read, and it asks.
    assert_eq!(render(&risk("CREATE PROCEDURE p() BEGIN END END; DROP TABLE t")), "ddl unrecognized commit");
}

#[test]
fn nested_events_are_not_read_into() {
    let text = format!("{}SELECT 1", "CREATE EVENT e ON SCHEDULE AT CURRENT_TIMESTAMP DO ".repeat(5000));
    assert!(text.len() < MAX_BYTES);
    let start = std::time::Instant::now();
    assert_eq!(render(&risk(&text)), "ddl unrecognized commit");
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
}

#[test]
fn an_event_body_of_table_ddl_is_read() {
    assert_eq!(
        render(&risk("CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO ALTER TABLE t ENGINE = BLACKHOLE")),
        "ddl truncate commit"
    );
    assert_eq!(render(&risk("CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO CREATE TABLE t2 (a INT)")), "ddl commit");
    assert_eq!(
        render(&risk(
            "CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO CREATE DEFINER = CURRENT_USER EVENT f ON SCHEDULE EVERY 1 DAY DO DELETE FROM t"
        )),
        "ddl unrecognized commit"
    );
}

#[test]
fn a_routine_whose_blocks_cannot_be_followed_asks() {
    // `end` as a name closes the block early: what follows reads as other statements.
    assert_eq!(
        render(&risk("CREATE PROCEDURE p() BEGIN SELECT end FROM t; END; DROP TABLE t")),
        "unknown unrecognized commit"
    );
    assert_eq!(render(&risk("CREATE PROCEDURE p() BEGIN SELECT 1;")), "ddl unrecognized commit");
    assert_eq!(render(&risk("CREATE PROCEDURE p() BEGIN SELECT 1; END")), "ddl commit");
}

#[test]
fn deep_nesting_is_read_in_linear_time() {
    let calls = format!("SELECT {}1{}", "abs(".repeat(MAX_DEPTH - 1), ")".repeat(MAX_DEPTH - 1));
    let points = format!("SELECT {}1{}", "POINT(1, ".repeat(MAX_DEPTH - 1), ")".repeat(MAX_DEPTH - 1));
    let cond = format!("DELETE FROM t WHERE {}a = 1{}", "(".repeat(MAX_DEPTH - 1), ")".repeat(MAX_DEPTH - 1));
    let ctes = format!(
        "WITH {} SELECT 1",
        (0..5000).map(|i| format!("c{i} (x) AS (SELECT 1)")).collect::<Vec<_>>().join(", ")
    );
    for (text, judge) in [(&calls, MYSQL), (&cond, MYSQL), (&ctes, MYSQL), (&points, MARIADB)] {
        let start = std::time::Instant::now();
        let r = classify(text, judge);
        assert!(r.danger.is_none() || r.danger == Some(Danger::DeleteAll), "{:?}", r.danger);
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
    }
}

/// Every prefix and suffix of every corpus text, in every sql mode, read without the thread
/// guard (which would turn a panic into a verdict): no panic.
#[test]
fn no_text_makes_the_classifier_panic() {
    let modes = [
        MODE,
        MySqlMode {
            ansi_quotes: true,
            no_backslash_escapes: true,
            dollar_quotes: DollarQuotes::Read,
            ignore_space: false,
        },
    ];
    let mut texts: Vec<&str> = CORPUS.iter().map(|(sql, _)| *sql).collect();
    texts.extend([
        "SET CHARACTER",
        "SET x = 1, CHARACTER",
        "SET @\u{e9} x = 1",
        "SET @@",
        "SET @ = 1",
        "SET =",
        "SET NAMES",
    ]);
    for text in texts {
        let cuts: Vec<usize> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
        for &i in &cuts {
            for part in [&text[..i], &text[i..]] {
                for mode in modes {
                    let _ = classify_as(part, Judge::mysql(mode));
                    let _ = names_as(part, Judge::mysql(mode));
                    let _ = without_semicolons(part, Judge::mysql(mode));
                }
            }
        }
    }
}

#[test]
fn the_built_in_lists_are_sorted_upper_case_and_known() {
    let lines: Vec<&str> = BUILTINS.lines().collect();
    assert!(lines.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    assert!(lines.iter().all(|l| l.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')));
    for name in SERVER_FILES.iter().chain(SERVER_ACTIONS).chain(VOLATILE) {
        assert!(builtin(name), "{name}");
    }
    for list in [SAFE_SETTINGS, RISKY_SETTINGS] {
        assert!(list.windows(2).all(|w| w[0] < w[1]));
    }
    assert!(SAFE_SETTINGS.iter().all(|s| !RISKY_SETTINGS.contains(s) && !READ_ONLY_SETTINGS.contains(s)));
    for op in ["AND", "OR", "XOR", "IN", "IS", "LIKE", "REGEXP", "DIV", "EXISTS"] {
        assert!(!builtin(op), "{op} is an operator");
    }
}

#[test]
fn the_names_mariadb_does_not_provide_are_sorted_built_ins() {
    let lines: Vec<&str> = NOT_ON_MARIADB.lines().collect();
    assert!(lines.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    for name in &lines {
        assert!(builtin(name), "{name}");
    }
    // What the read-only check refuses by name stays refused on MariaDB too.
    for name in SERVER_FILES.iter().chain(SERVER_ACTIONS) {
        assert!(builtin_in(name, MARIADB), "{name}");
    }
}

/// On MariaDB an unqualified call of a function MariaDB does not provide names a stored
/// function of the current database: it is an unchecked call there, as any function that is
/// not a built-in, and stays a built-in on MySQL.
#[test]
fn functions_mariadb_does_not_provide_are_unchecked_calls_there() {
    // Without arguments, as the server is asked about each (a geometry constructor given
    // arguments may be MariaDB's own: see the test after this one).
    for name in NOT_ON_MARIADB.lines() {
        let sql = format!("SELECT {name}() FROM t");
        let maria = classify(&sql, MARIADB);
        assert!(maria.unchecked_call.is_some() && maria.runs_code, "{sql}");
        if NEWER_THAN_OLDEST.contains(&name) {
            continue;
        }
        let mysql = classify(&sql, MYSQL);
        assert!(mysql.unchecked_call.is_none() && !mysql.runs_code && mysql.read_only().is_ok(), "{sql}");
        let maria = classify(&sql, MARIADB);
        assert!(maria.unchecked_call.is_some() && maria.runs_code, "{sql}");
        assert_eq!(maria.read_only(), Err(ReadOnlyBlock::UnknownFunction(name.to_string())), "{sql}");
    }
    let quoted = classify("SELECT `ANY_VALUE`(a) FROM t GROUP BY b", MARIADB);
    assert!(quoted.unchecked_call.is_some() && quoted.read_only().is_err());
    let r = classify("DO REGEXP_LIKE('a', 'b')", MARIADB);
    assert_eq!((r.class, r.danger), (Class::Procedural, Some(Danger::Procedural)));
    assert_eq!(render(&classify("DO REGEXP_LIKE('a', 'b')", MYSQL)), "read ro");
    let r = classify("EXPLAIN ANALYZE SELECT ANY_VALUE(a) FROM t", MARIADB);
    assert_eq!(r.danger, Some(Danger::AnalyzeRunsCode));
    assert_eq!(repeatable("SELECT BIN_TO_UUID(a) FROM t", MARIADB), Err(NotRepeatable::UserFunction));
    assert_eq!(repeatable("SELECT BIN_TO_UUID(a) FROM t", MYSQL), Ok(()));
    // An event's body counts as on the server it is created on.
    let event = "CREATE EVENT e ON SCHEDULE AT CURRENT_TIMESTAMP DO SELECT ANY_VALUE(a) FROM t";
    assert_eq!(classify(event, MARIADB).danger, classify(event, MYSQL).danger);
    // MariaDB's own functions stay built-ins there.
    for sql in ["SELECT ABS(a), JSON_EXTRACT(j, '$.a'), REGEXP_SUBSTR(b, 'x') FROM t", "SELECT GROUP_CONCAT(b) FROM t"]
    {
        let r = classify(sql, MARIADB);
        assert!(r.unchecked_call.is_none() && r.read_only().is_ok(), "{sql}");
    }
    assert_eq!(classify("SELECT GET_LOCK('a', 0)", MARIADB).danger, Some(Danger::ServerAction));
}

/// A built-in the grammar reads itself is one only written as the grammar reads it: with a
/// blank or a comment before its `(` ([`tight_calls`]), or quoted (those and the words of
/// [`MYSQL_WORD_CALLS`]), the server looks the name up as a stored function of the current
/// database. Other built-ins are found by their name, however they are written.
#[test]
fn built_ins_count_only_as_the_server_reads_them() {
    let unchecked = |sql: &str, judge: Judge, name: &str| {
        let r = classify(sql, judge);
        assert!(r.runs_code, "{sql}");
        assert_eq!(r.read_only(), Err(ReadOnlyBlock::UnknownFunction(name.to_string())), "{sql}");
    };
    let built_in = |sql: &str, judge: Judge| {
        let r = classify(sql, judge);
        assert!(r.unchecked_call.is_none() && !r.runs_code && r.read_only().is_ok(), "{sql}");
    };
    for mode in [MYSQL, MARIADB] {
        unchecked("SELECT now ()", mode, "now");
        unchecked("SELECT NOW\n()", mode, "NOW");
        unchecked("SELECT now\t()", mode, "now");
        unchecked("SELECT now/**/()", mode, "now");
        unchecked("SELECT now -- c\n()", mode, "now");
        unchecked("SELECT now # c\n()", mode, "now");
        unchecked("SELECT `now`()", mode, "`now`");
        unchecked("SELECT `now` ()", mode, "`now`");
        unchecked("SELECT COUNT (*) FROM t", mode, "COUNT");
        unchecked("SELECT a FROM t WHERE b = TRIM ('x')", mode, "TRIM");
        unchecked("SELECT `if`(a, 1, 2) FROM t", mode, "`if`");
        unchecked("DO now ()", mode, "now");
        built_in("SELECT now(), COUNT(*), TRIM('x'), DATABASE(), IF(a, 1, 2) FROM t", mode);
        built_in("SELECT DATABASE (), CURRENT_USER (), LEFT ('ab', 1), abs (1), `abs`(1), `ABS` (1)", mode);
        built_in("SELECT sum(a) FROM t", mode);
    }
    // Each server's own lists.
    unchecked("SELECT `coalesce`(a, 1) FROM t", MYSQL, "`coalesce`");
    unchecked("SELECT `DATABASE`()", MYSQL, "`DATABASE`");
    built_in("SELECT `DATABASE`()", MARIADB);
    built_in("SELECT `coalesce`(a, 1) FROM t", MARIADB);
    unchecked("SELECT rank () OVER (ORDER BY a) FROM t", MARIADB, "rank");
    built_in("SELECT rank () OVER (ORDER BY a), row_number () OVER (ORDER BY a) FROM t", MYSQL);
    unchecked("SELECT `sysdate`()", MARIADB, "`sysdate`");
    unchecked("SELECT sysdate ()", MYSQL, "sysdate");
    built_in("SELECT sysdate ()", MARIADB);
    // MariaDB reserves neither `LATERAL` nor `OF`: before `(` they name a stored function.
    unchecked("SELECT lateral()", MARIADB, "lateral");
    unchecked("SELECT a FROM t WHERE of(a) = 1", MARIADB, "of");
    // MariaDB's grammar reads `JSON_TABLE` only as the table function with its columns.
    let table = "SELECT * FROM JSON_TABLE('[1]', '$[*]' COLUMNS (a INT PATH '$')) AS jt";
    built_in(table, MARIADB);
    built_in(table, MYSQL);
    unchecked("SELECT JSON_TABLE(1)", MARIADB, "JSON_TABLE");
    unchecked("SELECT `JSON_TABLE`('[1]', '$[*]' COLUMNS (a INT PATH '$'))", MARIADB, "`JSON_TABLE`");
    // Only the real clause counts: the path, then `COLUMNS (`. A column or a call named
    // `columns` in the arguments is no clause.
    unchecked("SELECT JSON_TABLE(t.columns) FROM t", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE(columns) FROM t", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE(a, columns) FROM t", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE(columns(1))", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE(a, f(1) COLUMNS (x INT PATH '$'))", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE('[1]', '$' `COLUMNS` (a INT PATH '$'))", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE('[1]', '$' COLUMNS)", MARIADB, "JSON_TABLE");
    unchecked("SELECT JSON_TABLE('[1]', ('$' COLUMNS (a INT PATH '$')))", MARIADB, "JSON_TABLE");
    // A column's type with its precision in `JSON_TABLE`'s columns is no call.
    let typed = "SELECT * FROM JSON_TABLE('[1]', '$[*]' COLUMNS (d DATETIME(6) PATH '$', n NCHAR(10) PATH '$', \
                 NESTED PATH '$' COLUMNS (x DECIMAL(10, 2) PATH '$'))) AS jt";
    built_in(typed, MYSQL);
    built_in(typed, MARIADB);
    // MariaDB's geometry constructors are its own quoted too, given their arguments.
    built_in("SELECT `POINT`(1, 2), `LINESTRING`(POINT(0, 0), POINT(1, 1))", MARIADB);
    // Quoted under `ANSI_QUOTES` too.
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    unchecked("SELECT \"now\"()", Judge::mysql(ansi), "\"now\"");
    // With `IGNORE_SPACE` blanks are fine, comments and quotes are not.
    let ignore = MySqlMode { ignore_space: true, ..MODE };
    for ignore in [Judge::mysql(ignore), Judge::mariadb(ignore)] {
        built_in("SELECT now (), NOW\n(), COUNT (*) FROM t", ignore);
        unchecked("SELECT now/**/()", ignore, "now");
        unchecked("SELECT now -- c\n()", ignore, "now");
        unchecked("SELECT `now`()", ignore, "`now`");
    }
    // A call of a looked-up name is code the read check cannot see.
    assert_eq!(classify("EXPLAIN ANALYZE SELECT now ()", MYSQL).danger, Some(Danger::AnalyzeRunsCode));
    assert_eq!(repeatable("SELECT now () FROM t", MYSQL), Err(NotRepeatable::UserFunction));
}

/// Whether the server reads dollar quotes is told from its version, which a proxy may report
/// otherwise: a text with a `$` is read both ways, and the worse reading counts.
#[test]
fn a_dollar_is_read_with_and_without_dollar_quotes() {
    let sql = "SELECT 1 $a$, GET_LOCK('x', 10), 2 $a$";
    for dollar_quotes in [DollarQuotes::NotRead, DollarQuotes::Read] {
        let mode = MySqlMode { dollar_quotes, ..MODE };
        assert_eq!(classify(sql, Judge::mysql(mode)).danger, Some(Danger::ServerAction), "{dollar_quotes:?}");
        let r = classify("SELECT 1 $a$, my_fn(), 2 $a$", Judge::mysql(mode));
        assert!(r.unchecked_call.is_some() && r.read_only().is_err(), "{dollar_quotes:?}");
    }
    // A plain `$` in a name or a string reads the same both ways.
    for dollar_quotes in [DollarQuotes::NotRead, DollarQuotes::Read] {
        let mode = MySqlMode { dollar_quotes, ..MODE };
        assert_eq!(render(&classify("SELECT a$b, 'x$y$' FROM t", Judge::mysql(mode))), "read ro");
    }
}

/// A built-in only newer servers have is a stored function's name on the oldest: a call of
/// one is an unchecked call on every server.
#[test]
fn built_ins_newer_than_the_oldest_server_are_unchecked_calls() {
    for name in NEWER_THAN_OLDEST {
        assert!(builtin(name), "{name}");
        for mode in [MYSQL, MARIADB] {
            let sql = format!("SELECT {name}(a) FROM t");
            assert_eq!(classify(&sql, mode).read_only(), Err(ReadOnlyBlock::UnknownFunction(name.to_string())));
        }
    }
}

#[test]
fn the_lists_of_names_the_grammar_reads_are_sorted_built_ins() {
    for list in [MYSQL_TIGHT_CALLS, MARIADB_TIGHT_CALLS, MYSQL_WORD_CALLS, MARIADB_WORD_CALLS] {
        assert!(list.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
        assert!(list.iter().all(|n| builtin(n)), "built-ins");
    }
    for (tight, words) in [(MYSQL_TIGHT_CALLS, MYSQL_WORD_CALLS), (MARIADB_TIGHT_CALLS, MARIADB_WORD_CALLS)] {
        assert!(tight.iter().all(|n| !words.contains(n)), "in one list only");
    }
}

/// MariaDB's grammar reads its geometry constructors only with some numbers of arguments: so
/// written they are built-ins there, otherwise the name is a stored function's.
#[test]
fn mariadb_geometry_constructors_are_built_ins_by_their_arguments() {
    for sql in [
        "SELECT POINT(1, 2)",
        "SELECT `POINT`(ABS(a), COALESCE(b, 2)) FROM t",
        "SELECT LINESTRING(POINT(1, 1), POINT(2, 2))",
        "SELECT POLYGON(LINESTRING(POINT(0, 0), POINT(1, 0), POINT(0, 1), POINT(0, 0)))",
        "SELECT MULTIPOINT(POINT(1, 1)), MULTILINESTRING(g), MULTIPOLYGON(g), GEOMETRYCOLLECTION(g) FROM t",
    ] {
        let r = classify(sql, MARIADB);
        assert!(r.unchecked_call.is_none() && r.read_only().is_ok(), "{sql}");
    }
    for sql in [
        "SELECT POINT()",
        "SELECT POINT(1)",
        "SELECT POINT(COALESCE(1, 2))",
        "SELECT POINT(1, 2, 3)",
        "SELECT LINESTRING()",
        "SELECT GEOMETRYCOLLECTION()",
        "SELECT GEOMCOLLECTION(POINT(1, 1))",
    ] {
        let r = classify(sql, MARIADB);
        assert!(r.unchecked_call.is_some() && r.read_only().is_err(), "{sql}");
        assert!(classify(sql, MYSQL).unchecked_call.is_none(), "{sql}");
    }
    for (name, _, _) in MARIADB_CONSTRUCTORS {
        assert!(NOT_ON_MARIADB.lines().any(|n| n == *name), "{name}");
    }
}

#[test]
fn repeatable_is_the_plain_query_allowlist() {
    let ok = [
        "SELECT * FROM t",
        "SELECT a, b FROM t WHERE a > 1 ORDER BY a",
        "SELECT * FROM t;",
        "TABLE t",
        "VALUES ROW(1, 2)",
        "(SELECT 1) UNION (SELECT 2)",
        "WITH c AS (SELECT 1 AS n) SELECT * FROM c",
        "SELECT NOW(), CONCAT(a, 'x'), JSON_EXTRACT(j, '$.a') FROM t",
        "SELECT * FROM t # trailing comment",
        "SELECT zz_classify.f() FROM t",
    ];
    for sql in &ok[..ok.len() - 1] {
        assert_eq!(repeatable(sql, MYSQL), Ok(()), "{sql}");
    }
    let no = [
        ("SELECT 1; SELECT 2", NotRepeatable::NotOne),
        ("", NotRepeatable::NotOne),
        ("SHOW TABLES", NotRepeatable::NotSelect),
        ("EXPLAIN SELECT 1", NotRepeatable::NotSelect),
        ("DELETE FROM t", NotRepeatable::NotSelect),
        ("DO 1", NotRepeatable::NotSelect),
        ("WITH c AS (SELECT 1) DELETE FROM t", NotRepeatable::NotSelect),
        ("SELECT * FROM t FOR UPDATE", NotRepeatable::Writes),
        ("SELECT * FROM t LOCK IN SHARE MODE", NotRepeatable::Writes),
        ("SELECT 1 INTO @x", NotRepeatable::Writes),
        ("SELECT * FROM t INTO OUTFILE '/x'", NotRepeatable::Writes),
        ("SELECT LOAD_FILE('/x')", NotRepeatable::Writes),
        ("SELECT GET_LOCK('x', 1)", NotRepeatable::Writes),
        ("SELECT f()", NotRepeatable::UserFunction),
        ("SELECT zz_classify.f() FROM t", NotRepeatable::UserFunction),
        ("SELECT RAND()", NotRepeatable::Volatile("rand".into())),
        ("SELECT * FROM t ORDER BY rand()", NotRepeatable::Volatile("rand".into())),
        ("SELECT UUID()", NotRepeatable::Volatile("uuid".into())),
        ("SELECT SLEEP(1)", NotRepeatable::Volatile("sleep".into())),
        ("SELECT SYSDATE()", NotRepeatable::Volatile("sysdate".into())),
        ("SELECT * FROM t WHERE id = @x", NotRepeatable::Variable("@x".into())),
        ("SELECT @@sql_mode", NotRepeatable::Variable("@@sql_mode".into())),
        ("SELECT @x := 1", NotRepeatable::Variable("@x".into())),
        ("SELECT /*! 1 */", NotRepeatable::Writes),
        ("SELECT * FROM performance_schema.threads", NotRepeatable::NotATable("performance_schema.threads".into())),
        (
            "SELECT * FROM `information_schema`.`TABLES`",
            NotRepeatable::NotATable("`information_schema`.`TABLES`".into()),
        ),
        ("SELECT ANY_VALUE(a) FROM t GROUP BY b", NotRepeatable::Volatile("any_value".into())),
        ("SELECT /*+ SET_VAR(sql_mode = '') */ 1", NotRepeatable::Writes),
        // Read in another sql mode, the text calls RAND().
        ("SELECT 'a\\' , RAND() #'", NotRepeatable::Volatile("rand".into())),
        ("SELEC 1", NotRepeatable::NotSelect),
        ("SELECT (1", NotRepeatable::Unreadable),
    ];
    for (sql, why) in no {
        assert_eq!(repeatable(sql, MYSQL), Err(why), "{sql}");
    }
    assert_eq!(
        names("SELECT a, t.b FROM `shop`.`t` AS t JOIN u ON NOW() = u.c", MYSQL).unwrap(),
        ["a", "t.b", "`shop`.`t`", "t", "u", "u.c"]
    );
}

#[test]
fn repeatable_takes_the_session_zone_only_as_a_timestamp_filter_writes_it() {
    // As the table filter writes a TIMESTAMP's instant: the same instant in any zone.
    let ok = [
        "SELECT * FROM `shop`.`orders` WHERE `placed_at` = \
         CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)",
        "SELECT * FROM t WHERE NOT (placed_at <=> CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)) \
         ORDER BY id",
        "SELECT * FROM t WHERE placed_at = CONVERT_TZ(_utf8mb4 X'323032362D31312D31302030393A33303A3030', \
         '+00:00', @@session.time_zone)",
        "SELECT * FROM t WHERE placed_at IN (CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@SESSION.TIME_ZONE), \
         convert_tz('2026-11-10 10:30:00', '+00:00', @@time_zone))",
        "SELECT COUNT(*) FROM (\nSELECT * FROM t WHERE placed_at = \
         CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)\n) AS datarig_count",
    ];
    for sql in ok {
        assert_eq!(repeatable(sql, MYSQL), Ok(()), "{sql}");
        assert!(reads_zone(sql, MYSQL), "{sql}");
        assert!(count_query(sql, MYSQL).is_ok(), "{sql}");
    }
    let var = |v: &str| Err(NotRepeatable::Variable(v.into()));
    let no = [
        // Any other variable, also next to the zone or inside CONVERT_TZ.
        ("SELECT * FROM t WHERE a = @x", var("@x")),
        ("SELECT @@global.time_zone", var("@@global.time_zone")),
        ("SELECT @@session.time_zone", var("@@session.time_zone")),
        ("SELECT * FROM t WHERE a = @@session.sql_mode", var("@@session.sql_mode")),
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.sql_mode)",
            var("@@session.sql_mode"),
        ),
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@global.time_zone)",
            var("@@global.time_zone"),
        ),
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00', @@session.time_zone, '+00:00')",
            var("@@session.time_zone"),
        ),
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone) AND b = @x",
            var("@x"),
        ),
        // Not literals as the first two arguments.
        ("SELECT * FROM t WHERE a = CONVERT_TZ(b, '+00:00', @@session.time_zone)", var("@@session.time_zone")),
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00', c, @@session.time_zone)",
            var("@@session.time_zone"),
        ),
        ("SELECT * FROM t WHERE a = CONVERT_TZ(CONCAT('2026', @x), '+00:00', @@session.time_zone)", var("@x")),
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00' '', '+00:00', @@session.time_zone)",
            var("@@session.time_zone"),
        ),
        // The zone in an expression, or another function.
        (
            "SELECT * FROM t WHERE a = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone + 0)",
            var("@@session.time_zone"),
        ),
        ("SELECT * FROM t WHERE a = CONCAT('a', 'b', @@session.time_zone)", var("@@session.time_zone")),
        // A function of the user's of that name, or one quoted.
        (
            "SELECT * FROM t WHERE a = zz.CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)",
            Err(NotRepeatable::UserFunction),
        ),
        // An assignment to it.
        ("SELECT @@session.time_zone := '+00:00'", var("@@session.time_zone")),
    ];
    for (sql, want) in no {
        assert_eq!(repeatable(sql, MYSQL), want, "{sql}");
    }
    // A string under ANSI_QUOTES is a column there: not a literal.
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    let quoted = "SELECT * FROM t WHERE a = CONVERT_TZ(\"2026-11-10 09:30:00\", '+00:00', @@session.time_zone)";
    assert_eq!(repeatable(quoted, Judge::mysql(ansi)), var("@@session.time_zone"));
    // Only a variable counts: a string or a comment that says it does not.
    assert!(!reads_zone("SELECT '@@session.time_zone' FROM t # @@time_zone", MYSQL));
    assert!(!reads_zone("SELECT * FROM t WHERE a = @x", MYSQL));
    let c = Classifier::MySql(MODE, Judged::Own);
    assert!(c.reads_zone(ok[0]));
    assert!(!Classifier::Pg(Default::default()).reads_zone(ok[0]));
}

#[test]
fn ordered_reads_the_top_level_order_by() {
    for (sql, want) in [
        ("SELECT * FROM t ORDER BY a", true),
        ("(SELECT * FROM t ORDER BY a)", true),
        ("(SELECT 1) UNION (SELECT 2) ORDER BY 1", true),
        ("TABLE t ORDER BY a", true),
        ("SELECT * FROM t", false),
        ("SELECT * FROM (SELECT * FROM t ORDER BY a) AS d", false),
        ("SELECT ROW_NUMBER() OVER (ORDER BY a) FROM t", false),
        ("SELECT GROUP_CONCAT(a ORDER BY a) FROM t", false),
        ("WITH c AS (SELECT 1 AS n ORDER BY n) SELECT * FROM c", false),
        ("SELECT 'ORDER BY'", false),
    ] {
        assert_eq!(ordered(sql, MYSQL), want, "{sql}");
    }
}

#[test]
fn count_query_wraps_a_repeatable_query() {
    assert_eq!(
        count_query("SELECT * FROM t ORDER BY a;", MYSQL).as_deref(),
        Ok("SELECT COUNT(*) FROM (\nSELECT * FROM t ORDER BY a\n) AS datarig_count")
    );
    assert_eq!(
        count_query("SELECT * FROM t # note", MYSQL).as_deref(),
        Ok("SELECT COUNT(*) FROM (\nSELECT * FROM t # note\n) AS datarig_count")
    );
    assert_eq!(count_query("TABLE t", MYSQL).as_deref(), Ok("SELECT COUNT(*) FROM (\nTABLE t\n) AS datarig_count"));
    assert_eq!(count_query("SELECT RAND()", MYSQL), Err(NotRepeatable::Volatile("rand".into())));
    assert_eq!(count_query("DELETE FROM t", MYSQL), Err(NotRepeatable::NotSelect));
    // A trailing comment that would swallow the closing parenthesis on one line.
    assert!(count_query("SELECT 1 -- x", MYSQL).unwrap().contains("-- x\n)"));
}

/// Statements a read-only policy must refuse, whatever is written around their words.
const NOT_READS: &[&str] = &[
    "DELETE FROM t WHERE id = 1",
    "DELETE FROM t",
    "UPDATE t SET a = 1 WHERE id = 1",
    "INSERT INTO t (a) VALUES (1)",
    "REPLACE INTO t VALUES (1)",
    "WITH c AS (SELECT 1) DELETE FROM t",
    "DROP TABLE t",
    "TRUNCATE TABLE t",
    "ALTER TABLE t DROP COLUMN a",
    "CREATE TABLE t2 AS SELECT * FROM t",
    "CREATE TEMPORARY TABLE t3 (a INT)",
    "RENAME TABLE t TO t2",
    "GRANT SELECT ON db.* TO u",
    "SELECT * FROM t FOR UPDATE",
    "SELECT * FROM t LOCK IN SHARE MODE",
    "SELECT * FROM t INTO OUTFILE '/tmp/x'",
    "SELECT LOAD_FILE('/etc/passwd')",
    "SELECT GET_LOCK('x', 1)",
    "SELECT sys_exec('id')",
    "SELECT db.f()",
    "SET @x = (SELECT a FROM t WHERE id = 1 FOR SHARE)",
    "SET GLOBAL max_connections = 1",
    "SET SESSION TRANSACTION READ WRITE",
    "SET transaction_read_only = OFF",
    "SET NAMES utf8mb4, transaction_read_only = 0",
    "SET sql_log_bin = 0",
    "START TRANSACTION READ WRITE",
    "KILL 1",
    "FLUSH TABLES WITH READ LOCK",
    "LOCK TABLES t READ",
    "HANDLER t OPEN",
    "LOAD DATA INFILE '/tmp/x' INTO TABLE t",
    "CALL p()",
    "DO db.f()",
    "PREPARE s FROM 'DELETE FROM t'",
    "EXECUTE s",
    "EXPLAIN ANALYZE DELETE FROM t",
    "XA COMMIT 'x'",
    "SELECT 1; DELETE FROM t",
    "SHOW TABLES; DROP TABLE t",
    // Strings with backslashes and double quotes, read differently in the other sql modes.
    "SELECT 'a\\' , GET_LOCK('k', 0) #'",
    "SELECT \"a\\\" , LOAD_FILE('/x') #\"",
    "SELECT 'it\\'s', \"q\" FROM t; DELETE FROM t",
    "UPDATE t SET b = 'x\\'y', c = \"z\" WHERE 1 = 1",
];

/// A small deterministic random source (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// `sql` with random changes that keep what the server runs: blanks and comments between tokens,
/// the case of words, backticks around names, a statement or comment before it; or that make it
/// code the classifier must not read (an executable comment around a token).
fn mutate(sql: &str, rng: &mut Rng) -> String {
    let tokens = crate::sql::lexer::lex_in(sql, Dialect::MySql(MODE));
    let mut parts: Vec<String> = tokens.iter().map(|t| t.text(sql).to_string()).collect();
    // What goes before each token, kept apart so a later change to the token leaves it outside.
    let mut before = vec![String::new(); parts.len()];
    for _ in 0..1 + rng.below(4) {
        let i = rng.below(parts.len());
        let kind = tokens[i].kind;
        // A token not yet wrapped in an executable comment (its words are still its own).
        let plain = !parts[i].starts_with("/*");
        match rng.below(6) {
            0 | 1 => {
                // Between two tokens, unless both are operators (`<` `=` would become `< =`).
                let glued = i > 0 && tokens[i - 1].kind == Tok::Op && kind == Tok::Op;
                if !glued {
                    let filler = rng.pick(&[
                        " ",
                        "\t",
                        "\n",
                        "\r\n",
                        "\u{b}",
                        "\u{c}",
                        "/* x */",
                        "/**/",
                        "/*+ NO_ICP(t) */",
                        "-- x\n",
                        "#x\n",
                        " -- DELETE\n",
                        "/* ; DROP */",
                    ]);
                    before[i].push_str(filler);
                }
            }
            2 if plain && matches!(kind, Tok::Keyword | Tok::Ident) => {
                parts[i] = parts[i]
                    .chars()
                    .map(|c| if rng.below(2) == 0 { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() })
                    .collect();
            }
            // A keyword in backticks is a name: the server refuses the text, the classifier must too.
            3 if plain && matches!(kind, Tok::Ident | Tok::Keyword) && !parts[i].contains('`') => {
                parts[i] = format!("`{}`", parts[i])
            }
            4 => {
                let open = rng.pick(&["/*!", "/*!50000 ", "/*!80000", "/*!99999 ", "/*M!", "/*M!100000 "]);
                parts[i] = format!("{open}{} */", parts[i]);
            }
            _ => {
                let prefix = rng.pick(&["SELECT 1;\n", "/* lead */ ", "\n\n", "# lead\n", "-- lead\n"]);
                before[0].insert_str(0, prefix);
            }
        }
    }
    before.iter().zip(&parts).map(|(b, p)| format!("{b}{p}")).collect()
}

/// Thousands of rewritings of statements a read-only policy must refuse, in every sql mode:
/// none is let through as a read, and none of the dangerous ones stops asking.
#[test]
fn rewritten_writes_are_never_reads() {
    let modes = [MODE, MySqlMode { ansi_quotes: true, ..MODE }, MySqlMode { no_backslash_escapes: true, ..MODE }];
    let mut rng = Rng(0x5eed_da7a_c1a5_51f1);
    let mut checked = 0;
    for sql in NOT_READS {
        let base = risk(sql);
        assert!(base.read_only().is_err(), "{sql}: {}", render(&base));
        let asks = base.confirm(false).is_some();
        for _ in 0..150 {
            let text = mutate(sql, &mut rng);
            for mode in modes {
                let r = classify(&text, Judge::mysql(mode));
                assert!(r.read_only().is_err(), "let through: {text:?} in {mode:?}: {}", render(&r));
                if asks {
                    assert!(r.confirm(false).is_some(), "stopped asking: {text:?} in {mode:?}: {}", render(&r));
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 15_000);
}

/// What PostgreSQL alone refuses in a transaction block, or adds to an enum: never a MySQL
/// verdict. And on the corpus, a text of several statements opens the transaction, what commits
/// the open one never opens one, and every write or schema change that does not opens it.
#[test]
fn mysql_never_sets_the_postgres_transaction_flags() {
    let c = Classifier::new(crate::sql::dialect::Language::Sql(Dialect::MySql(MODE)));
    let extra =
        ["CREATE DATABASE zz", "DROP DATABASE zz", "OPTIMIZE TABLE t", "ALTER TABLE t ALTER COLUMN e SET DEFAULT 'x'"];
    for sql in CORPUS.iter().map(|(sql, _)| *sql).chain(extra) {
        let r = risk(sql);
        assert!(!r.no_tx_block && !r.adds_enum_value, "{sql}");
        let step = c.tx_step(sql);
        if form(sql, MYSQL).is_none() {
            assert_eq!(step, TxStep::Opens, "{sql}");
        } else if r.implicit_commit {
            assert_eq!(step, TxStep::Outside, "{sql}");
        } else if matches!(r.class, Class::Write | Class::Ddl | Class::Procedural | Class::Unknown) && !r.rolls_back() {
            assert_eq!(step, TxStep::Opens, "{sql}");
        }
    }
}

/// Every name a statement writes, for the driver's question of which tables a write measured by
/// `EXPLAIN ANALYZE` touches: a superset (columns and aliases too), in any statement.
#[test]
fn named_gives_every_name_of_every_statement() {
    let names = named("EXPLAIN ANALYZE DELETE x FROM `shop`.`t` x JOIN u ON u.id = x.id", MYSQL).unwrap();
    for n in ["`shop`.`t`", "x", "u", "u.id", "x.id"] {
        assert!(names.iter().any(|m| m == n), "{n}: {names:?}");
    }
    assert!(named("UPDATE a SET v = 1; DELETE FROM b", MYSQL).unwrap().iter().any(|n| n == "b"));
    // Every word counts (a keyword may be a table's name).
    assert_eq!(named("SELECT 1", MYSQL), Some(vec!["SELECT".to_string()]));
    // Text the session's mode cannot read gives no names: the driver counts it as not undone.
    assert_eq!(named("EXPLAIN /*!80000 ANALYZE */ DELETE FROM t", MYSQL), None);
    // A keyword is a table name unquoted.
    assert!(
        named("EXPLAIN ANALYZE DELETE FROM transaction WHERE id = 1", MYSQL)
            .unwrap()
            .iter()
            .any(|n| n == "transaction")
    );
    assert!(named("UPDATE view SET a = 1", MYSQL).unwrap().iter().any(|n| n.eq_ignore_ascii_case("view")));
}

/// `EXPLAIN ANALYZE` that calls a function that is not a built-in asks (it runs the function,
/// whose writes to a non-transactional table no rollback undoes) and read-only refuses it; a
/// built-in alone does neither.
#[test]
fn analyze_of_a_users_function_asks_and_read_only_refuses_it() {
    for sql in ["EXPLAIN ANALYZE SELECT f(1)", "EXPLAIN ANALYZE FORMAT=TREE SELECT db.f(id) FROM t"] {
        let r = risk(sql);
        assert_eq!(r.danger, Some(Danger::AnalyzeRunsCode), "{sql}");
        assert!(r.confirm(false).is_some() && r.read_only().is_err(), "{sql}");
    }
    let r = risk("EXPLAIN ANALYZE SELECT ABS(a) FROM t");
    assert!(r.confirm(false).is_none() && r.read_only().is_ok());
    // Without ANALYZE nothing runs: no question.
    assert_eq!(risk("EXPLAIN SELECT f(1)").danger, None);
}

/// Each engine's judge reads its own lists, the ones each engine always had: MySQL's has every
/// built-in, its own tight and word calls and reserves `LATERAL` and `OF`; MariaDB's lacks the
/// built-ins it does not provide, reads its constructors by their arguments, has its own tight
/// and word calls, reserves neither word and needs `JSON_TABLE`'s columns.
#[test]
fn each_engine_reads_its_own_tables() {
    let mysql = FamilyTables {
        not_builtin: "",
        constructors: &[],
        tight_calls: MYSQL_TIGHT_CALLS,
        word_calls: MYSQL_WORD_CALLS,
        unreserved_not_calls: &[],
        json_table_needs_columns: false,
    };
    let mariadb = FamilyTables {
        not_builtin: NOT_ON_MARIADB,
        constructors: MARIADB_CONSTRUCTORS,
        tight_calls: MARIADB_TIGHT_CALLS,
        word_calls: MARIADB_WORD_CALLS,
        unreserved_not_calls: MARIADB_NOT_RESERVED,
        json_table_needs_columns: true,
    };
    assert_eq!(MYSQL_TABLES, mysql);
    assert_eq!(MARIADB_TABLES, mariadb);
    assert_eq!(MYSQL.tables().collect::<Vec<_>>(), [&MYSQL_TABLES]);
    assert_eq!(MARIADB.tables().collect::<Vec<_>>(), [&MARIADB_TABLES]);
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    assert_eq!(Judge::of(Dialect::MySql(ansi)), Some(Judge::mysql(ansi)));
    assert_eq!(Judge::of(Dialect::MariaDb(ansi)), Some(Judge::mariadb(ansi)));
    assert_eq!(Judge::of(Dialect::Postgres), None);
    // What each read gives on each engine.
    assert!(builtin_in("BIN_TO_UUID", MYSQL) && !builtin_in("BIN_TO_UUID", MARIADB));
    assert!(builtin_in("NOW", MYSQL) && builtin_in("NOW", MARIADB));
    assert!(tight_call("SYSDATE", MYSQL) && !tight_call("SYSDATE", MARIADB));
    assert!(!tight_call("RANK", MYSQL) && tight_call("RANK", MARIADB));
    assert!(word_call("DATABASE", MYSQL) && !word_call("DATABASE", MARIADB));
    assert_eq!(render(&classify("SELECT POINT(1, 2)", MARIADB)), "read ro");
    assert_eq!(render(&classify("SELECT POINT()", MARIADB)), render(&classify("SELECT my_fn()", MARIADB)));
    assert_eq!(render(&classify("SELECT LATERAL(1)", MARIADB)), render(&classify("SELECT my_fn(1)", MARIADB)));
    assert_eq!(render(&classify("SELECT JSON_TABLE(1)", MARIADB)), render(&classify("SELECT my_fn(1)", MARIADB)));
}

/// The classifier of a session whose greeting names `greeting` and whose `VERSION()` line names
/// the other engine (a proxy wrote the greeting), as the app's rules build it: its text is read
/// in the greeting's dialect.
fn disagreeing(greeting: crate::engine::EngineId) -> Classifier {
    use crate::engine::{EngineFacts, EngineId, Fact, MyFacts, Rules, ServerFacts, Version, Why};
    use crate::sql::dialect::Language;
    let (handshake, line, d) = match greeting {
        EngineId::MySql => (Version::new(8, 4, 6), "11.4.13-MariaDB", Dialect::MySql(MODE)),
        EngineId::MariaDb | EngineId::Postgres => (Version::new(11, 4, 13), "8.4.6", Dialect::MariaDb(MODE)),
    };
    let my = MyFacts { handshake, greeting };
    let detail = match greeting {
        EngineId::MySql => EngineFacts::MariaDb(my),
        EngineId::MariaDb | EngineId::Postgres => EngineFacts::MySql(my),
    };
    let facts = ServerFacts {
        version: Fact::Unknown(Why::EnginesDisagree),
        version_text: Some(line.into()),
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Unknown(Why::EnginesDisagree),
        detail,
    };
    Rules::Server { facts: std::sync::Arc::new(facts), language: Language::Sql(d) }.classifier()
}

/// Statements one engine refuses on a read-only profile and the other allows, with the greeting
/// that allows them today: MariaDB refuses the first four (no such built-in, a constructor
/// called with no arguments, words it does not reserve), so they come behind a MySQL greeting;
/// MySQL refuses the last three (a blank before a built-in's `(`, built-ins it reads only
/// unquoted), so they come behind a MariaDB greeting.
const ONE_ENGINE_REFUSES: &[(crate::engine::EngineId, &str)] = &[
    (crate::engine::EngineId::MySql, "SELECT BIN_TO_UUID(a) FROM t"),
    (crate::engine::EngineId::MySql, "SELECT POINT()"),
    (crate::engine::EngineId::MySql, "SELECT LATERAL(1)"),
    (crate::engine::EngineId::MySql, "SELECT OF(1)"),
    (crate::engine::EngineId::MariaDb, "SELECT SYSDATE ()"),
    (crate::engine::EngineId::MariaDb, "SELECT `DATABASE`()"),
    (crate::engine::EngineId::MariaDb, "SELECT `COALESCE`(a, 1) FROM t"),
];

/// A session whose greeting and `VERSION()` line name different engines is judged on both
/// engines' rules, and the stricter result counts, verdict by verdict: the later class, a danger
/// that refuses on read-only by itself over one that does not, every risk flag either sets,
/// read-only allowed only when both allow it (the merged risk's answer is that one), a confirm
/// when either asks. What the greeting's engine allows today is refused when the other refuses
/// it; `SELECT NOW()` reads on both, as before.
#[test]
fn a_disagreeing_session_is_refused_what_either_engine_refuses() {
    use crate::engine::EngineId;
    let blocks = |r: &Risk| r.danger.is_some_and(|d| Risk::danger(Class::Read, d).read_only().is_err());
    for &(greeting, sql) in ONE_ENGINE_REFUSES.iter().chain(&[(EngineId::MySql, "SELECT NOW()")]) {
        let (own, other) = match greeting {
            EngineId::MySql => (classify(sql, MYSQL), classify(sql, MARIADB)),
            EngineId::MariaDb | EngineId::Postgres => (classify(sql, MARIADB), classify(sql, MYSQL)),
        };
        let got = disagreeing(greeting).classify(sql);
        let case = format!("{greeting:?} greeting: {sql}");
        assert_eq!(got.class, own.class.max(other.class), "{case}: class");
        match (own.danger, other.danger) {
            (None, None) => assert_eq!(got.danger, None, "{case}"),
            (Some(d), None) | (None, Some(d)) => assert_eq!(got.danger, Some(d), "{case}"),
            (Some(a), Some(b)) if blocks(&own) != blocks(&other) => {
                assert_eq!(got.danger, Some(if blocks(&own) { a } else { b }), "{case}")
            }
            (Some(a), Some(b)) => assert!(got.danger == Some(a) || got.danger == Some(b), "{case}"),
        }
        assert_eq!(got.writes, own.writes || other.writes, "{case}: writes");
        assert_eq!(got.read_write, own.read_write || other.read_write, "{case}: read_write");
        assert_eq!(got.implicit_commit, own.implicit_commit || other.implicit_commit, "{case}");
        assert_eq!(got.runs_code, own.runs_code || other.runs_code, "{case}: runs_code");
        assert_eq!(got.no_where.is_some(), own.no_where.is_some() || other.no_where.is_some(), "{case}");
        assert_eq!(
            got.unchecked_call.is_some(),
            own.unchecked_call.is_some() || other.unchecked_call.is_some(),
            "{case}: unchecked call"
        );
        let both = own.read_only().is_ok() && other.read_only().is_ok();
        assert_eq!(got.read_only().is_ok(), both, "{case}: read-only allowed only when both allow");
        for writes in [false, true] {
            let either = own.confirm(writes).is_some() || other.confirm(writes).is_some();
            assert_eq!(got.confirm(writes).is_some(), either, "{case}: confirm ({writes})");
        }
        if sql == "SELECT NOW()" {
            assert!(got.read_only().is_ok(), "{case}");
        } else {
            assert!(own.read_only().is_ok(), "{case}: the greeting's engine allows it today");
            assert!(other.read_only().is_err() && got.read_only().is_err(), "{case}: refused as the other refuses it");
        }
    }
}

/// On such a session the re-run allowlist, the count and the keyset guard refuse when either
/// engine refuses (`BIN_TO_UUID` is no built-in of MariaDB's; a quoted `DATABASE` none of
/// MySQL's), and a plain read keeps today's answers: the same count text, the zone and order
/// read as before.
#[test]
fn a_disagreeing_session_reruns_only_what_both_engines_rerun() {
    use crate::engine::EngineId;
    use crate::sql::dialect::Language;
    let my = disagreeing(EngineId::MySql);
    let maria = disagreeing(EngineId::MariaDb);
    assert_eq!(my.repeatable("SELECT BIN_TO_UUID(a) FROM t"), Err(NotRepeatable::UserFunction));
    assert_eq!(my.count_query("SELECT BIN_TO_UUID(a) FROM t"), Err(NotRepeatable::UserFunction));
    assert_eq!(maria.repeatable("SELECT `DATABASE`()"), Err(NotRepeatable::UserFunction));
    assert_eq!(maria.count_query("SELECT `DATABASE`()"), Err(NotRepeatable::UserFunction));
    let plain = "SELECT a FROM t ORDER BY a";
    let zone = "SELECT a FROM t WHERE ts > CONVERT_TZ('2026-01-01 00:00:00', '+00:00', @@session.time_zone)";
    for (c, own) in [
        (&my, Classifier::new(Language::Sql(Dialect::MySql(MODE)))),
        (&maria, Classifier::new(Language::Sql(Dialect::MariaDb(MODE)))),
    ] {
        assert_eq!(c.repeatable(plain), Ok(()));
        assert_eq!(c.count_query(plain), own.count_query(plain));
        assert_eq!((c.ordered(plain), c.ordered(zone)), (true, false));
        assert_eq!((c.reads_zone(plain), c.reads_zone(zone)), (false, true));
        assert_eq!(c.repeatable(zone), own.repeatable(zone));
    }
}
