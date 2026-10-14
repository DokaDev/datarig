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
use datarig_core::sql::risk::mysql::BUILTINS;
use datarig_core::sql::risk::{Class, Classifier};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

include!("../src/sql/risk/mysql/corpus.rs");

const MY: Language =
    Language::Sql(Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false }));

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

/// Run each of `statements` (sent whole, each after a marker and followed by `SHOW ERRORS`),
/// after `prelude`: the first error each one raised, by index.
fn errors(client: &[String], prelude: &str, statements: &[(usize, &str)]) -> BTreeMap<usize, u32> {
    const END: &str = "$zz_cls$";
    let mut script = format!("{prelude}\nDELIMITER {END}\n");
    for (i, sql) in statements {
        assert!(!sql.contains(END), "{sql}");
        script.push_str(&format!("SELECT 'zz_begin {i}'{END}\n{sql}\n{END}\nSHOW ERRORS{END}\n"));
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

    let mut wrong = Vec::new();
    let mut other = Vec::new();
    let mut denied = Vec::new();
    for (i, sql) in &statements {
        let (_, want) = CORPUS[*i];
        let class = Classifier::classify_once(MY, sql).class;
        let error = errors.get(i).copied();
        let server_allows = want.split(' ').any(|w| w == "srvok");
        match class {
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
    let unknown: Vec<&str> = errors.iter().filter(|&(_, &e)| e == 1305).map(|(i, _)| names[*i]).collect();
    eprintln!("built-ins this server does not have: {unknown:?}");
    // Functions only some supported versions have.
    const NOT_EVERYWHERE: &[&str] =
        &["MD5", "SHA1", "STRING_TO_VECTOR", "VECTOR_DIM", "VECTOR_TO_STRING", "WAIT_UNTIL_SQL_THREAD_AFTER_GTIDS"];
    let missing: Vec<&&str> = unknown.iter().filter(|n| !NOT_EVERYWHERE.contains(n)).collect();
    assert!(missing.is_empty(), "not functions of this server: {missing:?}");
}
