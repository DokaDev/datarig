//! The chunks a keyset export reads a table with, as each engine's core writes them (the driver
//! may bound a chunk when it sends it; that is not here).

use crate::Engine::{MariaDb, MySql, Pg};
use crate::{Case, Engine};
use datarig_core::driver::{ColumnMeta, ValueKind};
use datarig_core::export::keyset::Keyset;
use datarig_core::sql::dialect::Dialect;

/// A table read by its key: its columns (name, type, kind), the key's column names and the
/// last row read.
pub struct Table {
    pub schema: &'static str,
    pub table: &'static str,
    pub columns: &'static [(&'static str, &'static str, ValueKind)],
    pub key: &'static [&'static str],
    pub last: &'static [Option<&'static str>],
}

/// The first chunk and the chunk after the last row on `e`, or why the table is not read by
/// its key (text read in this session taken as the text).
pub fn keyset(e: Engine, t: &Table) -> String {
    keyset_in(e.dialect(), t)
}

/// [`keyset`] in dialect `d`.
pub fn keyset_in(d: Dialect, t: &Table) -> String {
    let columns: Vec<ColumnMeta> = t
        .columns
        .iter()
        .map(|(name, ty, kind)| ColumnMeta::new(name.to_string(), ty.to_string(), *kind, None))
        .collect();
    let names: Vec<String> = t.columns.iter().map(|(name, _, _)| name.to_string()).collect();
    let key: Vec<String> = t.key.iter().map(|k| k.to_string()).collect();
    match Keyset::new(d, t.schema, t.table, &key, &columns, &names, true) {
        Err(why) => format!("not keyset {why:?}"),
        Ok(k) => {
            let last: Vec<Option<String>> = t.last.iter().map(|v| v.map(str::to_string)).collect();
            format!("{} | {}", k.first(), k.after(&last).unwrap_or_else(|| "no next chunk".to_string()))
        }
    }
}

