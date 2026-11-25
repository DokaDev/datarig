//! How each engine's plans are asked for, shown and asked again (`ExplainFormat`): the statement
//! that asks for a plan, what the plan view shows of it, the server's "no plan" answer, and a
//! text `EXPLAIN` the user ran asked again for a plan, in the format of each engine and in a
//! format of the other engine (a profile's plan format is the last one any of its sessions
//! said, a tab's dialect its own session's). No case reads a session's facts.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, MySqlAnsiQuotes, Pg};
use datarig_core::driver::ExplainFormat;
use datarig_core::sql::plan::explain::{JsonExplain, NotJson};

/// A question of a plan format.
pub enum Ask {
    /// `ExplainFormat::explain_sql` of `statement` in the engine's format, with `analyze`.
    Explain { statement: &'static str, analyze: bool },
    /// `is_text`, `reports_statement_times` and `has_buffers` of the engine's format.
    Display,
    /// `says_no_plan` of the engine's format for a result of one column `column` holding `text`.
    SaysNoPlan { column: &'static str, text: &'static str },
    /// `again` and `again_text` in `format` of `sql`, written in the engine's dialect.
    Again { format: ExplainFormat, sql: &'static str },
}

/// The plan format a session of the engine says its server gives, at a version that gives
/// one: PostgreSQL's JSON, MySQL's tree (8.0.16 on), none on MariaDB.
fn format(e: Engine) -> Option<ExplainFormat> {
    match e {
        Pg => Some(ExplainFormat::PostgresJson),
        MySql | MySqlAnsiQuotes => Some(ExplainFormat::MySqlTree),
        MariaDb => None,
        other => panic!("{other:?} is not a column of the plan table"),
    }
}

/// The answer to `a` on engine `e`.
pub fn plan(e: Engine, a: &Ask) -> String {
    let own = format(e);
    let with_own = |f: &dyn Fn(ExplainFormat) -> String| own.map_or_else(|| "no format".to_string(), f);
    match a {
        Ask::Explain { statement, analyze } => with_own(&|f| f.explain_sql(statement, *analyze)),
        Ask::Display => with_own(&|f| {
            format!(
                "text {} | statement times {} | buffers {}",
                f.is_text(),
                f.reports_statement_times(),
                f.has_buffers()
            )
        }),
        Ask::SaysNoPlan { column, text } => with_own(&|f| f.says_no_plan(column, text).to_string()),
        Ask::Again { format, sql } => {
            let d = e.dialect();
            format!("{} | text {}", again(&format.again(sql, d)), again(&format.again_text(sql, d)))
        }
    }
}

/// An answer of `again`: the statement asked and what it wraps, whether it analyzes and
/// evaluates, or why it is not asked again.
fn again(r: &Result<JsonExplain, NotJson>) -> String {
    match r {
        Ok(j) => format!("{} / {} / analyze {} / evaluates {}", j.sql, j.statement, j.analyze, j.evaluates),
        Err(why) => format!("{why:?}"),
    }
}

const JSON: ExplainFormat = ExplainFormat::PostgresJson;
const TREE: ExplainFormat = ExplainFormat::MySqlTree;

pub fn plan_cases() -> Vec<Case<Ask>> {
    vec![
        Case {
            name: "plan statement",
            input: Ask::Explain { statement: "SELECT 1", analyze: false },
            expect: &[
                (Pg, "EXPLAIN (FORMAT JSON) SELECT 1"),
                (MySql, "EXPLAIN FORMAT=TREE SELECT 1"),
                (MariaDb, "no format"),
                (MySqlAnsiQuotes, "EXPLAIN FORMAT=TREE SELECT 1"),
            ],
        },
        Case {
            name: "plan statement with analyze",
            input: Ask::Explain { statement: "SELECT 1", analyze: true },
            expect: &[
                (Pg, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 1"),
                (MySql, "EXPLAIN ANALYZE FORMAT=TREE SELECT 1"),
                (MariaDb, "no format"),
                (MySqlAnsiQuotes, "EXPLAIN ANALYZE FORMAT=TREE SELECT 1"),
            ],
        },
        Case {
            name: "what the plan view shows",
            input: Ask::Display,
            expect: &[
                (Pg, "text false | statement times true | buffers true"),
                (MySql, "text true | statement times false | buffers false"),
                (MariaDb, "no format"),
                (MySqlAnsiQuotes, "text true | statement times false | buffers false"),
            ],
        },
        Case {
            name: "no plan for the statement",
            input: Ask::SaysNoPlan { column: "EXPLAIN", text: "-> <not executable by iterator executor>" },
            expect: &[(Pg, "false"), (MySql, "true"), (MariaDb, "no format"), (MySqlAnsiQuotes, "true")],
        },
        Case {
            name: "explain asked again as json",
            input: Ask::Again { format: JSON, sql: "EXPLAIN SELECT 1" },
            expect: &[
                (
                    Pg,
                    "EXPLAIN (FORMAT JSON) SELECT 1 / SELECT 1 / analyze false / evaluates false | text EXPLAIN (FORMAT JSON) SELECT 1 / SELECT 1 / analyze false / evaluates false",
                ),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "explain asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN SELECT 1" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (
                    MySql,
                    "EXPLAIN FORMAT=TREE SELECT 1 / SELECT 1 / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT 1 / SELECT 1 / analyze false / evaluates false",
                ),
                (
                    MariaDb,
                    "EXPLAIN FORMAT=TREE SELECT 1 / SELECT 1 / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT 1 / SELECT 1 / analyze false / evaluates false",
                ),
                (
                    MySqlAnsiQuotes,
                    "EXPLAIN FORMAT=TREE SELECT 1 / SELECT 1 / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT 1 / SELECT 1 / analyze false / evaluates false",
                ),
            ],
        },
        Case {
            name: "describe asked again as json",
            input: Ask::Again { format: JSON, sql: "DESCRIBE t" },
            expect: &[
                (Pg, "NotExplain | text NotExplain"),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "describe asked again as the tree",
            input: Ask::Again { format: TREE, sql: "DESCRIBE t" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (MySql, "NotExplain | text NotExplain"),
                (MariaDb, "NotExplain | text NotExplain"),
                (MySqlAnsiQuotes, "NotExplain | text NotExplain"),
            ],
        },
        Case {
            name: "explain analyze asked again as json",
            input: Ask::Again { format: JSON, sql: "EXPLAIN ANALYZE SELECT 1" },
            expect: &[
                (
                    Pg,
                    "EXPLAIN (ANALYZE, FORMAT JSON) SELECT 1 / SELECT 1 / analyze true / evaluates true | text EXPLAIN (ANALYZE, FORMAT JSON) SELECT 1 / SELECT 1 / analyze true / evaluates true",
                ),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "explain analyze asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN ANALYZE SELECT 1" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (MySql, "Analyzed | text Analyzed"),
                (MariaDb, "Analyzed | text Analyzed"),
                (MySqlAnsiQuotes, "Analyzed | text Analyzed"),
            ],
        },
        Case {
            name: "mysql executable comment asked again as json",
            input: Ask::Again { format: JSON, sql: "EXPLAIN SELECT 1 /*!50000 , 2 */" },
            expect: &[
                (
                    Pg,
                    "EXPLAIN (FORMAT JSON) SELECT 1 /*!50000 , 2 */ / SELECT 1 /*!50000 , 2 */ / analyze false / evaluates false | text EXPLAIN (FORMAT JSON) SELECT 1 /*!50000 , 2 */ / SELECT 1 /*!50000 , 2 */ / analyze false / evaluates false",
                ),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "mysql executable comment asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN SELECT 1 /*!50000 , 2 */" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "mariadb executable comment asked again as json",
            input: Ask::Again { format: JSON, sql: "EXPLAIN SELECT 1 /*M! , 2 */" },
            expect: &[
                (
                    Pg,
                    "EXPLAIN (FORMAT JSON) SELECT 1 /*M! , 2 */ / SELECT 1 /*M! , 2 */ / analyze false / evaluates false | text EXPLAIN (FORMAT JSON) SELECT 1 /*M! , 2 */ / SELECT 1 /*M! , 2 */ / analyze false / evaluates false",
                ),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "mariadb executable comment asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN SELECT 1 /*M! , 2 */" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (
                    MySql,
                    "Unreadable | text EXPLAIN FORMAT=TREE SELECT 1 /*M! , 2 */ / SELECT 1 /*M! , 2 */ / analyze false / evaluates false",
                ),
                (
                    MariaDb,
                    "Unreadable | text EXPLAIN FORMAT=TREE SELECT 1 /*M! , 2 */ / SELECT 1 /*M! , 2 */ / analyze false / evaluates false",
                ),
                (
                    MySqlAnsiQuotes,
                    "Unreadable | text EXPLAIN FORMAT=TREE SELECT 1 /*M! , 2 */ / SELECT 1 /*M! , 2 */ / analyze false / evaluates false",
                ),
            ],
        },
        Case {
            name: "double quotes asked again as json",
            input: Ask::Again { format: JSON, sql: "EXPLAIN SELECT \"a\" FROM t" },
            expect: &[
                (
                    Pg,
                    "EXPLAIN (FORMAT JSON) SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false | text EXPLAIN (FORMAT JSON) SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false",
                ),
                (MySql, "Unreadable | text Unreadable"),
                (MariaDb, "Unreadable | text Unreadable"),
                (MySqlAnsiQuotes, "Unreadable | text Unreadable"),
            ],
        },
        Case {
            name: "double quotes asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN SELECT \"a\" FROM t" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (
                    MySql,
                    "EXPLAIN FORMAT=TREE SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false",
                ),
                (
                    MariaDb,
                    "EXPLAIN FORMAT=TREE SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false",
                ),
                (
                    MySqlAnsiQuotes,
                    "EXPLAIN FORMAT=TREE SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT \"a\" FROM t / SELECT \"a\" FROM t / analyze false / evaluates false",
                ),
            ],
        },
    ]
}
