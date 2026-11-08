use super::*;
use datarig_core::monitor::Flavour;
use datarig_core::monitor::decode::pg::{COUNTERS, PROBE, QUERY_TEXT, SERVER_INFO, SESSIONS};
use std::time::Duration;

fn pg(version: u32) -> PgMonitor {
    PgMonitor { version, superuser: true, read_all_stats: true, signal_backend: true, flavour: Flavour::Standard }
}

const IDLE: SessionsWanted = SessionsWanted { idle: true };
const NO_IDLE: SessionsWanted = SessionsWanted { idle: false };

/// Every statement of a version, each with the shape its decoder reads.
fn every(version: u32) -> Vec<(&'static str, String, &'static [&'static str])> {
    let p = pg(version);
    vec![
        ("probe", probe(), PROBE),
        ("server info", server_info(), SERVER_INFO),
        ("counters", counters(), COUNTERS),
        ("sessions", sessions(&p, IDLE), SESSIONS),
        ("sessions without idle", sessions(&p, NO_IDLE), SESSIONS),
        ("query text", query_text(4211), QUERY_TEXT),
    ]
}

#[test]
fn statements_per_version() {
    for version in [120000, 130000, 170011] {
        let text: Vec<String> = every(version).iter().map(|(name, sql, _)| format!("-- {name}\n{sql}\n")).collect();
        insta::assert_snapshot!(format!("pg_{version}"), text.join("\n"));
    }
}

#[test]
fn every_provider_marker_is_asked_for() {
    let probe = probe();
    for (marker, _) in PG_MARKERS {
        let (Marker::Role(name) | Marker::Function(name)) = marker else { panic!("{marker:?}") };
        assert!(probe.contains(&format!("'{name}'")), "{name}");
    }
}

#[test]
fn a_tick_reads_what_is_wanted_in_order_in_a_bounded_read_only_transaction() {
    let p = pg(170011);
    let limit = Duration::from_secs(2);
    assert_eq!(tick(&p, &Want { sessions: None, server: false, limit }), [counters()]);
    let all = Want { sessions: Some(NO_IDLE), server: true, limit };
    assert_eq!(tick(&p, &all), [counters(), sessions(&p, NO_IDLE), server_info()]);
    let begin = begin(limit);
    assert!(begin.starts_with("BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY NOT DEFERRABLE;"), "{begin}");
    assert!(begin.contains("SET LOCAL search_path = pg_catalog, pg_temp"), "{begin}");
    assert!(
        begin.contains("SET LOCAL lock_timeout = '1s'") && begin.contains("SET LOCAL statement_timeout = '2000ms'")
    );
    assert!(super::begin(Duration::ZERO).contains("'1ms'"), "never 0, which means no limit");
}

/// Each statement of every version gives exactly the columns its decoder reads, in order: the
/// server describes it (prepared, never run). Against `DATARIG_TEST_PG_URL`; the older
/// versions' statements only leave columns out of newer catalogs, so they describe on any
/// newer server.
#[tokio::test]
async fn every_statement_gives_its_decoders_columns() {
    let url = match std::env::var("DATARIG_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            assert!(!std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1"), "DATARIG_TEST_PG_URL must be set");
            eprintln!("SKIPPED every_statement_gives_its_decoders_columns: set DATARIG_TEST_PG_URL");
            return;
        }
    };
    let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await.expect("connect");
    tokio::spawn(conn);
    for version in [120000, 130000, 170011] {
        for (name, sql, shape) in every(version) {
            let statement = client.prepare(&sql).await.unwrap_or_else(|e| panic!("{version} {name}: {e}"));
            let columns: Vec<&str> = statement.columns().iter().map(|c| c.name()).collect();
            assert_eq!(columns, shape, "{version} {name}");
        }
    }
}
