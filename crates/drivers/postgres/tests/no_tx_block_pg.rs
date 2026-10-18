//! The classifier's [`Risk::no_tx_block`] against a real PostgreSQL (`DATARIG_TEST_PG_URL`, see
//! `integration_pg.rs`; the user must be a superuser): every form it says the server refuses
//! inside a transaction block fails there with SQLSTATE 25001 (`active_sql_transaction`), and
//! every form it says runs there does. Each statement runs inside `BEGIN`, which is then rolled
//! back, so nothing a form would do outside a block happens (no database, tablespace, setting
//! or replication slot is made; a subscription is only ever created with `connect = false`,
//! inside the block, so the server never connects out).
//!
//! The objects the forms act on live in a schema `zz_ntx_<pid>` that the test creates and
//! drops (also when it fails); other names are `zz_ntx_`-prefixed and never exist.
//!
//! [`Risk::no_tx_block`]: datarig_core::sql::risk::Risk::no_tx_block

// Its table guard is not used here: this test drops a schema of its own.
#[allow(dead_code)]
mod pg_clean;

use datarig_core::sql::risk::classify;
use std::io::Write;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, NoTls};

fn pg_url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => {
            pg_clean::sweep_stale(&u);
            pg_clean::hold_slot();
            Some(u)
        }
        _ => {
            if std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_PG_URL must be set when DATARIG_REQUIRE_PG=1");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: set DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig"
            );
            None
        }
    }
}

/// Drops the test's schema when the test ends, also when it fails.
struct SchemaGuard(String, String);

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        let _ = pg_clean::run_fresh(&self.0, &format!("DROP SCHEMA IF EXISTS {} CASCADE", self.1));
    }
}

/// The forms the server refuses inside a block: what runs first in the same block, and the
/// form. `{s}` is the test's schema, `{sub}` the subscription's name.
const REFUSED: &[(&[&str], &str)] = &[
    (&[], "VACUUM"),
    (&[], "VACUUM {s}.t"),
    (&[], "VACUUM (ANALYZE) {s}.t"),
    (&[], "VACUUM FULL {s}.t"),
    (&[], "CREATE INDEX CONCURRENTLY zz_ntx_i ON {s}.t (id)"),
    (&[], "DROP INDEX CONCURRENTLY {s}.t_a"),
    (&[], "REINDEX INDEX CONCURRENTLY {s}.t_a"),
    (&[], "REINDEX (CONCURRENTLY) TABLE {s}.t"),
    (&[], "REINDEX SCHEMA {s}"),
    (&[], "REINDEX SYSTEM"),
    (&[], "REINDEX DATABASE"),
    (&[], "CLUSTER"),
    (&[], "CREATE DATABASE zz_ntx_db"),
    (&[], "DROP DATABASE IF EXISTS zz_ntx_none"),
    (&[], "ALTER DATABASE zz_ntx_none SET TABLESPACE pg_default"),
    (&[], "CREATE TABLESPACE zz_ntx_ts LOCATION '/zz_ntx_none'"),
    (&[], "DROP TABLESPACE IF EXISTS zz_ntx_none"),
    (&[], "ALTER SYSTEM SET work_mem = '64MB'"),
    (&[], "DISCARD ALL"),
    (&[], "COMMIT PREPARED 'zz_ntx_none'"),
    (&[], "ROLLBACK PREPARED 'zz_ntx_none'"),
    (&[], "ALTER TABLE {s}.p DETACH PARTITION {s}.p1 CONCURRENTLY"),
    // Refused before it would create the slot (or connect).
    (&[], "CREATE SUBSCRIPTION {sub} CONNECTION 'host=127.0.0.1 port=1 dbname=zz_ntx' PUBLICATION zz_ntx_p1"),
    // A subscription made with `connect = false` has a slot name: dropping it drops the slot.
    (&[SUBSCRIPTION_OFF], "DROP SUBSCRIPTION {sub}"),
    (&[SUBSCRIPTION_OFF], "ALTER SUBSCRIPTION {sub} SET (failover = true)"),
    // Refreshing needs an enabled subscription (enabled inside the block: no worker starts).
    (&[SUBSCRIPTION_OFF, ENABLE], "ALTER SUBSCRIPTION {sub} REFRESH PUBLICATION"),
    (&[SUBSCRIPTION_OFF, ENABLE], "ALTER SUBSCRIPTION {sub} REFRESH PUBLICATION WITH (copy_data = false)"),
    (&[SUBSCRIPTION_OFF, ENABLE], "ALTER SUBSCRIPTION {sub} SET PUBLICATION zz_ntx_p3"),
    (&[SUBSCRIPTION_OFF, ENABLE], "ALTER SUBSCRIPTION {sub} ADD PUBLICATION zz_ntx_p3"),
    (&[SUBSCRIPTION_OFF, ENABLE], "ALTER SUBSCRIPTION {sub} DROP PUBLICATION zz_ntx_p2"),
];

