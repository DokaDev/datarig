//! The same cases on every engine: what the core answers today for PostgreSQL, MySQL and
//! MariaDB, side by side, so that a change of one engine's answer is a change of this table.
//!
//! Each table lists cases (an input) with the exact output expected on every engine. The
//! engines are PostgreSQL ([`Dialect::Postgres`]), MySQL ([`Dialect::MySql`] in the default
//! [`MySqlMode`]) and MariaDB ([`Dialect::MariaDb`], what a MariaDB session says). A case may
//! also pin a MySQL sql mode where it changes the output (`ANSI_QUOTES`,
//! `NO_BACKSLASH_ESCAPES`, `IGNORE_SPACE`). A case without an expectation for one of the three engines fails
//! ([`every_case_names_every_engine`]): adding an engine means answering every case for it. A
//! case of the classifier or split table whose text has a `$` must also answer for a MySQL
//! session that does not know whether its server reads dollar quotes
//! ([`Engine::MySqlDollarNotKnown`]). A case of a table of rules asked of a session's facts
//! must also answer for sessions whose server's version is not known ([`UNKNOWN_VERSION`]).
//!
//! Every expectation is today's output, as it is, right or wrong: the table pins it, and a
//! change that moves it updates the table in the same change. No server is needed.

#[path = "engine_conformance/catalogue.rs"]
mod catalogue;
#[path = "engine_conformance/context.rs"]
mod context;
#[path = "engine_conformance/edit.rs"]
mod edit;
#[path = "engine_conformance/export.rs"]
mod export;
#[path = "engine_conformance/filter.rs"]
mod filter;
#[path = "engine_conformance/golden.rs"]
mod golden;
#[path = "engine_conformance/golden/values.rs"]
mod golden_values;
#[path = "engine_conformance/read_only.rs"]
mod read_only;
#[path = "engine_conformance/risk.rs"]
mod risk;
#[path = "engine_conformance/session.rs"]
mod session;
#[path = "engine_conformance/sort.rs"]
mod sort;
#[path = "engine_conformance/text.rs"]
mod text;
#[path = "engine_conformance/tx.rs"]
mod tx;
#[path = "engine_conformance/values.rs"]
mod values;

use datarig_core::sql::dialect::{Dialect, DollarQuotes, MySqlMode};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// An engine a case runs on: the three every case must answer, and the MySQL sql modes a case
/// may add where they change the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Engine {
    Pg,
    MySql,
    MariaDb,
    /// MySQL with `ANSI_QUOTES`.
    MySqlAnsiQuotes,
    /// MySQL with `NO_BACKSLASH_ESCAPES`.
    MySqlNoBackslashEscapes,
    /// MySQL whose server's own reports disagree on whether it reads dollar quotes.
    MySqlDollarNotKnown,
    /// MySQL with `IGNORE_SPACE`.
    MySqlIgnoreSpace,
    /// PostgreSQL whose version did not parse.
    PgVersionUnknown,
    /// MySQL whose version did not parse.
    MySqlVersionUnknown,
    /// MariaDB whose version did not parse.
    MariaDbVersionUnknown,
    /// MySQL whose own reports name different versions.
    MySqlVersionsDisagree,
    /// MariaDB whose own reports name different versions.
    MariaDbVersionsDisagree,
    /// MySQL 8.0.18, the last version that reads no time zone offset in a `TIMESTAMP` value.
    MySql8018,
    /// MySQL 8.0.19, the first version that reads one.
    MySql8019,
    /// MySQL whose greeting names MariaDB: its version is not known.
    MySqlEnginesDisagree,
    /// MySQL whose session has not said what its server is.
    MySqlNotChecked,
}

/// The engines every case must have an expectation for.
pub const ENGINES: [Engine; 3] = [Engine::Pg, Engine::MySql, Engine::MariaDb];

