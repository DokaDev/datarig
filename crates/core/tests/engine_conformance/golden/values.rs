//! Every answer of the value rules, recorded: each typed value of the edit input table
//! (`check_input`), each column of the copy as SQL table (`sql_cells`, each `cell` and its
//! `sql_value` literal), each table of the keyset table (its first chunk and the chunk after
//! the last row) and each column default of the default tests (`default_shown`), on
//! PostgreSQL, MySQL and MariaDB. A change of any of them is a change of `golden/values.txt`;
//! `DATARIG_BLESS=1` writes the file anew.

use crate::Engine;
use datarig_core::driver::structure::{ColumnFill, StructureColumn};
use datarig_core::edit::default_shown;
use datarig_core::sql::dialect::{Dialect, MySqlMode};

/// The engines every line is recorded on.
const ENGINES: [Engine; 3] = [Engine::Pg, Engine::MySql, Engine::MariaDb];

/// The column defaults of the default tests, as each server's driver writes them.
const DEFAULTS: &[&str] = &[
    "'ACTIVE'::character varying",
    "'it''s'::text",
    "'{}'::jsonb",
    "'{a,b}'::text[]",
    "'2020-01-01'::date",
    "0",
    "1.5",
    "(-1)",
    "'-1'::integer",
    "(0)::numeric",
    "(-1)::numeric",
    "('x'::text)::character varying",
    "(0) + 1",
    "('now'::text)::date",
    "'now'::timestamp with time zone",
    "'infinity'::date",
    "'now'::text",
    "'today'::character varying",
    "true",
    "NULL::character varying",
    "now()",
    "CURRENT_TIMESTAMP",
    "('a'::text || 'b'::text)",
    "gen_random_uuid()",
    "nextval('shop.users_id_seq'::regclass)",
    "NEXTVAL('s'::regclass)",
    "'ACTIVE'",
    "'it\\'s'",
    "1.50",
    "CURRENT_TIMESTAMP(3)",
    "CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP",
    "NULL ON UPDATE CURRENT_TIMESTAMP",
    "(rand())",
    "(1.5)",
    "'a'::text",
    "'a ON UPDATE b'",
    "'a ON UPDATE b' ON UPDATE CURRENT_TIMESTAMP",
    "(_utf8mb4'')",
    "(_utf8mb4'it\\'s')",
    "'a\\\\b'",
    "'a\\b'",
    "('a\\\\b')",
    "\"x\"",
    "'x'",
    "(nextval(s))",
];

/// The fills a default is shown for besides a plain column's.
fn fills() -> [ColumnFill; 6] {
    [
        ColumnFill::Default,
        ColumnFill::IdentityAlways,
        ColumnFill::IdentityByDefault,
        ColumnFill::AutoIncrement,
        ColumnFill::Stored("a + 1".into()),
        ColumnFill::Virtual("a + 1".into()),
    ]
}

fn column(not_null: bool, default: Option<&str>, fill: ColumnFill) -> StructureColumn {
    StructureColumn {
        name: "c".into(),
        type_name: "text".into(),
        not_null,
        default: default.map(str::to_string),
        fill,
        enum_labels: Vec::new(),
        collation: None,
    }
}

fn values() -> String {
    let mut out = String::new();
    for case in crate::values::input_cases() {
        for e in ENGINES {
            out.push_str(&format!("input {e:?} {:?}: {}\n", case.name, crate::values::input(e, &case.input)));
        }
    }
    for case in crate::values::cells_cases() {
        for e in ENGINES {
            out.push_str(&format!("cells {e:?} {:?}: {}\n", case.name, crate::values::cells(e, &case.input)));
        }
    }
    for case in crate::export::keyset_cases() {
        for e in ENGINES {
            out.push_str(&format!("keyset {e:?} {:?}: {}\n", case.name, crate::export::keyset(e, &case.input)));
        }
    }
    let modes = [
        ("NoBackslashEscapes", Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() })),
        ("AnsiQuotes", Dialect::MySql(MySqlMode { ansi_quotes: true, ..MySqlMode::default() })),
    ];
    let dialects = ENGINES.iter().map(|e| (format!("{e:?}"), e.dialect())).chain(modes.map(|(n, d)| (n.into(), d)));
    for (name, d) in dialects {
        for default in DEFAULTS {
            for not_null in [true, false] {
                let shown = default_shown(&column(not_null, Some(default), ColumnFill::Default), d);
                out.push_str(&format!("default {name} {default:?} not null {not_null}: {shown:?}\n"));
            }
        }
        for fill in fills() {
            for not_null in [true, false] {
                let shown = default_shown(&column(not_null, None, fill.clone()), d);
                out.push_str(&format!("default {name} none {fill:?} not null {not_null}: {shown:?}\n"));
            }
        }
    }
    out
}

#[test]
fn value_answers_are_todays() {
    let got = values();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engine_conformance/golden/values.txt");
    if std::env::var("DATARIG_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &got).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // Line by line: a checkout may end the file's lines with `\r\n`.
    let (got, want): (Vec<&str>, Vec<&str>) = (got.lines().collect(), want.lines().collect());
    for (n, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "line {} (DATARIG_BLESS=1 cargo test -p datarig-core --test engine_conformance)", n + 1);
    }
    assert_eq!(got.len(), want.len());
}
