//! The *monitor* connection (`SessionRole::Monitor`): what the server says about its sessions
//! and its load, for the Monitor tab. The statements are the driver's ([`crate::meta::monitor`],
//! next to the catalog's); the reading of their rows into the model is the core's
//! (`datarig_core::monitor::decode`). This runs them.
//!
//! Every statement is a read on its own in autocommit, on a session set up read-only, with a
//! metadata lock wait of a second, `SELECT`s under the tick's limit, no `sql_select_limit` and
//! times in the server's system time zone (`session::init_sql`). The statements read only `performance_schema`, `information_schema`
//! and the server's status and variables: no user's table is touched. A refusal for lack of a
//! privilege is an answer of the probe (what the account may read), never a failure of it.
//!
//! The server counts every statement a client sends in `Questions` as it starts it, so the
//! statements this connection sent are counted here one by one ([`Own::statements`]) and taken
//! out of the rates exactly.

use crate::link::{Link, Next};
use crate::meta::monitor as sql;
use crate::session::{my_error, quit};
use datarig_core::driver::{Cell, DbCommand, DbError, DbEvent};
use datarig_core::monitor::decode::{self, Answer, Rows};
use datarig_core::monitor::format::MYSQL_OLDEST;
use datarig_core::monitor::{FullText, MonitorFormat, MySqlMonitor, Own, Snapshot, TextAsk, Want};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Row, Value};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// How long the probe may take.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// How long the read of a statement's whole text may run when no tick has set a limit yet.
const TEXT_LIMIT: Duration = Duration::from_secs(5);

/// How much longer than its statements' limit a tick may take before the connection is given up
/// on (`max_execution_time` does not stop a `SHOW`; past this, the server or the network does not
/// answer).
const GRACE: Duration = Duration::from_secs(3);

/// The connection and what it sent.
struct Monitor {
    conn: Conn,
    /// Statements sent since the session opened (the set-up statements before are counted by
    /// the server too, but before any sample, so no rate spans them).
    sent: u64,
    /// The `max_execution_time` set on the session, once a tick set one.
    limit: Option<Duration>,
}

