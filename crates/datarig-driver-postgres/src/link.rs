//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended".

use crate::route::Cancel;
use datarig_core::driver::{DbCommand, DbError};
use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;

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

/// The UI closed the session while a request was in flight.
pub(crate) struct Closed;

pub(crate) struct Link {
    rx: UnboundedReceiver<DbCommand>,
    /// Commands that arrived while a request was in flight.
    queued: VecDeque<DbCommand>,
    /// Resolves when the connection ends.
    ended: Option<oneshot::Receiver<DbError>>,
}

impl Link {
    pub(crate) fn new(rx: UnboundedReceiver<DbCommand>, ended: oneshot::Receiver<DbError>) -> Self {
        Self { rx, queued: VecDeque::new(), ended: Some(ended) }
    }

    /// Put `cmd` first in line (it ended an open portal and is processed next).
    pub(crate) fn requeue(&mut self, cmd: DbCommand) {
        self.queued.push_front(cmd);
    }

    /// The next command, or why there is none. A lost connection is reported before queued
    /// commands are tried on it.
    pub(crate) async fn next(&mut self) -> Next {
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
    /// and `Closed` comes back at once. A lost connection makes `request` itself fail.
    pub(crate) async fn guard<T>(
        &mut self,
        cancel: Option<&Cancel>,
        request: impl Future<Output = T>,
    ) -> Result<T, Closed> {
        tokio::pin!(request);
        loop {
            tokio::select! {
                biased;
                out = &mut request => return Ok(out),
                cmd = self.rx.recv() => match cmd {
                    Some(c) => self.queued.push_back(c),
                    None => {
                        if let Some(c) = cancel {
                            let _ = tokio::time::timeout(CANCEL_WAIT, c.send()).await;
                        }
                        return Err(Closed);
                    }
                },
            }
        }
    }
}
