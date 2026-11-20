//! The risk classifier of each engine: class, danger and the read-only verdict of the same
//! statements.

use crate::Engine::{MariaDb, MySql, MySqlDollarNotKnown, MySqlNoBackslashEscapes, Pg};
use crate::{Case, Engine, verdict};
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::sql::risk::Classifier;

/// The class, the danger and the read-only verdict of `sql` on a session of `e` that prepared
/// nothing.
pub fn classify(e: Engine, sql: &&'static str) -> String {
    classify_in(e.dialect(), sql)
}

/// [`classify`] on a session of dialect `d`.
pub fn classify_in(d: Dialect, sql: &str) -> String {
    let risk = Classifier::classify_once(Language::Sql(d), sql);
    format!("{:?} {:?} read-only {}", risk.class, risk.danger, verdict(&risk.read_only()))
}

pub fn classifier_cases() -> Vec<Case<&'static str>> {
    vec![
        Case {
            name: "a write after a dollar quote",
            input: "SELECT $$ ' $$; DROP TABLE t; -- '",
            expect: &[
                (Pg, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MySql, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MariaDb, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MySqlDollarNotKnown, "Ddl Some(Drop) read-only Class(Ddl)"),
            ],
        },
        Case {
            name: "plain select",
            input: "SELECT 1",
            expect: &[
                (Pg, "Read None read-only ok"),
                (MySql, "Read None read-only ok"),
                (MariaDb, "Read None read-only ok"),
            ],
        },
        Case {
            name: "bin_to_uuid",
            input: "SELECT BIN_TO_UUID(a) FROM t",
            expect: &[
                (Pg, "Read None read-only UnknownFunction(\"bin_to_uuid\")"),
                (MySql, "Read None read-only ok"),
                (MariaDb, "Read None read-only UnknownFunction(\"BIN_TO_UUID\")"),
            ],
        },
        Case {
            name: "any_value",
            input: "SELECT ANY_VALUE(a) FROM t GROUP BY b",
            expect: &[
                (Pg, "Read None read-only ok"),
                (MySql, "Read None read-only ok"),
                (MariaDb, "Read None read-only UnknownFunction(\"ANY_VALUE\")"),
            ],
        },
        Case {
            name: "analyze table",
            input: "ANALYZE TABLE t",
            expect: &[
                (Pg, "Unknown Some(Unparsed) read-only Unparsed"),
                (MySql, "Maintenance None read-only Class(Maintenance)"),
                (MariaDb, "Maintenance None read-only Class(Maintenance)"),
            ],
        },
        Case {
            name: "analyze select",
            input: "ANALYZE SELECT * FROM t",
            expect: &[
                (Pg, "Unknown Some(Unparsed) read-only Unparsed"),
                (MySql, "Maintenance None read-only Class(Maintenance)"),
                (MariaDb, "Maintenance None read-only Class(Maintenance)"),
            ],
        },
        Case {
            name: "analyze bare",
            input: "ANALYZE t",
            expect: &[
                (Pg, "Maintenance None read-only Class(Maintenance)"),
                (MySql, "Maintenance None read-only Class(Maintenance)"),
                (MariaDb, "Maintenance None read-only Class(Maintenance)"),
            ],
        },
        Case {
            name: "update every row",
            input: "UPDATE t SET a = 1",
            expect: &[
                (Pg, "Write Some(UpdateAll) read-only Class(Write)"),
                (MySql, "Write Some(UpdateAll) read-only Class(Write)"),
                (MariaDb, "Write Some(UpdateAll) read-only Class(Write)"),
            ],
        },
        Case {
            name: "delete one row",
            input: "DELETE FROM t WHERE id = 1",
            expect: &[
                (Pg, "Write None read-only Class(Write)"),
                (MySql, "Write None read-only Class(Write)"),
                (MariaDb, "Write None read-only Class(Write)"),
            ],
        },
        Case {
            name: "drop table",
            input: "DROP TABLE t",
            expect: &[
                (Pg, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MySql, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MariaDb, "Ddl Some(Drop) read-only Class(Ddl)"),
            ],
        },
        Case {
            name: "truncate",
            input: "TRUNCATE t",
            expect: &[
                (Pg, "Ddl Some(Truncate) read-only Class(Ddl)"),
                (MySql, "Ddl Some(Truncate) read-only Class(Ddl)"),
                (MariaDb, "Ddl Some(Truncate) read-only Class(Ddl)"),
            ],
        },
        Case {
            name: "create table",
            input: "CREATE TABLE t (a int)",
            expect: &[
                (Pg, "Ddl None read-only Class(Ddl)"),
                (MySql, "Ddl None read-only Class(Ddl)"),
                (MariaDb, "Ddl None read-only Class(Ddl)"),
            ],
        },
        Case {
            name: "select for update",
            input: "SELECT * FROM t FOR UPDATE",
            expect: &[
                (Pg, "Write None read-only Class(Write)"),
                (MySql, "Write None read-only Class(Write)"),
                (MariaDb, "Write None read-only Class(Write)"),
            ],
        },
        Case {
            name: "lock tables",
            input: "LOCK TABLES t READ",
            expect: &[
                (Pg, "Unknown Some(Unparsed) read-only Unparsed"),
                (MySql, "Write Some(Locks) read-only Locks"),
                (MariaDb, "Write Some(Locks) read-only Locks"),
            ],
        },
        Case {
            name: "executable comment",
            input: "SELECT 1 /*!50000 , 2 */",
            expect: &[
                (Pg, "Read None read-only ok"),
                (MySql, "Unknown Some(ExecutableComment) read-only ExecutableComment"),
                (MariaDb, "Unknown Some(ExecutableComment) read-only ExecutableComment"),
            ],
        },
        Case {
            name: "mariadb executable comment",
            input: "SELECT 1 /*M!100000 , 2 */",
            expect: &[
                (Pg, "Read None read-only ok"),
                (MySql, "Unknown Some(ExecutableComment) read-only ExecutableComment"),
                (MariaDb, "Unknown Some(ExecutableComment) read-only ExecutableComment"),
            ],
        },
        Case {
            name: "set sql_mode",
            input: "SET sql_mode = 'ANSI_QUOTES'",
            expect: &[
                (Pg, "Session None read-only Setting"),
                (MySql, "Session Some(Setting) read-only Setting"),
                (MariaDb, "Session Some(Setting) read-only Setting"),
            ],
        },
        Case {
            name: "begin",
            input: "BEGIN",
            expect: &[(Pg, "Tx None read-only ok"), (MySql, "Tx None read-only ok"), (MariaDb, "Tx None read-only ok")],
        },
        Case {
            name: "explain analyze",
            input: "EXPLAIN ANALYZE SELECT * FROM t",
            expect: &[
                (Pg, "Read None read-only ok"),
                (MySql, "Read None read-only ok"),
                (MariaDb, "Read None read-only ok"),
            ],
        },
        Case {
            name: "sleep",
            input: "SELECT SLEEP(1)",
            expect: &[
                (Pg, "Read None read-only UnknownFunction(\"sleep\")"),
                (MySql, "Read None read-only ok"),
                (MariaDb, "Read None read-only ok"),
            ],
        },
        Case {
            name: "pg_read_file",
            input: "SELECT pg_read_file('/etc/hostname')",
            expect: &[
                (Pg, "Read Some(ServerFile) read-only ServerFile"),
                (MySql, "Read None read-only UnknownFunction(\"pg_read_file\")"),
                (MariaDb, "Read None read-only UnknownFunction(\"pg_read_file\")"),
            ],
        },
        Case {
            name: "user function",
            input: "SELECT my_function(1)",
            expect: &[
                (Pg, "Read None read-only UnknownFunction(\"my_function\")"),
                (MySql, "Read None read-only UnknownFunction(\"my_function\")"),
                (MariaDb, "Read None read-only UnknownFunction(\"my_function\")"),
            ],
        },
        Case {
            name: "backslash before a quote",
            input: "SELECT 'a\\'; DROP TABLE t; -- '",
            expect: &[
                (Pg, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MySql, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MariaDb, "Ddl Some(Drop) read-only Class(Ddl)"),
                (MySqlNoBackslashEscapes, "Ddl Some(Drop) read-only Class(Ddl)"),
            ],
        },
        Case {
            name: "not sql",
            input: "hello there",
            expect: &[
                (Pg, "Unknown Some(Unparsed) read-only Unparsed"),
                (MySql, "Unknown Some(Unrecognized) read-only Unrecognized"),
                (MariaDb, "Unknown Some(Unrecognized) read-only Unrecognized"),
            ],
        },
        Case {
            name: "insert returning",
            input: "INSERT INTO t VALUES (1) RETURNING id",
            expect: &[
                (Pg, "Write None read-only Class(Write)"),
                (MySql, "Write None read-only Class(Write)"),
                (MariaDb, "Write None read-only Class(Write)"),
            ],
        },
    ]
}