/// A subscription made inside the block without connecting (disabled, with a slot name: its
/// own), to two publications that need not exist.
const SUBSCRIPTION_OFF: &str = "CREATE SUBSCRIPTION {sub} CONNECTION 'host=127.0.0.1 port=1 dbname=zz_ntx' \
     PUBLICATION zz_ntx_p1, zz_ntx_p2 WITH (connect = false)";

const ENABLE: &str = "ALTER SUBSCRIPTION {sub} ENABLE";

/// Forms the server runs inside a block (some of them close to a refused one).
const RUNS: &[(&[&str], &str)] = &[
    (&[], "ANALYZE {s}.t"),
    (&[], "ANALYZE VERBOSE {s}.t (a)"),
    (&[], "CHECKPOINT"),
    (&[], "REFRESH MATERIALIZED VIEW CONCURRENTLY {s}.mv"),
    (&[], "REFRESH MATERIALIZED VIEW {s}.mv"),
    (&[], "CREATE INDEX zz_ntx_i ON {s}.t (a)"),
    (&[], "DROP INDEX {s}.t_a"),
    (&[], "REINDEX INDEX {s}.t_a"),
    (&[], "REINDEX TABLE {s}.t"),
    (&[], "REINDEX (CONCURRENTLY false) TABLE {s}.t"),
    (&[], "CLUSTER {s}.t USING t_a"),
    (&[], "DISCARD PLANS"),
    (&[], "DISCARD SEQUENCES"),
    (&[], "DISCARD TEMP"),
    (&[], "ALTER TABLE {s}.p DETACH PARTITION {s}.p1"),
    (&[], "ALTER TYPE {s}.e ADD VALUE 'b'"),
    (&[], "LOCK TABLE {s}.t"),
    (&[], "SELECT 1"),
    (&[], SUBSCRIPTION_OFF),
    (
        &[],
        "CREATE SUBSCRIPTION {sub} CONNECTION 'host=127.0.0.1 port=1 dbname=zz_ntx' PUBLICATION zz_ntx_p1 WITH (connect = false, create_slot = false)",
    ),
    (&[SUBSCRIPTION_OFF], "ALTER SUBSCRIPTION {sub} SET PUBLICATION zz_ntx_p3 WITH (refresh = false)"),
    (&[SUBSCRIPTION_OFF], "ALTER SUBSCRIPTION {sub} SET (binary = true)"),
    (&[SUBSCRIPTION_OFF, ENABLE], "ALTER SUBSCRIPTION {sub} ADD PUBLICATION zz_ntx_p3 WITH (refresh = false)"),
];

/// Run `prefix` and then `sql` inside a block that is then rolled back: the outcome of `sql`.
async fn in_block(client: &Client, prefix: &[String], sql: &str) -> Result<(), tokio_postgres::Error> {
    client.batch_execute("BEGIN").await.expect("BEGIN");
    for p in prefix {
        if let Err(e) = client.batch_execute(p).await {
            panic!("{p} (before {sql}): {e:?}");
        }
    }
    let outcome = client.batch_execute(sql).await;
    client.batch_execute("ROLLBACK").await.expect("ROLLBACK");
    outcome
}