/// The sessions whose server's version is not known that every case of a table of rules asked
/// of a session's facts must also have an expectation for.
pub const UNKNOWN_VERSION: [Engine; 5] = [
    Engine::PgVersionUnknown,
    Engine::MySqlVersionUnknown,
    Engine::MariaDbVersionUnknown,
    Engine::MySqlVersionsDisagree,
    Engine::MariaDbVersionsDisagree,
];

impl Engine {
    /// The dialect the core reads and writes this engine's SQL in today.
    pub fn dialect(self) -> Dialect {
        match self {
            Engine::Pg | Engine::PgVersionUnknown => Dialect::Postgres,
            Engine::MySql
            | Engine::MySqlVersionUnknown
            | Engine::MySqlVersionsDisagree
            | Engine::MySql8018
            | Engine::MySql8019
            | Engine::MySqlEnginesDisagree
            | Engine::MySqlNotChecked => Dialect::MySql(MySqlMode::default()),
            Engine::MariaDb | Engine::MariaDbVersionUnknown | Engine::MariaDbVersionsDisagree => {
                Dialect::MariaDb(MySqlMode::default())
            }
            Engine::MySqlAnsiQuotes => Dialect::MySql(MySqlMode { ansi_quotes: true, ..MySqlMode::default() }),
            Engine::MySqlNoBackslashEscapes => {
                Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() })
            }
            Engine::MySqlDollarNotKnown => {
                Dialect::MySql(MySqlMode { dollar_quotes: DollarQuotes::NotKnown, ..MySqlMode::default() })
            }
            Engine::MySqlIgnoreSpace => Dialect::MySql(MySqlMode { ignore_space: true, ..MySqlMode::default() }),
        }
    }
}

/// One case: an input and its output on each engine, exactly.
pub struct Case<I> {
    pub name: &'static str,
    pub input: I,
    pub expect: &'static [(Engine, &'static str)],
}

/// What [`every_case_names_every_engine`] reads of a case: its table, its name, the engines it
/// has expectations for and the engines it must have them for.
pub struct Named {
    pub table: &'static str,
    pub case: &'static str,
    pub engines: Vec<Engine>,
    pub required: Vec<Engine>,
}

/// The cases of table `table` as [`every_case_names_every_engine`] reads them.
pub fn named<I>(table: &'static str, cases: &[Case<I>]) -> Vec<Named> {
    cases
        .iter()
        .map(|c| Named {
            table,
            case: c.name,
            engines: c.expect.iter().map(|(e, _)| *e).collect(),
            required: ENGINES.to_vec(),
        })
        .collect()
}

/// [`named`] for a table of texts that the dollar-quote reading changes: a text with a `$` must
/// also answer for [`Engine::MySqlDollarNotKnown`].
pub fn named_text(table: &'static str, cases: &[Case<&'static str>]) -> Vec<Named> {
    let mut out = named(table, cases);
    for (n, c) in out.iter_mut().zip(cases) {
        if c.input.contains('$') {
            n.required.push(Engine::MySqlDollarNotKnown);
        }
    }
    out
}

/// [`named`] for a table of rules asked of a session's facts: every case must also answer for
/// the sessions of [`UNKNOWN_VERSION`].
pub fn named_versioned<I>(table: &'static str, cases: &[Case<I>]) -> Vec<Named> {
    let mut out = named(table, cases);
    for n in &mut out {
        n.required.extend(UNKNOWN_VERSION);
    }
    out
}

/// What is wrong with the expectations of `cases`: a case that misses an engine it must answer
/// for, names an engine twice, or shares its name with another case of its table.
pub fn gaps(cases: &[Named]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for c in cases {
        if !seen.insert((c.table, c.case)) {
            out.push(format!("{}: case `{}` is listed twice", c.table, c.case));
        }
        for &e in &c.required {
            if !c.engines.contains(&e) {
                out.push(format!("{}: case `{}` has no expectation for {e:?}", c.table, c.case));
            }
        }
        let mut engines = BTreeSet::new();
        for e in &c.engines {
            if !engines.insert(*e) {
                out.push(format!("{}: case `{}` names {e:?} twice", c.table, c.case));
            }
        }
    }
    out
}

