use super::*;
use crate::sql::risk::{Classifier, ReadOnlyBlock};

include!("corpus.rs");

const MODE: MySqlMode = MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false };

fn risk(sql: &str) -> Risk {
    classify(sql, MODE)
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
            Danger::Setting => "setting",
            Danger::DynamicSql => "dynamic-sql",
            Danger::ServerCommand => "server-command",
            Danger::FileAccess => "file-access",
        };
        let how = match r.no_where {
            Some(NoWhere::Missing) => ":missing",
            Some(NoWhere::AlwaysTrue) => ":always",
            Some(NoWhere::NoColumn) => ":nocol",
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
        (r.unchecked_call, "udf"),
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
    assert_eq!(c, Classifier::MySql(MODE));
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
    assert_eq!(c, Classifier::MySql(MODE));
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
            assert!(r.danger.is_none() && !r.read_write && !r.unchecked_call, "{sql}");
            assert!(r.class != Class::Session || r.safe_setting, "{sql}");
        }
    }
}

#[test]
fn read_only_says_why_it_refuses() {
    let why = |sql: &str| risk(sql).read_only().err();
    assert_eq!(why("SELECT /*! 1 */"), Some(ReadOnlyBlock::ExecutableComment));
    assert_eq!(why("SELEC 1"), Some(ReadOnlyBlock::Unrecognized));
    assert_eq!(why("SELECT sys_exec('x')"), Some(ReadOnlyBlock::UnknownFunction));
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
    assert_eq!(render(&classify("SELECT \"a\" FROM t", ansi)), "read ro");
    assert_eq!(render(&classify("SELECT \"a\" FROM t", MODE)), "read ro");
    // The server reads no escape in a quoted name: `"a\"` ends there.
    assert_eq!(render(&classify("SELECT \"a\\\" FROM t; DELETE FROM t\"", ansi)), "unknown unrecognized");
    // Without backslash escapes, `'\'` is a whole string and what follows is code.
    let text = "SELECT '\\'; DELETE FROM t; -- '";
    assert_eq!(render(&classify(text, MODE)), "read ro");
    assert_eq!(render(&classify(text, raw)), "write delete-all:missing writes");
    let text = "SELECT * FROM t WHERE b = 'x\\' OR 1 = 1; DELETE FROM t -- '";
    assert_eq!(render(&classify(text, MODE)), "read ro");
    assert_eq!(classify(text, raw).danger, Some(Danger::DeleteAll));
    // Dollar quotes only where the server reads them.
    let dollar = MySqlMode { dollar_quotes: true, ..MODE };
    assert_eq!(render(&classify("SELECT $$a; DROP TABLE t$$", dollar)), "read ro");
    assert_eq!(render(&classify("SELECT $$a; DROP TABLE t$$", MODE)), "ddl drop commit");
}

#[test]
fn a_hint_with_a_semicolon_or_a_write_stays_a_comment() {
    assert_eq!(render(&risk("SELECT /*+ ; DELETE FROM t */ 1")), "read ro");
    assert_eq!(render(&risk("SELECT /*+ SET_VAR(transaction_read_only = OFF) */ 1")), "read ro");
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
        assert!(risk(sql).unchecked_call, "{sql}");
    }
    for sql in [
        "SELECT * FROM t WHERE a = ANY (SELECT 1)",
        "SELECT * FROM t WHERE a > SOME (SELECT 1)",
        "SELECT MATCH (a) AGAINST ('x') FROM t",
        "SELECT * FROM JSON_TABLE('[1]', '$[*]' COLUMNS (x INT PATH '$')) AS j",
        "SELECT CAST(a AS SIGNED), CAST(a AS NCHAR(2)) FROM t",
    ] {
        assert!(!risk(sql).unchecked_call, "{sql}");
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
    assert_eq!(render(&risk("CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO DELETE FROM t; SELECT 1")), "ddl commit");
    // Blocks that cannot be followed: the whole text is the one statement.
    assert_eq!(render(&risk("CREATE PROCEDURE p() BEGIN END END; DROP TABLE t")), "ddl commit");
}

#[test]
fn deep_nesting_is_read_in_linear_time() {
    let calls = format!("SELECT {}1{}", "abs(".repeat(MAX_DEPTH - 1), ")".repeat(MAX_DEPTH - 1));
    let cond = format!("DELETE FROM t WHERE {}a = 1{}", "(".repeat(MAX_DEPTH - 1), ")".repeat(MAX_DEPTH - 1));
    let ctes = format!(
        "WITH {} SELECT 1",
        (0..5000).map(|i| format!("c{i} (x) AS (SELECT 1)")).collect::<Vec<_>>().join(", ")
    );
    for text in [&calls, &cond, &ctes] {
        let start = std::time::Instant::now();
        let r = risk(text);
        assert!(r.danger.is_none() || r.danger == Some(Danger::DeleteAll), "{:?}", r.danger);
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
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
        assert_eq!(repeatable(sql, MODE), Ok(()), "{sql}");
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
        ("SELECT /*! 1 */", NotRepeatable::Unreadable),
        ("SELEC 1", NotRepeatable::NotSelect),
        ("SELECT (1", NotRepeatable::Unreadable),
    ];
    for (sql, why) in no {
        assert_eq!(repeatable(sql, MODE), Err(why), "{sql}");
    }
    assert_eq!(
        names("SELECT a, t.b FROM `shop`.`t` AS t JOIN u ON NOW() = u.c", MODE).unwrap(),
        ["a", "t.b", "`shop`.`t`", "t", "u", "u.c"]
    );
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
        assert_eq!(ordered(sql, MODE), want, "{sql}");
    }
}

#[test]
fn count_query_wraps_a_repeatable_query() {
    assert_eq!(
        count_query("SELECT * FROM t ORDER BY a;", MODE).as_deref(),
        Ok("SELECT COUNT(*) FROM (\nSELECT * FROM t ORDER BY a\n) AS datarig_count")
    );
    assert_eq!(
        count_query("SELECT * FROM t # note", MODE).as_deref(),
        Ok("SELECT COUNT(*) FROM (\nSELECT * FROM t # note\n) AS datarig_count")
    );
    assert_eq!(count_query("TABLE t", MODE).as_deref(), Ok("SELECT COUNT(*) FROM (\nTABLE t\n) AS datarig_count"));
    assert_eq!(count_query("SELECT RAND()", MODE), Err(NotRepeatable::Volatile("rand".into())));
    assert_eq!(count_query("DELETE FROM t", MODE), Err(NotRepeatable::NotSelect));
    // A trailing comment that would swallow the closing parenthesis on one line.
    assert!(count_query("SELECT 1 -- x", MODE).unwrap().contains("-- x\n)"));
}
