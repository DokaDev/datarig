//! The MySQL classifier against MySQL servers: every statement of its corpus that it calls a
//! read runs on a session whose `transaction_read_only` is on (as a read-only profile's is)
//! without failing as a write (error 1792), and every one it calls a write or a schema change
//! fails there as one, unless the corpus says the server lets it through (only the classifier
//! stops those). Every name it calls a built-in function is one on the server.
//!
//! Needs a MySQL client command that reads a script on stdin, connected to a database where
//! it may create and drop `zz_cls`-prefixed objects (the corpus's database name `zz_classify`
//! becomes the current one). A user without the privileges some writes need (`FILE`, `CREATE
//! USER`, …) gets them refused for that first; the test lists those, and a user with every
//! privilege shows them refused as writes:
//! `DATARIG_TEST_MYSQL_CLIENT="docker exec -i datarig-mysql mysql -udatarig -pdatarig shop"`
//! (split on blanks, no shell). Skipped without it, unless `DATARIG_REQUIRE_MYSQL=1`.

use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::risk::mysql::{BUILTINS, MARIADB_CONSTRUCTORS, NOT_ON_MARIADB};
use datarig_core::sql::risk::{Class, Classifier};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

include!("../src/sql/risk/mysql/corpus.rs");

const MODE: MySqlMode = MySqlMode {
    ansi_quotes: false,
    no_backslash_escapes: false,
    dollar_quotes: false,
    mariadb: false,
    ignore_space: false,
};

const MY: Language = Language::Sql(Dialect::MySql(MODE));

/// The error MySQL gives a write in a read-only transaction.
const READ_ONLY: u32 = 1792;

/// Errors of a user without a privilege the statement needs, which the server checks before
/// read-only: such a write is not shown to be refused as one (the test says which).
const DENIED: &[u32] = &[1044, 1045, 1142, 1227, 1370];

/// The objects the corpus reads and writes.
const SETUP: &str = "
DROP VIEW IF EXISTS zz_cls_v;
DROP TABLE IF EXISTS zz_cls_u, zz_cls_t;
DROP PROCEDURE IF EXISTS zz_cls_p;
CREATE TABLE zz_cls_t (id INT PRIMARY KEY, a INT, b VARCHAR(20), j JSON, d DATE, KEY zz_cls_a (a));
CREATE TABLE zz_cls_u (id INT PRIMARY KEY, t_id INT, CONSTRAINT zz_cls_fk FOREIGN KEY (t_id) REFERENCES zz_cls_t (id));
INSERT INTO zz_cls_t VALUES (1, 1, 'a', '{\"a\": 1}', '2024-01-01'), (2, 2, 'b', '{\"a\": 2}', '2024-01-02');
INSERT INTO zz_cls_u VALUES (1, 1), (2, 2);
CREATE VIEW zz_cls_v AS SELECT id FROM zz_cls_t;
CREATE PROCEDURE zz_cls_p() SELECT 1;
";

/// Everything the corpus may have made if the server let a write through, and the fixtures
/// (a user without the privilege to make some of them gets errors for those, which it may).
const CLEANUP: &str = "
DROP VIEW IF EXISTS zz_cls_v, zz_cls_v2;
DROP TABLE IF EXISTS zz_cls_u, zz_cls_t, zz_cls_new, zz_cls_t9, zz_x;
DROP TEMPORARY TABLE IF EXISTS zz_cls_tmp;
DROP PROCEDURE IF EXISTS zz_cls_p;
DROP PROCEDURE IF EXISTS zz_cls_p2;
DROP FUNCTION IF EXISTS zz_cls_f;
DROP FUNCTION IF EXISTS zz_cls_f2;
DROP EVENT IF EXISTS zz_cls_e;
DROP DATABASE IF EXISTS zz_cls_db;
DROP USER IF EXISTS zz_cls_user;
DROP ROLE IF EXISTS zz_cls_role;
";

/// What the corpus left in the current database: objects named `zz_cls…`.
const LEFT: &str = "SELECT CONCAT('left ', table_name) FROM information_schema.tables WHERE table_schema = DATABASE() \
AND table_name LIKE 'zz\\_cls%' UNION ALL SELECT CONCAT('left ', routine_name) FROM information_schema.routines \
WHERE routine_schema = DATABASE() AND routine_name LIKE 'zz\\_cls%';";

