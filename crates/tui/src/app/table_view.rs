//! The columns hidden from table tabs' grids ([`datarig_core::table_view`]): kept per table
//! in `<state>/table-view.toml` (mode 0600 on Unix, never in the workspace file), so they follow
//! the table into any tab, a reopened one or another workspace. A view only: the query stays
//! `SELECT *`, every column is still read, and only the grid leaves them out (export, the
//! chart, the inspector and edits see every column).
//!
//! `C` in the grid hides the cursor's column (and the column menu's entry); the filter and sort
//! dialog hides and shows them (`Space` or `C` on its column list). Each change is one the
//! dialog's undo brings back, as a change of the query is.
//!
//! The file is read the first time a table tab shows its rows. A file that is there but cannot
//! be used is said once; hidden columns are then kept for the session only, and the file is
//! never written over (unknown is not absent). Without a state directory (a second instance)
//! they live in memory only. Writes go to a thread of their own as the filter history's do: the
//! latest text only, a second after the last change, and before the app quits. A name no longer
//! among a table's columns is dropped only once the table's catalogue was read, never from the
//! columns of a result.

use super::filter_history::{DEBOUNCE, LogLine, Out, QUIT_FLUSH, Saved, Write, Writer, unix_secs};
use super::table_query::OrderKeysState;
use super::*;
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::table_view::{self as tv, TableView, Unreadable};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

/// The file as far as the app knows it.
#[derive(Default, PartialEq, Eq)]
enum Held {
    /// Not read yet.
    #[default]
    NotLoaded,
    /// Read (or not there): what is hidden is written back to it.
    Loaded,
    /// There but not usable: what is hidden is kept for the session only, never written.
    Unreadable,
}

/// The hidden columns' state in the app.
#[derive(Default)]
pub struct TableViewState {
    held: Held,
    /// What is hidden, per table (from the file, then as changed).
    view: TableView,
    /// Changed since the last write was sent.
    dirty: bool,
    /// When the next write is due.
    due: Option<Instant>,
    writer: Option<Writer>,
    /// The number of the last write sent.
    seq: u64,
    /// Headless: the writer's answers, and how many are still to come.
    local: Option<(mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>)>,
    pending: Arc<AtomicUsize>,
}

impl App {
    /// What is hidden, read from the file the first time. A file that cannot be used is said
    /// once (and recorded in the error log, never its text); the hidden columns are then the
    /// session's own.
    fn table_view_held(&mut self) -> &mut TableView {
        if self.table_view.held == Held::NotLoaded {
            let read = match self.paths.table_view_file() {
                None => Ok(TableView::default()),
                Some(path) => tv::load(&path).map_err(|u| (u, path)),
            };
            match read {
                Ok(v) => {
                    self.table_view.view = v;
                    self.table_view.held = Held::Loaded;
                }
                Err((u, path)) => {
                    self.table_view.held = Held::Unreadable;
                    let error = self.table_view_unreadable(u, &path);
                    self.flash(Notice::new(Msg::TableViewUnavailable { error }, Level::Warning));
                }
            }
        }
        &mut self.table_view.view
    }

    /// Why the file at `path` cannot be used, in words; the error log gets what failed and the
    /// path.
    fn table_view_unreadable(&mut self, u: Unreadable, path: &std::path::Path) -> String {
        let at = path.display().to_string();
        let fault = match u {
            Unreadable::Io(kind) => Fault::new(FaultKind::Io(kind), at.clone()),
            Unreadable::Malformed { line } => Fault::new(FaultKind::Toml { line }, at.clone()),
            Unreadable::TooLarge => Fault::other(format!("{at}: larger than the read limit")),
            Unreadable::Newer(version) => Fault::other(format!("{at}: version {version}")),
            Unreadable::NotAFile => Fault::other(format!("{at}: not a plain file")),
        };
        let why = fault_reason(&fault);
        if let Some(log) = self.paths.errors_log()
            && let Some(w) = self.table_view_writer()
        {
            w.log(LogLine { log, context: "table_view.unreadable", fault });
        }
        match u {
            Unreadable::Io(_) | Unreadable::Malformed { .. } => self.reason_text(&why),
            Unreadable::TooLarge => self.i18n.label(Label::TableHistoryTooLarge).to_string(),
            Unreadable::Newer(version) => {
                self.i18n.msg(&Msg::TableHistoryNewer { version: version.to_string() }).to_string()
            }
            Unreadable::NotAFile => self.i18n.msg(&Msg::TableHistoryNotAFile { path: at }).to_string(),
        }
    }

