//! Every verdict of the classifiers, recorded: the statements of the MySQL-family corpus and of
//! the classifier table, built through each engine's rules (`RiskRules::new_classifier`) and
//! judged on its own engine and on both of the family, and the function verdicts of the context
//! table on every engine. A change of any classifier's answer is a change of
//! `golden/verdicts.txt`; `DATARIG_BLESS=1` writes the file anew.

use crate::verdict;
use datarig_core::sql::dialect::{Dialect, MySqlMode};
use datarig_core::sql::risk::mysql::Judged;

include!("../../src/sql/risk/mysql/corpus.rs");

/// One line per verdict: the engine, how it judges, the statement, the risk, the read-only
/// verdict and whether it may run again.
fn line(out: &mut String, d: Dialect, judged: Judged, sql: &str) {
    let mut c = d.engine().spec().risk.new_classifier(d, judged);
    let risk = c.classify(sql);
    let again = c.repeatable(sql);
    out.push_str(&format!(
        "{:?} {judged:?} {sql:?}: {risk:?} | read-only {} | again {}\n",
        d.engine(),
        verdict(&risk.read_only()),
        verdict(&again)
    ));
}

fn verdicts() -> String {
    let mut out = String::new();
    let family = [Dialect::MySql(MySqlMode::default()), Dialect::MariaDb(MySqlMode::default())];
    for (sql, _) in CORPUS {
        for d in family {
            for judged in [Judged::Own, Judged::WithSibling] {
                line(&mut out, d, judged, sql);
            }
        }
    }
    for case in crate::risk::classifier_cases() {
        for d in [Dialect::Postgres, family[0], family[1]] {
            for judged in [Judged::Own, Judged::WithSibling] {
                line(&mut out, d, judged, case.input);
            }
        }
    }
    for case in crate::context::cases() {
        for d in [Dialect::Postgres, family[0], family[1]] {
            out.push_str(&format!(
                "context {:?} {:?}: {}\n",
                d.engine(),
                case.input,
                crate::context::verdicts_in(d, case.input)
            ));
        }
    }
    out
}

#[test]
fn classifier_verdicts_are_todays() {
    let got = verdicts();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engine_conformance/golden/verdicts.txt");
    if std::env::var("DATARIG_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &got).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (got, want): (Vec<&str>, Vec<&str>) = (got.lines().collect(), want.lines().collect());
    for (n, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "line {} (DATARIG_BLESS=1 cargo test -p datarig-core --test engine_conformance)", n + 1);
    }
    assert_eq!(got.len(), want.len());
}

/// The filter check's call lists of each engine (its classifier rules' `filter_calls`), one
/// name per line under its engine and list.
fn filter_calls() -> String {
    use datarig_core::engine::EngineId;
    let mut out = String::new();
    for (engine, id) in [("Postgres", EngineId::Postgres), ("MySql", EngineId::MySql), ("MariaDb", EngineId::MariaDb)] {
        let lists = id.spec().risk.filter_calls();
        for (list, names) in [("allowed", lists.allowed), ("refused", lists.refused)] {
            for name in names {
                out.push_str(&format!("{engine} {list} {name}\n"));
            }
        }
    }
    out
}

#[test]
fn the_filter_call_lists_are_todays() {
    let got = filter_calls();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engine_conformance/golden/filter_calls.txt");
    if std::env::var("DATARIG_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &got).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(got, want, "DATARIG_BLESS=1 cargo test -p datarig-core --test engine_conformance");
}

/// What each engine's classifier rules answer besides the classifier: whether the environment's
/// read-only functions apply (PostgreSQL only), whether a styled first page is checked by a
/// re-run (PostgreSQL only: the allowlist's answer), and that no family risk ever names a call
/// for the read-only context to look up (so that context is PostgreSQL's alone).
#[test]
fn risk_rules_on_every_engine() {
    use datarig_core::engine::{EngineId, Fact, Why};
    use datarig_core::sql::risk::mysql::Judged;
    for id in EngineId::ALL {
        let risk = id.spec().risk;
        let pg = *id == EngineId::Postgres;
        let want = if pg { Fact::Known(()) } else { Fact::No(Why::NotOnEngine) };
        assert_eq!(risk.read_only_calls(), want, "{id:?}");
        for (sql, again) in [("SELECT 1", true), ("SELECT random()", false), ("DELETE FROM t", false)] {
            assert_eq!(risk.rerun_style(sql), pg.then_some(again), "{id:?} {sql}");
        }
    }
    let family = [Dialect::MySql(MySqlMode::default()), Dialect::MariaDb(MySqlMode::default())];
    for (sql, _) in CORPUS {
        for d in family {
            for judged in [Judged::Own, Judged::WithSibling] {
                let risk = d.engine().spec().risk.new_classifier(d, judged).classify(sql);
                assert!(risk.unchecked_calls.is_empty(), "{d:?} {judged:?} {sql}: {:?}", risk.unchecked_calls);
            }
        }
    }
}