pub(crate) async fn monitor_loop(conn: Conn, mut link: Link, events: UnboundedSender<DbEvent>) {
    let mut m = Monitor { conn, sent: 0, limit: None };
    let probed = match link.guard(None, tokio::time::timeout(PROBE_LIMIT, m.probe())).await {
        Ok(Ok(probed)) => probed,
        Ok(Err(_)) => {
            link.cut();
            let _ = events.send(DbEvent::Lost { error: DbError::NoAnswer(PROBE_LIMIT) });
            return;
        }
        // Closed by the app: the connection was cut.
        Err(_) => return,
    };
    let format = match probed {
        Ok(my) => {
            let _ = events.send(DbEvent::Monitor(Ok(MonitorFormat::MySql(my.clone()))));
            Some(my)
        }
        Err(error) => {
            let _ = events.send(DbEvent::Monitor(Err(error)));
            None
        }
    };
    loop {
        let cmd = match link.next().await {
            Next::Command(c) => c,
            Next::Closed => {
                quit(m.conn).await;
                return;
            }
            Next::Lost(error) => {
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
        };
        let (my, request) = match (&cmd, &format) {
            (DbCommand::MonitorTick { request, .. } | DbCommand::MonitorQueryText { request, .. }, Some(my)) => {
                (my, *request)
            }
            // Without a format there is nothing to read with; anything else is another
            // session's work.
            _ => {
                if events.send(cmd.not_supported()).is_err() {
                    return;
                }
                continue;
            }
        };
        let (deadline, answer) = match &cmd {
            DbCommand::MonitorTick { want, .. } => {
                let deadline = want.limit + GRACE;
                let read = tokio::time::timeout(deadline, m.tick(my, want));
                let answer =
                    link.guard(None, read).await.map(|r| r.map(|result| DbEvent::MonitorSnapshot { request, result }));
                (deadline, answer)
            }
            DbCommand::MonitorQueryText { ask, .. } => {
                let deadline = m.limit.unwrap_or(TEXT_LIMIT) + GRACE;
                let read = tokio::time::timeout(deadline, m.query_text(ask));
                let answer =
                    link.guard(None, read).await.map(|r| r.map(|result| DbEvent::MonitorQueryText { request, result }));
                (deadline, answer)
            }
            _ => continue,
        };
        let event = match answer {
            Ok(Ok(event)) => event,
            Ok(Err(_)) => {
                link.cut();
                let error = DbError::NoAnswer(deadline);
                let _ = events.send(match &cmd {
                    DbCommand::MonitorQueryText { .. } => {
                        DbEvent::MonitorQueryText { request, result: Err(error.clone()) }
                    }
                    _ => DbEvent::MonitorSnapshot { request, result: Err(error.clone()) },
                });
                let _ = events.send(DbEvent::Lost { error });
                return;
            }
            Err(_) => return,
        };
        if events.send(event).is_err() {
            return;
        }
        if m.conn.is_disconnected() {
            let _ = events.send(DbEvent::Lost { error: DbError::Closed });
            return;
        }
    }
}

impl Monitor {
    /// The rows of `sql`, as text cells.
    async fn rows(&mut self, sql: &str) -> Result<Rows, mysql_async::Error> {
        self.sent += 1;
        let rows: Vec<Row> = self.conn.query(sql).await?;
        Ok(rows.into_iter().map(|row| row.unwrap().into_iter().map(text).collect()).collect())
    }

    /// What the server offers, the account may read and the provider is.
    async fn probe(&mut self) -> Result<MySqlMonitor, DbError> {
        let mut answers = Vec::new();
        for statement in sql::probe() {
            answers.push(match self.rows(&statement).await {
                Ok(rows) => Answer::Rows(rows),
                Err(e) => Answer::Error { code: server_code(&e), message: my_error(&e).raw().into_owned() },
            });
        }
        decode::mysql::probe(&answers).map_err(|e| e.into_db(MYSQL_OLDEST))
    }

    async fn tick(&mut self, my: &MySqlMonitor, want: &Want) -> Result<Snapshot, DbError> {
        if self.limit != Some(want.limit) {
            self.rows(&sql::limit(want.limit)).await.map_err(|e| my_error(&e))?;
            self.limit = Some(want.limit);
        }
        let mut rows = Vec::new();
        let mut own = Own::default();
        for (i, statement) in sql::tick(my, want).iter().enumerate() {
            rows.push(self.rows(statement).await.map_err(|e| my_error(&e))?);
            // The counters are the second statement: the server counted every statement sent
            // until then, that one with them.
            if i == 1 {
                own.statements = self.sent;
            }
        }
        decode::mysql::tick(my, want, &rows, own).map_err(|e| e.into_db(MYSQL_OLDEST))
    }

    /// The whole text of the statement `ask` names, in a statement of its own (under the
    /// session's statement limit, set first when no tick has set one).
    async fn query_text(&mut self, ask: &TextAsk) -> Result<FullText, DbError> {
        if self.limit.is_none() {
            self.rows(&sql::limit(TEXT_LIMIT)).await.map_err(|e| my_error(&e))?;
            self.limit = Some(TEXT_LIMIT);
        }
        let rows = self.rows(&sql::query_text(ask.session.id)).await.map_err(|e| my_error(&e))?;
        decode::mysql::query_text(ask, &rows).map_err(|e| e.into_db(MYSQL_OLDEST))
    }
}

/// A value of the text protocol as the text the server sent.
fn text(value: Value) -> Cell {
    match value {
        Value::NULL => None,
        Value::Bytes(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        other => Some(other.as_sql(false)),
    }
}

/// The server's error code of a failed statement, `None` for a failure of another kind.
fn server_code(e: &mysql_async::Error) -> Option<u32> {
    match e {
        mysql_async::Error::Server(s) => Some(u32::from(s.code)),
        _ => None,
    }
}