/// Run every case of table `table` on every engine it names, with `f` giving the output, and
/// fail with the list of every difference (expected and actual, as Rust literals) and of every
/// gap in the expectations.
pub fn run<I>(table: &'static str, cases: &[Case<I>], f: impl Fn(Engine, &I) -> String) {
    check(table, cases, &named(table, cases), f);
}

/// [`run`] for a table of texts whose expectations [`named_text`] reads.
pub fn run_text(table: &'static str, cases: &[Case<&'static str>], f: impl Fn(Engine, &&'static str) -> String) {
    check(table, cases, &named_text(table, cases), f);
}

/// [`run`] for a table of rules whose expectations [`named_versioned`] reads.
pub fn run_versioned<I>(table: &'static str, cases: &[Case<I>], f: impl Fn(Engine, &I) -> String) {
    check(table, cases, &named_versioned(table, cases), f);
}

/// Every difference of `cases` from what `f` gives, and every gap of `named`, or nothing.
fn check<I>(table: &'static str, cases: &[Case<I>], named: &[Named], f: impl Fn(Engine, &I) -> String) {
    let mut report = String::new();
    for gap in gaps(named) {
        let _ = writeln!(report, "{gap}");
    }
    for c in cases {
        for (engine, expected) in c.expect {
            let actual = f(*engine, &c.input);
            if actual != *expected {
                let _ = writeln!(
                    report,
                    "{table}: case `{}` on {engine:?}\n  expected: {expected:?}\n  actual:   {actual:?}",
                    c.name
                );
            }
        }
    }
    assert!(report.is_empty(), "{report}");
}

/// `r` as `ok` or the `Debug` of its error.
pub fn verdict<T, E: std::fmt::Debug>(r: &Result<T, E>) -> String {
    match r {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("{e:?}"),
    }
}

#[test]
fn classifier_cases_on_every_engine() {
    run_text("classifier", &risk::classifier_cases(), risk::classify);
}

#[test]
fn function_verdicts_per_context() {
    run("function verdicts", &context::cases(), context::verdicts);
}

#[test]
fn quoting_cases_on_every_engine() {
    run("quoting", &text::quoting_cases(), text::quoting);
}

#[test]
fn split_cases_on_every_engine() {
    run_text("split", &text::split_cases(), text::split);
}

#[test]
fn error_offset_cases_on_every_engine() {
    run("error position", &text::error_offset_cases(), text::error_offset);
}

#[test]
fn filter_render_cases_on_every_engine() {
    run("filter render", &filter::render_cases(), filter::render);
}

#[test]
fn compares_cases_on_every_engine() {
    run("compares", &filter::compares_cases(), filter::compares);
}

#[test]
fn group_count_cases_on_every_engine() {
    run("group count", &filter::group_count_cases(), filter::group_count);
}

#[test]
fn filter_read_cases_on_every_engine() {
    run("filter read", &filter::read_cases(), filter::read);
}

#[test]
fn filter_check_cases_on_every_engine() {
    run("filter check", &filter::check_cases(), filter::check);
}

#[test]
fn sort_rules_on_every_engine() {
    run("sort rules", &sort::sort_cases(), sort::sort);
}

#[test]
fn edit_input_cases_on_every_engine() {
    run("edit input", &values::input_cases(), values::input);
}

#[test]
fn sql_cells_cases_on_every_engine() {
    run("sql cells", &values::cells_cases(), values::cells);
}

#[test]
fn sql_literal_cases_on_every_engine() {
    run_versioned("sql literal", &values::literal_cases(), values::literal);
}

#[test]
fn keyset_chunk_sql_on_every_engine() {
    run("keyset chunk", &export::keyset_cases(), export::keyset);
}

#[test]
fn tx_step_cases_on_every_engine() {
    run("tx step", &tx::cases(), tx::step);
}

#[test]
fn tx_statement_cases_on_every_engine() {
    run("tx statement", &tx::statement_cases(), tx::verdict);
}

