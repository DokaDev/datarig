//! Where a statement runs on a session that commits by hand while no transaction is open
//! (`Classifier::tx_step`), with the risk flags it reads; and what the engine's transaction
//! rules say of it (`TxRules::statement`).

use crate::Engine::{MariaDb, MySql, Pg};
use crate::{Case, Engine};
use datarig_core::engine::TxStatement;
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

/// One statement of a run, as the transaction checks read it: its text, whether a transaction
/// is open when it is reached, and whether the session commits by hand.
pub struct TxInput {
    pub sql: &'static str,
    pub open: bool,
    pub manual: bool,
}

/// What the engine's transaction rules say of the statement of `input` on `e`
/// (`TxRules::statement`), its step and risk read by a fresh classifier of `e`'s dialect.
pub fn verdict(e: Engine, input: &TxInput) -> String {
    let d = e.dialect();
    let mut c = Classifier::new(Language::Sql(d));
    let step = c.tx_step(input.sql);
    let risk = c.classify(input.sql);
    let s = TxStatement { sql: input.sql, risk: &risk, step, open: input.open, manual: input.manual };
    let v = d.engine().spec().tx.statement(&s);
    format!("refused={:?} ask={:?} notes={:?} ends_tx={}", v.refused, v.ask, v.notes, v.ends_tx)
}

const NOTHING: &str = "refused=None ask=None notes=[] ends_tx=false";

pub fn statement_cases() -> Vec<Case<TxInput>> {
    vec![
        Case {
            name: "no tx block, open",
            input: TxInput { sql: "VACUUM users", open: true, manual: true },
            expect: &[
                (Pg, "refused=Some(NoTxBlock) ask=None notes=[] ends_tx=false"),
                (MySql, NOTHING),
                (MariaDb, NOTHING),
            ],
        },
        Case {
            name: "no tx block, manual, none open",
            input: TxInput { sql: "VACUUM users", open: false, manual: true },
            expect: &[
                (Pg, "refused=None ask=Some(OutsideTx) notes=[] ends_tx=false"),
                (MySql, NOTHING),
                (MariaDb, NOTHING),
            ],
        },
        Case {
            name: "no tx block, automatic commit",
            input: TxInput { sql: "VACUUM users", open: false, manual: false },
            expect: &[(Pg, NOTHING), (MySql, NOTHING), (MariaDb, NOTHING)],
        },
        Case {
            name: "ddl, open",
            input: TxInput { sql: "ALTER TABLE users ADD COLUMN zz_c int", open: true, manual: true },
            expect: &[
                (Pg, "refused=None ask=None notes=[DdlLock(Exclusive(\"users\"))] ends_tx=false"),
                (MySql, "refused=None ask=Some(ImplicitCommit) notes=[] ends_tx=true"),
                (MariaDb, "refused=None ask=Some(ImplicitCommit) notes=[] ends_tx=true"),
            ],
        },
        Case {
            name: "ddl, open, automatic commit",
            input: TxInput { sql: "ALTER TABLE users ADD COLUMN zz_c int", open: true, manual: false },
            expect: &[
                (Pg, "refused=None ask=None notes=[DdlLock(Exclusive(\"users\"))] ends_tx=false"),
                (MySql, "refused=None ask=None notes=[] ends_tx=true"),
                (MariaDb, "refused=None ask=None notes=[] ends_tx=true"),
            ],
        },
        Case {
            name: "ddl, manual, none open",
            input: TxInput { sql: "CREATE TABLE zz_t (id int)", open: false, manual: true },
            expect: &[
                (Pg, NOTHING),
                (MySql, "refused=None ask=None notes=[MysqlDdl] ends_tx=true"),
                (MariaDb, "refused=None ask=None notes=[MysqlDdl] ends_tx=true"),
            ],
        },
        Case {
            name: "enum value, manual, none open",
            input: TxInput { sql: "ALTER TYPE mood ADD VALUE 'meh'", open: false, manual: true },
            expect: &[
                (Pg, "refused=None ask=None notes=[EnumValue] ends_tx=false"),
                (MySql, NOTHING),
                (MariaDb, NOTHING),
            ],
        },
        Case {
            name: "enum value, automatic commit",
            input: TxInput { sql: "ALTER TYPE mood ADD VALUE 'meh'", open: false, manual: false },
            expect: &[(Pg, NOTHING), (MySql, NOTHING), (MariaDb, NOTHING)],
        },
        Case {
            name: "lock tables, manual",
            input: TxInput { sql: "LOCK TABLES users WRITE", open: false, manual: true },
            expect: &[
                (Pg, NOTHING),
                (MySql, "refused=Some(LockTables) ask=None notes=[] ends_tx=false"),
                (MariaDb, "refused=Some(LockTables) ask=None notes=[] ends_tx=false"),
            ],
        },
        Case {
            name: "lock tables, automatic commit",
            input: TxInput { sql: "LOCK TABLES users WRITE", open: false, manual: false },
            expect: &[
                (Pg, NOTHING),
                (MySql, "refused=None ask=None notes=[] ends_tx=true"),
                (MariaDb, "refused=None ask=None notes=[] ends_tx=true"),
            ],
        },
        Case {
            name: "create index, open",
            input: TxInput { sql: "CREATE INDEX i ON t (a)", open: true, manual: true },
            expect: &[
                (Pg, "refused=None ask=None notes=[DdlLock(Share(\"t\"))] ends_tx=false"),
                (MySql, "refused=None ask=Some(ImplicitCommit) notes=[] ends_tx=true"),
                (MariaDb, "refused=None ask=Some(ImplicitCommit) notes=[] ends_tx=true"),
            ],
        },
        Case {
            name: "write, open",
            input: TxInput { sql: "UPDATE users SET name = 'x' WHERE id = 1", open: true, manual: true },
            expect: &[(Pg, NOTHING), (MySql, NOTHING), (MariaDb, NOTHING)],
        },
    ]
}
