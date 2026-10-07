//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended" (the server closed it while the
//! session waited, [`Wire::closed`]).

use crate::wire::Wire;
use datarig_core::driver::{DbCommand, DbError};
use tokio::sync::mpsc::UnboundedReceiver;

/// What the session task does next.
pub(crate) enum Next {
    Command(DbCommand),
    /// The UI closed the session (the command channel closed): stop without another event.
    Closed,
    /// The connection ended by itself; the reason.
    Lost(DbError),
}

pub(crate) struct Link {
    rx: UnboundedReceiver<DbCommand>,
    wire: Wire,
}

impl Link {
    pub(crate) fn new(rx: UnboundedReceiver<DbCommand>, wire: Wire) -> Self {
        Self { rx, wire }
    }

    /// The next command, or why there is none: the server's last words when it closed the
    /// connection with an error (`wait_timeout`), else that it closed.
    pub(crate) async fn next(&mut self) -> Next {
        tokio::select! {
            biased;
            () = self.wire.closed() => Next::Lost(self.wire.last_words().unwrap_or(DbError::Closed)),
            cmd = self.rx.recv() => cmd.map_or(Next::Closed, Next::Command),
        }
    }
}
