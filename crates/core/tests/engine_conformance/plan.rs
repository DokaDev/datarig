//! How each engine's plans are asked for, shown and asked again (`ExplainFormat`): the statement
//! that asks for a plan, what the plan view shows of it, the server's "no plan" answer, and a
//! text `EXPLAIN` the user ran asked again for a plan, in the format of each engine and in a
//! format of the other engine (a profile's plan format is the last one any of its sessions
//! said, a tab's dialect its own session's). No case of [`plan_cases`] reads a session's
//! facts; [`plan_format_cases`] asks the plan rules of the engine of a session's greeting
//! (`engine::plan_of`) on every shape of its facts.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, MySqlAnsiQuotes, MySqlNoBackslashEscapes, Pg};
use datarig_core::driver::ExplainFormat;
use datarig_core::engine::{
    EngineFacts, EngineId, Fact, MyFacts, PgFacts, ServerFacts, Topology, Version, Why, plan_of,
};
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
        MySql | MySqlAnsiQuotes | MySqlNoBackslashEscapes => Some(ExplainFormat::MySqlTree),
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

/// An answer of `again`: the statement asked and what it wraps, whether it analyses and
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
        Case {
            name: "a mysql-only function asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN SELECT BIN_TO_UUID(x) FROM t" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (
                    MySql,
                    "EXPLAIN FORMAT=TREE SELECT BIN_TO_UUID(x) FROM t / SELECT BIN_TO_UUID(x) FROM t / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT BIN_TO_UUID(x) FROM t / SELECT BIN_TO_UUID(x) FROM t / analyze false / evaluates false",
                ),
                (
                    MariaDb,
                    "EXPLAIN FORMAT=TREE SELECT BIN_TO_UUID(x) FROM t / SELECT BIN_TO_UUID(x) FROM t / analyze false / evaluates true | text EXPLAIN FORMAT=TREE SELECT BIN_TO_UUID(x) FROM t / SELECT BIN_TO_UUID(x) FROM t / analyze false / evaluates false",
                ),
                (
                    MySqlAnsiQuotes,
                    "EXPLAIN FORMAT=TREE SELECT BIN_TO_UUID(x) FROM t / SELECT BIN_TO_UUID(x) FROM t / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT BIN_TO_UUID(x) FROM t / SELECT BIN_TO_UUID(x) FROM t / analyze false / evaluates false",
                ),
            ],
        },
        Case {
            name: "an escaped quote asked again as the tree",
            input: Ask::Again { format: TREE, sql: "EXPLAIN SELECT 'a\\'; SELECT 1'" },
            expect: &[
                (Pg, "Unreadable | text Unreadable"),
                (
                    MySql,
                    "EXPLAIN FORMAT=TREE SELECT 'a\\'; SELECT 1' / SELECT 'a\\'; SELECT 1' / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT 'a\\'; SELECT 1' / SELECT 'a\\'; SELECT 1' / analyze false / evaluates false",
                ),
                (
                    MariaDb,
                    "EXPLAIN FORMAT=TREE SELECT 'a\\'; SELECT 1' / SELECT 'a\\'; SELECT 1' / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT 'a\\'; SELECT 1' / SELECT 'a\\'; SELECT 1' / analyze false / evaluates false",
                ),
                (
                    MySqlAnsiQuotes,
                    "EXPLAIN FORMAT=TREE SELECT 'a\\'; SELECT 1' / SELECT 'a\\'; SELECT 1' / analyze false / evaluates false | text EXPLAIN FORMAT=TREE SELECT 'a\\'; SELECT 1' / SELECT 'a\\'; SELECT 1' / analyze false / evaluates false",
                ),
                (MySqlNoBackslashEscapes, "Several | text Several"),
            ],
        },
    ]
}

/// A server of the plan format table: an older one, or one of today.
pub enum Server {
    /// PostgreSQL 12.0, MySQL 8.0.15 (before the tree), MariaDB 10.6.0.
    Older,
    /// PostgreSQL 17.2, MySQL 8.4.6, MariaDB 11.4.13.
    Today,
}

