//! Where a statement runs on a session that commits by hand while no transaction is open
//! (`Classifier::tx_step`), with the risk flags it reads.

use crate::Engine::{MariaDb, MySql, Pg};
use crate::{Case, Engine};
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::sql::risk::Classifier;

/// The step of `sql` on `e`, and whether it commits the open transaction, may not run in one or
/// adds a value to an enum.
pub fn step(e: Engine, sql: &&'static str) -> String {
    step_in(e.dialect(), sql)
}

/// [`step`] on a session of dialect `d`.
pub fn step_in(d: Dialect, sql: &str) -> String {
    let lang = Language::Sql(d);
    let step = Classifier::new(lang).tx_step(sql);
    let risk = Classifier::classify_once(lang, sql);
    format!(
        "{step:?} implicit_commit={} no_tx_block={} adds_enum_value={}",
        risk.implicit_commit, risk.no_tx_block, risk.adds_enum_value
    )
}

pub fn cases() -> Vec<Case<&'static str>> {
    vec![
        Case {
            name: "constant",
            input: "SELECT 1",
            expect: &[
                (Pg, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "table read",
            input: "SELECT * FROM t",
            expect: &[
                (Pg, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "now",
            input: "SELECT NOW()",
            expect: &[
                (Pg, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "user function",
            input: "SELECT f()",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "insert",
            input: "INSERT INTO t VALUES (1)",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "create table",
            input: "CREATE TABLE t (a int)",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "truncate",
            input: "TRUNCATE t",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "vacuum",
            input: "VACUUM t",
            expect: &[
                (Pg, "Outside implicit_commit=false no_tx_block=true adds_enum_value=false"),
                (MySql, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "create index concurrently",
            input: "CREATE INDEX CONCURRENTLY i ON t (a)",
            expect: &[
                (Pg, "Outside implicit_commit=false no_tx_block=true adds_enum_value=false"),
                (MySql, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "begin",
            input: "BEGIN",
            expect: &[
                (Pg, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "commit",
            input: "COMMIT",
            expect: &[
                (Pg, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "set a setting",
            input: "SET autocommit = 0",
            expect: &[
                (Pg, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "show",
            input: "SHOW TABLES",
            expect: &[
                (Pg, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "explain",
            input: "EXPLAIN SELECT * FROM t",
            expect: &[
                (Pg, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "explain analyze",
            input: "EXPLAIN ANALYZE SELECT * FROM t",
            expect: &[
                (Pg, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "variable",
            input: "SELECT @@autocommit",
            expect: &[
                (Pg, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "ReadOutside implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "two statements",
            input: "SELECT 1; SELECT 2",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "lock tables",
            input: "LOCK TABLES t READ",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Outside implicit_commit=true no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "discard temp",
            input: "DISCARD TEMP",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MySql, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
        Case {
            name: "alter enum add value",
            input: "ALTER TYPE e ADD VALUE 'x'",
            expect: &[
                (Pg, "Opens implicit_commit=false no_tx_block=false adds_enum_value=true"),
                (MySql, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
                (MariaDb, "Opens implicit_commit=false no_tx_block=false adds_enum_value=false"),
            ],
        },
    ]
}
