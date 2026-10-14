//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended".

use crate::route::{Cancel, Unsent};
use datarig_core::driver::{DbCommand, DbError};
use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

/// How long closing a session waits for the server to take the cancel request.
const CANCEL_WAIT: Duration = Duration::from_secs(2);

/// What the session task does next.
pub(crate) enum Next {
    Command(DbCommand),
    /// The UI closed the session (the command channel closed): stop without another event.
    Closed,
    /// The connection ended by itself; the reason.
    Lost(DbError),
}

/// Why a request in flight was left without its answer.
pub(crate) enum Closed {
    /// The UI closed the session.
    Ui,
    /// A cancel of the running statement could not reach the server (the reason): the
    /// connection was closed.
    CancelNotSent(DbError),
}

pub(crate) struct Link {
    rx: UnboundedReceiver<DbCommand>,
    /// Commands that arrived while a request was in flight.
    queued: VecDeque<DbCommand>,
    /// Resolves when the connection ends.
    ended: Option<oneshot::Receiver<DbError>>,
    /// The task that drives the connection: aborting it closes the connection at once.
    conn: AbortHandle,
    /// The user's cancels that could not be sent (a session that cancels its statements).
    unsent: Option<Unsent>,
}

impl Link {
    pub(crate) fn new(
        rx: UnboundedReceiver<DbCommand>,
        ended: oneshot::Receiver<DbError>,
        conn: AbortHandle,
        unsent: Option<Unsent>,
    ) -> Self {
        Self { rx, queued: VecDeque::new(), ended: Some(ended), conn, unsent }
    }

    /// Put `cmd` first in line (it ended an open portal and is processed next).
    pub(crate) fn requeue(&mut self, cmd: DbCommand) {
        self.queued.push_front(cmd);
    }

    /// The next command, or why there is none. A lost connection is reported before queued
    /// commands are tried on it. A command begins a new request ([`Unsent::next_request`]).
    pub(crate) async fn next(&mut self) -> Next {
        let next = self.take().await;
        if let (Next::Command(_), Some(unsent)) = (&next, &self.unsent) {
            unsent.next_request();
        }
        next
    }

    async fn take(&mut self) -> Next {
        if let Some(reason) = self.ended.as_mut().and_then(|e| e.try_recv().ok()) {
            self.ended = None;
            return Next::Lost(reason);
        }
        if let Some(c) = self.queued.pop_front() {
            return Next::Command(c);
        }
        match self.ended.as_mut() {
            Some(ended) => tokio::select! {
                biased;
                reason = ended => {
                    self.ended = None;
                    Next::Lost(reason.unwrap_or(DbError::Closed))
                }
                cmd = self.rx.recv() => cmd.map_or(Next::Closed, Next::Command),
            },
            None => self.rx.recv().await.map_or(Next::Closed, Next::Command),
        }
    }

    /// Wait for `request` while watching the command channel: commands that arrive meanwhile
    /// are queued; if the UI closes the session, the running statement is cancelled (`cancel`)
    /// and `Closed::Ui` comes back at once. A lost connection makes `request` itself fail.
    ///
    /// A cancel that cannot reach the server leaves the statement running: the connection is
    /// closed rather than waited on or read on in the background. The statement may still run
    /// to its end (and commit outside a transaction block); the server rolls an open block back
    /// once it notices the closed connection. Closing the session does so when its own cancel
    /// does not go out; a request of the statement `cancel` stops comes back with
    /// `Closed::CancelNotSent` when the user's cancel could not be sent.
    pub(crate) async fn guard<T>(
        &mut self,
        cancel: Option<&Cancel>,
        request: impl Future<Output = T>,
    ) -> Result<T, Closed> {
        tokio::pin!(request);
        let unsent = cancel.and(self.unsent.clone());
        let not_sent = async {
            match &unsent {
                Some(unsent) => unsent.current().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(not_sent);
        loop {
            // What is watched beside the request is polled first: a request may be ready poll
            // after poll while its answer streams in, and the close or the failed cancel must
            // not wait for it.
            tokio::select! {
                biased;
                cmd = self.rx.recv() => match cmd {
                    Some(c) => self.queued.push_back(c),
                    None => {
                        if let Some(c) = cancel
                            && !matches!(tokio::time::timeout(CANCEL_WAIT, c.send()).await, Ok(Ok(())))
                        {
                            self.conn.abort();
                        }
                        return Err(Closed::Ui);
                    }
                },
                why = &mut not_sent => {
                    self.conn.abort();
                    return Err(Closed::CancelNotSent(why));
                }
                out = &mut request => return Ok(out),
            }
        }
    }
}
