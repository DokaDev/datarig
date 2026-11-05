//! A range pasted into the result grid: the terminal's own paste (a bracketed paste, which reads
//! no clipboard), or `space d v`, which reads the system clipboard on a worker (only with
//! `clipboard = "auto"` or `"system"`, and never over OSC 52). The text is read as TSV, CSV, one
//! value per line or one value ([`paste::parse`]) and laid over the grid from the cursor (or the
//! top-left of the selection; one value fills every selected cell), by the columns as the grid
//! shows them and the rows as it shows them: rows read, then new rows; rows past the last one
//! are new rows. It lands as pending changes, one step `u` undoes.
//!
//! Text with commas and no tab is CSV, or one column (one value) of values with commas in
//! them: the status line asks which (`c` or `1`; [`CsvAsk`]), never guessing.
//!
//! A paste that does not fit is refused whole, said, changing nothing: wider than the columns
//! from the cursor, over a column that takes no value or a row marked for deletion, past the
//! last row read while the server has more, or past the end where no row can be added. A paste
//! of more than [`ASK_ROWS`] rows asks first on the status line.
//!
//! A pasted value is never converted: one its column does not take is kept and marked
//! ([`Invalids`]: derived from the changes, made again only when they move), shown in the grid
//! and first in the Changes tab, and the changes are not applied until it is fixed, reverted
//! or undone (the planner refuses them). `space d V` reads the last paste again as the next
//! format, while it is the newest change.
//!
//! `g p` pastes the cells `y` or `Y` last copied in a grid ([`GridCopy`]) the same way, without
//! reading any text: each value as the grid showed it, NULL as NULL.

use super::*;
use crate::app::action::Action;
use crate::clipboard::{self, Plan as ClipPlan};
use datarig_core::driver::ColumnOrigin;
use datarig_core::edit::paste::{self, Format, PasteError, PasteOpts};
use datarig_core::edit::{Origin, PasteOps, PasteRefused};
use datarig_core::fault::{ErrorLog, Fault};
use datarig_core::i18n::{Lang, fmt_count};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, mpsc};

/// A paste that changes more rows than this asks first.
pub const ASK_ROWS: usize = 1_000;
/// The longest a read of the system clipboard may take.
pub const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(2);

/// Text pasted into the grid of tab `tab`'s result `result`, waiting for the result's table
/// (its structure is read first); `format`: the format to read it as (`None`: as it looks);
/// `again`: for `space d V`, the paste it reads again, whose step was undone for it.
#[derive(Debug)]
pub struct PasteIn {
    tab: TabId,
    result: u64,
    text: String,
    format: Option<Format>,
    again: Option<LastPaste>,
    /// `g p`: the cells copied in a grid, pasted in place of `text`.
    copy: Option<Arc<GridCopy>>,
    /// `p`: the copy goes in as new rows after the last one.
    rows: bool,
    /// A first line naming the columns is pasted as values, not skipped (`space d V`).
    keep_header: bool,
}

/// The cells `y` or `Y` last copied in a grid, which `g p` pastes: a row of values each, as
/// the grid showed them (a pending value, NULL as NULL), their columns as read, and for each
/// column the fixed offset it was read in when its values are shown in the session's time zone
/// (a MySQL `TIMESTAMP`): pasted only into a result read in the same one.
#[derive(Debug)]
pub struct GridCopy {
    /// The profile and the database (`None`: the profile's own) the cells were read from;
    /// `None` without a profile.
    pub source: Option<(ProfileId, Option<String>)>,
    pub columns: Vec<datarig_core::driver::ColumnMeta>,
    pub offsets: Vec<Option<String>>,
    pub rows: Vec<Vec<Value>>,
}

/// The last paste into a grid, read again by `space d V` while it is the newest change: the
/// undo step it added (`None`: none, then while the changes are at `revision`), its text and
/// the format it was read as (`None`: it could not be read), the cursor and the selection it
/// was pasted at, and whether its first line was skipped as the columns' names (`space d V`
/// then reads it again keeping that line).
#[derive(Debug)]
pub struct LastPaste {
    tab: TabId,
    result: u64,
    revision: u64,
    step: Option<u64>,
    text: String,
    format: Option<Format>,
    at: Spot,
    header: bool,
}

impl LastPaste {
    /// It is the newest change of `rs`: its step is the next one `u` undoes, or it added none
    /// and nothing changed since.
    fn newest(&self, rs: &ResultSet) -> bool {
        let b = &rs.edit.batch;
        match self.step {
            Some(id) => b.last_step() == Some(id),
            None => b.revision() == self.revision,
        }
    }
}

/// The grid's cursor and selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spot {
    cursor: (usize, usize),
    anchor: Option<(usize, usize)>,
    shape: Shape,
}

/// A paste of many rows waiting for `y`: its tab, result and binding, the revision of the
/// changes it was made for, what it does, and (`space d V`) the paste it reads again.
#[derive(Debug)]
pub struct PasteAsk {
    tab: TabId,
    result: u64,
    bound: Bound,
    revision: u64,
    plan: PastePlan,
    last: Option<LastPaste>,
    again: Option<LastPaste>,
}

/// Text with commas and no tab waiting for `c` (CSV) or `1` (one column, or one value): its
/// tab, result and binding, the revision of the changes and the grid spot it was pasted at;
/// the text; how many columns it is as CSV and the format it is as one column, those that read
/// it.
#[derive(Debug)]
pub struct CsvAsk {
    tab: TabId,
    result: u64,
    bound: Bound,
    revision: u64,
    at: Spot,
    text: String,
    pub(super) csv: Option<usize>,
    pub(super) plain: Option<Format>,
}

/// What a paste does, every check passed: its cells and new rows, the rows read it changes (as
/// read, from row `first`), the grid row and column it starts at, how big it is, the format it
/// was read as (`None`: cells copied in a grid), whether it went to the selection's corner, how
/// many rows it touches and adds, the column each pasted column lands in (`(pasted, grid)`), and
/// whether it is a copy added as new rows (`p`).
#[derive(Debug)]
struct PastePlan {
    ops: PasteOps,
    first: usize,
    read: Vec<Vec<datarig_core::driver::Cell>>,
    top: (usize, usize),
    size: (usize, usize),
    format: Option<Format>,
    corner: bool,
    rows: usize,
    new: usize,
    map: Vec<(usize, usize)>,
    as_rows: bool,
    header: bool,
    /// Copied columns the table computes or numbers itself, left to it (`p`).
    generated: usize,
}

/// Why a paste is refused, in words, and in fewer words for a narrow status bar.
type Refusal = (String, Option<String>);

/// The cells of a result's changes whose value their column does not take, for the changes at
/// `revision`: why, in words, by cell; and the same cells as the planner takes them. With them,
/// what else of new rows the planner refuses: the cells left to a default their column lacks
/// ([`edit::no_default`]), and the new rows (by id) of a table with such a column the result
/// does not show ([`edit::not_shown`]). The grid marks all of them.
#[derive(Debug, Default)]
pub struct Invalids {
    pub revision: u64,
    pub cells: BTreeMap<(RowAt, usize), String>,
    pub set: BTreeSet<(Origin, usize)>,
    pub no_default: BTreeSet<(RowAt, usize)>,
    pub not_shown: BTreeSet<u64>,
}

/// What the invalid cells were found for: the changes, the words, the NULL token, and what says
/// whether a value is stored as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InvalidKey {
    revision: u64,
    lang: Lang,
    null: datarig_core::config::PasteNull,
    dialect: Dialect,
    zone: Option<String>,
    read_in: datarig_core::driver::ReadIn,
}

/// A result's invalid cells as last found.
#[derive(Debug, Default)]
pub struct InvalidCache {
    made: RefCell<Option<(InvalidKey, EditTarget, Arc<Invalids>)>>,
}