#[test]
fn session_rules_on_every_engine() {
    run_versioned("session rules", &session::cases(), session::answer);
}

#[test]
fn duplicates_choice_on_every_engine() {
    run("duplicates choice", &edit::cases(), edit::choice);
}

#[test]
fn read_only_open_on_every_engine() {
    run("read-only open", &read_only::cases(), read_only::verdict);
}

/// The engines of [`ENGINES`] are every engine of the registry, each once, in its order: a
/// table that names each of [`ENGINES`] names every engine.
fn engines_are_the_registrys() {
    use datarig_core::engine::EngineId;
    assert_eq!(ENGINES.map(|e| e.dialect().engine()), EngineId::ALL);
}

#[test]
fn catalogue_rules_on_every_engine() {
    engines_are_the_registrys();
    run("catalogue rules", &catalogue::catalogue_cases(), catalogue::result_tables);
}

#[test]
fn filter_type_cases_on_every_engine() {
    engines_are_the_registrys();
    run("filter types", &catalogue::filter_type_cases(), catalogue::filter_types);
}

#[test]
fn ddl_rules_on_every_engine() {
    engines_are_the_registrys();
    run("ddl rules", &catalogue::ddl_cases(), catalogue::ddl);
}

#[test]
fn hierarchy_on_every_engine() {
    engines_are_the_registrys();
    run("hierarchy", &catalogue::hierarchy_cases(), catalogue::hierarchy);
}

#[test]
fn every_case_names_every_engine() {
    let all: Vec<Named> = [
        named_text("classifier", &risk::classifier_cases()),
        named("function verdicts", &context::cases()),
        named("quoting", &text::quoting_cases()),
        named_text("split", &text::split_cases()),
        named("error position", &text::error_offset_cases()),
        named("filter render", &filter::render_cases()),
        named("compares", &filter::compares_cases()),
        named("group count", &filter::group_count_cases()),
        named("filter read", &filter::read_cases()),
        named("filter check", &filter::check_cases()),
        named("sort rules", &sort::sort_cases()),
        named("edit input", &values::input_cases()),
        named("sql cells", &values::cells_cases()),
        named_versioned("sql literal", &values::literal_cases()),
        named("keyset chunk", &export::keyset_cases()),
        named("tx step", &tx::cases()),
        named("tx statement", &tx::statement_cases()),
        named_versioned("session rules", &session::cases()),
        named("read-only open", &read_only::cases()),
        named("catalogue rules", &catalogue::catalogue_cases()),
        named("ddl rules", &catalogue::ddl_cases()),
        named("hierarchy", &catalogue::hierarchy_cases()),
        named("filter types", &catalogue::filter_type_cases()),
        named("duplicates choice", &edit::cases()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let found = gaps(&all);
    assert!(found.is_empty(), "{}", found.join("\n"));
    // The check itself: a case that leaves out an engine, or names one twice, is reported.
    let partial = [
        Case { name: "no MariaDB", input: (), expect: &[(Engine::Pg, ""), (Engine::MySql, "")] },
        Case {
            name: "MySQL twice",
            input: (),
            expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MySql, ""), (Engine::MariaDb, "")],
        },
        Case { name: "only a mode", input: (), expect: &[(Engine::MySqlAnsiQuotes, "")] },
    ];
    assert_eq!(
        gaps(&named("partial", &partial)),
        [
            "partial: case `no MariaDB` has no expectation for MariaDb",
            "partial: case `MySQL twice` names MySql twice",
            "partial: case `only a mode` has no expectation for Pg",
            "partial: case `only a mode` has no expectation for MySql",
            "partial: case `only a mode` has no expectation for MariaDb",
        ]
    );
    // A text with a `$` answers for a session that does not know the dollar reading too.
    let dollar = [Case {
        name: "dollar",
        input: "SELECT $$",
        expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MariaDb, "")],
    }];
    assert_eq!(
        gaps(&named_text("dollar", &dollar)),
        ["dollar: case `dollar` has no expectation for MySqlDollarNotKnown"]
    );
    assert!(gaps(&named("dollar", &dollar)).is_empty(), "only the text tables ask for it");
    // A case of rules asked of a session's facts answers for an unknown version too.
    let versioned = [Case {
        name: "known only",
        input: (),
        expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MariaDb, ""), (Engine::PgVersionUnknown, "")],
    }];
    assert_eq!(
        gaps(&named_versioned("versioned", &versioned)),
        [
            "versioned: case `known only` has no expectation for MySqlVersionUnknown",
            "versioned: case `known only` has no expectation for MariaDbVersionUnknown",
            "versioned: case `known only` has no expectation for MySqlVersionsDisagree",
            "versioned: case `known only` has no expectation for MariaDbVersionsDisagree",
        ]
    );
    assert!(gaps(&named("versioned", &versioned)).is_empty(), "only the tables of rules ask for it");
    let twice = [
        Case { name: "same", input: (), expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MariaDb, "")] },
        Case { name: "same", input: (), expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MariaDb, "")] },
    ];
    assert_eq!(gaps(&named("twice", &twice)), ["twice: case `same` is listed twice"]);
}