/// What a session of column `e` on server `s` found it to be, reached directly: its reports
/// agreeing, or a version not known (it did not parse, the reports name different numbers, or
/// the greeting names MariaDB and the line MySQL).
fn facts(e: Engine, s: &Server) -> ServerFacts {
    let (pg, my, maria) = match s {
        Server::Older => (Version::new(12, 0, 0), Version::new(8, 0, 15), Version::new(10, 6, 0)),
        Server::Today => (Version::new(17, 2, 0), Version::new(8, 4, 6), Version::new(11, 4, 13)),
    };
    let unknown = |why| Fact::Unknown(why);
    let (version, detail) = match e {
        Pg => (Fact::Known(pg), EngineFacts::Postgres(PgFacts::default())),
        Engine::PgVersionUnknown => (unknown(Why::VersionUnknown), EngineFacts::Postgres(PgFacts::default())),
        MySql => (Fact::Known(my), EngineFacts::MySql(MyFacts::new(my, EngineId::MySql))),
        Engine::MySqlVersionUnknown => {
            (unknown(Why::VersionUnknown), EngineFacts::MySql(MyFacts::new(my, EngineId::MySql)))
        }
        Engine::MySqlVersionsDisagree => {
            (unknown(Why::VersionsDisagree), EngineFacts::MySql(MyFacts::new(my, EngineId::MySql)))
        }
        Engine::MySqlEnginesDisagree => {
            (unknown(Why::EnginesDisagree), EngineFacts::MySql(MyFacts::new(my, EngineId::MariaDb)))
        }
        MariaDb => (Fact::Known(maria), EngineFacts::MariaDb(MyFacts::new(maria, EngineId::MariaDb))),
        Engine::MariaDbVersionUnknown => {
            (unknown(Why::VersionUnknown), EngineFacts::MariaDb(MyFacts::new(maria, EngineId::MariaDb)))
        }
        Engine::MariaDbVersionsDisagree => {
            (unknown(Why::VersionsDisagree), EngineFacts::MariaDb(MyFacts::new(maria, EngineId::MariaDb)))
        }
        other => panic!("{other:?} is not a column of the plan format table"),
    };
    ServerFacts {
        version,
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail,
    }
}

/// The plan format a session of column `e` on server `s` gives (`engine::plan_of`).
pub fn plan_format(e: Engine, s: &Server) -> String {
    format!("{:?}", plan_of(&facts(e, s)))
}

pub fn plan_format_cases() -> Vec<Case<Server>> {
    vec![
        Case {
            name: "an older server",
            input: Server::Older,
            expect: &[
                (Pg, "Known(PostgresJson)"),
                (Engine::PgVersionUnknown, "Known(PostgresJson)"),
                (MySql, "No(VersionTooOld { needed: Version { major: 8, minor: 0, patch: 16 } })"),
                (
                    Engine::MySqlVersionUnknown,
                    "No(VersionTooOld { needed: Version { major: 8, minor: 0, patch: 16 } })",
                ),
                (
                    Engine::MySqlVersionsDisagree,
                    "No(VersionTooOld { needed: Version { major: 8, minor: 0, patch: 16 } })",
                ),
                (Engine::MySqlEnginesDisagree, "No(NotBuilt)"),
                (MariaDb, "No(NotBuilt)"),
                (Engine::MariaDbVersionUnknown, "No(NotBuilt)"),
                (Engine::MariaDbVersionsDisagree, "No(NotBuilt)"),
            ],
        },
        Case {
            name: "a server of today",
            input: Server::Today,
            expect: &[
                (Pg, "Known(PostgresJson)"),
                (Engine::PgVersionUnknown, "Known(PostgresJson)"),
                (MySql, "Known(MySqlTree)"),
                (Engine::MySqlVersionUnknown, "Known(MySqlTree)"),
                (Engine::MySqlVersionsDisagree, "Known(MySqlTree)"),
                (Engine::MySqlEnginesDisagree, "No(NotBuilt)"),
                (MariaDb, "No(NotBuilt)"),
                (Engine::MariaDbVersionUnknown, "No(NotBuilt)"),
                (Engine::MariaDbVersionsDisagree, "No(NotBuilt)"),
            ],
        },
    ]
}
