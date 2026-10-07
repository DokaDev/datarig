//! Round trips per statement of the MySQL driver through the latency proxy, as `rtt` measures
//! the PostgreSQL driver's: what the query session costs for a small result, a page of a large
//! one (not held by default: the session's `sql_select_limit` stops it), the next page (the
//! statement run again past the rows it has, after the allowlist's question to the server), a
//! held result's next page, a count, a write, and the same inside the user's transaction and on a
//! read-only profile. The rows are a temporary table of the session's own (the server needs no
//! test data, only a database the user may make a temporary table in).

use crate::rtt::{WiredSession, report, scenario};
use datarig_core::driver::{DbCommand, DbEvent, PagingMode, SessionRole};
use datarig_core::sql::dialect::MySqlMode;
use datarig_driver_mysql::MyDriver;
use serde_json::{Value, json};
use std::time::Duration;

/// The rows of the temporary table: more than four pages.
const ROWS: u32 = 2_200;

async fn open(url: &str, one_way: Duration) -> Result<WiredSession, String> {
    let mut s = WiredSession::open_with(&MyDriver, url, one_way, false, SessionRole::Query).await?;
    s.cost("CREATE TEMPORARY TABLE zz_bench_rtt (id INT PRIMARY KEY, v VARCHAR(40))").await?;
    let fill = format!(
        "INSERT INTO zz_bench_rtt WITH RECURSIVE d (i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM d WHERE i < 99) \
         SELECT g.n, CONCAT('row ', g.n) FROM (SELECT a.i + b.i * 100 + 1 AS n FROM d a, d b) g WHERE g.n <= {ROWS}"
    );
    s.cost(&fill).await?;
    Ok(s)
}

pub async fn run(url: &str, one_way: Duration, runs: usize) -> Result<Value, String> {
    println!("rtt_mysql: {runs} runs per statement, {} ms one way", one_way.as_millis());
    let mut out = Vec::new();
    // Opening a session: the server's greeting, the login, and the session's settings in one
    // statement. The login as the server has cached it: its first after a start (a fresh CI
    // service) also asks for the server's public key and sends the password encrypted with it,
    // two round trips more, so one login goes first, not measured.
    WiredSession::open_with(&MyDriver, url, Duration::ZERO, false, SessionRole::Query).await?.quiet().await;
    let mut costs = Vec::new();
    for _ in 0..runs.min(5) {
        let s = WiredSession::open_with(&MyDriver, url, one_way, false, SessionRole::Query).await?;
        s.quiet().await;
        let n = s.flights();
        costs.push(crate::rtt::Cost { to_result: n, total: n, ms: 0.0 });
    }
    out.push(report("session_open", &costs));
    let mut s = open(url, one_way).await?;
    let small = "SELECT * FROM zz_bench_rtt WHERE id <= 100";
    let big = "SELECT * FROM zz_bench_rtt";
    out.push(scenario(&mut s, "select_small", runs, 1, |_| small.to_string()).await?);
    out.push(scenario(&mut s, "select_page_of_many", runs, 1, |_| big.to_string()).await?);
    // The next page: the statement runs again past the rows it has (the allowlist's question,
    // the larger limit, the statement); the limit goes back with the next statement that needs
    // it.
    let mut costs = Vec::new();
    for _ in 0..runs {
        let id = s.next_id();
        let resume = DbCommand::Resume { id, sql: big.to_string(), skip: 500, paging: PagingMode::NoHold };
        let (c, ev) = s
            .measure(resume, |e| matches!(e, DbEvent::Page { id: p, .. } | DbEvent::Failed { id: p, .. } if *p == id))
            .await?;
        if !matches!(ev, DbEvent::Page { more: true, .. }) {
            return Err(format!("resume: {ev:?}"));
        }
        costs.push(c);
    }
    out.push(report("resume", &costs));
    out.push(scenario(&mut s, "select_page_after_resume", 1, 0, |_| big.to_string()).await?);
    // A held result (`paging = "hold"`): its pages are read from the result as it comes.
    s.paging = PagingMode::Hold;
    out.push(scenario(&mut s, "hold_select_page_of_many", runs, 1, |_| big.to_string()).await?);
    let pages = s.next_pages(big, 3).await?;
    out.push(report("next_page", &pages));
    s.paging = PagingMode::NoHold;
    let count =
        datarig_core::sql::risk::mysql::count_query(small, MySqlMode::default()).map_err(|e| format!("{e:?}"))?;
    let mut costs = Vec::new();
    for _ in 0..runs {
        let id = s.next_id();
        let (c, ev) =
            s.measure(DbCommand::Count { id, sql: count.clone() }, |e| matches!(e, DbEvent::Counted { .. })).await?;
        if !matches!(ev, DbEvent::Counted { result: Ok(100), .. }) {
            return Err(format!("count: {ev:?}"));
        }
        costs.push(c);
    }
    out.push(report("count", &costs));
    out.push(
        scenario(&mut s, "insert", runs, 0, |i| format!("INSERT INTO zz_bench_rtt VALUES ({}, 'x')", 10_000 + i))
            .await?,
    );
    s.cost("BEGIN").await?;
    out.push(scenario(&mut s, "select_small_in_block", runs, 1, |_| small.to_string()).await?);
    out.push(
        scenario(&mut s, "insert_in_block", runs, 0, |i| {
            format!("INSERT INTO zz_bench_rtt VALUES ({}, 'y')", 20_000 + i)
        })
        .await?,
    );
    s.cost("ROLLBACK").await?;
    // A read-only profile (the session is read-only on the server: no table of its own).
    let mut r = WiredSession::open_with(&MyDriver, url, one_way, true, SessionRole::Query).await?;
    let ro_small = "SELECT * FROM (SELECT 1 AS id UNION ALL SELECT 2) AS t";
    out.push(scenario(&mut r, "ro_select_small", runs, 1, |_| ro_small.to_string()).await?);
    Ok(json!({ "one_way_ms": one_way.as_millis() as u64, "scenarios": out }))
}