/// Runs [`CLEANUP`] when dropped, also when a test fails.
struct Cleanup<'a>(&'a [String]);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        run(self.0, CLEANUP);
    }
}

fn client() -> Option<Vec<String>> {
    match std::env::var("DATARIG_TEST_MYSQL_CLIENT") {
        Ok(c) if !c.trim().is_empty() => Some(c.split_whitespace().map(str::to_string).collect()),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_MYSQL_CLIENT must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED mysql_classify: set DATARIG_TEST_MYSQL_CLIENT=\"docker exec -i datarig-mysql mysql -udatarig -pdatarig shop\""
            );
            None
        }
    }
}

/// Run `script` through the client (batch mode, no column names, going on past errors, with
/// the comments sent as written): its stdout and stderr.
fn run(client: &[String], script: &str) -> (String, String) {
    let mut child = Command::new(&client[0])
        .args(&client[1..])
        .args(["--default-character-set=utf8mb4", "--batch", "--skip-column-names", "--force", "--comments"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the MySQL client starts");
    child.stdin.take().expect("stdin").write_all(script.as_bytes()).expect("the script is written");
    let out = child.wait_with_output().expect("the client ends");
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Whether the client's server is MariaDB, from its own version line (asked once per process).
fn mariadb(client: &[String]) -> bool {
    static MARIADB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MARIADB.get_or_init(|| run(client, "SELECT VERSION();").0.contains("MariaDB"))
}

/// What MariaDB does otherwise, said on stderr with why.
fn on_mariadb(test: &str, reason: &str) {
    let _ = writeln!(std::io::stderr(), "MARIADB {test}: {reason}");
}

/// Run each of `statements` (sent whole, each after a marker and followed by `SHOW ERRORS`),
/// after `prelude`: the first error each one raised, by index.
fn errors(client: &[String], prelude: &str, statements: &[(usize, &str)]) -> BTreeMap<usize, u32> {
    const END: &str = "$zz_cls$";
    let mut script = format!("{prelude}\nDELIMITER {END}\n");
    for (i, sql) in statements {
        assert!(!sql.contains(END), "{sql}");
        // The marker reads a table: MariaDB keeps the errors of the statement before for one
        // that reads none.
        script.push_str(&format!(
            "SELECT 'zz_begin {i}' FROM information_schema.schemata LIMIT 1{END}\n{sql}\n{END}\nSHOW ERRORS{END}\n"
        ));
    }
    script.push_str("DELIMITER ;\n");
    let (out, _) = run(client, &script);
    let mut found = BTreeMap::new();
    let mut current: Option<usize> = None;
    let mut seen = 0;
    for line in out.lines() {
        if let Some(i) = line.strip_prefix("zz_begin ") {
            current = i.parse().ok();
            seen += 1;
        } else if let (Some(i), Some(rest)) = (current, line.strip_prefix("Error\t")) {
            let code = rest.split('\t').next().and_then(|c| c.parse().ok()).expect("an error code");
            found.entry(i).or_insert(code);
        }
    }
    assert_eq!(seen, statements.len(), "every statement ran:\n{out}");
    found
}

#[test]
fn reads_run_on_a_read_only_session_and_writes_fail_there() {
    let Some(client) = client() else { return };
    run(&client, CLEANUP);
    let cleanup = Cleanup(&client);
    let (_, err) = run(&client, SETUP);
    assert!(!err.contains("ERROR"), "setup: {err}");

    let (db, _) = run(&client, "SELECT DATABASE();");
    let db = db.trim();
    assert!(!db.is_empty() && db.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "database {db:?}");
    let mut texts = Vec::new();
    for (i, (sql, want)) in CORPUS.iter().enumerate() {
        let class = Classifier::classify_once(MY, sql).class;
        if want.split(' ').any(|w| w == "norun") || !matches!(class, Class::Read | Class::Write | Class::Ddl) {
            continue;
        }
        texts.push((i, sql.replace("zz_classify", db)));
    }
    let statements: Vec<(usize, &str)> = texts.iter().map(|(i, s)| (*i, s.as_str())).collect();
    let errors = errors(&client, "SET SESSION transaction_read_only = ON;", &statements);
    drop(cleanup);
    let (left, _) = run(&client, LEFT);
    assert!(left.trim().is_empty(), "{left}");

    let mariadb = mariadb(&client);
    let mut wrong = Vec::new();
    let mut other = Vec::new();
    let mut denied = Vec::new();
    // MariaDB: MySQL's syntax it cannot parse (never reaching the read-only check), and the plan
    // of a locking read, which it refuses in a read-only transaction too.
    let mut unparsed = Vec::new();
    let mut stricter = Vec::new();
    for (i, sql) in &statements {
        let (_, want) = CORPUS[*i];
        let class = Classifier::classify_once(MY, sql).class;
        let error = errors.get(i).copied();
        let server_allows = want.split(' ').any(|w| w == "srvok");
        let locking_plan = sql.starts_with("EXPLAIN ") && sql.contains(" FOR UPDATE");
        match class {
            Class::Read if mariadb && locking_plan && error == Some(READ_ONLY) => stricter.push(sql.to_string()),
            Class::Read if error == Some(READ_ONLY) => wrong.push(format!("read refused as a write: {sql}")),
            Class::Read => {
                if let Some(e) = error {
                    other.push(format!("{e} {sql}"));
                }
            }
            _ if server_allows && error == Some(READ_ONLY) => {
                wrong.push(format!("marked as let through, but refused (drop `srvok`): {sql}"));
            }
            _ if server_allows => {}
            _ if error.is_some_and(|e| DENIED.contains(&e)) => denied.push(format!("{error:?} {sql}")),
            _ if mariadb && error == Some(1064) => unparsed.push(sql.to_string()),
            _ if error != Some(READ_ONLY) => wrong.push(format!("{class:?} not refused ({error:?}): {sql}")),
            _ => {}
        }
    }
    let reads = statements.iter().filter(|(_, sql)| Classifier::classify_once(MY, sql).class == Class::Read).count();
    eprintln!(
        "{} statements run ({reads} reads); reads that failed otherwise than as writes:\n{}\n\
         writes refused for a privilege first (not shown to be refused as writes):\n{}",
        statements.len(),
        other.join("\n"),
        denied.join("\n")
    );
    let test = "reads_run_on_a_read_only_session_and_writes_fail_there";
    if !unparsed.is_empty() {
        on_mariadb(
            test,
            &format!("MySQL syntax the server does not parse, not shown refused as writes:\n{}", unparsed.join("\n")),
        );
    }
    if !stricter.is_empty() {
        on_mariadb(
            test,
            &format!("the server refuses the plan of a locking read as a write:\n{}", stricter.join("\n")),
        );
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_built_in_is_a_function_of_the_server() {
    let Some(client) = client() else { return };
    // A name the server does not know as a native function is looked up as a stored function of
    // the current database: error 1305. Every built-in must be known to at least one of the
    // servers the test runs on; this one lists those it does not know.
    let names: Vec<&str> = BUILTINS.lines().collect();
    let statements: Vec<(usize, String)> =
        names.iter().enumerate().map(|(i, n)| (i, format!("SELECT {n}()"))).collect();
    let refs: Vec<(usize, &str)> = statements.iter().map(|(i, s)| (*i, s.as_str())).collect();
    let errors = errors(&client, "", &refs);
    // 1630: the same, for a name the server's grammar has otherwise (MariaDB's `JSON_TABLE`, a
    // table function only there: the test after this one).
    let unknown: Vec<&str> = errors
        .iter()
        .filter(|&(i, &e)| e == 1305 || (e == 1630 && names[*i] != "JSON_TABLE"))
        .map(|(i, _)| names[*i])
        .collect();
    eprintln!("built-ins this server does not have: {unknown:?}");
    if mariadb(&client) {
        functions_mariadb_does_not_provide(&unknown);
        return geometry_constructors_on_mariadb(&client);
    }
    // Functions only some supported versions have.
    const NOT_EVERYWHERE: &[&str] =
        &["MD5", "SHA1", "STRING_TO_VECTOR", "VECTOR_DIM", "VECTOR_TO_STRING", "WAIT_UNTIL_SQL_THREAD_AFTER_GTIDS"];
    let missing: Vec<&&str> = unknown.iter().filter(|n| !NOT_EVERYWHERE.contains(n)).collect();
    assert!(missing.is_empty(), "not functions of this server: {missing:?}");
    // MySQL's own functions are built-ins to the read check, whatever MariaDB has.
    for name in NOT_ON_MARIADB.lines() {
        let risk = Classifier::classify_once(MY, &format!("SELECT {name}()"));
        assert!(risk.unchecked_call.is_none(), "{name}");
    }
}

/// Every built-in written otherwise than right before its `(` (a blank, a newline, a comment,
/// quoted): the server looks the name up as a stored function of the current database (error
/// 1305 or 1630) exactly when the read check of a session in that sql mode (with
/// `IGNORE_SPACE` or not) takes the call as an unchecked one, and a read-only profile refuses
/// each such call.
#[test]
fn built_ins_count_as_the_server_reads_them_however_written() {
    let Some(client) = client() else { return };
    let maria = mariadb(&client);
    const FORMS: &[(&str, &str)] =
        &[("", "()"), ("", " ()"), ("", "\n()"), ("", "/* c */()"), ("", " -- c\n()"), ("`", "`()")];
    let names: Vec<&str> = BUILTINS.lines().collect();
    let mut texts = Vec::new();
    for name in &names {
        for (open, close) in FORMS {
            texts.push(format!("SELECT {open}{}{close}", name.to_ascii_lowercase()));
        }
    }
    let statements: Vec<(usize, &str)> = texts.iter().enumerate().map(|(i, s)| (i, s.as_str())).collect();
    for (prelude, ignore_space) in [
        ("SET SESSION sql_mode = REPLACE(@@SESSION.sql_mode, 'IGNORE_SPACE', '');", false),
        ("SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',IGNORE_SPACE');", true),
    ] {
        let errors = errors(&client, prelude, &statements);
        let session = Language::Sql(Dialect::MySql(MySqlMode { mariadb: maria, ignore_space, ..MODE }));
        let looked_up = |i: usize| matches!(errors.get(&i), Some(1305 | 1630));
        let mut wrong = Vec::new();
        for (n, _) in names.iter().enumerate() {
            let first = n * FORMS.len();
            // A name the server, or the read check, does not have as a built-in at all is
            // another test's.
            if looked_up(first) || Classifier::classify_once(session, &texts[first]).unchecked_call.is_some() {
                continue;
            }
            for (i, text) in texts.iter().enumerate().skip(first + 1).take(FORMS.len() - 1) {
                let r = Classifier::classify_once(session, text);
                let (unchecked, refused) = (r.unchecked_call.is_some(), r.read_only().is_err());
                if looked_up(i) && !refused || unchecked && !looked_up(i) {
                    wrong.push(format!("{text:?}: server {:?}, unchecked {unchecked}", errors.get(&i)));
                }
            }
        }
        assert!(wrong.is_empty(), "IGNORE_SPACE {ignore_space}:\n{}", wrong.join("\n"));
    }
}

/// Words and forms the server's grammar reads itself: whether it runs a stored function of the
/// current database (error 1305 or 1630) for each is whether the read check takes the text as
/// calling one and a read-only profile refuses it; a text the server runs otherwise is a plain
/// read to the check. A syntax error says nothing.
#[test]
fn words_and_table_functions_count_as_the_server_reads_them() {
    let Some(client) = client() else { return };
    let session = Language::Sql(Dialect::MySql(MySqlMode { mariadb: mariadb(&client), ..MODE }));
    let columns = "'[1]', '$[*]' COLUMNS (d DATETIME(6) PATH '$', n NCHAR(10) PATH '$', \
                   NESTED PATH '$' COLUMNS (x DECIMAL(10, 2) PATH '$'))";
    let texts = [
        "SELECT lateral()".to_string(),
        "SELECT of(1)".to_string(),
        "SELECT 1 FROM DUAL WHERE lateral (1)".to_string(),
        "SELECT JSON_TABLE(1)".to_string(),
        "SELECT `JSON_TABLE`(1)".to_string(),
        format!("SELECT * FROM JSON_TABLE({columns}) AS jt"),
        format!("SELECT * FROM `JSON_TABLE`({columns}) AS jt"),
        "SELECT `POINT`(1, 2) IS NULL".to_string(),
        "SELECT `LINESTRING`(POINT(0, 0), POINT(1, 1)) IS NULL".to_string(),
        "SELECT `POLYGON`(LINESTRING(POINT(0, 0), POINT(1, 0), POINT(0, 1), POINT(0, 0))) IS NULL".to_string(),
        "SELECT `MULTIPOINT`(POINT(1, 1)) IS NULL".to_string(),
        "SELECT `MULTILINESTRING`(LINESTRING(POINT(0, 0), POINT(1, 1))) IS NULL".to_string(),
        "SELECT `MULTIPOLYGON`(POLYGON(LINESTRING(POINT(0, 0), POINT(1, 0), POINT(0, 1), POINT(0, 0)))) IS NULL"
            .to_string(),
        "SELECT `GEOMETRYCOLLECTION`(POINT(1, 1)) IS NULL".to_string(),
    ];
    let statements: Vec<(usize, &str)> = texts.iter().enumerate().map(|(i, s)| (i, s.as_str())).collect();
    let errors = errors(&client, "", &statements);
    let mut wrong = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let r = Classifier::classify_once(session, text);
        let (refused, plain) = (r.read_only().is_err(), r.unchecked_call.is_none() && r.read_only().is_ok());
        let ok = match errors.get(&i) {
            Some(1305 | 1630) => refused,
            Some(1064) => true,
            _ => plain,
        };
        if !ok {
            wrong.push(format!("{text:?}: server {:?}, check {r:?}", errors.get(&i)));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// On MariaDB: the names of [`BUILTINS`] that `unknown` (this server looks them up as stored
/// functions) are those of `NOT_ON_MARIADB`, less the functions MariaDB added after 10.6, which
/// this server has; and the read check of a MariaDB session takes a call of each as an unchecked
/// call, which a read-only profile refuses.
fn functions_mariadb_does_not_provide(unknown: &[&str]) {
    const ADDED_AFTER_10_6: &[&str] =
        &["FORMAT_PICO_TIME", "JSON_OVERLAPS", "JSON_PRETTY", "JSON_SCHEMA_VALID", "RANDOM_BYTES"];
    let listed: Vec<&str> = NOT_ON_MARIADB.lines().collect();
    let not_listed: Vec<&&str> = unknown.iter().filter(|n| !listed.contains(n)).collect();
    assert!(not_listed.is_empty(), "add to NOT_ON_MARIADB: {not_listed:?}");
    let has: Vec<&&str> = listed.iter().filter(|n| !unknown.contains(n) && !ADDED_AFTER_10_6.contains(n)).collect();
    assert!(has.is_empty(), "functions of this server, drop from NOT_ON_MARIADB: {has:?}");
    let session = Language::Sql(Dialect::MySql(MySqlMode { mariadb: true, ..MODE }));
    for name in &listed {
        let risk = Classifier::classify_once(session, &format!("SELECT {name}()"));
        assert!(risk.unchecked_call.is_some() && risk.read_only().is_err(), "{name}");
    }
}

/// On MariaDB: a geometry constructor given the arguments its grammar reads is MariaDB's own
/// and a built-in to the read check of a MariaDB session; given others, the server looks it up
/// as a stored function and the read check takes it as an unchecked call.
fn geometry_constructors_on_mariadb(client: &[String]) {
    let session = Language::Sql(Dialect::MySql(MySqlMode { mariadb: true, ..MODE }));
    let mut texts = Vec::new();
    for &(name, fewest, _) in MARIADB_CONSTRUCTORS {
        let point = if name == "POINT" { "1" } else { "POINT(1, 1)" };
        for n in [0, fewest, fewest + 1] {
            let args = vec![point; n].join(", ");
            texts.push((
                format!("SELECT {name}({args}) IS NULL"),
                MARIADB_CONSTRUCTORS.iter().any(|&(c, lo, hi)| c == name && (lo..=hi).contains(&n)),
            ));
        }
    }
    let statements: Vec<(usize, &str)> = texts.iter().enumerate().map(|(i, (sql, _))| (i, sql.as_str())).collect();
    let errors = errors(client, "", &statements);
    for (i, (sql, native)) in texts.iter().enumerate() {
        assert_eq!(errors.get(&i) != Some(&1305), *native, "{sql}: {:?}", errors.get(&i));
        let risk = Classifier::classify_once(session, sql);
        assert_eq!(risk.unchecked_call.is_some(), !native, "{sql}");
    }
}
