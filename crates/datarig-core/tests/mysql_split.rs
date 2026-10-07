//! The MySQL splitter against MySQL's own command-line client and server: a script split by
//! datarig gives the statements the client sends (`mysql -vvv` echoes each), and each of them,
//! sent alone, is one statement the server runs (no syntax error from a wrong cut).
//!
//! Needs a MySQL client command that reads a script on stdin, connected to a database where
//! it may create and drop `zz_`-prefixed objects:
//! `DATARIG_TEST_MYSQL_CLIENT="docker exec -i datarig-mysql mysql -udatarig -pdatarig shop"`
//! (split on blanks, no shell). Skipped without it, unless `DATARIG_REQUIRE_MYSQL=1`.

use datarig_core::sql::dialect::{Dialect, MySqlMode};
use datarig_core::sql::lexer::lex_in;
use datarig_core::sql::split::split_in;
use std::io::Write;
use std::process::{Command, Stdio};

/// The tests create and drop the same objects: one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false });

/// Statements that all run (in this order; what they create they drop).
const VALID: &str = r#"-- a comment line; with a semicolon
SELECT 1 AS a, 2 AS b # a hash comment; it's here
;
SELECT 1--1 AS minus_minus;
SELECT 'it\'s; fine' AS s, "dq; \" x" AS d, 'a''b;' AS e;
SELECT `x;y` FROM (SELECT 1 AS `x;y`) AS `t;t`;
SELECT /* block; */ 1 /*!80000 + 1 */ AS v, /*!99999 + 100 */ 2 AS w;
SELECT /*+ MAX_EXECUTION_TIME(1000) */ 3 AS hinted;
SET @a = 'x;y', @`b;c` = 2, @'d;e' := 3;
SELECT @a, @`b;c`, @@session.sql_mode IS NOT NULL AS m;
SELECT x'41' AS hx, 0x42 AS hn, b'1' AS bit1, _utf8mb4'u;v' AS intro, N'n\';m' AS nat, 1e1 AS f;
SELECT 'multi
line; string' AS ml;
DROP PROCEDURE IF EXISTS zz_split_p;
DELIMITER //
CREATE PROCEDURE zz_split_p()
BEGIN
  DECLARE v INT DEFAULT 0; -- inside; the body
  SET v = v + 1; # still the body
  SELECT v, 'a//b' AS s;
END//
CALL zz_split_p()//
DELIMITER ;
DROP PROCEDURE zz_split_p;
CREATE TABLE zz_split_t (id INT PRIMARY KEY, `1e5` INT);
delimiter $$
CREATE PROCEDURE zz_split_q(IN n INT)
BEGIN
  INSERT INTO zz_split_t VALUES (n, n * 2);
END$$
CALL zz_split_q(1)$$
  Delimiter  ;  with the rest of the line ignored
SELECT t.1e5 FROM zz_split_t AS t;
DROP PROCEDURE zz_split_q;
DROP TABLE zz_split_t;
DELIMITER ~~
SELECT 1; SELECT 2 ~~
delimiter ;
SELECT 'done' AS done"#;

/// Text the client cuts where the server then fails: only the cuts are compared.
const QUIRKS: &str = "SELECT 1 /*! , 2; */ AS x;\nSELECT 3 --;\n;\nSELECT 4 AS\ndelimiter;\nSELECT 5;";

fn client() -> Option<Vec<String>> {
    match std::env::var("DATARIG_TEST_MYSQL_CLIENT") {
        Ok(c) if !c.trim().is_empty() => Some(c.split_whitespace().map(str::to_string).collect()),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_MYSQL_CLIENT must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED mysql_split: set DATARIG_TEST_MYSQL_CLIENT=\"docker exec -i datarig-mysql mysql -udatarig -pdatarig shop\""
            );
            None
        }
    }
}

