//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended" (the server closed it while the
//! session waited, [`Wire::closed`]).

use crate::session::{Kill, Killer};
use crate::wire::Wire;
use datarig_core::driver::{DbCommand, DbError};
use std::collections::VecDeque;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

/// How long closing a session waits for the server to take the cancel (`KILL QUERY`).
const CANCEL_WAIT: Duration = Duration::from_secs(2);

/// How long a statement the server agreed to stop (`KILL QUERY` taken) may take to end before the
/// session stops reading it and cuts the connection (a kill refused, or not sent, cuts it at
/// once). MySQL checks the kill between rows and steps, so a statement it stops mostly ends
/// within milliseconds, but not always: a hash join building its table checks seldom, and was
/// seen to take some 7 s under load. Cutting the connection costs the session and the user's
/// open transaction, so a statement that is stopping is given this long; one that runs on past
/// it ignores the kill, and its result may have no end.
const TAKE_WAIT: Duration = Duration::from_secs(10);

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
    /// A `KILL QUERY` sent for the statement did not take ([`Kill`]): the connection was cut
    /// rather than read on (a result may have no end).
    CancelNotTaken,
}

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
    /// at most [`CANCEL_WAIT`]), the connection cut, and `Closed::Ui` comes back at once. A lost
    /// connection makes `request` itself fail.
    ///
    /// A request of the statement `cancel` stops also watches what became of its kills: one the
    /// server refused, or one it took while the request does not end within [`TAKE_WAIT`] (of
    /// the kill, or of the request when it came later), has the connection cut and `Closed::CancelNotTaken` come back: the rest of the result is never
    /// read (it may have no end). The server ends the statement once it finds the connection
    /// gone.
    pub(crate) async fn guard<T>(
        &mut self,
        cancel: Option<&Killer>,
        request: impl Future<Output = T>,
    ) -> Result<T, Closed> {
        tokio::pin!(request);
        let mut kills = cancel.map(Killer::watch);
        let began = Instant::now();
        loop {
            let kill = kills.as_mut().map_or(Kill::None, |k| *k.borrow_and_update());
            let give_up = async {
                match kill {
                    Kill::None => std::future::pending().await,
                    Kill::Taken(at) => tokio::time::sleep_until((at.max(began) + TAKE_WAIT).into()).await,
                    Kill::Failed => {}
                }
            };
            // The request's reads use up the task's budget of the runtime's own resources (the
            // stream) poll after poll: what is watched beside it is polled first, or a kill that
            // did not take and a closed channel would never be seen while a result streams in.
            tokio::select! {
                biased;
                () = give_up => {
                    self.wire.cut();
                    return Err(Closed::CancelNotTaken);
                }
                changed = async {
                    match kills.as_mut() {
                        Some(k) => k.changed().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if changed.is_err() {
                        kills = None;
                    }
                }
                cmd = self.rx.recv() => match cmd {
                    Some(c) => self.queued.push_back(c),
                    None => {
                        if let Some(k) = cancel {
                            let _ = tokio::time::timeout(CANCEL_WAIT, k.kill_query()).await;
                        }
                        self.wire.cut();
                        return Err(Closed::Ui);
                    }
                },
                out = &mut request => return Ok(out),
            }
        }
    }

    /// Cut the connection: every read and write fails from now on (a result is never read to
    /// its end in the background).
    pub(crate) fn cut(&self) {
        self.wire.cut();
    }
}