/// Why a read of the system clipboard gave no text.
#[derive(Debug)]
pub enum ClipFail {
    /// Its reason (no access, empty, not text).
    Error(String),
    /// It did not answer within [`CLIPBOARD_TIMEOUT`].
    Slow,
}

/// The answer of read `id` of the system clipboard, with the clipboard back (none when it is
/// left behind, not answering).
pub struct ClipAnswer {
    id: u64,
    got: Result<String, ClipFail>,
    clip: Option<Box<dyn clipboard::SystemClipboard>>,
}

impl std::fmt::Debug for ClipAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipAnswer").field("id", &self.id).field("got", &self.got).finish_non_exhaustive()
    }
}

/// The reads of the system clipboard: the next one's number, the one waited for (its tab,
/// result, the revision of its changes and grid spot), and (headless) the answers not handed
/// to the app yet.
#[derive(Default)]
pub struct ClipReads {
    next: u64,
    wait: Option<(u64, TabId, u64, u64, Spot)>,
    pending: usize,
    local: Option<(mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>)>,
}

/// The name of format `f`.
fn format_label(f: Format) -> Label {
    match f {
        Format::Tsv => Label::CopyFormatTsv,
        Format::Csv => Label::CopyFormatCsv,
        Format::Column => Label::EditPasteFormatColumn,
        Format::One => Label::EditPasteFormatOne,
    }
}

/// The format a paste read as `f` (`None`: it could not be read as it looks) is read as next.
fn next_format(f: Option<Format>) -> Format {
    f.map_or(Format::One, Format::next)
}

/// `n` bytes as the paste limits say them (`1 MiB`, `1.5 MiB`).
fn mib(n: usize) -> String {
    let m = n as f64 / f64::from(1u32 << 20);
    if n.is_multiple_of(1 << 20) { format!("{} MiB", n >> 20) } else { format!("{m:.1} MiB") }
}

impl App {
    /// Text pasted while the grid has the focus: laid over the shown result's rows (with the
    /// Changes tab, the messages, a plan or a chart shown: said to go to the grid).
    pub(in crate::app) fn grid_paste(&mut self, text: &str) {
        let t = self.tab();
        if matches!(t.results, Results::Rows(_)) && t.exec.view != tabs::ResultView::Rows {
            return self.flash(Notice::new(Label::EditPasteGridOnly, Level::Info));
        }
        let Some(rs) = rows_of(t) else { return };
        self.paste_in = Some(PasteIn {
            tab: t.id,
            result: rs.id,
            text: text.to_string(),
            format: None,
            again: None,
            copy: None,
            rows: false,
            keep_header: false,
        });
        self.edit_action(EditAction::Pasted);
    }

    /// `g p`: the cells last copied in a grid, pasted at the cursor of the shown result as a
    /// paste is; `p` (`rows`): added as new rows after the last one. With none, said which keys
    /// copy.
    pub(super) fn paste_register(&mut self, rows: bool) {
        let Some(copy) = self.grid_copy.clone() else {
            let key = if rows { EditAction::PasteRows } else { EditAction::PasteRegister };
            let (cell, row, paste) = (
                self.key_for(Action::Grid(GridAction::CopyCell), Ctx::Grid),
                self.key_for(Action::Grid(GridAction::CopyRow), Ctx::Grid),
                self.key_for(Action::Edit(key), Ctx::Grid),
            );
            return self.flash(Notice::new(Msg::EditPasteRegisterEmpty { copy: cell, row, paste }, Level::Info));
        };
        let t = self.tab();
        if matches!(t.results, Results::Rows(_)) && t.exec.view != tabs::ResultView::Rows {
            return self.flash(Notice::new(Label::EditPasteGridOnly, Level::Info));
        }
        let Some(rs) = rows_of(t) else { return };
        let (tab, result) = (t.id, rs.id);
        let copy = Some(copy);
        let text = String::new();
        self.paste_in = Some(PasteIn { tab, result, text, format: None, again: None, copy, rows, keep_header: false });
        self.edit_action(EditAction::Pasted);
    }

