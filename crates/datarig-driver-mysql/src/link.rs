//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended" (the server closed it while the
//! session waited, [`Wire::closed`]).

use crate::session::{Kill, Killer};
use crate::wire::Wire;
use datarig_core::driver::{CancelFailure, DbCommand, DbError, DbEvent};
use std::collections::VecDeque;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// How long closing a session waits for the server to take the cancel (`KILL QUERY`).
const CANCEL_WAIT: Duration = Duration::from_secs(2);

/// How long after a kill the server took (`KILL QUERY` answered OK) its statement may still
/// send rows before the session takes it for one that ignores the kill and cuts the connection.
/// Only rows that keep coming count: a statement that stops may then roll back for a long time
/// (a large `UPDATE` undone row by row, some 12 s seen) and says nothing meanwhile; it is waited
/// for however long that takes, as the user's open transaction depends on it.
const TAKE_WAIT: Duration = Duration::from_secs(10);

/// How often the session looks whether rows still come after [`TAKE_WAIT`].
const SENDING_TICK: Duration = Duration::from_millis(500);

/// How often the session says it still waits for a statement whose cancel the server took
/// ([`DbEvent::Stopping`]), once the server confirmed the statement is still there.
const STOPPING_TICK: Duration = Duration::from_secs(5);

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
    /// A `KILL QUERY` sent for the statement did not take ([`Kill`]), for this reason: the
    /// connection was cut rather than read on (a result may have no end).
    CancelNotTaken(CancelFailure),
}

pub(crate) struct Link {
    rx: UnboundedReceiver<DbCommand>,
    /// Commands that arrived while a request was in flight, or that wait for a result to end.
    queued: VecDeque<DbCommand>,
    wire: Wire,
    /// Where the query session says it still waits for a statement a cancel stopped
    /// ([`DbEvent::Stopping`]), and the run (or count) that statement is of.
    stopping: Option<(UnboundedSender<DbEvent>, u64)>,
}

impl Link {
    pub(crate) fn new(rx: UnboundedReceiver<DbCommand>, wire: Wire) -> Self {
        Self { rx, queued: VecDeque::new(), wire, stopping: None }
    }

