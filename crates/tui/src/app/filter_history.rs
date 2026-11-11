//! The filter history of table tabs ([`datarig_core::filter_history`]): the filters and orders
//! each table ran, kept in `<state>/filter-history.toml` (mode 0600 on Unix, never in the
//! workspace file) unless `[history] filters = "off"`.
//!
//! A query is kept once the server answered it with its first page: a run refused, cancelled,
//! failed or answered after a newer one is not (sent is not succeeded). The file is read the
//! first time something is kept or listed, not at launch. A file that is there but cannot be
//! used turns the history off for the session: said once, never taken for an empty history and
//! never written over (a clear is refused, naming the file). Without a state directory the
//! history lives in memory only; a second instance (the state directory is the first one's)
//! reads the file and keeps what it adds in memory.
//!
//! Writes go to one thread of their own, the latest text only (what waits behind a slow write
//! is one text, never a queue): a second after the last change
//! (a run again only moves its entry to the front), at once for a clear, and before the app
//! quits (waited for at most [`QUIT_FLUSH`]). The texts never go to the error log, only what
//! failed and the file's path, written by a thread of its own that is handed each line without
//! waiting (dropped when it is [`LOG_QUEUE`] lines behind): an error log that blocks never holds
//! up the UI thread nor a write of the history.
//!
//! `Ctrl+R` in the filter dialog (`Space r H` from the grid) lists the table's entries, the most
//! recent first, with both texts under the list; one picked fills the dialog's condition and
//! order (the limit stays), and nothing runs until `Enter`, which checks it as any text typed.

use super::*;
use crate::text::{sanitize_cell, width, wrap_words};
use crate::widgets::list_picker::{Entry as Item, ListPicker, Outcome};
use crate::widgets::text_input::TextInput;
use datarig_core::config::FilterHistory;
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::filter_history::{self as fh, Entry, History, TableKey, Unreadable};
use datarig_core::query::{KeyOrigin, Query};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError, mpsc};

/// A change is written this long after the last one.
pub(super) const DEBOUNCE: Duration = Duration::from_secs(1);

/// How long quitting waits for the last write.
pub(super) const QUIT_FLUSH: Duration = Duration::from_secs(1);

/// The error log lines waiting for their thread at most; more are dropped.
pub(super) const LOG_QUEUE: usize = 16;

/// The lines of a picked entry's texts under the list.
pub(crate) const PREVIEW_LINES: u16 = 6;

/// The writer wrote the text of change `seq` (and every one before it), or failed to.
#[derive(Debug)]
pub struct Saved {
    pub(super) seq: u64,
    pub(super) path: PathBuf,
    pub(super) result: std::io::Result<()>,
}

/// Where the writer's answers go.
#[derive(Clone)]
pub(super) enum Out {
    App(tokio::sync::mpsc::UnboundedSender<AppEvent>),
    Local(mpsc::Sender<AppEvent>),
}

impl Out {
    pub(super) fn send(&self, ev: AppEvent) {
        match self {
            Out::App(tx) => {
                let _ = tx.send(ev);
            }
            Out::Local(tx) => {
                let _ = tx.send(ev);
            }
        }
    }
}

/// A write for the writer: the file's text as of change `seq`.
pub(super) struct Write {
    pub(super) path: PathBuf,
    pub(super) text: String,
    pub(super) seq: u64,
    pub(super) out: Out,
    /// The error log a failure is recorded in (what failed and the path).
    pub(super) log: Option<PathBuf>,
    /// The answer of the write, as the app gets it.
    pub(super) event: fn(Saved) -> AppEvent,
    /// What a failure is recorded as in the error log (a message key).
    pub(super) context: &'static str,
}

/// A line for the error log: where, its context (a message key) and what failed.
pub(super) struct LogLine {
    pub(super) log: PathBuf,
    pub(super) context: &'static str,
    pub(super) fault: Fault,
}

/// How the writer writes a file (the tests stand in for it).
pub(super) type WriteFile = Box<dyn Fn(&std::path::Path, &str) -> std::io::Result<()> + Send>;

