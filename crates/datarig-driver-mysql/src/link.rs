//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended" (the server closed it while the
//! session waited, [`Wire::closed`]).

use crate::wire::Wire;
use datarig_core::driver::{DbCommand, DbError};
use std::collections::VecDeque;
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
    /// Commands that arrived while a request was in flight.
    queued: VecDeque<DbCommand>,
    wire: Wire,
}

impl Link {
    pub(crate) fn new(rx: UnboundedReceiver<DbCommand>, wire: Wire) -> Self {
        Self { rx, queued: VecDeque::new(), wire }
    }

    /// The next command, or why there is none.
    pub(crate) async fn next(&mut self) -> Next {
        if let Some(c) = self.queued.pop_front() {
            return Next::Command(c);
        }
        tokio::select! {
            biased;
            () = self.wire.closed() => Next::Lost(DbError::Closed),
            cmd = self.rx.recv() => cmd.map_or(Next::Closed, Next::Command),
        }
    }
}
