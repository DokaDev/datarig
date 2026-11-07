//! The *monitor* connection (`SessionRole::Monitor`): what the server says about its sessions
//! and its load, for the Monitor tab. The statements are the driver's ([`crate::meta::monitor`],
//! next to the catalog's); the reading of their rows into the model is the core's
//! (`datarig_core::monitor::decode`). This runs them.
//!
//! Every read is one transaction of its own, `READ ONLY`, with a short lock wait and a
//! statement limit ([`crate::meta::monitor::begin`]), opened, read and committed in one round trip: the
//! `BEGIN`, each statement (simple queries, so every value comes back as text and nothing is
//! prepared that a pooler could lose) and the `COMMIT` go out together. A failed statement
//! leaves the transaction aborted, which the `COMMIT` rolls back, so nothing stays open on the
//! connection; everything the transaction set ends with it, so nothing stays on a server
//! connection a pooler hands to another client.
//!
//! The statements read only the activity and statistics views and settings: no user's table is
//! touched; the only locks are the brief `ACCESS SHARE` locks of the catalogs those views read,
//! waited for at most a second. The connection runs nothing before its probe (no read-only
//! check, context or limits: [`crate::connect`] skips them for a monitor), so the
//! transactions counted in [`Own`] are all it ever committed.

use crate::connect::db_error;
use crate::link::{Link, Next};
use crate::meta::monitor as sql;
use datarig_core::driver::{Cell, DbCommand, DbError, DbEvent};
use datarig_core::monitor::decode::{self, Rows};
use datarig_core::monitor::format::PG_OLDEST;
use datarig_core::monitor::{MonitorFormat, Own, PgMonitor, Snapshot, Want};
use futures::future::join_all;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_postgres::{Client, SimpleQueryMessage};

/// How long the probe's statement may run.
const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// How much longer than its statements' limit a read may take before the connection is given
/// up on (the server stops each statement at the limit; past this, it or the network does not
/// answer).
const GRACE: Duration = Duration::from_secs(3);

pub(crate) async fn monitor_loop(client: Client, mut link: Link, events: UnboundedSender<DbEvent>) {
    // Transactions this connection committed: a tick's counters are read inside the next one,
    // so they count every earlier one and not the current.
    let mut own = Own::default();
    // Closed by the app while a read waits: the connection is closed at once, not left to the
    // read's own limit.
    let Ok(probed) = link.guard(None, probe(&client, &mut own)).await else {
        link.close();
        return;
    };
    let format = match probed {
        Ok(format) => Some(format),
        Err(Failure::Lost(error)) => {
            link.close();
            let _ = events.send(DbEvent::Lost { error });
            return;
        }
        Err(Failure::Failed(error)) => {
            let _ = events.send(DbEvent::Monitor(Err(error)));
            None
        }
    };
    if let Some(pg) = &format {
        let _ = events.send(DbEvent::Monitor(Ok(MonitorFormat::Postgres(pg.clone()))));
    }
    loop {
        let cmd = match link.next().await {
            Next::Command(c) => c,
            Next::Closed => return,
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        let (request, want) = match (&cmd, &format) {
            (DbCommand::MonitorTick { request, want }, Some(_)) => (*request, *want),
            // Without a format there is nothing to read with; anything else is another
            // session's work.
            _ => {
                if events.send(cmd.not_supported()).is_err() {
                    return;
                }
                continue;
            }
        };
        let Some(pg) = &format else { continue };
        let Ok(result) = link.guard(None, tick(&client, pg, &want, &mut own)).await else {
            link.close();
            return;
        };
        let result = match result {
            Ok(snapshot) => Ok(snapshot),
            Err(Failure::Failed(error)) => Err(error),
            Err(Failure::Lost(error)) => {
                link.close();
                let _ = events.send(DbEvent::MonitorSnapshot { request, result: Err(error.clone()) });
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        if events.send(DbEvent::MonitorSnapshot { request, result }).is_err() {
            return;
        }
    }
}

/// Why a read gave no answer.
enum Failure {
    /// The server refused or failed it; the connection is fine.
    Failed(DbError),
    /// It took past its limit and grace: the connection cannot be trusted any more.
    Lost(DbError),
}

async fn probe(client: &Client, own: &mut Own) -> Result<PgMonitor, Failure> {
    let rows = read(client, &[sql::probe()], PROBE_LIMIT, own).await?;
    decode::pg::probe(&rows[0]).map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))
}

async fn tick(client: &Client, pg: &PgMonitor, want: &Want, own: &mut Own) -> Result<Snapshot, Failure> {
    let before = *own;
    let rows = read(client, &sql::tick(pg, want), want.limit, own).await?;
    decode::pg::tick(want, &rows, before).map_err(|e| Failure::Failed(e.into_db(PG_OLDEST)))
}

/// Run `statements` in one read-only transaction, in one round trip, and count it in `own` when
/// it committed.
async fn read(client: &Client, statements: &[String], limit: Duration, own: &mut Own) -> Result<Vec<Rows>, Failure> {
    let begin = sql::begin(limit);
    let all = async {
        tokio::join!(
            client.batch_execute(&begin),
            join_all(statements.iter().map(|s| client.simple_query(s))),
            client.batch_execute("COMMIT"),
        )
    };
    let Ok((begun, answers, committed)) = tokio::time::timeout(limit + GRACE, all).await else {
        return Err(Failure::Lost(DbError::NoAnswer(limit + GRACE)));
    };
    begun.map_err(|e| Failure::Failed(db_error(&e)))?;
    let mut rows = Vec::with_capacity(answers.len());
    for answer in answers {
        rows.push(text_rows(answer.map_err(|e| Failure::Failed(db_error(&e)))?));
    }
    committed.map_err(|e| Failure::Failed(db_error(&e)))?;
    own.transactions += 1;
    Ok(rows)
}

/// A simple query's rows as text cells.
fn text_rows(messages: Vec<SimpleQueryMessage>) -> Rows {
    messages
        .into_iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(row) => {
                Some((0..row.len()).map(|i| row.get(i).map(str::to_owned)).collect::<Vec<Cell>>())
            }
            _ => None,
        })
        .collect()
}