    /// `space d v`: the system clipboard's text, read on a worker, pasted at the cursor once it
    /// comes. Refused as an edit key is, and with the `osc52` clipboard (then nothing is read).
    pub(super) fn paste_key(&mut self) {
        if self.edit_gate().is_none() {
            return;
        }
        let t = self.tab();
        let Some(rs) = rows_of(t).filter(|_| t.exec.view == tabs::ResultView::Rows) else { return };
        let ssh = clipboard::in_ssh(|k| (self.env)(k));
        if clipboard::plan(self.prefs.clipboard, ssh) == ClipPlan::Osc52 {
            return self.flash(Notice::new(Label::EditPasteNeedsSystem, Level::Info));
        }
        let spot = Spot { cursor: (t.grid.row, t.grid.col), anchor: t.grid.anchor, shape: t.grid.shape };
        let (tab, result, revision) = (t.id, rs.id, rs.edit.batch.revision());
        let clip = match self.clipboard.take() {
            Some(c) => c,
            None => match (self.clipboard_opener)() {
                Ok(c) => c,
                Err(e) => return self.clip_failed(e),
            },
        };
        let r = &mut self.clip_reads;
        r.next += 1;
        let id = r.next;
        r.wait = Some((id, tab, result, revision, spot));
        let out = match &self.tx {
            Some(tx) => Ok(tx.clone()),
            None => {
                r.pending += 1;
                Err(r.local.get_or_insert_with(mpsc::channel).0.clone())
            }
        };
        // A waiter that answers within the limit either way, and a reader, which may hang.
        std::thread::spawn(move || {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let mut clip = clip;
                let got = clip.get_text();
                let _ = tx.send((got, clip));
            });
            let answer = match rx.recv_timeout(CLIPBOARD_TIMEOUT) {
                Ok((got, clip)) => ClipAnswer { id, got: got.map_err(ClipFail::Error), clip: Some(clip) },
                Err(mpsc::RecvTimeoutError::Timeout) => ClipAnswer { id, got: Err(ClipFail::Slow), clip: None },
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    ClipAnswer { id, got: Err(ClipFail::Error("the reader stopped".into())), clip: None }
                }
            };
            let ev = AppEvent::Clipboard(answer);
            let _ = match out {
                Ok(tx) => tx.send(ev).map_err(|_| ()),
                Err(tx) => tx.send(ev).map_err(|_| ()),
            };
        });
        self.flash(Notice::new(Label::EditPasteReading, Level::Info));
    }

    /// Headless (without the event loop's channel): hand the answers of the clipboard reads to
    /// the app, waiting up to `wait` while one is still out. How many came.
    pub fn pump_clipboard_reads(&mut self, wait: Duration) -> usize {
        let deadline = std::time::Instant::now() + wait;
        let mut n = 0;
        while self.clip_reads.pending > 0 {
            let Some((_, rx)) = &self.clip_reads.local else { break };
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(ev) = rx.recv_timeout(left) else { break };
            self.clip_reads.pending -= 1;
            n += 1;
            self.on_app_event(ev);
        }
        n
    }

    /// A read of the system clipboard answered: its text is pasted when it is the read waited
    /// for and nothing moved meanwhile (the same tab, result, changes, cursor and selection,
    /// the focus on the grid; no dialog, question, waiting Apply, cell editor or prompt open);
    /// else said.
    pub(in crate::app) fn on_clipboard_answer(&mut self, a: ClipAnswer) {
        if self.clipboard.is_none() {
            self.clipboard = a.clip;
        }
        let Some((_, tab, result, revision, spot)) = self.clip_reads.wait.take_if(|w| w.0 == a.id) else {
            return;
        };
        let text = match a.got {
            Ok(text) => text,
            Err(ClipFail::Slow) => return self.flash(Notice::new(Label::EditPasteReadSlow, Level::Warning)),
            Err(ClipFail::Error(e)) => return self.clip_failed(e),
        };
        if self.transient.as_ref().is_some_and(|(n, _)| n.msg == Label::EditPasteReading.into()) {
            self.transient = None;
        }
        let still = !self.tabs.is_empty()
            && !self.modal_open()
            && self.focus == Focus::Results
            && self.ask.is_none()
            && self.apply_wait.is_none()
            && !self.cell_edit_open()
            && !self.grid_prompt_open()
            && {
                let t = self.tab();
                t.id == tab
                    && t.exec.view == tabs::ResultView::Rows
                    && rows_of(t).is_some_and(|rs| rs.id == result && rs.edit.batch.revision() == revision)
                    && Spot { cursor: (t.grid.row, t.grid.col), anchor: t.grid.anchor, shape: t.grid.shape } == spot
            };
        if !still {
            return self.flash(Notice::new(Label::EditPasteReadMoved, Level::Info));
        }
        self.grid_paste(&text);
    }

    /// The system clipboard could not be read, for `e`: logged and said.
    fn clip_failed(&mut self, e: String) {
        ErrorLog::new(self.paths.errors_log()).record("paste.system", &Fault::other(e.clone()));
        self.flash(Notice::new(Msg::EditPasteReadFailed { error: e }, Level::Warning));
    }

    /// `space d V`: the last paste read again as the next format, at the cursor and selection it
    /// was pasted at, in its place (its step undone first), while it is the newest change of the
    /// shown result. Refused, or its question not answered `y`, the paste is put back as it was.
    pub(super) fn paste_again(&mut self) {
        let t = self.tab();
        let newest = rows_of(t)
            .and_then(|rs| self.last_paste.as_ref().filter(|l| l.tab == t.id && l.result == rs.id && l.newest(rs)));
        if newest.is_none() {
            return self.flash(Notice::new(Label::EditPasteRereadNone, Level::Info));
        }
        if self.edit_gate().is_none() {
            return;
        }
        let Some(last) = self.last_paste.take() else { return };
        let t = self.tabs.active_mut();
        let Some(rs) = shown_mut(&mut t.results, t.exec.view) else { return };
        if last.step.is_some() {
            rs.edit.batch.undo();
        }
        let g = &mut t.grid;
        (g.row, g.col) = last.at.cursor;
        g.anchor = last.at.anchor;
        g.shape = last.at.shape;
        self.edit_grid_sync();
        // A first line skipped as the columns' names: the same format again, keeping it.
        let keep_header = last.header;
        let format = Some(last.format.filter(|_| keep_header).unwrap_or_else(|| next_format(last.format)));
        let (tab, result, text) = (last.tab, last.result, last.text.clone());
        self.paste_in =
            Some(PasteIn { tab, result, text, format, again: Some(last), copy: None, rows: false, keep_header });
        self.edit_action(EditAction::Pasted);
        // Not read now (refused before it, or waiting for the table): the paste as it was.
        if let Some(PasteIn { again: Some(again), .. }) = self.paste_in.take_if(|p| p.again.is_some()) {
            let format = again.format;
            self.paste_put_back(again, format);
        }
    }

    /// Paste `again`, whose step `space d V` undid, put back as it was (its step redone while it
    /// is the next one to redo), the last paste still, read as `format` (the next `space d V`
    /// reads it as the format after that one).
    fn paste_put_back(&mut self, again: LastPaste, format: Option<Format>) {
        if let Some(id) = again.step
            && let Some(t) = self.tabs.get_mut(again.tab)
            && let Results::Rows(rs) = &mut t.results
            && rs.id == again.result
            && rs.edit.batch.next_redo() == Some(id)
        {
            rs.edit.batch.redo();
        }
        self.last_paste = Some(LastPaste { format, ..again });
        if !self.tabs.is_empty() {
            self.edit_grid_sync();
        }
    }

    /// A paste read as `last` was not done: `space d V` reads it again; or, when it read paste
    /// `again` again, that one is put back, the next `space d V` reading it as the format after
    /// this one's.
    fn paste_not_done(&mut self, last: Option<LastPaste>, again: Option<LastPaste>) {
        match (again, last) {
            (Some(a), last) => self.paste_put_back(a, last.and_then(|l| l.format)),
            (None, Some(last)) => self.last_paste = Some(last),
            (None, None) => {}
        }
    }

    /// The question about a paste of many rows not answered `y` (another key, or dropped): the
    /// paste it read again, if any, put back.
    pub(super) fn paste_ask_dropped(&mut self, a: PasteAsk) {
        let PasteAsk { last, again, .. } = a;
        if let Some(again) = again {
            self.paste_put_back(again, last.and_then(|l| l.format));
        }
    }

    /// The question about text with commas not answered (another key, or dropped): `space d V`
    /// reads the text it asked about.
    pub(super) fn csv_ask_dropped(&mut self, a: CsvAsk) {
        let CsvAsk { tab, result, revision, at, text, .. } = a;
        let last = LastPaste { tab, result, revision, step: None, text, format: None, at, header: false };
        self.paste_not_done(Some(last), None);
    }

    /// How many rows `plan` changes on the active tab's shown result: rows with a value set to
    /// another one, and the rows it adds.
    fn paste_changed_rows(&self, plan: &PastePlan) -> usize {
        let Some(rs) = rows_of(self.tab()) else { return 0 };
        let mut b = rs.edit.batch.clone();
        b.paste(plan.ops.clone(), |r| plan.read.get(r - plan.first).cloned().unwrap_or_default()).map_or(0, |d| d.rows)
    }

    /// The paste waiting in [`App::paste_in`], on a result resolved to `target`: read, laid over
    /// the grid and checked; done, asked about when it changes many rows, or refused, said.
    pub(super) fn edit_paste(&mut self, target: &EditTarget) {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let at = Spot { cursor: (t.grid.row, t.grid.col), anchor: t.grid.anchor, shape: t.grid.shape };
        let (tab, result, revision) = (t.id, rs.id, rs.edit.batch.revision());
        let Some(p) = self.paste_in.take_if(|p| p.tab == tab && p.result == result) else { return };
        let PasteIn { text, format, again, copy, rows, keep_header, .. } = p;
        if let Some(copy) = copy {
            if rows {
                return match self.copy_rows_plan(target, &copy) {
                    Ok(plan) => self.paste_planned(plan, Some(&copy), None, None),
                    Err(reason) => self.paste_refused(reason),
                };
            }
            return self.paste_rows(target, &copy.rows, None, Some(&copy), None, None, false);
        }
        let opts = PasteOpts { null: self.prefs.paste_null.token().to_string() };
        let read = match format {
            None => paste::parse(&text, &opts),
            Some(f) => paste::parse_as(&text, f, &opts),
        };
        let mut last = LastPaste { tab, result, revision, step: None, text, format, at, header: false };
        let pasted = match read {
            Ok(x) => x,
            // Commas and no tab: which it is is asked.
            Err(PasteError::Ambiguous { csv, plain }) => {
                let gate = if self.ask.is_some() { None } else { self.edit_gate() };
                let Some((_, _, bound)) = gate else {
                    self.paste_not_done(Some(last), again);
                    return self.flash(Notice::new(Label::EditPasteCancelled, Level::Info));
                };
                let forms = self.csv_ask_forms(&csv, &plain);
                let (csv, plain) = (csv.ok(), plain.ok());
                let text = last.text;
                let ask = CsvAsk { tab, result, bound, revision, at, text, csv, plain };
                self.ask = Some(StatusAsk { forms, tone: Level::Warning, action: AskAction::Csv(Box::new(ask)) });
                return;
            }
            Err(e) => {
                let reason = self.paste_error_text(&e, next_format(last.format));
                self.paste_not_done(Some(last), again);
                return self.paste_refused(reason);
            }
        };
        last.format = Some(pasted.format);
        // A first line that names the columns it lands in, in order, is a header: skipped.
        let header = !keep_header && pasted.rows.len() > 1 && self.names_columns(&pasted.rows[0]);
        last.header = header;
        let rows = if header { &pasted.rows[1..] } else { &pasted.rows[..] };
        self.paste_rows(target, rows, Some(pasted.format), None, Some(last), again, header);
    }

    /// Values `line` are the names of the active tab's shown result's columns from where a
    /// paste starts (the selection's top-left corner, else the cursor), one each, in order.
    fn names_columns(&self, line: &[Value]) -> bool {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return false };
        let c0 = t.grid.selection(rs.rows.len(), rs.columns.len()).map_or(t.grid.col, |(_, c)| c.start);
        line.iter().enumerate().all(|(k, v)| match v {
            Value::Text(s) => rs.columns.get(c0 + k).is_some_and(|c| c.meta.name == *s),
            _ => false,
        })
    }

    /// Paste `rows` (read as `format`; `None`: cells copied in a grid) on the active tab's shown
    /// result, resolved to `target`: laid over the grid and checked; done, asked about when it
    /// changes many rows, or refused, said. `copy`: the cells copied in a grid they are;
    /// `last`: the paste as `space d V` reads it again; `header`: its first line was skipped.
    #[allow(clippy::too_many_arguments)]
    fn paste_rows(
        &mut self,
        target: &EditTarget,
        rows: &[Vec<Value>],
        format: Option<Format>,
        copy: Option<&GridCopy>,
        last: Option<LastPaste>,
        again: Option<LastPaste>,
        header: bool,
    ) {
        let plan = match self.paste_plan(target, rows, format) {
            Ok(p) => PastePlan { header, ..p },
            Err(reason) => {
                self.paste_not_done(last, again);
                return self.paste_refused(reason);
            }
        };
        self.paste_planned(plan, copy, last, again);
    }

    /// Paste `plan` on the active tab's shown result: refused when cells copied in a grid
    /// (`copy`) would be other instants there, asked about when it changes many rows, else done.
    fn paste_planned(
        &mut self,
        plan: PastePlan,
        copy: Option<&GridCopy>,
        last: Option<LastPaste>,
        again: Option<LastPaste>,
    ) {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let (tab, result, revision) = (t.id, rs.id, rs.edit.batch.revision());
        if let Some(reason) = copy.and_then(|c| self.copy_zone_refused(c, &plan)) {
            self.paste_not_done(last, again);
            return self.paste_refused((reason, None));
        }
        let rows = if plan.rows > ASK_ROWS { self.paste_changed_rows(&plan) } else { 0 };
        if rows > ASK_ROWS {
            // A question already open stays: this paste is not done.
            let gate = if self.ask.is_some() { None } else { self.edit_gate() };
            let Some((_, _, bound)) = gate else {
                self.paste_not_done(last, again);
                return self.flash(Notice::new(Label::EditPasteCancelled, Level::Info));
            };
            let forms = self.paste_ask_forms(rows, &plan);
            let action = AskAction::Paste(Box::new(PasteAsk { tab, result, bound, revision, plan, last, again }));
            self.ask = Some(StatusAsk { forms, tone: Level::Warning, action });
            return;
        }
        self.paste_do(plan, last, again);
    }

    /// The status line's question about text with commas, longest first: CSV of `csv` columns or
    /// `plain` (one column, one value), those that read it; the other one's refusal said.
    fn csv_ask_forms(
        &self,
        csv: &Result<usize, Box<PasteError>>,
        plain: &Result<Format, Box<PasteError>>,
    ) -> Vec<String> {
        let i18n = &self.i18n;
        let columns = |n: usize| i18n.msg(&Msg::EditPasteColumns { count: n as u64 }).to_string();
        let plain_name = |f: Format| i18n.label(format_label(f)).to_string();
        let msgs = match (csv, plain) {
            (Ok(n), Ok(f)) => {
                let (columns, plain) = (columns(*n), plain_name(*f));
                vec![
                    Msg::EditPasteAskCsv { columns: columns.clone(), plain: plain.clone() },
                    Msg::EditPasteAskCsvMid { columns: columns.clone(), plain: plain.clone() },
                    Msg::EditPasteAskCsvShort { columns, plain },
                ]
            }
            (Ok(n), Err(_)) => {
                let columns = columns(*n);
                vec![Msg::EditPasteAskCsvOnly { columns: columns.clone() }, Msg::EditPasteAskCsvOnlyShort { columns }]
            }
            (Err(e), Ok(f)) => {
                let (reason, _) = self.paste_error_msg(e);
                let (reason, plain) = (i18n.msg(&reason).to_string(), plain_name(*f));
                vec![
                    Msg::EditPasteAskPlainOnly { reason, plain: plain.clone() },
                    Msg::EditPasteAskPlainOnlyShort { plain },
                ]
            }
            // The core says the CSV refusal when neither reads it.
            (Err(_), Err(_)) => vec![Label::EditPreviewInternal.into()],
        };
        msgs.iter().map(|m| i18n.msg(m).to_string()).collect()
    }

    /// `c` (`format` CSV) or `1` (one column, one value) to the question about text with
    /// commas: the text read so, through the paste's own way (its refusals, its question about
    /// many rows), when the changes, the result, its binding and the grid are as they were when
    /// it asked; else said.
    pub(super) fn csv_ask_answer(&mut self, a: CsvAsk, format: Format) {
        if !self.csv_current(&a) {
            self.csv_ask_dropped(a);
            return self.ask_ended(Label::EditPasteMoved);
        }
        let (tab, result, text) = (a.tab, a.result, a.text);
        self.paste_in = Some(PasteIn {
            tab,
            result,
            text,
            format: Some(format),
            again: None,
            copy: None,
            rows: false,
            keep_header: false,
        });
        self.edit_action(EditAction::Pasted);
    }

    /// The question about text with commas is still about the active tab's shown result as it
    /// was asked: the same changes, binding, cursor and selection, no dialog open.
    pub(super) fn csv_current(&self, a: &CsvAsk) -> bool {
        if self.tabs.is_empty() || self.modal_open() {
            return false;
        }
        let t = self.tab();
        t.id == a.tab
            && t.exec.view == tabs::ResultView::Rows
            && rows_of(t).is_some_and(|rs| rs.id == a.result && rs.edit.batch.revision() == a.revision)
            && Spot { cursor: (t.grid.row, t.grid.col), anchor: t.grid.anchor, shape: t.grid.shape } == a.at
            && self.edit_check().is_ok_and(|(_, _, b)| b == a.bound)
    }

    /// The status line's question about a paste that changes `rows` rows, longest first.
    fn paste_ask_forms(&self, rows: usize, plan: &PastePlan) -> Vec<String> {
        let rows = fmt_count(rows as u64);
        let long = match plan.new {
            0 => Msg::EditPasteAskPlain { rows: rows.clone() },
            n => Msg::EditPasteAsk { rows: rows.clone(), new: fmt_count(n as u64) },
        };
        vec![self.i18n.msg(&long).to_string(), self.i18n.msg(&Msg::EditPasteAskShort { rows }).to_string()]
    }

    /// `y` to a paste of many rows: done when the changes, the result and its binding are as
    /// they were when it asked; else said.
    pub(super) fn paste_ask_yes(&mut self, a: PasteAsk) {
        if !self.paste_current(&a) {
            self.paste_ask_dropped(a);
            return self.ask_ended(Label::EditPasteMoved);
        }
        self.paste_do(a.plan, a.last, a.again);
    }

    /// The question about a paste of many rows is still about the active tab's shown result as
    /// it was asked: the same changes and binding, no dialog open.
    pub(super) fn paste_current(&self, a: &PasteAsk) -> bool {
        if self.tabs.is_empty() || self.modal_open() {
            return false;
        }
        let t = self.tab();
        t.id == a.tab
            && rows_of(t).is_some_and(|rs| rs.id == a.result && rs.edit.batch.revision() == a.revision)
            && self.edit_check().is_ok_and(|(_, _, b)| b == a.bound)
    }

    /// Do paste `plan` (read at `last`): one step of the changes; the cursor on its first cell,
    /// the selection dropped; what it did said.
    fn paste_do(&mut self, plan: PastePlan, last: Option<LastPaste>, again: Option<LastPaste>) {
        let PastePlan { ops, first, read, top, size, format, corner, map, as_rows, header, generated, .. } = plan;
        let pasted: Vec<(RowAt, usize)> = (ops.set.iter().map(|((r, c), _)| (RowAt::Read(*r), *c)))
            .chain(ops.set_new.iter().map(|((id, c), _)| (RowAt::New(*id), *c)))
            .collect();
        let cols: Vec<usize> = map.iter().map(|&(_, c)| c).collect();
        let t = self.tabs.active_mut();
        let Some(rs) = shown_mut(&mut t.results, t.exec.view) else { return };
        let before = rs.edit.batch.revision();
        let done = rs.edit.batch.paste(ops, |r| read.get(r - first).cloned().unwrap_or_default());
        let done = match done {
            Ok(d) => d,
            Err(why) => {
                let reason = self.paste_refused_text(&why);
                self.paste_not_done(last, again);
                return self.paste_refused((reason, None));
            }
        };
        let revision = rs.edit.batch.revision();
        let step = if revision != before { rs.edit.batch.last_step() } else { None };
        let mut pasted = pasted;
        pasted.extend(done.added.iter().flat_map(|id| cols.iter().map(|c| (RowAt::New(*id), *c))));
        // Cells copied in a grid are no text `space d V` reads again.
        self.last_paste = last.map(|l| LastPaste { revision, step, ..l });
        self.edit_grid_sync();
        let t = self.tabs.active_mut();
        (t.grid.row, t.grid.col) = top;
        t.grid.anchor = None;
        t.grid.detached = false;
        self.selecting_done();
        self.edit_grid_sync();
        // New rows added after the last one: their page is shown.
        if let Some(&id) = done.added.first().filter(|_| as_rows) {
            self.edit_cursor_to(RowAt::New(id), top.1);
        }
        // What it did, the first invalid cell named; shorter forms drop the counts of cells and
        // new rows, then name no cell, then the size in words, then the format.
        let i18n = &self.i18n;
        let t = self.tab();
        let invalid = rows_of(t).map(|rs| self.invalid_cells(t, rs)).unwrap_or_default();
        let bad: Vec<&(RowAt, usize)> = pasted.iter().filter(|k| invalid.cells.contains_key(k)).collect();
        let long_size = format!(
            "{} \u{D7} {}",
            i18n.msg(&Msg::EditPasteRows { count: size.0 as u64 }),
            i18n.msg(&Msg::EditPasteColumns { count: size.1 as u64 })
        );
        let short_size = format!("{}\u{D7}{}", size.0, size.1);
        let mut more = Vec::new();
        if corner {
            more.push(i18n.msg(&Msg::EditPasteCorner { size: short_size.clone() }).to_string());
        }
        if !as_rows {
            more.push(i18n.msg(&Msg::EditPasteCells { count: done.changed as u64 }).to_string());
        }
        if !done.added.is_empty() {
            more.push(i18n.msg(&Msg::EditPasteNewRows { count: done.added.len() as u64 }).to_string());
        }
        if generated > 0 {
            more.push(i18n.msg(&Msg::EditPasteRowsGenerated { count: generated as u64 }).to_string());
        }
        let (named, count) = match bad.iter().min() {
            Some(&&(at, c)) => {
                let rs = rows_of(t);
                let row = rs.map_or_else(|| "?".into(), |rs| self.row_name(rs, at));
                let column = rs.map(|rs| self.preview_column(rs, c)).unwrap_or_default();
                let n = bad.len() as u64;
                let named = i18n.msg(&Msg::EditPasteInvalid { count: n, row, column }).to_string();
                (Some(named), Some(i18n.msg(&Msg::EditPasteInvalidCount { count: n }).to_string()))
            }
            None => (None, None),
        };
        let level = if count.is_some() { Level::Warning } else { Level::Info };
        let undo = self.keymap.hint_keys(Action::Edit(EditAction::Undo), Ctx::Grid, self.enhanced_keys);
        let undo = undo.map(|k| i18n.msg(&Msg::EditPasteUndo { key: crate::keymap::keys::label(&k) }).to_string());
        // A header line skipped: said in every form, with the key that keeps it.
        let header = header.then(|| {
            let key = self.key_for(Action::Edit(EditAction::PasteAs), Ctx::Grid);
            i18n.msg(&Msg::EditPasteHeaderSkipped { key }).to_string()
        });
        let details = |size: &str, invalid: &Option<String>, more: &[String]| {
            let parts =
                header.clone().into_iter().chain([size.to_string()]).chain(invalid.clone()).chain(more.iter().cloned());
            parts.chain(undo.clone()).collect::<Vec<_>>().join(" \u{B7} ")
        };
        let format = format.map(|f| i18n.label(format_label(f)).to_string());
        let done = |details: String| match &format {
            Some(format) => Msg::EditPasteDone { format: format.clone(), details },
            None if as_rows => Msg::EditPasteRowsDone { details },
            None => Msg::EditPasteRegisterDone { details },
        };
        let mut shorter = vec![
            done(details(&long_size, &named, &[])),
            done(details(&long_size, &count, &[])),
            done(details(&short_size, &count, &[])),
            Msg::EditPasteDoneShort { details: details(&short_size, &count, &[]) },
        ];
        let msg = done(details(&long_size, &named, &more));
        shorter.dedup();
        shorter.retain(|m| *m != msg);
        self.flash_shorter(Notice::new(msg, level), shorter);
    }

    /// Why cells copied in a grid are not pasted as `plan` lays them: a value shown in the time
    /// zone it was read in, without an offset (a MySQL `TIMESTAMP`), lands in a column of a time
    /// with a zone: of MySQL, whose values the active tab's shown result shows in another zone
    /// or one not known, where the same text is another instant; of another server, which reads
    /// the text in its session's zone.
    fn copy_zone_refused(&self, copy: &GridCopy, plan: &PastePlan) -> Option<String> {
        let t = self.tab();
        let rs = rows_of(t)?;
        let mysql = matches!(self.tab_dialect(t.id), Dialect::MySql(_));
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        plan.map.iter().find_map(|&(j, c)| {
            let from = copy.offsets.get(j)?.as_ref()?;
            let valued = copy.rows.iter().any(|r| matches!(r.get(j), Some(Value::Text(_))));
            if !valued || rs.columns.get(c)?.meta.kind != ValueKind::TimestampTz {
                return None;
            }
            let column = copy.columns.get(j).map(|m| m.name.clone()).unwrap_or_default();
            if !mysql {
                let msg = Msg::EditPasteRegisterZoneOther { column, from: from.clone() };
                return Some(self.i18n.msg(&msg).to_string());
            }
            let label = |l: Label| self.i18n.label(l).to_string();
            let to = match &rs.read_in {
                datarig_core::driver::ReadIn::Values(v) => match export::fixed_offset(&v.time_zone, zone) {
                    Some(o) => Ok(o),
                    None if v.time_zone.eq_ignore_ascii_case("SYSTEM") => Err(label(Label::EditPasteZoneServer)),
                    None => Err(v.time_zone.clone()),
                },
                datarig_core::driver::ReadIn::Changed => Err(label(Label::EditPasteZoneChanged)),
                datarig_core::driver::ReadIn::Unknown => Err(label(Label::EditPasteZoneUnknown)),
            };
            let to = match to {
                Ok(o) if o == *from => return None,
                Ok(o) | Err(o) => o,
            };
            Some(self.i18n.msg(&Msg::EditPasteRegisterZone { column, from: from.clone(), to }).to_string())
        })
    }

    /// A paste refused for `reason` (and its shorter form), said.
    fn paste_refused(&mut self, (reason, short): Refusal) {
        let shorter = short.map(|reason| vec![Msg::EditPasteRefused { reason }]).unwrap_or_default();
        self.flash_shorter(Notice::new(Msg::EditPasteRefused { reason }, Level::Info), shorter);
    }

    /// The keys of `space d V` saying the text is read again as `next` (with ` · ` before it),
    /// and its shorter form; nothing when no key does it.
    fn reread_hint(&self, next: Format) -> (String, String) {
        let keys = self.keymap.hint_keys(Action::Edit(EditAction::PasteAs), Ctx::Grid, self.enhanced_keys);
        let Some(k) = keys else { return (String::new(), String::new()) };
        let (key, format) = (crate::keymap::keys::label(&k), self.i18n.label(format_label(next)).to_string());
        let hint = self.i18n.msg(&Msg::EditPasteRereadHint { key: key.clone(), format: format.clone() });
        let short = self.i18n.msg(&Msg::EditPasteRereadHintShort { key, format });
        (format!(" \u{B7} {hint}"), format!(" \u{B7} {short}"))
    }

    /// Why text could not be read as a paste (`next`: the format `space d V` reads it as), in
    /// words.
    fn paste_error_text(&self, e: &PasteError, next: Format) -> Refusal {
        let (msg, short) = self.paste_error_msg(e);
        let (hint, short_hint) = match e {
            PasteError::Empty | PasteError::TooLarge { .. } | PasteError::TooManyRows { .. } => Default::default(),
            _ => self.reread_hint(next),
        };
        let reason = self.i18n.msg(&msg).to_string();
        let short = self.i18n.msg(short.as_ref().unwrap_or(&msg)).to_string();
        (format!("{reason}{hint}"), Some(format!("{short}{short_hint}")))
    }

    /// Why text could not be read as a paste, and in fewer words when there are.
    fn paste_error_msg(&self, e: &PasteError) -> (Msg, Option<Msg>) {
        match e {
            PasteError::Empty => (Label::EditPasteEmptyText.into(), None),
            PasteError::TooLarge { bytes, max } => (Msg::EditPasteTooLarge { size: mib(*bytes), max: mib(*max) }, None),
            // It stopped reading one row past the most it takes: the size is not known.
            PasteError::TooManyRows { max, .. } => (Msg::EditPasteTooManyRows { max: fmt_count(*max as u64) }, None),
            PasteError::LoneCr { row } => (Msg::EditPasteLoneCr { row: row.to_string() }, None),
            PasteError::Quote { row } => {
                (Msg::EditPasteQuote { row: row.to_string() }, Some(Msg::EditPasteQuoteShort { row: row.to_string() }))
            }
            PasteError::Ragged { row, values, first_row, first_values } => {
                let (row, first, first_values) = (row.to_string(), first_row.to_string(), first_values.to_string());
                let short = Msg::EditPasteRaggedShort {
                    row: row.clone(),
                    first: first.clone(),
                    values: values.to_string(),
                    first_values: first_values.clone(),
                };
                (Msg::EditPasteRagged { row, count: *values as u64, first, first_values }, Some(short))
            }
            // Asked about before it is read (never read again so: a reread names its format).
            PasteError::Ambiguous { .. } => (Label::EditPreviewInternal.into(), None),
        }
    }

    /// Why the batch refused a paste, in words (the checks before it make these rare).
    fn paste_refused_text(&self, why: &PasteRefused) -> String {
        let msg = match why {
            PasteRefused::Deleted { row } => Msg::EditWhyRowDeleted { row: (row + 1).to_string() },
            _ => Label::EditPreviewInternal.into(),
        };
        self.i18n.msg(&msg).to_string()
    }

    /// What `pasted` (read as `format`) does laid over the active tab's shown result (resolved
    /// to `target`) from the cursor or the selection's top-left; else why it is refused, in words.
    fn paste_plan(
        &self,
        target: &EditTarget,
        pasted: &[Vec<Value>],
        format: Option<Format>,
    ) -> Result<PastePlan, Refusal> {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return Err((String::new(), None)) };
        let i18n = &self.i18n;
        let text = |m: Msg| i18n.msg(&m).to_string();
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let shown = total + rs.edit.new_shown();
        let sel = t.grid.selection(total, ncols).map(|(r, c)| (r.start..r.end.min(shown), c));
        let (h, w) = (pasted.len(), pasted.first().map_or(0, Vec::len));
        let many = sel.as_ref().is_some_and(|(r, c)| r.len() * c.len() > 1);
        let one = h == 1 && w == 1;
        let (r0, c0) = sel.as_ref().map_or((t.grid.row, t.grid.col), |(r, c)| (r.start, c.start));
        // One value fills the selection; a block goes at its corner.
        let (rows, cols) = match &sel {
            Some((r, c)) if one && many => (r.clone(), c.clone()),
            _ => (r0..r0 + h, c0..c0 + w),
        };
        let value =
            |r: usize, c: usize| -> Value { if one { pasted[0][0].clone() } else { pasted[r - r0][c - c0].clone() } };
        if cols.end > ncols {
            let (width, room) = (w.to_string(), ncols.saturating_sub(c0).to_string());
            let short = text(Msg::EditPasteTooWideShort { width: width.clone(), room: room.clone() });
            return Err((text(Msg::EditPasteTooWide { width, room }), Some(short)));
        }
        let new = rows.end.saturating_sub(shown);
        // Where each row is: a row read, a new row, or one an Apply added (no row to edit).
        let mut at = Vec::with_capacity(rows.len().saturating_sub(new));
        for r in rows.start..rows.end.min(shown) {
            match row_at(rs, r) {
                Some(a) => at.push((r, a)),
                None => return Err((i18n.label(Label::EditWhyApplied).to_string(), None)),
            }
        }
        // Past the last row read while the server has more: rows the user may mean are not here
        // (a fill adds no row). The next page adds them only while the portal is open; else it
        // shows other rows in their place, or there is none.
        if rs.more && rows.start < total && rows.end > total && !(one && many) {
            if matches!(t.exec.paging, Paging::Open { .. }) {
                let key = self.next_page_key(self.grid_ctx());
                let short = text(Msg::EditPasteMoreRowsShort { key: key.clone() });
                return Err((text(Msg::EditPasteMoreRows { key }), Some(short)));
            }
            let short = i18n.label(Label::EditPasteMoreRowsPagedShort).to_string();
            return Err((i18n.label(Label::EditPasteMoreRowsPaged).to_string(), Some(short)));
        }
        let reads = at.iter().any(|(_, a)| matches!(a, RowAt::Read(_)));
        let inserts = new > 0 || at.iter().any(|(_, a)| matches!(a, RowAt::New(_)));
        for c in cols.clone() {
            for (on, w) in [(reads, Write::Update), (inserts, Write::Insert)] {
                match target.allows(c, w) {
                    Err(r) if on => return Err((text(column_reason(r, rs.columns[c].meta.name.clone())), None)),
                    _ => {}
                }
            }
        }
        if let Some((r, _)) = at.iter().find(|(_, a)| matches!(a, RowAt::Read(r) if rs.edit.batch.deleted(*r))) {
            return Err((text(Msg::EditWhyRowDeleted { row: (r + 1).to_string() }), None));
        }
        if new > 0 {
            if self.insert_column(target, ncols).is_err() {
                let reason = i18n.label(Label::EditWhyNoInsertColumn).to_string();
                let short = i18n.label(Label::EditPasteNoInsertShort).to_string();
                return Err((text(Msg::EditPasteNoNewRows { reason }), Some(short)));
            }
            if let Some(column) = target.unshown_required.first() {
                let reason = text(Msg::EditPasteUnshown { column: column.clone() });
                let short = text(Msg::EditPasteUnshownShort { column: column.clone() });
                return Err((text(Msg::EditPasteNoNewRows { reason }), Some(short)));
            }
        }
        // The rows read, as read: each found again by values shown whole.
        let read_range = rows.start.min(total)..rows.end.min(total);
        let mut read = Vec::new();
        if !read_range.is_empty() {
            let Some(got) = read_rows(rs, read_range.clone()) else {
                return Err((text(Msg::EditWhyNotRead { row: (read_range.start + 1).to_string() }), None));
            };
            if let Some(k) = got.iter().position(|r| r.len() < ncols) {
                return Err((text(Msg::EditWhyNotRead { row: (read_range.start + k + 1).to_string() }), None));
            }
            let d = self.tab_dialect(t.id);
            let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
            if let Some((r, why)) = not_whole(d, rs, target, &got, read_range.start, zone) {
                let reason = self.not_whole_reason(&why.why);
                return Err((text(Msg::EditWhyNotWhole { row: (r + 1).to_string(), reason }), None));
            }
            read = got;
        }
        let mut ops = PasteOps { width: ncols, ..PasteOps::default() };
        for &(r, a) in &at {
            for c in cols.clone() {
                match a {
                    RowAt::Read(i) => ops.set.push(((i, c), value(r, c))),
                    RowAt::New(id) => ops.set_new.push(((id, c), value(r, c))),
                }
            }
        }
        for r in shown.max(rows.start)..rows.end {
            let mut cells = vec![Value::Default; ncols];
            for c in cols.clone() {
                cells[c] = value(r, c);
            }
            ops.add.push(cells);
        }
        // One value fills every selected cell.
        let map = cols.clone().map(|c| (if one { 0 } else { c - c0 }, c)).collect();
        Ok(PastePlan {
            map,
            as_rows: false,
            header: false,
            generated: 0,
            ops,
            first: read_range.start,
            read,
            top: (rows.start, cols.start),
            size: (rows.len(), cols.len()),
            format,
            corner: !one && many,
            rows: rows.len(),
            new,
        })
    }

    /// `p`: cells copied in a grid as new rows after the last row of the active tab's shown
    /// result (resolved to `target`). Copied on the same connection and database from the
    /// target table, each value goes to the result column of its own table column, never by
    /// position: the other columns are left to their defaults, those the table computes or
    /// numbers itself to the server, and a copied column not shown, copied twice, computed or
    /// of another table is refused by name. A copy with a column of this result outside the
    /// target table (a join's other table) is refused too. Else (another table or database,
    /// computed columns only) by position from the first column, refused when the copy is
    /// wider. Refused as a paste of new rows is.
    fn copy_rows_plan(&self, target: &EditTarget, copy: &GridCopy) -> Result<PastePlan, Refusal> {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return Err((String::new(), None)) };
        let i18n = &self.i18n;
        let text = |m: Msg| i18n.msg(&m).to_string();
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let shown = total + rs.edit.new_shown();
        let (h, w) = (copy.rows.len(), copy.columns.len());
        // The table of a column, as its origin names it.
        let table_of = |o: &ColumnOrigin| match o {
            ColumnOrigin::Pg { table, .. } => (Some(*table), None),
            ColumnOrigin::Named { schema, table, .. } => (None, Some((schema.clone(), table.clone()))),
        };
        let this = rs
            .columns
            .iter()
            .enumerate()
            .find_map(|(c, col)| target.columns.get(c)?.as_ref().and(col.meta.origin.as_ref()))
            .map(table_of);
        // Origins name columns of one connection's database only.
        let here = t.profile.map(|p| (p, self.other_database(t).map(str::to_string)));
        let same_db = copy.source.is_some() && copy.source == here;
        let from_here =
            |m: &datarig_core::driver::ColumnMeta| same_db && this.is_some() && m.origin.as_ref().map(table_of) == this;
        let named = |m: &datarig_core::driver::ColumnMeta| m.name.clone();
        let refuse = |m: Msg, short: Msg| Err((text(m), Some(text(short))));
        // Copied from this table: each value to its own column, never by position.
        let mut generated = 0;
        let map: Vec<Option<usize>> = if copy.columns.iter().any(from_here) {
            let mut map = Vec::with_capacity(w);
            for (j, m) in copy.columns.iter().enumerate() {
                let column = named(m);
                if !from_here(m) {
                    return refuse(
                        Msg::EditPasteRowsComputed { column: column.clone() },
                        Msg::EditPasteRowsComputedShort { column },
                    );
                }
                if copy.columns[..j].iter().any(|o| o.origin == m.origin) {
                    return refuse(
                        Msg::EditPasteRowsTwice { column: column.clone() },
                        Msg::EditPasteRowsTwiceShort { column },
                    );
                }
                let Some(c) = rs.columns.iter().position(|c| c.meta.origin == m.origin) else {
                    return refuse(
                        Msg::EditPasteRowsNotShown { column: column.clone() },
                        Msg::EditPasteRowsNotShownShort { column },
                    );
                };
                // A column the table computes or numbers itself: left to it.
                let fill = target.columns.get(c).and_then(Option::as_ref).map(|tc| &tc.column.fill);
                if matches!(fill, Some(ColumnFill::Stored(_) | ColumnFill::Virtual(_) | ColumnFill::IdentityAlways)) {
                    generated += 1;
                    map.push(None);
                } else {
                    map.push(Some(c));
                }
            }
            map
        } else if let Some(m) =
            copy.columns.iter().find(|m| {
                // A column of this result the target is not (a join's other table).
                same_db
                    && m.origin.is_some()
                    && rs.columns.iter().enumerate().any(|(c, col)| {
                        col.meta.origin == m.origin && target.columns.get(c).is_none_or(Option::is_none)
                    })
            })
        {
            let column = named(m);
            return refuse(
                Msg::EditPasteRowsComputed { column: column.clone() },
                Msg::EditPasteRowsComputedShort { column },
            );
        } else if w > ncols {
            let (width, room) = (w.to_string(), ncols.to_string());
            let short = Msg::EditPasteRowsTooWideShort { width: width.clone(), room: room.clone() };
            return refuse(Msg::EditPasteRowsTooWide { width, room }, short);
        } else {
            // From another table or result: by position.
            (0..w).map(Some).collect()
        };
        // The new rows go after the last row: past the rows read while the server has more.
        if rs.more {
            if matches!(t.exec.paging, Paging::Open { .. }) {
                let key = self.next_page_key(self.grid_ctx());
                return refuse(Msg::EditPasteRowsMore { key: key.clone() }, Msg::EditPasteRowsMoreShort { key });
            }
            let short = i18n.label(Label::EditPasteRowsMorePagedShort).to_string();
            return Err((i18n.label(Label::EditPasteRowsMorePaged).to_string(), Some(short)));
        }
        for &c in map.iter().flatten() {
            if let Err(r) = target.allows(c, Write::Insert) {
                return Err((text(column_reason(r, rs.columns[c].meta.name.clone())), None));
            }
        }
        if self.insert_column(target, ncols).is_err() {
            let reason = i18n.label(Label::EditWhyNoInsertColumn).to_string();
            let short = i18n.label(Label::EditPasteNoInsertShort).to_string();
            return Err((text(Msg::EditPasteNoNewRows { reason }), Some(short)));
        }
        if let Some(column) = target.unshown_required.first() {
            let reason = text(Msg::EditPasteUnshown { column: column.clone() });
            let short = text(Msg::EditPasteUnshownShort { column: column.clone() });
            return Err((text(Msg::EditPasteNoNewRows { reason }), Some(short)));
        }
        let add = copy
            .rows
            .iter()
            .map(|r| {
                let mut cells = vec![Value::Default; ncols];
                for (j, c) in map.iter().enumerate() {
                    if let Some(c) = *c {
                        cells[c] = r.get(j).cloned().unwrap_or(Value::Default);
                    }
                }
                cells
            })
            .collect();
        let pairs: Vec<(usize, usize)> = map.iter().enumerate().filter_map(|(j, c)| c.map(|c| (j, c))).collect();
        let first_col = pairs.iter().map(|&(_, c)| c).min().unwrap_or(0);
        Ok(PastePlan {
            ops: PasteOps { width: ncols, add, ..PasteOps::default() },
            first: 0,
            read: Vec::new(),
            top: (shown, first_col),
            size: (h, w),
            format: None,
            corner: false,
            rows: h,
            new: h,
            map: pairs,
            as_rows: true,
            header: false,
            generated,
        })
    }

    /// The invalid cells of `rs`'s changes (shown in tab `t`); none while its table is not
    /// known. Found again only when the changes, the words, the NULL token, the target or what
    /// says whether a value is stored as typed moved.
    pub(crate) fn invalid_cells(&self, t: &Tab, rs: &ResultSet) -> Arc<Invalids> {
        let Target::Ready(target) = &rs.edit.target else {
            return Arc::new(Invalids { revision: rs.edit.batch.revision(), ..Invalids::default() });
        };
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let key = InvalidKey {
            revision: rs.edit.batch.revision(),
            lang: self.i18n.lang,
            null: self.prefs.paste_null,
            dialect: self.tab_dialect(t.id),
            zone: zone.map(str::to_string),
            read_in: rs.read_in.clone(),
        };
        let cache = &rs.edit.invalid;
        if let Some((k, x, m)) = &*cache.made.borrow()
            && *k == key
            && *x == **target
        {
            return m.clone();
        }
        let m = Arc::new(self.find_invalid(t, rs, target, key.revision));
        *cache.made.borrow_mut() = Some((key, (**target).clone(), m.clone()));
        m
    }

    /// The invalid cells of the active tab's shown result, while its table is known.
    pub fn shown_invalid(&self) -> Option<Arc<Invalids>> {
        if self.tabs.is_empty() {
            return None;
        }
        let t = self.tab();
        let rs = rows_of(t).filter(|rs| matches!(rs.edit.target, Target::Ready(_)))?;
        Some(self.invalid_cells(t, rs))
    }

    /// Every changed cell of `rs` (resolved to `target`, its changes at `revision`) whose value
    /// its column does not take, as the cell editor judges a typed one: NULL in a NOT NULL
    /// column; a text the session would not store as typed, or not of the column's type (an
    /// empty one: said to be pasted as the NULL token for NULL).
    fn find_invalid(&self, t: &Tab, rs: &ResultSet, target: &EditTarget, revision: u64) -> Invalids {
        use export::NotWhole as N;
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let null = self.prefs.paste_null.token();
        let typed = |r: &export::Refused| {
            matches!(r.why, N::Charset(_) | N::TimeZone(_) | N::SessionUnknown | N::SessionChanged)
        };
        let mut sessions: BTreeMap<usize, Result<export::SqlCells, export::Refused>> = BTreeMap::new();
        let mut out = Invalids { revision, ..Invalids::default() };
        let mut check = |at: RowAt, c: usize, v: &Value, w: Write| {
            let Ok(tc) = target.allows(c, w) else { return };
            let column = crate::text::escape_controls(&tc.column.name);
            let why = match v {
                Value::Default => None,
                Value::Null => tc.column.not_null.then(|| self.i18n.msg(&Msg::EditWhyNotNull { column }).to_string()),
                Value::Text(s) => {
                    let Some(m) = rs.columns.get(c).map(|c| &c.meta) else { return };
                    let cells = sessions.entry(c).or_insert_with(|| {
                        let cols = [export::SqlColumn {
                            name: &m.name,
                            type_name: &m.type_name,
                            kind: m.kind,
                            auto_increment: false,
                        }];
                        export::sql_cells(d, &cols, &rs.read_in, zone)
                    });
                    let refused = match cells {
                        Err(r) => typed(r).then(|| r.clone()),
                        Ok(cells) => {
                            cells.cell(0, s).err().filter(typed).or_else(|| cells.column_refused(0).filter(typed))
                        }
                    };
                    match refused {
                        Some(r) => {
                            let reason = self.not_whole_reason(&r.why);
                            let column = crate::text::escape_controls(&r.column);
                            Some(self.i18n.msg(&Msg::EditWhyColumnNotWhole { column, reason }).to_string())
                        }
                        None => match self.typed_ok(d, tc, s) {
                            Ok(()) => None,
                            Err(_) if s.is_empty() && !null.is_empty() => {
                                Some(self.i18n.msg(&Msg::EditPasteEmptyValue { null: null.to_string() }).to_string())
                            }
                            Err(_) if s.is_empty() => {
                                Some(self.i18n.label(Label::EditPasteEmptyValuePlain).to_string())
                            }
                            Err(reason) => {
                                let value = super::preview::literals::quoted_escaped(s, super::preview::VALUE_W);
                                Some(self.i18n.msg(&Msg::EditPasteValueReason { reason, value }).to_string())
                            }
                        },
                    }
                }
            };
            if let Some(why) = why {
                out.cells.insert((at, c), why);
            }
        };
        let batch = &rs.edit.batch;
        for (r, e) in batch.rows().filter(|(_, e)| !e.delete) {
            for (&c, v) in &e.set {
                check(RowAt::Read(r), c, v, Write::Update);
            }
        }
        for dr in batch.drafts() {
            for (c, v) in dr.cells.iter().enumerate() {
                check(RowAt::New(dr.id), c, v, Write::Insert);
            }
        }
        out.set = out
            .cells
            .keys()
            .map(|&(at, c)| match at {
                RowAt::Read(r) => (Origin::Row(r), c),
                RowAt::New(id) => (Origin::New(id), c),
            })
            .collect();
        out.no_default = edit::no_default(target, batch).into_iter().map(|(id, c)| (RowAt::New(id), c)).collect();
        out.not_shown = edit::not_shown(target, batch).into_iter().map(|(id, _)| id).collect();
        out
    }

    /// Why the value at row `at`, column `col` of the active tab's shown result is invalid.
    pub(super) fn invalid_reason(&self, at: RowAt, col: usize) -> Option<String> {
        self.shown_invalid()?.cells.get(&(at, col)).cloned()
    }

    /// Whether the cell at row `at`, column `col` of the active tab's shown result is a new
    /// row's left to a default its column lacks.
    pub(super) fn value_needed(&self, at: RowAt, col: usize) -> bool {
        self.shown_invalid().is_some_and(|inv| inv.no_default.contains(&(at, col)))
    }

    /// Why an Apply is refused for the value at row `at`, column `col` of the active tab's shown
    /// result: invalid, or (the Changes tab's words) a new row's that needs a value.
    pub(crate) fn problem_reason(&self, at: RowAt, col: usize) -> Option<String> {
        if let Some(why) = self.invalid_reason(at, col) {
            return Some(why);
        }
        let (RowAt::New(id), true) = (at, self.value_needed(at, col)) else { return None };
        let rs = rows_of(self.tab())?;
        let column = self.preview_column(rs, col);
        let bare = self.i18n.msg(&Msg::EditPreviewNoDefault { column }).to_string();
        Some(self.at_row(rs, RowAt::New(id), &bare))
    }

    /// How many problems of `rs`'s changes the grid marks, when it is the active tab's shown
    /// result: cells invalid or that need a value, and new rows of a table with a column the
    /// result does not show that needs one.
    pub(super) fn invalid_count(&self, rs: &ResultSet) -> usize {
        if self.tabs.is_empty() {
            return 0;
        }
        let t = self.tab();
        match rows_of(t) {
            Some(shown) if shown.id == rs.id => {
                let inv = self.invalid_cells(t, rs);
                inv.cells.len() + inv.no_default.len() + inv.not_shown.len()
            }
            _ => 0,
        }
    }
}