    /// The names of the columns hidden from table tab `id`'s grid (none for any other tab).
    pub fn hidden_columns(&self, id: TabId) -> Vec<String> {
        self.filter_history_key(id).map(|k| self.table_view.view.hidden(&k).to_vec()).unwrap_or_default()
    }

    /// Table tab `id`'s grid shows what is hidden of its table: its columns of those names are
    /// left out (the cursor moves off them), its search looks again once that changed. Names
    /// the table's catalogue, as read, no longer has are dropped.
    pub(super) fn table_view_sync(&mut self, id: TabId) {
        let Some(key) = self.filter_history_key(id) else { return };
        // Only a list of the table's columns read from its catalogue is a word on what it has.
        let catalogue: Option<Vec<String>> = self.tabs.get(id).and_then(|t| {
            let tq = t.doc.table_query.as_ref()?;
            let on = self.keys_on(t)?;
            match &tq.order_keys {
                OrderKeysState::Known { on: o, keys } if *o == on => {
                    keys.columns.as_ref().map(|c| c.iter().map(|c| c.name.clone()).collect())
                }
                _ => None,
            }
        });
        let view = self.table_view_held();
        if let Some(cat) = catalogue {
            let names: Vec<&str> = cat.iter().map(String::as_str).collect();
            if view.prune(&key, &names) {
                self.table_view_changed();
            }
        }
        let hidden = self.table_view.view.hidden(&key).to_vec();
        let Some(t) = self.tabs.get_mut(id) else { return };
        if let Some(tq) = t.doc.table_query.as_mut() {
            tq.hidden = hidden.clone();
        }
        let Results::Rows(rs) = &t.results else { return };
        let mask: Vec<bool> = rs.columns.iter().map(|c| hidden.contains(&c.meta.name)).collect();
        let total = rs.columns.len();
        let before = t.grid.vis_rev;
        t.grid.set_hidden(mask, total);
        if t.grid.vis_rev != before {
            self.grid_search_columns_changed(id);
        }
    }

    /// Table tab `id`'s table's hidden columns become `names`; every tab of that table shows
    /// it. `record`: the change before it is kept for the dialog's undo (not when a run sent
    /// with it kept it already). Whether anything changed.
    pub(crate) fn set_hidden_columns(&mut self, id: TabId, names: &[String], record: bool) -> bool {
        let Some(key) = self.filter_history_key(id) else { return false };
        let before = self.table_view_held().hidden(&key).to_vec();
        let now = unix_secs(self.wall_now());
        if !self.table_view.view.set(key.clone(), names, now) {
            return false;
        }
        if record && let Some(tq) = self.table_query_mut(id) {
            tq.before_last = Some(tq.applied().clone());
            tq.before_hidden = Some(before);
        }
        self.table_view_changed();
        let tabs: Vec<TabId> = self.tabs.iter().map(|t| t.id).collect();
        for t in tabs {
            if self.filter_history_key(t).as_ref() == Some(&key) {
                self.table_view_sync(t);
            }
        }
        true
    }

