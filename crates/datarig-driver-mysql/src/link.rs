//! The session task's view of its command channel and of its connection: queued commands,
//! "the UI closed the session" and "the connection ended" (the server closed it while the
//! session waited, [`Wire::closed`]).

use crate::session::{Kill, KillFailure, Killer};
use crate::wire::Wire;
use datarig_core::driver::{CancelFailure, DbCommand, DbError, DbEvent};
use futures::future::BoxFuture;
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

/// How often the session looks whether rows still come (after a refused kill, or past
/// [`TAKE_WAIT`] after a taken one).
const SENDING_TICK: Duration = Duration::from_millis(500);

/// In how many ticks in a row rows must come before they count as coming on.
const SENDING_TICKS: u32 = 3;

/// How often the session says it still waits for a statement after a cancel: one the server
/// took ([`DbEvent::Stopping`], once the server confirmed the statement is still there), or
/// one it refused that sends nothing ([`DbEvent::CancelRefused`]).
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
    /// A request of the statement `cancel` stops also watches what became of its kills
    /// ([`watching`]): the connection is cut (`Closed::CancelNotTaken` comes back) only where
    /// that stops what reading on would not, and the session says why it waits otherwise.
    pub(crate) async fn guard<T>(
        &mut self,
        cancel: Option<&Killer>,
        request: impl Future<Output = T>,
    ) -> Result<T, Closed> {
        tokio::pin!(request);
        let mut kills = cancel.map(Killer::watch);
        let began = Instant::now();
        let mut kill = kills.as_mut().map_or(Kill::None, |k| *k.borrow_and_update());
        let mut watch = watching(kill, began, &self.wire, cancel, &self.stopping);
        loop {
            // What is watched beside the request is polled before it: its reads use up the
            // task's budget of the runtime's resources poll after poll while a result streams
            // in, and a kill's news and a closed channel would never be seen after them. The
            // request comes before giving up: what it can read now still ends it. What is
            // watched about the kill is made once per kill, not per pass: commands that keep
            // coming never put a cut off.
            tokio::select! {
                biased;
                changed = async {
                    match kills.as_mut() {
                        Some(k) => k.changed().await,
                        None => std::future::pending().await,
                    }
                } => match (changed, kills.as_mut()) {
                    (Ok(()), Some(k)) => {
                        let now = *k.borrow_and_update();
                        if now != kill {
                            kill = now;
                            watch = watching(kill, began, &self.wire, cancel, &self.stopping);
                        }
                    }
                    _ => kills = None,
                },
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
                why = &mut watch => {
                    self.wire.cut();
                    return Err(Closed::CancelNotTaken(why));
                }
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

/// What the session does about kill `kill` of the statement a request waits for (the request
/// began at `began`); resolves with why when the connection is to be cut.
///
/// * None: nothing.
/// * Not sent: cut at once (the server cannot be reached to stop it; reading on would wait on
///   a result that may have no end).
/// * Refused (the statement runs as another account): cut only while its rows keep coming
///   ([`SENDING_TICKS`] ticks in a row): a statement that sends ends when it finds the
///   connection gone. One that sends nothing would run on to its end all the same, cut or not
///   (the server does not look at its connection meanwhile): it is waited for, the session
///   says so every [`STOPPING_TICK`] ([`DbEvent::CancelRefused`]) and asks the server again
///   each time (kept only if taken).
/// * Taken: waited for however long the statement takes to stop (a rollback says nothing
///   meanwhile), the session saying so while the server says the statement is still there
///   ([`DbEvent::Stopping`]); cut only when its rows still keep coming more than [`TAKE_WAIT`]
///   after the kill (or after the request began, when that was later).
///
/// Its timers are not held back by the task's budget, which the request's reads use up while
/// rows stream in.
fn watching(
    kill: Kill,
    began: Instant,
    wire: &Wire,
    killer: Option<&Killer>,
    stopping: &Option<(UnboundedSender<DbEvent>, u64)>,
) -> BoxFuture<'static, CancelFailure> {
    let (wire, killer, stopping) = (wire.clone(), killer.cloned(), stopping.clone());
    Box::pin(tokio::task::unconstrained(async move {
        let say = |ev: fn(u64) -> DbEvent| {
            if let Some((events, id)) = &stopping {
                let _ = events.send(ev(*id));
            }
        };
        match kill {
            Kill::None => std::future::pending().await,
            Kill::Failed(KillFailure::NotSent) => CancelFailure::NotSent,
            Kill::Failed(KillFailure::Refused(_)) => {
                let runs_on = async {
                    // Past the time rows that keep coming take to cut it: said only of one
                    // that sends nothing.
                    tokio::time::sleep(SENDING_TICK * (SENDING_TICKS + 1)).await;
                    loop {
                        say(|id| DbEvent::CancelRefused { id });
                        tokio::time::sleep(STOPPING_TICK).await;
                        // The statement may run as another account only for moments (a
                        // definer function called for every row): asked again, the server
                        // may take it now, and the session then waits for it to stop.
                        if let Some(k) = &killer {
                            k.try_again().await;
                        }
                    }
                };
                tokio::select! {
                    () = runs_on => unreachable!("says so until the statement ends"),
                    () = keeps_sending(&wire) => CancelFailure::Refused,
                }
            }
            Kill::Taken(at) => {
                let stops = async {
                    loop {
                        tokio::time::sleep(STOPPING_TICK).await;
                        if let Some(k) = &killer
                            && k.still_there().await
                        {
                            say(|id| DbEvent::Stopping { id });
                        }
                    }
                };
                let sends_on = async {
                    tokio::time::sleep_until((at.max(began) + TAKE_WAIT).into()).await;
                    keeps_sending(&wire).await;
                };
                tokio::select! {
                    () = stops => unreachable!("says so until the statement ends"),
                    () = sends_on => CancelFailure::KeptSending,
                }
            }
        }
    }))
}

/// Resolves once the server's bytes came in [`SENDING_TICKS`] ticks in a row: rows that keep
/// coming, not a backlog a slow link (a tunnel) still delivers in a burst.
async fn keeps_sending(wire: &Wire) {
    let (mut seen, mut ticks) = (wire.received(), 0);
    loop {
        tokio::time::sleep(SENDING_TICK).await;
        let now = wire.received();
        ticks = if now > seen { ticks + 1 } else { 0 };
        if ticks >= SENDING_TICKS {
            return;
        }
        seen = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::Route;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::unbounded_channel;

    /// A link of run 3 over an in-memory stream, as the session sees it after kill `kill`:
    /// the link, the client's end to read the result from, the server's end, the killer, what
    /// the session says, and the UI's end of the command channel.
    struct Setup {
        link: Link,
        stream: crate::wire::WireStream,
        server: tokio::io::DuplexStream,
        killer: Killer,
        events: tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        commands: tokio::sync::mpsc::UnboundedSender<DbCommand>,
    }

    fn after(kill: Kill) -> Setup {
        let (client, server) = tokio::io::duplex(1024);
        let (wire, stream) = Wire::new(Box::new(client));
        let (commands, rx) = unbounded_channel();
        let killer = Killer::new(
            Route::new("127.0.0.1", 1, None),
            mysql_async::Opts::from_url("mysql://datarig@127.0.0.1:1/").unwrap(),
            7,
        );
        killer.record(kill, || true);
        let (tx, events) = unbounded_channel();
        let mut link = Link::new(rx, wire);
        link.serving(&tx, 3);
        std::mem::forget(tx);
        Setup { link, stream, server, killer, events, commands }
    }

    fn taken(ago: Duration) -> Kill {
        Kill::Taken(Instant::now().checked_sub(ago).unwrap())
    }

    const REFUSED: Kill = Kill::Failed(KillFailure::Refused(1095));

    /// Read the result without end.
    async fn read_on(mut stream: crate::wire::WireStream) {
        let mut buf = [0u8; 64];
        while stream.read(&mut buf).await.is_ok_and(|n| n > 0) {}
    }

    /// Rows without end, every 20 ms.
    fn send_on(mut server: tokio::io::DuplexStream) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while server.write_all(b"row").await.is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    }

    #[tokio::test]
    async fn rows_that_keep_coming_after_a_taken_kill_cut_the_connection() {
        // Taken long ago: the bound counts from when the request began.
        let Setup { mut link, stream, server, killer, commands: _commands, .. } = after(taken(TAKE_WAIT * 2));
        let send = send_on(server);
        let t0 = Instant::now();
        let out = tokio::time::timeout(TAKE_WAIT * 2, link.guard(Some(&killer), read_on(stream))).await;
        assert!(matches!(out, Ok(Err(Closed::CancelNotTaken(CancelFailure::KeptSending)))));
        assert!(t0.elapsed() >= TAKE_WAIT + SENDING_TICK * SENDING_TICKS, "{:?}", t0.elapsed());
        send.abort();
    }

    #[tokio::test]
    async fn commands_that_keep_coming_never_put_a_cut_off() {
        let Setup { mut link, stream, server, killer, commands, .. } = after(taken(Duration::ZERO));
        let send = send_on(server);
        let spam = tokio::spawn(async move {
            while commands.send(DbCommand::FetchMore { id: 3 }).is_ok() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let out = tokio::time::timeout(TAKE_WAIT * 2, link.guard(Some(&killer), read_on(stream))).await;
        assert!(matches!(out, Ok(Err(Closed::CancelNotTaken(CancelFailure::KeptSending)))));
        send.abort();
        spam.abort();
    }

    #[tokio::test]
    async fn a_backlog_after_a_taken_kill_is_read_not_cut() {
        // A burst once past the bound (what a slow link still held), then nothing: not rows
        // that keep coming.
        let Setup { mut link, stream, mut server, killer, commands: _commands, .. } = after(taken(Duration::ZERO));
        let burst = tokio::spawn(async move {
            tokio::time::sleep(TAKE_WAIT + SENDING_TICK).await;
            let _ = server.write_all(&[0u8; 512]).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let wait = TAKE_WAIT + SENDING_TICK * (SENDING_TICKS + 4);
        let out = tokio::time::timeout(wait, link.guard(Some(&killer), read_on(stream))).await;
        assert!(out.is_err(), "still waited for");
        burst.abort();
    }

    #[tokio::test]
    async fn a_statement_silent_after_a_taken_kill_is_waited_for() {
        // Silent (a rollback) past the bound: never cut.
        let Setup { mut link, stream, server, killer, commands: _commands, .. } = after(taken(Duration::ZERO));
        let wait = TAKE_WAIT + SENDING_TICK * 4;
        let out = tokio::time::timeout(wait, link.guard(Some(&killer), read_on(stream))).await;
        assert!(out.is_err(), "still waited for");
        drop(server);
    }

    #[tokio::test]
    async fn a_refused_kill_of_a_statement_that_sends_cuts_the_connection() {
        let Setup { mut link, stream, server, killer, commands: _commands, .. } = after(REFUSED);
        let send = send_on(server);
        let t0 = Instant::now();
        let out = tokio::time::timeout(Duration::from_secs(5), link.guard(Some(&killer), read_on(stream))).await;
        assert!(matches!(out, Ok(Err(Closed::CancelNotTaken(CancelFailure::Refused)))));
        assert!(t0.elapsed() >= SENDING_TICK * SENDING_TICKS, "{:?}", t0.elapsed());
        send.abort();
    }

    #[tokio::test]
    async fn a_refused_kill_of_a_silent_statement_is_waited_for_and_said() {
        // Cutting would not stop it: the session waits, and says why every few seconds.
        let Setup { mut link, stream, server, killer, mut events, commands: _commands } = after(REFUSED);
        let wait = STOPPING_TICK + SENDING_TICK * (SENDING_TICKS + 2);
        let out = tokio::time::timeout(wait, link.guard(Some(&killer), read_on(stream))).await;
        assert!(out.is_err(), "still waited for");
        let mut said = 0;
        while let Ok(ev) = events.try_recv() {
            assert!(matches!(ev, DbEvent::CancelRefused { id: 3 }), "{ev:?}");
            said += 1;
        }
        assert_eq!(said, 2, "once past the time a cut takes, then every tick");
        drop(server);
    }

    #[tokio::test]
    async fn commands_that_keep_coming_never_silence_what_the_session_says() {
        let Setup { mut link, stream, server, killer, mut events, commands } = after(REFUSED);
        let spam = tokio::spawn(async move {
            while commands.send(DbCommand::FetchMore { id: 3 }).is_ok() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let wait = STOPPING_TICK + SENDING_TICK * (SENDING_TICKS + 2);
        let out = tokio::time::timeout(wait, link.guard(Some(&killer), read_on(stream))).await;
        assert!(out.is_err(), "still waited for");
        let said = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|e| matches!(e, DbEvent::CancelRefused { .. }))
            .count();
        assert_eq!(said, 2);
        spam.abort();
        drop(server);
    }

    #[tokio::test]
    async fn a_kill_that_was_not_sent_cuts_the_connection_at_once() {
        let Setup { mut link, stream, server, killer, commands: _commands, .. } =
            after(Kill::Failed(KillFailure::NotSent));
        let out = tokio::time::timeout(Duration::from_secs(1), link.guard(Some(&killer), read_on(stream))).await;
        assert!(matches!(out, Ok(Err(Closed::CancelNotTaken(CancelFailure::NotSent)))));
        drop(server);
    }
}
