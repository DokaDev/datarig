use super::*;
use datarig_core::monitor::Flavour;
use datarig_core::monitor::decode::mysql::{CLOCK, NAME_VALUE, PROBE, PROBE_LOCKS, QUERY_TEXT, SERVER_INFO, SESSIONS};
use std::time::Duration;

fn my(version: u32, performance_schema: bool, readable: Readable) -> MySqlMonitor {
    MySqlMonitor {
        version,
        performance_schema,
        user: "datarig".into(),
        process: Readable::Unknown,
        innodb_trx: readable,
        metadata_locks: readable,
        flavour: Flavour::Standard,
    }
}

const IDLE: SessionsWanted = SessionsWanted { idle: true };
const NO_IDLE: SessionsWanted = SessionsWanted { idle: false };

/// Every statement of a version, each with the shape its decoder reads.
fn every(m: &MySqlMonitor) -> Vec<(&'static str, String, &'static [&'static str])> {
    let probe = probe();
    let shapes: [&'static [&'static str]; 5] = [PROBE, NAME_VALUE, &["PRIVILEGE_TYPE"], &["trx_id"], PROBE_LOCKS];
    let mut out: Vec<_> = probe.into_iter().zip(shapes).map(|(sql, shape)| ("probe", sql, shape)).collect();
    let info = server_info();
    out.push(("server info", info[0].clone(), SERVER_INFO));
    out.push(("server info uptime", info[1].clone(), NAME_VALUE));
    let c = counters();
    out.push(("clock", c[0].clone(), CLOCK));
    out.push(("status", c[1].clone(), NAME_VALUE));
    out.push(("sessions", sessions(m, IDLE).remove(0), SESSIONS));
    out.push(("sessions without idle", sessions(m, NO_IDLE).remove(0), SESSIONS));
    out.push(("query text", query_text(4211), QUERY_TEXT));
    out
}

fn variants() -> [(&'static str, MySqlMonitor); 4] {
    [
        ("8_0_17", my(80017, true, Readable::Yes)),
        ("8_0_22_no_trx", my(80022, true, Readable::Refused)),
        ("8_4", my(80411, true, Readable::Yes)),
        ("8_4_no_performance_schema", my(80411, false, Readable::Yes)),
    ]
}

#[test]
fn statements_per_version() {
    for (name, m) in variants() {
        let text: Vec<String> = every(&m).iter().map(|(n, sql, _)| format!("-- {n}\n{sql}\n")).collect();
        insta::assert_snapshot!(format!("mysql_{name}"), text.join("\n"));
    }
}

#[test]
fn every_provider_marker_is_asked_for_and_never_the_host_name() {
    let probe = probe().join("\n");
    for (marker, _) in MYSQL_MARKERS {
        match marker {
            Marker::Variable(name) => assert!(probe.contains(&format!("'{name}'")), "{name}"),
            Marker::Basedir(_) => assert!(probe.contains("'basedir'")),
            other => panic!("{other:?} is not a MySQL marker"),
        }
    }
    assert!(!probe.contains("hostname"));
    assert!(super::probe()[2].contains("PRIVILEGE_TYPE = 'PROCESS'"), "SUPER does not show sessions");
}

#[test]
fn idle_sessions_are_left_out_only_when_their_transactions_are_known() {
    let with = sessions(&my(80411, true, Readable::Yes), NO_IDLE).join("");
    assert!(with.contains("p.COMMAND <> 'Sleep' OR t.trx_id IS NOT NULL OR l.id IS NOT NULL"));
    for unknown in [Readable::Refused, Readable::Unknown] {
        let without = sessions(&my(80411, true, unknown), NO_IDLE).join("");
        assert!(!without.contains("WHERE"), "{without}");
    }
    // InnoDB transactions known, metadata locks not: a session may still hold a transaction.
    let locks_unknown = MySqlMonitor { metadata_locks: Readable::Refused, ..my(80411, true, Readable::Yes) };
    assert!(!sessions(&locks_unknown, NO_IDLE)[0].contains("p.COMMAND <> 'Sleep'"));
    assert!(sessions(&my(80021, true, Readable::Yes), IDLE)[0].contains("information_schema.PROCESSLIST"));
    assert!(sessions(&my(80022, true, Readable::Yes), IDLE)[0].contains("performance_schema.processlist"));
}

#[test]
fn a_tick_reads_what_is_wanted_in_order() {
    let m = my(80411, true, Readable::Yes);
    let limit = Duration::from_secs(2);
    assert_eq!(tick(&m, &Want { sessions: None, server: false, limit }), counters());
    let statements = tick(&m, &Want { sessions: Some(IDLE), server: true, limit });
    assert_eq!(statements.len(), 5);
    assert_eq!(statements[..2], counters()[..]);
    assert_eq!(statements[2..3], sessions(&m, IDLE)[..]);
    assert_eq!(statements[3..], server_info()[..]);
    assert_eq!(super::limit(limit), "SET SESSION max_execution_time = 2000");
    assert_eq!(super::limit(Duration::ZERO), "SET SESSION max_execution_time = 1");
}

/// Each statement of every variant gives exactly the columns its decoder reads, in order, on
/// the real server (`DATARIG_TEST_MYSQL_ADMIN_URL`, an account that may read them all). They
/// only read; the older versions' statements read tables the newer servers still have.
#[tokio::test]
async fn every_statement_gives_its_decoders_columns() {
    let url = match std::env::var("DATARIG_TEST_MYSQL_ADMIN_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            let required = std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1");
            assert!(!required, "DATARIG_TEST_MYSQL_ADMIN_URL must be set");
            eprintln!("SKIPPED every_statement_gives_its_decoders_columns: set DATARIG_TEST_MYSQL_ADMIN_URL");
            return;
        }
    };
    use mysql_async::prelude::Queryable;
    let opts =
        mysql_async::OptsBuilder::from_opts(mysql_async::Opts::from_url(&url).expect("a URL")).prefer_socket(false);
    let mut conn = mysql_async::Conn::new(opts).await.expect("connect");
    for (variant, m) in variants() {
        for (name, sql, shape) in every(&m) {
            let result = conn.query_iter(sql.as_str()).await.unwrap_or_else(|e| panic!("{variant} {name}: {e}"));
            let columns: Vec<String> = result.columns_ref().iter().map(|c| c.name_str().into_owned()).collect();
            drop(result.drop_result().await);
            assert_eq!(columns, shape, "{variant} {name}");
        }
    }
    let _ = conn.disconnect().await;
}