    /// The active tab's grid has no cell at its cursor: its rows have columns and the cursor's
    /// is hidden (with every column hidden, the cursor stays on the last one, which addresses
    /// nothing until a column is shown again).
    pub(super) fn grid_no_cell(&self) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let t = self.tab();
        matches!(&t.results, Results::Rows(rs) if !rs.columns.is_empty() && !t.grid.shown(t.grid.col))
    }

    /// What a key that reads or writes the cursor's cell or the selection says when the grid
    /// has no cell there ([`App::grid_no_cell`]): how to show a column again.
    pub(super) fn no_cell_notice(&self) -> Notice {
        let open = self.key_for(Action::Table(super::table_filter::TableAction::EditFilter), Ctx::Grid);
        Notice::new(Msg::TableAllHidden { open }, Level::Info)
    }

    /// Said ([`App::no_cell_notice`]) when the grid has no cell at its cursor: then a key that
    /// reads or writes cells does nothing.
    pub(super) fn refused_without_cell(&mut self) -> bool {
        if !self.grid_no_cell() {
            return false;
        }
        self.flash(self.no_cell_notice());
        true
    }

    /// `C` (and the column menu's entry): the cursor's column hidden from the grid; the cursor
    /// goes to the next column shown.
    pub(super) fn table_hide_column(&mut self) {
        let id = self.tab().id;
        let column = match &self.tab().results {
            Results::Rows(rs) => rs.columns.get(self.tab().grid.col).map(|c| c.meta.name.clone()),
            _ => None,
        };
        let Some(column) = column else {
            return self.flash(Notice::new(Label::TableSortNoColumns, Level::Info));
        };
        self.close_search_prompt();
        let mut names = self.hidden_columns(id);
        if !names.contains(&column) {
            names.push(column);
        }
        self.set_hidden_columns(id, &names, true);
    }

    /// What is hidden changed: written a second after the last change (never while the file
    /// cannot be used).
    fn table_view_changed(&mut self) {
        if self.table_view.held == Held::Loaded {
            self.table_view.dirty = true;
            self.table_view.due = Some(self.now() + DEBOUNCE);
        }
    }

    /// The writer thread, started the first time; `None` when it cannot start.
    fn table_view_writer(&mut self) -> Option<&Writer> {
        if self.table_view.writer.is_none() {
            self.table_view.writer = Writer::spawn("datarig-table-view", Box::new(tv::write)).ok();
        }
        self.table_view.writer.as_ref()
    }

    /// When the next write of the hidden columns is due.
    pub(super) fn table_view_due(&self) -> Option<Instant> {
        self.table_view.due
    }

    /// The timer: the write that is due.
    pub(super) fn table_view_tick(&mut self, now: Instant) {
        if self.table_view.due.is_some_and(|d| now >= d) {
            self.table_view_send();
        }
    }

    /// Send the text to the writer when it changed. Nothing is written without a state
    /// directory this instance may write, nor when the file could not be used.
    fn table_view_send(&mut self) {
        self.table_view.due = None;
        if !std::mem::take(&mut self.table_view.dirty) || self.table_view.held != Held::Loaded {
            return;
        }
        let Some(path) = self.state_dir().map(|s| s.join(tv::FILE)) else { return };
        let text = self.table_view.view.to_toml();
        let out = match &self.tx {
            Some(tx) => Out::App(tx.clone()),
            None => {
                self.table_view.pending.fetch_add(1, Ordering::SeqCst);
                Out::Local(self.table_view.local.get_or_insert_with(mpsc::channel).0.clone())
            }
        };
        self.table_view.seq += 1;
        let seq = self.table_view.seq;
        let log = self.paths.errors_log();
        let Some(writer) = self.table_view_writer() else {
            let error = std::io::Error::other("the writer thread cannot start");
            return out.send(AppEvent::TableView(Saved { seq, path, result: Err(error) }));
        };
        let (event, context) = (AppEvent::TableView as fn(Saved) -> AppEvent, "table_view.save_failed");
        if let Some(old) = writer.write(Write { path, text, seq, out, log, event, context })
            && let Out::Local(_) = old.out
        {
            self.table_view.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// The writer's answer: a failure is said, and the text is still to be written unless a
    /// newer one was sent since.
    pub(super) fn on_table_view_saved(&mut self, s: Saved) {
        if let Err(e) = s.result {
            if s.seq == self.table_view.seq {
                self.table_view.dirty = true;
            }
            let error = self.reason_text(&fault_reason(&Fault::io_at(&e, &s.path)));
            self.flash(Notice::new(Msg::TableViewSaveFailed { error }, Level::Error));
        }
    }

    /// Write what changed now and wait (at most [`QUIT_FLUSH`]) until the writer is done
    /// (quitting; the tests). Headless, its answers are handed to the app.
    pub fn flush_table_view(&mut self) {
        self.table_view_send();
        if let Some(w) = &self.table_view.writer {
            let (tx, rx) = mpsc::channel();
            w.flush(tx);
            let _ = rx.recv_timeout(QUIT_FLUSH);
        }
        while self.table_view.pending.load(Ordering::SeqCst) > 0 {
            let Some((_, rx)) = &self.table_view.local else { break };
            let Ok(ev) = rx.recv_timeout(QUIT_FLUSH) else { break };
            self.table_view.pending.fetch_sub(1, Ordering::SeqCst);
            self.on_app_event(ev);
        }
    }
}
