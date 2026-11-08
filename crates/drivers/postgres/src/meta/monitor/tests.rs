use super::*;
use datarig_core::monitor::decode::pg::{COUNTERS, PROBE, QUERY_TEXT, SERVER_INFO, SESSIONS};
use datarig_core::monitor::{Flavour, Route};
use std::time::Duration;

fn pg(version: u32) -> PgMonitor {
    PgMonitor {
        version,
        superuser: true,
        read_all_stats: true,
        signal_backend: true,
        flavour: Flavour::Standard,
        route: Route::Unknown,
    }
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
        ("session", session(&p, 1), SESSIONS),
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

fn target(stamp: Option<StatementStamp>) -> ActionTarget {
    ActionTarget {
        key: datarig_core::monitor::SessionKey { id: 4242, started: Some(Timestamp(1_794_068_700_000_001)) },
        user: Some("zz_app".into()),
        thread: None,
        stamp,
    }
}

const RUNNING: Option<StatementStamp> =
    Some(StatementStamp::Pg { query_start: Some(Timestamp(1_794_068_760_000_002)) });

/// The action statements, and the follow-up read of one session.
#[test]
fn action_statements() {
    let direct = PgMonitor { route: Route::Direct, ..pg(170011) };
    let pooled = PgMonitor { route: Route::Pooled, ..pg(170011) };
    let never = Some(StatementStamp::Pg { query_start: None });
    let text = [
        ("cancel", action(&direct, ActionKind::Cancel, &target(RUNNING))),
        ("cancel, never ran a statement", action(&direct, ActionKind::Cancel, &target(never))),
        ("cancel, no stamp", action(&direct, ActionKind::Cancel, &target(None))),
        ("terminate, direct", action(&direct, ActionKind::Terminate, &target(RUNNING))),
        ("terminate, pooled", action(&pooled, ActionKind::Terminate, &target(RUNNING))),
        ("terminate, route unknown", action(&pg(170011), ActionKind::Terminate, &target(RUNNING))),
        ("backend pid", backend_pid()),
        ("follow-up", session(&direct, 4242)),
    ]
    .map(|(name, sql)| format!("-- {name}\n{sql}\n"));
    insta::assert_snapshot!("actions", text.join("\n"));
}

#[test]
fn an_action_compares_times_as_the_tick_reads_them_and_never_signals_itself() {
    let direct = PgMonitor { route: Route::Direct, ..pg(170011) };
    let cancel = action(&direct, ActionKind::Cancel, &target(RUNNING));
    // The same expression as the session list, so the integers compare exactly.
    assert!(sessions(&direct, IDLE).contains(&us("s.backend_start")));
    assert!(cancel.contains(&format!("{} = 1794068700000001", us("a.backend_start"))), "{cancel}");
    assert!(cancel.contains(&format!("{} IS NOT DISTINCT FROM 1794068760000002", us("a.query_start"))), "{cancel}");
    assert!(cancel.contains("WHERE a.pid = 4242"), "{cancel}");
    assert!(
        cancel.contains(
            "CASE WHEN NOT g.own AND g.session AND g.tracked AND g.statement THEN pg_cancel_backend(g.pid) END"
        )
    );
    assert!(cancel.contains("COALESCE(a.state <> 'disabled', false) AS tracked"), "{cancel}");
    // No start then: nothing tells a later client apart.
    let never = action(&direct, ActionKind::Terminate, &target(None));
    assert!(never.contains("false AS tracked"), "{never}");
    // An end goes by the last statement too (the session may be a pooler's server connection,
    // serving another client after it), not by its running where the monitor connects directly.
    let end = action(&direct, ActionKind::Terminate, &target(RUNNING));
    assert!(end.contains(
        "CASE WHEN NOT g.own AND g.session AND g.tracked AND g.statement THEN pg_terminate_backend(g.pid) END"
    ));
    assert!(end.contains(&format!("{} IS NOT DISTINCT FROM 1794068760000002, false)", us("a.query_start"))), "{end}");
    let pooled = action(&PgMonitor { route: Route::Pooled, ..direct }, ActionKind::Terminate, &target(RUNNING));
    assert!(pooled.contains("AND g.tracked AND g.statement THEN pg_terminate_backend(g.pid)"), "{pooled}");
    assert!(session(&direct, 4242).contains("WHERE a.pid = 4242 OR a.monitor ORDER BY a.pid"));
}