    /// The query session's requests from now on are of run (or count) `id`; while one waits
    /// for a statement whose cancel the server took, the session says so on `events`.
    pub(crate) fn serving(&mut self, events: &UnboundedSender<DbEvent>, id: u64) {
        self.stopping = Some((events.clone(), id));
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
    /// A request of the statement `cancel` stops also watches what became of its kills. One the
    /// server refused, or that could not be sent, has the connection cut and
    /// `Closed::CancelNotTaken` come back at once: the rest of the result is never read (it may
    /// have no end). One the server took is waited for, however long the statement takes to
    /// stop, unless its rows still come more than [`TAKE_WAIT`] after it (or after the request
    /// began, when that was later): then it is cut too. The server ends a statement whose
    /// connection is cut once it finds it gone. While a taken kill is waited for, the session
    /// says so every [`STOPPING_TICK`] (asking the server first whether the statement is still
    /// there), so the UI knows it waits on the server and keeps it.
    pub(crate) async fn guard<T>(
        &mut self,
        cancel: Option<&Killer>,
        request: impl Future<Output = T>,
    ) -> Result<T, Closed> {
        tokio::pin!(request);
        let mut kills = cancel.map(Killer::watch);
        let began = Instant::now();
        let wire = self.wire.clone();
        loop {
            let kill = kills.as_mut().map_or(Kill::None, |k| *k.borrow_and_update());
            let wire = &wire;
            // Its timers are not held back by the task's budget, which the request's reads use up
            // while rows stream in.
            let give_up = tokio::task::unconstrained(async move {
                match kill {
                    Kill::None => std::future::pending().await,
                    Kill::Failed(f) => f.into(),
                    Kill::Taken(at) => {
                        tokio::time::sleep_until((at.max(began) + TAKE_WAIT).into()).await;
                        let seen = wire.received();
                        loop {
                            tokio::time::sleep(SENDING_TICK).await;
                            if wire.received() > seen {
                                return CancelFailure::KeptSending;
                            }
                        }
                    }
                }
            });
            // While a taken kill is waited for: the server says the statement is still there.
            let stopping = self.stopping.clone();
            let beat = tokio::task::unconstrained(async move {
                let (Kill::Taken(_), Some(k), Some((events, id))) = (kill, cancel, stopping) else {
                    return std::future::pending().await;
                };
                loop {
                    tokio::time::sleep(STOPPING_TICK).await;
                    if k.still_there().await {
                        let _ = events.send(DbEvent::Stopping { id });
                    }
                }
            });
            // What is watched beside the request is polled before it: its reads use up the
            // task's budget of the runtime's resources poll after poll while a result streams
            // in, and a kill's news and a closed channel would never be seen after them. The
            // request comes before giving up: what it can read now still ends it.
            tokio::select! {
                biased;
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
                why = give_up => {
                    self.wire.cut();
                    return Err(Closed::CancelNotTaken(why));
                }
                () = beat => {}
            }
        }
    }

    /// Commands that arrived and were not taken yet, in their order (those still on the
    /// channel too).
    pub(crate) fn take_queued(&mut self) -> Vec<DbCommand> {
        while let Ok(c) = self.rx.try_recv() {
            self.queued.push_back(c);
        }
        self.queued.drain(..).collect()
    }

    /// Cut the connection: every read and write fails from now on (a result is never read to
    /// its end in the background).
    pub(crate) fn cut(&self) {
        self.wire.cut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::Route;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::unbounded_channel;

    /// A link over an in-memory stream (the server's end, and the client's to read the result
    /// from), and a killer whose last kill was taken `ago`.
    fn taken(ago: Duration) -> (Link, crate::wire::WireStream, tokio::io::DuplexStream, Killer) {
        let (client, server) = tokio::io::duplex(1024);
        let (wire, stream) = Wire::new(Box::new(client));
        let (_tx, rx) = unbounded_channel();
        // The sender goes: the channel closes. Kept open by leaking it for the test's life.
        std::mem::forget(_tx);
        let killer = Killer::new(
            Route::new("127.0.0.1", 1, None),
            mysql_async::Opts::from_url("mysql://datarig@127.0.0.1:1/").unwrap(),
            7,
        );
        killer.record(Kill::Taken(Instant::now().checked_sub(ago).unwrap()), || true);
        (Link::new(rx, wire), stream, server, killer)
    }

    /// Read the result without end.
    async fn read_on(mut stream: crate::wire::WireStream) {
        let mut buf = [0u8; 64];
        while stream.read(&mut buf).await.is_ok_and(|n| n > 0) {}
    }

    #[tokio::test]
    async fn rows_that_keep_coming_after_a_taken_kill_cut_the_connection() {
        // Taken long ago: the bound counts from when the request began.
        let (mut link, stream, mut server, killer) = taken(TAKE_WAIT * 2);
        let send = tokio::spawn(async move {
            while server.write_all(b"row").await.is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let t0 = Instant::now();
        let out = tokio::time::timeout(TAKE_WAIT * 2, link.guard(Some(&killer), read_on(stream))).await;
        assert!(matches!(out, Ok(Err(Closed::CancelNotTaken(CancelFailure::KeptSending)))));
        assert!(t0.elapsed() >= TAKE_WAIT, "{:?}", t0.elapsed());
        send.abort();
    }

    #[tokio::test]
    async fn a_statement_silent_after_a_taken_kill_is_waited_for() {
        // Silent (a rollback) past the bound: never cut.
        let (mut link, stream, server, killer) = taken(Duration::ZERO);
        let wait = TAKE_WAIT + SENDING_TICK * 4;
        let out = tokio::time::timeout(wait, link.guard(Some(&killer), read_on(stream))).await;
        assert!(out.is_err(), "still waited for");
        drop(server);
    }

    #[tokio::test]
    async fn a_refused_kill_cuts_the_connection_at_once() {
        let (mut link, stream, _server, killer) = taken(Duration::ZERO);
        killer.record(Kill::Failed(crate::session::KillFailure::Refused(1095)), || true);
        let out = tokio::time::timeout(Duration::from_secs(1), link.guard(Some(&killer), read_on(stream))).await;
        assert!(matches!(out, Ok(Err(Closed::CancelNotTaken(CancelFailure::Refused)))));
    }
}