/// What each engine's value rules answer besides the typed value checks: which kinds are shown in
/// the time zone of the session that read them (the family's `TIMESTAMP` only), and what a
/// bytes value's text starts with.
#[test]
fn value_rules_on_every_engine() {
    use datarig_core::driver::{ArrayElement, ValueKind};
    use datarig_core::engine::EngineId;
    let kinds = [
        ValueKind::Text,
        ValueKind::Integer,
        ValueKind::Decimal,
        ValueKind::Float,
        ValueKind::Bool,
        ValueKind::Json,
        ValueKind::Bytes,
        ValueKind::Bit,
        ValueKind::Date,
        ValueKind::Time,
        ValueKind::Timestamp,
        ValueKind::TimestampTz,
        ValueKind::Interval,
        ValueKind::Array(ArrayElement::Text),
        ValueKind::Other,
    ];
    for id in EngineId::ALL {
        let values = id.spec().values;
        let (zoned, prefix): (&[ValueKind], &str) = match id {
            EngineId::Postgres => (&[], "\\x"),
            EngineId::MySql | EngineId::MariaDb => (&[ValueKind::TimestampTz], "0x"),
        };
        for kind in kinds {
            assert_eq!(values.shown_in_session_zone(kind), zoned.contains(&kind), "{id:?} {kind:?}");
        }
        assert_eq!(values.bytes_shown_prefix(), prefix, "{id:?}");
    }
}