/// The settings of the server's configuration files (`ALTER SYSTEM` writes one of them).
async fn file_settings(client: &Client) -> Vec<String> {
    let rows = client
        .query("SELECT concat_ws(' ', sourcefile, name, setting) FROM pg_file_settings ORDER BY 1", &[])
        .await
        .expect("pg_file_settings");
    rows.iter().map(|r| r.get::<_, String>(0)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_statements_said_to_need_no_transaction_block_are_refused_in_one() {
    let Some(url) = pg_url("the_statements_said_to_need_no_transaction_block_are_refused_in_one") else { return };
    let schema = format!("zz_ntx_{}", std::process::id());
    let sub = format!("zz_ntx_sub_{}", std::process::id());
    let _guard = SchemaGuard(url.clone(), schema.clone());
    let fill = |sql: &str| sql.replace("{s}", &schema).replace("{sub}", &sub);
    let (client, conn) = tokio_postgres::connect(&url, NoTls).await.expect("connect");
    tokio::spawn(conn);
    client
        .batch_execute(&fill(
            "CREATE SCHEMA {s};
             CREATE TABLE {s}.t (id int PRIMARY KEY, a int);
             INSERT INTO {s}.t VALUES (1, 1), (2, 2);
             CREATE INDEX t_a ON {s}.t (a);
             CREATE MATERIALIZED VIEW {s}.mv AS SELECT id, a FROM {s}.t;
             CREATE UNIQUE INDEX mv_id ON {s}.mv (id);
             CREATE TABLE {s}.p (id int) PARTITION BY LIST (id);
             CREATE TABLE {s}.p1 PARTITION OF {s}.p FOR VALUES IN (1);
             CREATE TYPE {s}.e AS ENUM ('a');",
        ))
        .await
        .expect("setup");

    let settings_before = file_settings(&client).await;
    let mut wrong = Vec::new();
    for (prefix, sql) in REFUSED {
        let (prefix, sql): (Vec<String>, String) = (prefix.iter().map(|p| fill(p)).collect(), fill(sql));
        if !classify(&sql).no_tx_block {
            wrong.push(format!("classifier: not flagged: {sql}"));
        }
        match in_block(&client, &prefix, &sql).await {
            Err(e) if e.code() == Some(&SqlState::ACTIVE_SQL_TRANSACTION) => {}
            Err(e) => wrong.push(format!("server: not 25001: {sql}: {e:?}")),
            Ok(()) => wrong.push(format!("server: ran in a block: {sql}")),
        }
    }
    for (prefix, sql) in RUNS {
        let (prefix, sql): (Vec<String>, String) = (prefix.iter().map(|p| fill(p)).collect(), fill(sql));
        if classify(&sql).no_tx_block {
            wrong.push(format!("classifier: flagged: {sql}"));
        }
        if let Err(e) = in_block(&client, &prefix, &sql).await {
            wrong.push(format!("server: failed in a block: {sql}: {e:?}"));
        }
    }
    // Nothing a refused or rolled back form would make is left.
    let left = client
        .query_one(
            "SELECT (SELECT count(*) FROM pg_database WHERE datname LIKE 'zz\\_ntx%')
                  + (SELECT count(*) FROM pg_tablespace WHERE spcname LIKE 'zz\\_ntx%')
                  + (SELECT count(*) FROM pg_subscription WHERE subname LIKE 'zz\\_ntx%')
                  + (SELECT count(*) FROM pg_replication_slots WHERE slot_name LIKE 'zz\\_ntx%')",
            &[],
        )
        .await
        .expect("leftovers");
    if left.get::<_, i64>(0) != 0 {
        wrong.push("left behind: a zz_ntx database, tablespace, subscription or slot".to_string());
    }
    assert_eq!(file_settings(&client).await, settings_before, "ALTER SYSTEM changed postgresql.auto.conf");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