pub fn keyset_cases() -> Vec<Case<Table>> {
    use ValueKind as V;
    vec![
        Case {
            name: "number key",
            input: Table {
                schema: "shop",
                table: "orders",
                columns: &[("id", "int", V::Integer), ("note", "text", V::Text)],
                key: &["id"],
                last: &[Some("42"), Some("x")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"shop\".\"orders\" ORDER BY \"id\" LIMIT 10000 | SELECT * FROM \"shop\".\"orders\" WHERE \"id\" OPERATOR(pg_catalog.>) 42 ORDER BY \"id\" LIMIT 10000",
                ),
                (
                    MySql,
                    "SELECT * FROM `shop`.`orders` ORDER BY `id` LIMIT 10000 | SELECT * FROM `shop`.`orders` WHERE `id` > 42 ORDER BY `id` LIMIT 10000",
                ),
                (
                    MariaDb,
                    "SELECT * FROM `shop`.`orders` ORDER BY `id` LIMIT 10000 | SELECT * FROM `shop`.`orders` WHERE `id` > 42 ORDER BY `id` LIMIT 10000",
                ),
            ],
        },
        Case {
            name: "two column key",
            input: Table {
                schema: "shop",
                table: "lines",
                columns: &[("order_id", "int", V::Integer), ("code", "varchar", V::Text), ("qty", "int", V::Integer)],
                key: &["order_id", "code"],
                last: &[Some("7"), Some("it's"), Some("1")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"shop\".\"lines\" ORDER BY \"order_id\", \"code\" LIMIT 10000 | SELECT * FROM \"shop\".\"lines\" WHERE (\"order_id\", \"code\") OPERATOR(pg_catalog.>) (7, 'it''s') ORDER BY \"order_id\", \"code\" LIMIT 10000",
                ),
                (
                    MySql,
                    "SELECT * FROM `shop`.`lines` ORDER BY `order_id`, `code` LIMIT 10000 | SELECT * FROM `shop`.`lines` WHERE `order_id` > 7 OR (`order_id` = 7 AND (`code` > 'it''s')) ORDER BY `order_id`, `code` LIMIT 10000",
                ),
                (
                    MariaDb,
                    "SELECT * FROM `shop`.`lines` ORDER BY `order_id`, `code` LIMIT 10000 | SELECT * FROM `shop`.`lines` WHERE `order_id` > 7 OR (`order_id` = 7 AND (`code` > 'it''s')) ORDER BY `order_id`, `code` LIMIT 10000",
                ),
            ],
        },
        Case {
            name: "names that need quotes",
            input: Table {
                schema: "My Schema",
                table: "a`b\"c",
                columns: &[("Key", "int", V::Integer)],
                key: &["Key"],
                last: &[Some("1")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"My Schema\".\"a`b\"\"c\" ORDER BY \"Key\" LIMIT 10000 | SELECT * FROM \"My Schema\".\"a`b\"\"c\" WHERE \"Key\" OPERATOR(pg_catalog.>) 1 ORDER BY \"Key\" LIMIT 10000",
                ),
                (
                    MySql,
                    "SELECT * FROM `My Schema`.`a``b\"c` ORDER BY `Key` LIMIT 10000 | SELECT * FROM `My Schema`.`a``b\"c` WHERE `Key` > 1 ORDER BY `Key` LIMIT 10000",
                ),
                (
                    MariaDb,
                    "SELECT * FROM `My Schema`.`a``b\"c` ORDER BY `Key` LIMIT 10000 | SELECT * FROM `My Schema`.`a``b\"c` WHERE `Key` > 1 ORDER BY `Key` LIMIT 10000",
                ),
            ],
        },
        Case {
            name: "text key with a backslash",
            input: Table {
                schema: "s",
                table: "t",
                columns: &[("code", "varchar", V::Text)],
                key: &["code"],
                last: &[Some("a\\b")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"s\".\"t\" ORDER BY \"code\" LIMIT 10000 | SELECT * FROM \"s\".\"t\" WHERE \"code\" OPERATOR(pg_catalog.>) E'a\\\\b' ORDER BY \"code\" LIMIT 10000",
                ),
                (
                    MySql,
                    "SELECT * FROM `s`.`t` ORDER BY `code` LIMIT 10000 | SELECT * FROM `s`.`t` WHERE `code` > _utf8mb4 X'615C62' ORDER BY `code` LIMIT 10000",
                ),
                (
                    MariaDb,
                    "SELECT * FROM `s`.`t` ORDER BY `code` LIMIT 10000 | SELECT * FROM `s`.`t` WHERE `code` > _utf8mb4 X'615C62' ORDER BY `code` LIMIT 10000",
                ),
            ],
        },
        Case {
            name: "bytes key",
            input: Table {
                schema: "s",
                table: "t",
                columns: &[("b", "varbinary", V::Bytes)],
                key: &["b"],
                last: &[Some("0x0A")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"s\".\"t\" ORDER BY \"b\" LIMIT 10000 | SELECT * FROM \"s\".\"t\" WHERE \"b\" OPERATOR(pg_catalog.>) '0x0A' ORDER BY \"b\" LIMIT 10000",
                ),
                (
                    MySql,
                    "SELECT * FROM `s`.`t` ORDER BY `b` LIMIT 10000 | SELECT * FROM `s`.`t` WHERE `b` > X'0A' ORDER BY `b` LIMIT 10000",
                ),
                (
                    MariaDb,
                    "SELECT * FROM `s`.`t` ORDER BY `b` LIMIT 10000 | SELECT * FROM `s`.`t` WHERE `b` > X'0A' ORDER BY `b` LIMIT 10000",
                ),
            ],
        },
        Case {
            name: "null key value",
            input: Table {
                schema: "s",
                table: "t",
                columns: &[("id", "int", V::Integer)],
                key: &["id"],
                last: &[None],
            },
            expect: &[
                (Pg, "SELECT * FROM \"s\".\"t\" ORDER BY \"id\" LIMIT 10000 | no next chunk"),
                (MySql, "SELECT * FROM `s`.`t` ORDER BY `id` LIMIT 10000 | no next chunk"),
                (MariaDb, "SELECT * FROM `s`.`t` ORDER BY `id` LIMIT 10000 | no next chunk"),
            ],
        },
        Case {
            name: "zoned timestamp key",
            input: Table {
                schema: "s",
                table: "t",
                columns: &[("at", "timestamp", V::TimestampTz)],
                key: &["at"],
                last: &[Some("2026-11-18 10:00:00")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"s\".\"t\" ORDER BY \"at\" LIMIT 10000 | SELECT * FROM \"s\".\"t\" WHERE \"at\" OPERATOR(pg_catalog.>) '2026-11-18 10:00:00' ORDER BY \"at\" LIMIT 10000",
                ),
                (MySql, "not keyset Kind(\"at\")"),
                (MariaDb, "not keyset Kind(\"at\")"),
            ],
        },
        Case {
            name: "enum key",
            input: Table {
                schema: "s",
                table: "t",
                columns: &[("e", "enum", V::Text)],
                key: &["e"],
                last: &[Some("a")],
            },
            expect: &[
                (
                    Pg,
                    "SELECT * FROM \"s\".\"t\" ORDER BY \"e\" LIMIT 10000 | SELECT * FROM \"s\".\"t\" WHERE \"e\" OPERATOR(pg_catalog.>) 'a' ORDER BY \"e\" LIMIT 10000",
                ),
                (MySql, "not keyset Kind(\"e\")"),
                (MariaDb, "not keyset Kind(\"e\")"),
            ],
        },
        Case {
            name: "float key",
            input: Table {
                schema: "s",
                table: "t",
                columns: &[("f", "double", V::Float)],
                key: &["f"],
                last: &[Some("0.5")],
            },
            expect: &[
                (Pg, "not keyset Kind(\"f\")"),
                (MySql, "not keyset Kind(\"f\")"),
                (MariaDb, "not keyset Kind(\"f\")"),
            ],
        },
    ]
}
