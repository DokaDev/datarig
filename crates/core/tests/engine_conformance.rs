//! The same cases on every engine: what the core answers today for PostgreSQL, MySQL and
//! MariaDB, side by side, so that a change of one engine's answer is a change of this table.
//!
//! Each table lists cases (an input) with the exact output expected on every engine. The
//! engines are PostgreSQL ([`Dialect::Postgres`]), MySQL (the default [`MySqlMode`]) and
//! MariaDB (`MySqlMode { mariadb: true, .. }`, what a MariaDB session says today). A case may
//! also pin a MySQL sql mode where it changes the output (`ANSI_QUOTES`,
//! `NO_BACKSLASH_ESCAPES`). A case without an expectation for one of the three engines fails
//! ([`every_case_names_every_engine`]): adding an engine means answering every case for it.
//!
//! Every expectation is today's output, as it is, right or wrong: the table pins it, and a
//! change that moves it updates the table in the same change. No server is needed.

#[path = "engine_conformance/context.rs"]
mod context;
#[path = "engine_conformance/export.rs"]
mod export;
#[path = "engine_conformance/filter.rs"]
mod filter;
#[path = "engine_conformance/risk.rs"]
mod risk;
#[path = "engine_conformance/text.rs"]
mod text;
#[path = "engine_conformance/tx.rs"]
mod tx;
#[path = "engine_conformance/values.rs"]
mod values;

use datarig_core::sql::dialect::{Dialect, MySqlMode};
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
}

/// The engines every case must have an expectation for.
pub const ENGINES: [Engine; 3] = [Engine::Pg, Engine::MySql, Engine::MariaDb];

impl Engine {
    /// The dialect the core reads and writes this engine's SQL in today.
    pub fn dialect(self) -> Dialect {
        match self {
            Engine::Pg => Dialect::Postgres,
            Engine::MySql => Dialect::MySql(MySqlMode::default()),
            Engine::MariaDb => Dialect::MySql(MySqlMode { mariadb: true, ..MySqlMode::default() }),
            Engine::MySqlAnsiQuotes => Dialect::MySql(MySqlMode { ansi_quotes: true, ..MySqlMode::default() }),
            Engine::MySqlNoBackslashEscapes => {
                Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() })
            }
        }
    }
}

/// One case: an input and its output on each engine, exactly.
pub struct Case<I> {
    pub name: &'static str,
    pub input: I,
    pub expect: &'static [(Engine, &'static str)],
}

/// What [`every_case_names_every_engine`] reads of a case: its table, its name and the engines
/// it has expectations for.
pub struct Named {
    pub table: &'static str,
    pub case: &'static str,
    pub engines: Vec<Engine>,
}

/// The cases of table `table` as [`every_case_names_every_engine`] reads them.
pub fn named<I>(table: &'static str, cases: &[Case<I>]) -> Vec<Named> {
    cases.iter().map(|c| Named { table, case: c.name, engines: c.expect.iter().map(|(e, _)| *e).collect() }).collect()
}

/// What is wrong with the expectations of `cases`: a case that misses one of [`ENGINES`],
/// names an engine twice, or shares its name with another case of its table.
pub fn gaps(cases: &[Named]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for c in cases {
        if !seen.insert((c.table, c.case)) {
            out.push(format!("{}: case `{}` is listed twice", c.table, c.case));
        }
        for e in ENGINES {
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
    let mut report = String::new();
    for gap in gaps(&named(table, cases)) {
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
    run("classifier", &risk::classifier_cases(), risk::classify);
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
    run("split", &text::split_cases(), text::split);
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
fn edit_input_cases_on_every_engine() {
    run("edit input", &values::input_cases(), values::input);
}

#[test]
fn sql_cells_cases_on_every_engine() {
    run("sql cells", &values::cells_cases(), values::cells);
}

#[test]
fn keyset_chunk_sql_on_every_engine() {
    run("keyset chunk", &export::keyset_cases(), export::keyset);
}

#[test]
fn tx_step_cases_on_every_engine() {
    run("tx step", &tx::cases(), tx::step);
}

/// The dialect today's MariaDB session reads in (`MySqlMode { mariadb: true, .. }`), in the
/// MariaDB variant: what a MariaDB session says once sessions name their engine.
fn mariadb_variant() -> Dialect {
    Dialect::MariaDb(MySqlMode { mariadb: true, ..MySqlMode::default() })
}

/// The differences between `f`'s output on each case of `table` and the case's MariaDB
/// expectation.
fn against_mariadb<I>(report: &mut String, table: &str, cases: &[Case<I>], f: impl Fn(&I) -> String) {
    for c in cases {
        let Some((_, today)) = c.expect.iter().find(|(e, _)| *e == Engine::MariaDb) else {
            let _ = writeln!(report, "{table}: case `{}` has no MariaDB expectation", c.name);
            continue;
        };
        let (expected, actual) = (*today, f(&c.input));
        if actual != expected {
            let _ = writeln!(
                report,
                "{table}: case `{}` on the MariaDB variant\n  expected: {expected:?}\n  actual:   {actual:?}",
                c.name
            );
        }
    }
}

/// Every table answers on the MariaDB variant exactly as on today's MariaDB: risk, quoting
/// and splitting, the transaction step, the verdicts of a call in every context (the filter
/// bar's included), the filter's SQL and what it compares, typed and copied values, and the
/// keyset export's chunks.
#[test]
fn the_mariadb_variant_answers_as_today() {
    let d = mariadb_variant();
    let mut report = String::new();
    against_mariadb(&mut report, "classifier", &risk::classifier_cases(), |s| risk::classify_in(d, s));
    against_mariadb(&mut report, "quoting", &text::quoting_cases(), |s| text::quoting_in(d, s));
    against_mariadb(&mut report, "split", &text::split_cases(), |s| text::split_in_dialect(d, s));
    against_mariadb(&mut report, "tx step", &tx::cases(), |s| tx::step_in(d, s));
    against_mariadb(&mut report, "function verdicts", &context::cases(), |s| context::verdicts_in(d, s));
    against_mariadb(&mut report, "filter render", &filter::render_cases(), |i| filter::render_in(d, i));
    against_mariadb(&mut report, "compares", &filter::compares_cases(), |c| filter::compares_in(d, c));
    against_mariadb(&mut report, "edit input", &values::input_cases(), |t| values::input_in(d, t));
    against_mariadb(&mut report, "sql cells", &values::cells_cases(), |c| values::cells_in(d, c));
    against_mariadb(&mut report, "keyset chunk", &export::keyset_cases(), |t| export::keyset_in(d, t));
    assert!(report.is_empty(), "{report}");
}

#[test]
fn every_case_names_every_engine() {
    let all: Vec<Named> = [
        named("classifier", &risk::classifier_cases()),
        named("function verdicts", &context::cases()),
        named("quoting", &text::quoting_cases()),
        named("split", &text::split_cases()),
        named("filter render", &filter::render_cases()),
        named("compares", &filter::compares_cases()),
        named("edit input", &values::input_cases()),
        named("sql cells", &values::cells_cases()),
        named("keyset chunk", &export::keyset_cases()),
        named("tx step", &tx::cases()),
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
    let twice = [
        Case { name: "same", input: (), expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MariaDb, "")] },
        Case { name: "same", input: (), expect: &[(Engine::Pg, ""), (Engine::MySql, ""), (Engine::MariaDb, "")] },
    ];
    assert_eq!(gaps(&named("twice", &twice)), ["twice: case `same` is listed twice"]);
}
