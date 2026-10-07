//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended" (the server closed it while the
//! session waited, [`Wire::closed`]).

use crate::session::Killer;
use crate::wire::Wire;
use datarig_core::driver::{DbCommand, DbError};
use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

/// How long closing a session waits for the server to take the cancel (`KILL QUERY`).
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
    /// Commands that arrived while a request was in flight, or that wait for a result to end.
    queued: VecDeque<DbCommand>,
    wire: Wire,
}

impl Link {
    pub(crate) fn new(rx: UnboundedReceiver<DbCommand>, wire: Wire) -> Self {
        Self { rx, queued: VecDeque::new(), wire }
    }

    /// Put `cmds` first in line, in their order (they wait no longer).
    pub(crate) fn requeue_all(&mut self, cmds: Vec<DbCommand>) {
        for c in cmds.into_iter().rev() {
            self.queued.push_front(c);
        }
    }

    /// The next command, or why there is none, while nothing is asked of the server: the
    /// server's last words when it closed the connection with an error (`wait_timeout`), else
    /// that it closed.
    pub(crate) async fn next(&mut self) -> Next {
        if let Some(c) = self.queued.pop_front() {
            return Next::Command(c);
        }
        tokio::select! {
            biased;
            () = self.wire.closed() => Next::Lost(self.wire.last_words().unwrap_or(DbError::Closed)),
            cmd = self.rx.recv() => cmd.map_or(Next::Closed, Next::Command),
        }
    }

    /// The next command while a result is still coming: the stream is not watched (its bytes
    /// are the result's; a connection that ends shows when the result is read on).
    pub(crate) async fn next_command(&mut self) -> Next {
        if let Some(c) = self.queued.pop_front() {
            return Next::Command(c);
        }
        self.rx.recv().await.map_or(Next::Closed, Next::Command)
    }

    /// Wait for `request` while watching the command channel: commands that arrive meanwhile
    /// are queued; if the UI closes the session, the running statement is cancelled (`cancel`,
    /// at most [`CANCEL_WAIT`]), the connection cut, and `Closed` comes back at once. A lost
    /// connection makes `request` itself fail.
    pub(crate) async fn guard<T>(
        &mut self,
        cancel: Option<&Killer>,
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
                        if let Some(k) = cancel {
                            let _ = tokio::time::timeout(CANCEL_WAIT, k.kill_query()).await;
                        }
                        self.wire.cut();
                        return Err(Closed);
                    }
                },
            }
        }
    }
}