/// How each engine's value rules write a value as an SQL literal, judge the columns of a
/// result copied as SQL, and write the clause of an identity column's `INSERT`, as today.
#[test]
fn value_rules_write_sql_on_every_engine() {
    use datarig_core::driver::{ReadIn, SessionValues, ValueKind};
    use datarig_core::engine::EngineId;
    use datarig_core::export::{Kind, SqlColumn};
    // A value of a kind, then its literal on PostgreSQL and on the MySQL family.
    let literals: &[(Kind, Option<&str>, &str, &str)] = &[
        (Kind::Number, None, "NULL", "NULL"),
        (Kind::Text, None, "NULL", "NULL"),
        (Kind::Number, Some("12"), "12", "12"),
        (Kind::Number, Some("-0"), "'-0'", "'-0'"),
        (Kind::Number, Some("NaN"), "'NaN'", "'NaN'"),
        (Kind::Bool, Some("true"), "true", "true"),
        (Kind::Bool, Some("t"), "'t'", "'t'"),
        (Kind::Text, Some("it's"), "'it''s'", "'it''s'"),
        (Kind::Text, Some("a\\b"), "E'a\\\\b'", "_utf8mb4 X'615C62'"),
        (Kind::Text, Some("a\0b"), "'a\0b'", "_utf8mb4 X'610062'"),
        (Kind::Bytes, Some("0x01FF"), "'0x01FF'", "X'01FF'"),
        (Kind::Bytes, Some("0x1"), "'0x1'", "'0x1'"),
        (Kind::Bytes, Some("\\x01ff"), "E'\\\\x01ff'", "_utf8mb4 X'5C7830316666'"),
        (Kind::Bit, Some("b'101'"), "'b''101'''", "b'101'"),
        (Kind::Bit, Some("101"), "'101'", "'101'"),
    ];
    let column = |name, type_name, kind| SqlColumn { name, type_name, kind, auto_increment: false };
    let columns = [
        column("t", "varchar", ValueKind::Text),
        column("b", "varbinary", ValueKind::Bytes),
        column("at", "timestamp", ValueKind::TimestampTz),
        SqlColumn { auto_increment: true, ..column("id", "int", ValueKind::Integer) },
    ];
    let cells: &[(usize, &str)] = &[(0, "x"), (1, "0x0"), (1, "0x00"), (2, "2024-01-01 00:00:00"), (3, "0"), (3, "5")];
    let floats = [column("f", "float", ValueKind::Float)];
    let read_in = ReadIn::Values(SessionValues { time_zone: "+02:00".into(), charset: "utf8mb4".into() });
    for id in EngineId::ALL {
        let values = id.spec().values;
        let pg = match id {
            EngineId::Postgres => true,
            EngineId::MySql | EngineId::MariaDb => false,
        };
        for (kind, v, on_pg, on_family) in literals {
            let want = if pg { on_pg } else { on_family };
            assert_eq!(values.literal(*kind, *v), *want, "{id:?} {kind:?} {v:?}");
        }
        let rules = values.sql_cells(&columns, &read_in, None).unwrap();
        let got: Vec<String> = cells
            .iter()
            .map(|(i, v)| match rules.cell(*i, v) {
                Ok(taken) => taken.into_owned(),
                Err(r) => format!("{} {:?}", r.column, r.why),
            })
            .collect();
        let want: &[&str] = if pg {
            &["x", "0x0", "0x00", "2024-01-01 00:00:00", "0", "5"]
        } else {
            &["x", "b NotShownWhole", "0x00", "2024-01-01 00:00:00+02:00", "id AutoIncrementZero", "5"]
        };
        assert_eq!(got, want, "{id:?}");
        let float =
            values.sql_cells(&floats, &read_in, None).map(|_| ()).map_err(|r| format!("{} {:?}", r.column, r.why));
        assert_eq!(float, if pg { Ok(()) } else { Err("f Float".to_string()) }, "{id:?}");
        let overriding = [values.insert_overriding(true), values.insert_overriding(false)];
        assert_eq!(overriding, if pg { [" OVERRIDING SYSTEM VALUE", ""] } else { ["", ""] }, "{id:?}");
    }
}