/// Run `script` through the client with `args` (in `utf8mb4`): its stdout and stderr.
fn run(client: &[String], args: &[&str], script: &str) -> (String, String) {
    let mut child = Command::new(&client[0])
        .args(&client[1..])
        .arg("--default-character-set=utf8mb4")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the MySQL client starts");
    child.stdin.take().expect("stdin").write_all(script.as_bytes()).expect("the script is written");
    let out = child.wait_with_output().expect("the client ends");
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// `text` without the comments and blanks around it, as datarig's lexer reads them.
fn code(text: &str) -> &str {
    let toks: Vec<_> = lex_in(text, MY).into_iter().filter(|t| !t.is_trivia()).collect();
    match (toks.first(), toks.last()) {
        (Some(a), Some(b)) => &text[a.start..b.end],
        _ => "",
    }
}

/// The statements the client sent, as `-vvv` echoes them (between dashed lines), comments and
/// blanks around them left out; ones that are only comments are not statements to datarig.
fn sent(client: &[String], script: &str) -> Vec<String> {
    let (out, _) = run(client, &["-vvv", "--comments", "--force"], script);
    let mut stmts = Vec::new();
    let mut cur: Option<Vec<&str>> = None;
    for line in out.lines() {
        if line == "--------------" {
            match cur.take() {
                Some(lines) => stmts.push(lines.join("\n")),
                None => cur = Some(Vec::new()),
            }
        } else if let Some(lines) = cur.as_mut() {
            lines.push(line);
        }
    }
    stmts.iter().map(|s| code(s).to_string()).filter(|s| !s.is_empty()).collect()
}

fn bodies(script: &str) -> Vec<String> {
    split_in(script, MY).iter().map(|s| s.body(script).to_string()).collect()
}

/// [`bodies`] as the server reads each alone (`SELECT 3 --` ends in a comment there).
fn codes(script: &str) -> Vec<String> {
    bodies(script).iter().map(|b| code(b).to_string()).collect()
}

#[test]
fn mysql_statements_are_the_ones_the_client_sends() {
    let Some(client) = client() else { return };
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for script in [VALID, QUIRKS] {
        assert_eq!(codes(script), sent(&client, script), "{script}");
    }
}

#[test]
fn each_mysql_statement_runs_alone() {
    let Some(client) = client() else { return };
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let stmts = bodies(VALID);
    assert!(stmts.len() > 20, "{stmts:?}");
    // Each statement sent alone: a terminator none of them holds, on a line of its own.
    let mut script = String::from("DELIMITER ~zz~split~\n");
    for s in &stmts {
        assert!(!s.contains("~zz~split~"));
        script.push_str(s);
        script.push_str("\n~zz~split~\n");
    }
    let (out, err) = run(&client, &["--comments"], &script);
    let cleanup =
        "DROP PROCEDURE IF EXISTS zz_split_q; DROP TABLE IF EXISTS zz_split_t; DROP PROCEDURE IF EXISTS zz_split_p;";
    run(&client, &[], cleanup);
    let errors: Vec<&str> = err.lines().filter(|l| l.starts_with("ERROR")).collect();
    assert!(errors.is_empty(), "{errors:?}\n{out}");
    assert!(out.contains("done"), "{out}");
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02X}")).collect()
}

/// What the app writes for MySQL (`Dialect::quote_literal`, `Dialect::quote_ident`) is read
/// back by the server as the text and the names it was made from.
#[test]
fn mysql_literals_and_names_read_back_as_written() {
    let Some(client) = client() else { return };
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let texts = [
        "plain",
        "it's",
        "C:\\new",
        "\\",
        "\\'",
        "''",
        "a\0b",
        "\\%_",
        "line\nbreak\ttab\r",
        "\u{1F418} caf\u{e9}",
        "",
    ];
    let script: String = texts.iter().map(|t| format!("SELECT HEX({});\n", MY.quote_literal(t))).collect();
    let (out, err) = run(&client, &["-N"], &script);
    assert!(err.lines().all(|l| !l.starts_with("ERROR")), "{err}");
    let want: Vec<String> = texts.iter().map(|t| hex(t)).collect();
    assert_eq!(out.lines().collect::<Vec<_>>(), want);

    // A session under `NO_BACKSLASH_ESCAPES` reads the literals of that mode back.
    let nbe = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: true });
    let texts = ["it's", "C:\\new", "\\", "\\'", "\\%_", "\u{1F418}"];
    let mut script = String::from("SET SESSION sql_mode = CONCAT(@@sql_mode, ',NO_BACKSLASH_ESCAPES');\n");
    script.extend(texts.iter().map(|t| format!("SELECT HEX({});\n", nbe.quote_literal(t))));
    let (out, err) = run(&client, &["-N"], &script);
    assert!(err.lines().all(|l| !l.starts_with("ERROR")), "{err}");
    assert_eq!(out.lines().collect::<Vec<_>>(), texts.iter().map(|t| hex(t)).collect::<Vec<_>>());

    let names = ["order", "Select", "Users", "users_2", "a`b", "a b", "1e5", "status", "rank", "caf\u{e9}"];
    let cols: Vec<String> = names.iter().map(|n| format!("{} INT", MY.quote_ident(n))).collect();
    let script = format!(
        "DROP TABLE IF EXISTS zz_split_names;\nCREATE TABLE zz_split_names ({});\n\
         SELECT HEX(column_name) FROM information_schema.columns WHERE table_schema = DATABASE() \
         AND table_name = 'zz_split_names' ORDER BY ordinal_position;\nDROP TABLE zz_split_names;\n",
        cols.join(", ")
    );
    let (out, err) = run(&client, &["-N"], &script);
    assert!(err.lines().all(|l| !l.starts_with("ERROR")), "{err}");
    let want: Vec<String> = names.iter().map(|n| hex(n)).collect();
    assert_eq!(out.lines().collect::<Vec<_>>(), want);
}