/// What the error log's thread is handed: a line, or a sender answered once every line
/// handed before it is written.
enum ToLog {
    Line(LogLine),
    Drained(mpsc::Sender<()>),
}

/// The thread of the error log's lines: handed each one without waiting, best effort (a line
/// is dropped when [`LOG_QUEUE`] wait, as behind an error log that blocks whoever writes it).
#[derive(Clone)]
struct Logger(mpsc::SyncSender<ToLog>);

impl Logger {
    /// Start the thread; `None` when it cannot start (the lines are dropped then).
    fn spawn() -> Option<Self> {
        let (tx, rx) = mpsc::sync_channel::<ToLog>(LOG_QUEUE);
        std::thread::Builder::new()
            .name("datarig-filter-history-log".into())
            .spawn(move || {
                for item in rx {
                    match item {
                        ToLog::Line(l) => {
                            datarig_core::fault::ErrorLog::new(Some(l.log)).record(l.context, &l.fault);
                        }
                        ToLog::Drained(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .ok()?;
        Some(Logger(tx))
    }

    /// Hand `line` over, never waiting.
    fn log(&self, line: LogLine) {
        let _ = self.0.try_send(ToLog::Line(line));
    }

    /// `done` is answered once every line handed before it is written; `false` (never
    /// answered) when the queue is full.
    fn drained(&self, done: mpsc::Sender<()>) -> bool {
        self.0.try_send(ToLog::Drained(done)).is_ok()
    }
}

/// What waits for the writer: at most one write, the latest (a newer one replaces it), and
/// the flushes, each answered once every write sent before it is done.
#[derive(Default)]
struct Queue {
    write: Option<Write>,
    flushes: Vec<mpsc::Sender<()>>,
    /// The app is gone: the thread ends once the queue is done.
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The writer thread of the history: its files are written one at a time, in the order of
/// their changes, off the UI thread; its lines of the error log go to a [`Logger`] of their
/// own. However long a write takes, what waits behind it stays one text: an older text is
/// never written after a newer one.
pub(super) struct Writer {
    shared: Arc<Shared>,
    logger: Option<Logger>,
}

impl Writer {
    /// Start the thread `name` (and the error log's).
    pub(super) fn spawn(name: &str, write_file: WriteFile) -> std::io::Result<Self> {
        let shared = Arc::<Shared>::default();
        let theirs = shared.clone();
        let logger = Logger::spawn();
        let log = logger.clone();
        std::thread::Builder::new().name(name.into()).spawn(move || run_writer(&theirs, &write_file, log.as_ref()))?;
        Ok(Writer { shared, logger })
    }

    /// Queue `w`, replacing the write still waiting, if any: that one is never written (`w`
    /// is newer) and is handed back.
    pub(super) fn write(&self, w: Write) -> Option<Write> {
        let old = self.shared.queue().write.replace(w);
        self.shared.ready.notify_one();
        old
    }

    /// Record `line` in the error log, never waiting for it.
    pub(super) fn log(&self, line: LogLine) {
        if let Some(l) = &self.logger {
            l.log(line);
        }
    }

    /// `done` is answered once every line of the error log handed over before it is written;
    /// `false` (never answered) without the error log's thread or when it is too far behind.
    fn log_drained(&self, done: mpsc::Sender<()>) -> bool {
        self.logger.as_ref().is_some_and(|l| l.drained(done))
    }

    /// `done` is answered once every write queued before it is done.
    pub(super) fn flush(&self, done: mpsc::Sender<()>) {
        self.shared.queue().flushes.push(done);
        self.shared.ready.notify_one();
    }

    /// The writes waiting (not the one being written).
    #[cfg(test)]
    fn waiting(&self) -> usize {
        usize::from(self.shared.queue().write.is_some())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shared.queue().closed = true;
        self.shared.ready.notify_one();
    }
}

/// The writer thread: waits for work, takes all of it, writes the latest text (a failure is
/// answered, then handed to the error log's thread) and answers the flushes; ends once the app
/// is gone and nothing waits.
fn run_writer(shared: &Shared, write_file: &WriteFile, logger: Option<&Logger>) {
    loop {
        let (write, flushes) = {
            let mut q = shared.queue();
            while q.write.is_none() && q.flushes.is_empty() && !q.closed {
                q = shared.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
            if q.write.is_none() && q.flushes.is_empty() {
                return;
            }
            (q.write.take(), std::mem::take(&mut q.flushes))
        };
        if let Some(Write { path, text, seq, out, log, event, context }) = write {
            let result = write_file(&path, &text);
            let failed = result.as_ref().err().map(|e| Fault::io_at(e, &path));
            out.send(event(Saved { seq, path, result }));
            if let (Some(fault), Some(log), Some(logger)) = (failed, log, logger) {
                logger.log(LogLine { log, context, fault });
            }
        }
        for f in flushes {
            let _ = f.send(());
        }
    }
}

/// The history as far as the app knows it.
#[derive(Default)]
enum Held {
    /// The file was not read yet.
    #[default]
    NotLoaded,
    Loaded(History),
    /// The file is there but cannot be used (why): off for the session.
    Off(Msg),
}

/// The filter history's state in the app.
#[derive(Default)]
pub struct FilterHistoryState {
    held: Held,
    /// Changed since the last write was sent.
    dirty: bool,
    /// When the next write is due.
    due: Option<Instant>,
    writer: Option<Writer>,
    /// The number of the last write sent.
    seq: u64,
    /// The number of the last write the writer said was done (every one before it is done or
    /// replaced by it): the file holds the history as of that write.
    saved: u64,
    /// What to say once write `seq` (or a later one) is done (the clears), dropped when it
    /// fails (said as failed): one per write, the latest (a clear asked again for the same
    /// write replaces it, and a write replaced before it was written passes its own to the
    /// newer one), so it never grows with the clears asked.
    announce: Vec<(u64, Notice)>,
    /// The table a clear asks about (`None`: every table).
    clear_asked: Option<TableKey>,
    /// Headless: the writer's answers, and how many are still to come.
    local: Option<(mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>)>,
    pending: Arc<AtomicUsize>,
}

impl App {
    /// The table of table tab `id` as the history names it: its profile, its database by name
    /// (the profile's own too, so a tab in it with or without its name is the same table), its
    /// schema and name. `None` without a profile.
    pub(super) fn filter_history_key(&self, id: TabId) -> Option<TableKey> {
        let t = self.tabs.get(id).filter(|t| t.is_table())?;
        let table = t.doc.table.as_ref()?;
        let profile = t.profile?;
        let database = t.context.database.clone().unwrap_or_else(|| self.own_database(profile));
        Some(TableKey { profile, database, schema: table.schema.clone(), table: table.name.clone() })
    }

    /// When the next write of the history is due.
    pub(super) fn filter_history_due(&self) -> Option<Instant> {
        self.filter_history.due
    }

    /// The history, read from the file the first time. A file that cannot be used turns it
    /// off for the session, says so once and records what failed (never its text) in the error
    /// log: `Err(why)` from then on.
    fn filter_history_held(&mut self) -> Result<&mut History, Msg> {
        if matches!(self.filter_history.held, Held::NotLoaded) {
            let read = match self.paths.filter_history_file() {
                None => Ok(History::default()),
                Some(path) => fh::load(&path).map_err(|u| (u, path)),
            };
            self.filter_history.held = match read {
                Ok(h) => Held::Loaded(h),
                Err((u, path)) => {
                    let why = self.filter_history_unreadable(u, &path);
                    let error = self.i18n.msg(&why).to_string();
                    self.flash(Notice::new(Msg::TableHistoryUnavailable { error }, Level::Warning));
                    Held::Off(why)
                }
            };
        }
        match &mut self.filter_history.held {
            Held::Loaded(h) => Ok(h),
            Held::Off(why) => Err(why.clone()),
            Held::NotLoaded => Err(Label::FaultOther.into()),
        }
    }

    /// Why the file at `path` cannot be used, in words; the error log gets what failed and the
    /// path (written by the writer thread).
    fn filter_history_unreadable(&mut self, u: Unreadable, path: &std::path::Path) -> Msg {
        let at = path.display().to_string();
        let (fault, why) = match u {
            Unreadable::Io(kind) => {
                let f = Fault::new(FaultKind::Io(kind), at);
                let why = fault_reason(&f);
                (f, why)
            }
            Unreadable::Malformed { line } => {
                let f = Fault::new(FaultKind::Toml { line }, at);
                let why = fault_reason(&f);
                (f, why)
            }
            Unreadable::TooLarge => {
                (Fault::other(format!("{at}: larger than the read limit")), Label::TableHistoryTooLarge.into())
            }
            Unreadable::Newer(version) => (
                Fault::other(format!("{at}: version {version}")),
                Msg::TableHistoryNewer { version: version.to_string() },
            ),
            Unreadable::NotAFile => {
                (Fault::other(format!("{at}: not a plain file")), Msg::TableHistoryNotAFile { path: at.clone() })
            }
        };
        if let Some(log) = self.paths.errors_log()
            && let Some(w) = self.filter_history_writer()
        {
            w.log(LogLine { log, context: "filter_history.unreadable", fault });
        }
        why
    }

    /// The writer thread, started the first time; `None` when it cannot start.
    fn filter_history_writer(&mut self) -> Option<&Writer> {
        if self.filter_history.writer.is_none() {
            self.filter_history.writer = Writer::spawn("datarig-filter-history", Box::new(fh::write)).ok();
        }
        self.filter_history.writer.as_ref()
    }

    /// The server answered query `q` of table tab `id` with its rows: its filter and order go
    /// to the front of the table's history (nothing with the history off, without a filter or
    /// an order, or when the file cannot be used).
    pub(super) fn filter_history_record(&mut self, id: TabId, q: &Query) {
        if self.prefs.filter_history == FilterHistory::Off {
            return;
        }
        let Some(key) = self.filter_history_key(id) else { return };
        let filter = table_query::filter_text(q).unwrap_or_default().to_string();
        let keys: Vec<_> = q.order.iter().filter(|k| k.origin == KeyOrigin::User).cloned().collect();
        let order = datarig_core::query::order_text(&keys, self.tab_dialect(id));
        // Only texts the filter dialog's fields hold as they are: a pick never alters what it fills in.
        if (filter.trim().is_empty() && order.trim().is_empty())
            || !TextInput::holds(&filter)
            || !TextInput::holds(&order)
        {
            return;
        }
        let now = unix_secs(self.wall_now());
        let Ok(h) = self.filter_history_held() else { return };
        if h.record(key, &filter, &order, now) {
            self.filter_history.dirty = true;
            self.filter_history.due = Some(self.now() + DEBOUNCE);
        }
    }

    /// The timer: the write that is due.
    pub(super) fn filter_history_tick(&mut self, now: Instant) {
        if self.filter_history.due.is_some_and(|d| now >= d) {
            self.filter_history_send();
        }
    }

    /// Send the history's text to the writer when it changed. Nothing is written without a
    /// state directory this instance may write, nor when the file could not be used.
    fn filter_history_send(&mut self) {
        self.filter_history.due = None;
        if !self.filter_history.dirty {
            return;
        }
        self.filter_history.dirty = false;
        let Held::Loaded(h) = &self.filter_history.held else { return };
        let Some(path) = self.state_dir().map(|s| s.join(fh::FILE)) else { return };
        let text = h.to_toml();
        let out = self.filter_history_out();
        self.filter_history.seq += 1;
        let seq = self.filter_history.seq;
        let log = self.paths.errors_log();
        let Some(writer) = self.filter_history_writer() else {
            // It cannot start: said as a failed write.
            let error = std::io::Error::other("the writer thread cannot start");
            return out.send(AppEvent::FilterHistory(Saved { seq, path, result: Err(error) }));
        };
        // A write still waiting is replaced by this newer one: it is never written nor answered,
        // so what it was to say waits for the newer one.
        let (event, context) = (AppEvent::FilterHistory as fn(Saved) -> AppEvent, "filter_history.save_failed");
        if let Some(old) = writer.write(Write { path, text, seq, out, log, event, context }) {
            if let Out::Local(_) = old.out {
                self.filter_history.pending.fetch_sub(1, Ordering::SeqCst);
            }
            if let Some(n) = self.filter_history.take_announce(old.seq) {
                self.filter_history.announce_at(seq, n);
            }
        }
    }

    /// Where the writer answers; headless, one more answer is waited for.
    fn filter_history_out(&mut self) -> Out {
        match &self.tx {
            Some(tx) => Out::App(tx.clone()),
            None => {
                self.filter_history.pending.fetch_add(1, Ordering::SeqCst);
                Out::Local(self.filter_history.local.get_or_insert_with(mpsc::channel).0.clone())
            }
        }
    }

    /// The writer's answer: a failure is said (the writer records it with the path, never the
    /// texts),
    /// and the history is still to be written unless a newer text was sent since (the next
    /// change, clear or quitting writes it); what a clear waits for is said once its write is
    /// done.
    pub(super) fn on_filter_history_saved(&mut self, s: Saved) {
        match s.result {
            Ok(()) => {
                self.filter_history.saved = self.filter_history.saved.max(s.seq);
                let done: Vec<_> = self.filter_history.announce.extract_if(.., |(seq, _)| s.seq >= *seq).collect();
                for (_, n) in done {
                    self.flash(n);
                }
            }
            Err(e) => {
                self.filter_history.announce.retain(|(seq, _)| s.seq < *seq);
                if s.seq == self.filter_history.seq {
                    self.filter_history.dirty = true;
                }
                let error = self.reason_text(&fault_reason(&Fault::io_at(&e, &s.path)));
                self.flash(Notice::new(Msg::TableHistorySaveFailed { error }, Level::Error));
            }
        }
    }

    /// Headless (without the event loop's channel): hand the writer's answers to the app,
    /// waiting up to `wait` for each one still to come. How many came.
    pub fn pump_filter_history(&mut self, wait: Duration) -> usize {
        let deadline = Instant::now() + wait;
        let mut n = 0;
        while self.filter_history.pending.load(Ordering::SeqCst) > 0 {
            let Some((_, rx)) = &self.filter_history.local else { break };
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(ev) = rx.recv_timeout(left) else { break };
            self.filter_history.pending.fetch_sub(1, Ordering::SeqCst);
            n += 1;
            self.on_app_event(ev);
        }
        n
    }

    /// Write what changed now and wait (at most [`QUIT_FLUSH`]) until the writer is done
    /// (quitting; the tests). Headless, its answers are handed to the app.
    pub fn flush_filter_history(&mut self) {
        self.filter_history_send();
        if let Some(w) = &self.filter_history.writer {
            let (tx, rx) = mpsc::channel();
            w.flush(tx);
            let _ = rx.recv_timeout(QUIT_FLUSH);
        }
        self.pump_filter_history(Duration::ZERO);
    }

    /// Wait at most `wait` until the error log's thread wrote every line the history handed it
    /// so far (after [`App::flush_filter_history`], the failures of the writes it waited for
    /// too); whether it did. Never waits when that thread is too far behind (an error log that
    /// blocks). The tests read the error log after it.
    pub fn drain_filter_history_log(&self, wait: Duration) -> bool {
        let Some(w) = &self.filter_history.writer else { return true };
        let (tx, rx) = mpsc::channel();
        w.log_drained(tx) && rx.recv_timeout(wait).is_ok()
    }

    /// `Ctrl+R` / `Space r H`: the list of the filters table tab `id`'s table ran before.
    pub(super) fn open_filter_history(&mut self, id: TabId) {
        if self.prefs.filter_history == FilterHistory::Off {
            return self.say_over_filter_dialog(Notice::new(Label::TableHistoryOff, Level::Info));
        }
        let Some(key) = self.filter_history_key(id) else {
            return self.say_over_filter_dialog(Notice::new(Label::TableHistoryEmpty, Level::Info));
        };
        let was_loaded = !matches!(self.filter_history.held, Held::NotLoaded);
        let items = match self.filter_history_held() {
            Ok(h) => h.entries(&key).to_vec(),
            // Read just now: said already.
            Err(_) if !was_loaded => return,
            Err(why) => {
                let error = self.i18n.msg(&why).to_string();
                return self
                    .say_over_filter_dialog(Notice::new(Msg::TableHistoryUnavailable { error }, Level::Warning));
            }
        };
        if items.is_empty() {
            return self.say_over_filter_dialog(Notice::new(Label::TableHistoryEmpty, Level::Info));
        }
        let now = unix_secs(self.wall_now());
        let none = self.i18n.label(Label::TableHistoryNoCondition).to_string();
        let entries = items
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let label = if e.filter.is_empty() { none.clone() } else { sanitize_cell(&e.filter) };
                let age = self.age_text(u128::from(now.saturating_sub(e.used).max(0).unsigned_abs()) * 1000);
                let hint = if e.order.is_empty() {
                    age
                } else {
                    self.i18n.msg(&Msg::TableHistoryHint { order: sanitize_cell(&e.order), age }).to_string()
                };
                Item::new(i, label, hint)
            })
            .collect();
        let list = ListPicker::new(entries, None, Some(&0)).with_max_width(100);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Picker(super::picker::Picker::FilterHistory { tab: id, key, items, list }));
    }

    /// What the list of a table's filters did: a pick becomes the condition and order of that
    /// tab's filter dialog (the limit stays), opened for it when it is not open; nothing runs.
    /// Only while the tab is the active one and its table the one listed, and only texts shown
    /// on one line as they are (a line break: refused, said why).
    pub(super) fn filter_history_picked(&mut self, out: Outcome<usize>) {
        if out == Outcome::Pending {
            return;
        }
        let picked = match (self.overlays.picker(), out) {
            (Some(super::picker::Picker::FilterHistory { tab, key, items, .. }), Outcome::Picked(i)) => {
                items.get(i).map(|e| (*tab, key.clone(), e.clone()))
            }
            _ => None,
        };
        self.overlays.close(OverlayKind::Picker);
        let Some((tab, key, e)) = picked else { return };
        if self.tabs.is_empty() || self.tab().id != tab || self.filter_history_key(tab).as_ref() != Some(&key) {
            return;
        }
        if !TextInput::holds(&e.filter) || !TextInput::holds(&e.order) {
            return self.say_over_filter_dialog(Notice::new(Label::TableHistoryNotOneLine, Level::Warning));
        }
        if let Some(tq) = self.table_query_mut(tab) {
            tq.refusal = None;
        }
        self.filter_dialog_from_history(tab, &e.filter, &e.order);
    }

    /// `:filter history clear` (`id`: the table of that tab) or `:filter history clear all`
    /// (`None`): asked first, `Enter` cancels.
    pub(super) fn ask_filter_history_clear(&mut self, id: Option<TabId>) {
        let key = match id {
            None => None,
            Some(id) => match self.filter_history_key(id) {
                Some(k) => Some(k),
                None => return self.flash(Notice::new(Label::TableHistoryEmpty, Level::Info)),
            },
        };
        let text = match &key {
            Some(k) => Msg::TableHistoryClearText { table: format!("{}.{}", k.schema, k.table) },
            None => Label::TableHistoryClearTextAll.into(),
        };
        let all = key.is_none();
        self.filter_history.clear_asked = key;
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Confirm(super::overlay::Confirm {
            title: Label::TableHistoryClearTitle,
            text,
            details: Vec::new(),
            keys: Label::TableHistoryClearKeys,
            action: ConfirmAction::FilterHistoryClear { all },
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// `y` to a clear: the table's entries (`all`: every one) are forgotten and the file
    /// written at once; said once the write is done. Works with the history off too. A file
    /// that cannot be used is left as it is and the clear refused, naming it.
    pub(super) fn filter_history_clear_confirmed(&mut self, all: bool) {
        let key = self.filter_history.clear_asked.take();
        if !all && key.is_none() {
            return;
        }
        let h = match self.filter_history_held() {
            Ok(h) => h,
            Err(_) => {
                let path = self.paths.filter_history_file().map(|p| p.display().to_string()).unwrap_or_default();
                return self.flash(Notice::new(Msg::TableHistoryUnavailablePath { path }, Level::Error));
            }
        };
        let count = match &key {
            Some(k) if !all => h.clear_table(k),
            _ => h.clear_all(),
        } as u64;
        let done = Notice::new(
            if all { Msg::TableHistoryClearedAll { count } } else { Msg::TableHistoryCleared { count } },
            Level::Success,
        );
        // Another instance holds the state directory: the file is its to write.
        if self.read_only && self.paths.state.is_some() {
            return self.flash(Notice::new(Label::TableHistoryClearedHere, Level::Warning));
        }
        if self.state_dir().is_none() {
            return self.flash(done);
        }
        let fh = &mut self.filter_history;
        // Nothing forgotten now and nothing left unwritten (a clear whose write failed is):
        if count == 0 && !fh.dirty {
            // the file holds it already,
            if fh.saved == fh.seq {
                return self.flash(done);
            }
            // or will once the write on its way is done: said then (or its failure).
            let seq = fh.seq;
            return fh.announce_at(seq, done);
        }
        fh.dirty = true;
        self.filter_history_send();
        let seq = self.filter_history.seq;
        self.filter_history.announce_at(seq, done);
    }
}

impl FilterHistoryState {
    /// Say `n` once write `seq` is done, in place of what that write was to say.
    fn announce_at(&mut self, seq: u64, n: Notice) {
        match self.announce.iter_mut().find(|(s, _)| *s == seq) {
            Some((_, was)) => *was = n,
            None => self.announce.push((seq, n)),
        }
    }

    /// What write `seq` was to say, no longer said for it.
    fn take_announce(&mut self, seq: u64) -> Option<Notice> {
        let i = self.announce.iter().position(|(s, _)| *s == seq)?;
        Some(self.announce.remove(i).1)
    }
}

/// The lines under the list for entry `e`, `w` columns wide: `WHERE …` and `ORDER BY …`
/// wrapped at words, at most [`PREVIEW_LINES`], the last one cut with `…` when there is more.
pub(crate) fn preview(e: &Entry, w: usize) -> Vec<Line<'static>> {
    let th = crate::theme::cur();
    let (kw, text) = (Style::new().fg(th.accent).add_modifier(Modifier::BOLD), Style::new().fg(th.fg));
    let max = usize::from(PREVIEW_LINES);
    // (the keyword when the line starts with it, the line)
    let mut lines: Vec<(Option<&'static str>, String)> = Vec::new();
    for (word, part) in [("WHERE", &e.filter), ("ORDER BY", &e.order)] {
        if part.is_empty() {
            continue;
        }
        for (i, l) in wrap_words(&format!("{word} {}", sanitize_cell(part)), w).into_iter().enumerate() {
            lines.push(((i == 0 && l.starts_with(word)).then_some(word), l));
        }
    }
    if lines.len() > max {
        lines.truncate(max);
        let (_, last) = &mut lines[max - 1];
        let more = format!("{last}…");
        *last = if width(&more) > w { crate::text::fit(&more, w, crate::text::Align::Left) } else { more };
    }
    lines
        .into_iter()
        .map(|(word, l)| match word {
            Some(word) => Line::from(vec![Span::styled(word, kw), Span::styled(l[word.len()..].to_string(), text)]),
            None => Line::from(Span::styled(l, text)),
        })
        .collect()
}

/// `t` in Unix seconds (before 1970: 0).
pub(super) fn unix_secs(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests;