/// A copy as SQL and a keyset chunk on a MySQL session in another sql mode (`ANSI_QUOTES`,
/// `NO_BACKSLASH_ESCAPES`) are written as in the default mode, as today: a string with a
/// backslash is written as its bytes, which every mode reads the same.
#[test]
fn sql_cells_and_keyset_chunks_under_other_mysql_modes() {
    use datarig_core::driver::{ReadIn, SessionValues, ValueKind};
    use datarig_core::export::SqlColumn;
    const TEXT: &[SqlColumn<'static>; 1] =
        &[SqlColumn { name: "t", type_name: "varchar", kind: ValueKind::Text, auto_increment: false }];
    let copied = values::Copied {
        columns: TEXT,
        read_in: ReadIn::Values(SessionValues { time_zone: "+00:00".into(), charset: "utf8mb4".into() }),
        server_zone: None,
        values: &[Some("it's"), Some("a\\b"), None],
    };
    let table = export::Table {
        schema: "shop",
        table: "lines",
        columns: &[
            ("order_id", "int", ValueKind::Integer),
            ("code", "varchar", ValueKind::Text),
            ("qty", "int", ValueKind::Integer),
        ],
        key: &["order_id", "code"],
        last: &[Some("7"), Some("a\\b"), Some("1")],
    };
    for engine in [Engine::MySqlAnsiQuotes, Engine::MySqlNoBackslashEscapes] {
        let d = engine.dialect();
        assert_eq!(values::cells_in(d, &copied), "'it''s' | _utf8mb4 X'615C62' | NULL", "{engine:?}");
        assert_eq!(
            export::keyset_in(d, &table),
            "SELECT * FROM `shop`.`lines` ORDER BY `order_id`, `code` LIMIT 10000 | SELECT * FROM `shop`.`lines` WHERE `order_id` > 7 OR (`order_id` = 7 AND (`code` > _utf8mb4 X'615C62')) ORDER BY `order_id`, `code` LIMIT 10000",
            "{engine:?}"
        );
    }
}

/// What each engine's export rules answer: the kind of a key column of each kind, with text read
/// whole in the session and without (`-`: not a key), the condition past a key of one and of two
/// columns, whether a session's text is read whole, and whether the server drops a slow reader.
#[test]
fn export_rules_on_every_engine() {
    use datarig_core::driver::{ArrayElement, ColumnMeta, SessionValues, ValueKind};
    use datarig_core::engine::{EngineId, Fact};
    use datarig_core::export::Kind;
    // Type name, kind, then PostgreSQL's answer and the MySQL family's without and with text
    // read whole.
    let keys: &[(&str, ValueKind, &str, &str, &str)] = &[
        ("int", ValueKind::Integer, "Number", "Number", "Number"),
        ("numeric", ValueKind::Decimal, "Number", "Number", "Number"),
        ("money", ValueKind::Decimal, "-", "-", "-"),
        ("double", ValueKind::Float, "-", "-", "-"),
        ("text", ValueKind::Text, "Text", "-", "Text"),
        ("enum", ValueKind::Text, "Text", "-", "-"),
        ("SET", ValueKind::Text, "Text", "-", "-"),
        ("json", ValueKind::Json, "-", "-", "-"),
        ("bytea", ValueKind::Bytes, "Bytes", "Bytes", "Bytes"),
        ("bit", ValueKind::Bit, "-", "-", "-"),
        ("date", ValueKind::Date, "Text", "Text", "Text"),
        ("time", ValueKind::Time, "-", "-", "-"),
        ("timestamp", ValueKind::Timestamp, "Text", "Text", "Text"),
        ("timestamptz", ValueKind::TimestampTz, "Text", "-", "-"),
        ("interval", ValueKind::Interval, "-", "-", "-"),
        ("bool", ValueKind::Bool, "Bool", "Bool", "Bool"),
        ("int4[]", ValueKind::Array(ArrayElement::Number), "-", "-", "-"),
        ("uuid", ValueKind::Other, "Text", "-", "-"),
        ("inet", ValueKind::Other, "-", "-", "-"),
    ];
    let s = |v: &[&str]| v.iter().map(|v| v.to_string()).collect::<Vec<String>>();
    // Key columns (quoted), the last row's literals and the key's kinds.
    let one = (s(&["k"]), s(&["1"]), vec![Kind::Number]);
    let two = (s(&["k", "t"]), s(&["1", "'x'"]), vec![Kind::Number, Kind::Text]);
    let bytes_as_text = (s(&["b"]), s(&["'\\x01'"]), vec![Kind::Bytes]);
    let bytes = (s(&["b"]), s(&["X'01'"]), vec![Kind::Bytes]);
    let charset = |cs: &str| SessionValues { time_zone: "+00:00".into(), charset: cs.into() };
    let sessions =
        [None, Some(charset("utf8mb4")), Some(charset("UTF8MB4")), Some(charset("utf8mb3")), Some(charset("latin1"))];
    for id in EngineId::ALL {
        let Fact::Known(rules) = id.spec().export else { panic!("{id:?}: tables are read for an export") };
        let pg = match id {
            EngineId::Postgres => true,
            EngineId::MySql | EngineId::MariaDb => false,
        };
        for (type_name, kind, pg_kind, off, on) in keys {
            let c = ColumnMeta::new("c".into(), type_name.to_string(), *kind, None);
            for (text_ok, family) in [(false, off), (true, on)] {
                let want = if pg { pg_kind } else { family };
                let got = rules.key_kind(&c, text_ok).map_or("-".to_string(), |k| format!("{k:?}"));
                assert_eq!(got, *want, "{id:?} {type_name} text_ok {text_ok}");
            }
        }
        let after = |(n, v, k): &(Vec<String>, Vec<String>, Vec<Kind>)| rules.after(n, v, k);
        let (one_want, two_want, bytes_as_text_want, bytes_want) = if pg {
            (
                Some("k OPERATOR(pg_catalog.>) 1"),
                Some("(k, t) OPERATOR(pg_catalog.>) (1, 'x')"),
                Some("b OPERATOR(pg_catalog.>) '\\x01'"),
                Some("b OPERATOR(pg_catalog.>) X'01'"),
            )
        } else {
            (Some("k > 1"), Some("k > 1 OR (k = 1 AND (t > 'x'))"), None, Some("b > X'01'"))
        };
        assert_eq!(after(&one).as_deref(), one_want, "{id:?}");
        assert_eq!(after(&two).as_deref(), two_want, "{id:?}");
        assert_eq!(after(&bytes_as_text).as_deref(), bytes_as_text_want, "{id:?}");
        assert_eq!(after(&bytes).as_deref(), bytes_want, "{id:?}");
        let whole: Vec<bool> = sessions.iter().map(|s| rules.text_read_whole(s.as_ref())).collect();
        let want = if pg { [true; 5] } else { [false, true, true, false, false] };
        assert_eq!(whole, want, "{id:?}");
        assert_eq!(rules.drops_slow_readers(), !pg, "{id:?}");
    }
}

/// Which edits of a result's rows each engine's servers take, by the version of the server's
/// greeting: the format, or why there is none (MariaDB's, for the engine). The answer is the
/// same when the version the facts know is not (the rules read the greeting's number).
#[test]
fn edit_rules_on_every_engine() {
    use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PgFacts, ServerFacts, Topology, Version, Why};
    let facts = |id: EngineId, v: Version, version: Fact<Version>| ServerFacts {
        version,
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail: match id {
            EngineId::Postgres => EngineFacts::Postgres(PgFacts::default()),
            EngineId::MySql => EngineFacts::MySql(MyFacts::new(v, id)),
            EngineId::MariaDb => EngineFacts::MariaDb(MyFacts::new(v, id)),
        },
    };
    let mysql = "Known(MySql(MySqlEdit { nowait: true }))";
    for id in EngineId::ALL {
        // The servers asked, each with the engine's answer.
        let servers: &[((u16, u16, u16), &str)] = match id {
            EngineId::Postgres => &[((17, 2, 0), "Known(Postgres)")],
            EngineId::MySql => &[
                ((8, 0, 16), "No(VersionTooOld { needed: Version { major: 8, minor: 0, patch: 17 } })"),
                ((8, 0, 17), mysql),
                ((8, 4, 6), mysql),
            ],
            EngineId::MariaDb => &[((11, 4, 13), "No(NotBuilt)")],
        };
        for &((major, minor, patch), want) in servers {
            let v = Version::new(major, minor, patch);
            for version in [Fact::Known(v), Fact::Unknown(Why::VersionUnknown), Fact::Unknown(Why::VersionsDisagree)] {
                let f = facts(*id, v, version.clone());
                let got = match &id.spec().edit {
                    Fact::Known(rules) => format!("{:?}", rules.writer(&f)),
                    Fact::No(why) => format!("No({why:?})"),
                    Fact::Unknown(why) => format!("Unknown({why:?})"),
                };
                assert_eq!(got, want, "{id:?} {v:?} {version:?}");
            }
        }
    }
}
