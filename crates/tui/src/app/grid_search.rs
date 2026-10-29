//! Search inside the result grid: `/` and `?` open a prompt on the line under the grid, the
//! cursor goes to the first matching cell while the pattern is typed, `Enter` searches; while
//! the search is on, `n` and `N` go from match to match (`grid.matches`), and `Esc` ends it.
//!
//! What is searched: the full value of every cell of the rows fetched so far (in memory and in
//! the spill file), as the driver gave it (not the cut the grid shows), with the editor's
//! pattern rules (a Rust regular expression, case sensitive unless it starts with `(?i)`). NULL
//! never matches; column names are not searched. A match is a cell, in row-major order (left to
//! right, then the next row); a cell matching twice counts once. Nothing is fetched or run for
//! a search: rows that come later are searched by the next `/`, `n` or `N`.
//!
//! The rows are read on a worker ([`scan`]), never on the UI thread: each job has a number of
//! its own and is bound to its tab, its result and the rows it read; an answer for a job the tab
//! no longer waits for is dropped, and a replaced job is told to stop. A job answers twice: the
//! first match from where it started (the cursor moves at once), then every match (capped at
//! [`MAX_MATCHES`]; past the cap `n` and `N` search again from the cursor). Headless (tests,
//! benches), [`App::pump_grid_search`] hands the answers to the app.

use super::*;
use crate::widgets::search::{SearchNotice, compile};
use crate::widgets::text_input::InputResult;
use datarig_core::fault::Fault;
use datarig_core::results::RowsSnapshot;
use regex::Regex;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

/// The most matches a search keeps; past them the count says `1,000,000+`.
pub const MAX_MATCHES: usize = 1_000_000;

/// Rows read at a time by a job (it checks whether to stop between them).
const CHUNK: usize = 1024;

/// A cell of a result in row-major order: `row * columns + column`.
pub type CellIdx = u64;

/// What the grid's search keys do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchAction {
    /// `/` (`forward`) or `?`: the prompt.
    Open { forward: bool },
    /// `n` (`same`: in the search's direction) or `N`.
    Next { same: bool },
    /// `Esc` while the search is on: it ends.
    Clear,
}

/// Where the cursor and the view were, and the range selected then: the prompt's `Esc` goes
/// back there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spot {
    page: usize,
    row: usize,
    col: usize,
    top: usize,
    left: usize,
    anchor: Option<(usize, usize)>,
    shape: crate::widgets::grid::Shape,
    detached: bool,
}

impl Spot {
    fn of(g: &GridState) -> Self {
        Self {
            page: g.page,
            row: g.row,
            col: g.col,
            top: g.top,
            left: g.left,
            anchor: g.anchor,
            shape: g.shape,
            detached: g.detached,
        }
    }

    fn restore(self, g: &mut GridState) {
        (g.page, g.row, g.col, g.top, g.left) = (self.page, self.row, self.col, self.top, self.left);
        (g.anchor, g.shape, g.detached) = (self.anchor, self.shape, self.detached);
    }
}

/// The matching cells a job found, of the rows it read.
#[derive(Debug)]
pub struct Matches {
    result: u64,
    rows: usize,
    ncols: usize,
    cells: Vec<CellIdx>,
    /// There were more than [`MAX_MATCHES`]: `cells` has the first ones.
    capped: bool,
}

impl Matches {
    /// Whether these are the matches of `rs` as it is now (no rows came since).
    fn current(&self, rs: &ResultSet) -> bool {
        self.result == rs.id && self.rows == rs.rows.len() && self.ncols == rs.columns.len()
    }

    /// Whether cell (row, col) is a match.
    pub fn has(&self, row: usize, col: usize) -> bool {
        row < self.rows && col < self.ncols && self.cells.binary_search(&cell(row, col, self.ncols)).is_ok()
    }
}

/// The `/` or `?` prompt being typed.
struct Prompt {
    input: TextInput,
    forward: bool,
    /// The result it searches.
    result: u64,
    /// Where the cursor was when it opened: the preview searches from there.
    origin: Spot,
    /// The matches of the pattern typed so far, once its job found them all.
    preview: Option<Matches>,
}

/// The search that is on (after `Enter`).
struct Active {
    re: Regex,
    forward: bool,
    result: u64,
    /// Its matches, once a job found them all.
    found: Option<Matches>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// The pattern being typed: the cursor previews its first match, nothing is said; every
    /// match is found.
    Preview,
    /// `Enter`, or `n`/`N` once rows came since the matches were found: the cursor goes to the
    /// match (going around an end or finding nothing is said) and every match is found.
    Search,
    /// `n`/`N` where the matches found cannot tell (past the cap, or while they are still
    /// being found): only the next match from the cursor is looked for.
    Seek,
}

/// A job out on a worker.
struct Running {
    id: u64,
    purpose: Purpose,
    result: u64,
    /// The rows (and columns) it reads: its matches are of these.
    rows: usize,
    ncols: usize,
    forward: bool,
    pattern: String,
    /// The cursor when it started: its first match moves only a cursor that is still there.
    from: (usize, usize),
    /// Its first match came.
    found_one: bool,
    stop: Arc<AtomicBool>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A tab's search of its grid.
#[derive(Default)]
pub struct GridFind {
    prompt: Option<Prompt>,
    active: Option<Active>,
    /// The job that finds every match (of the prompt's pattern or of the search that is on).
    job: Option<Running>,
    /// A job of `n`/`N` that looks for the next match only, beside it.
    seek: Option<Running>,
}

impl GridFind {
    /// The prompt is open.
    pub fn prompting(&self) -> bool {
        self.prompt.is_some()
    }

    /// End everything: the prompt (the cursor stays), the search, its jobs.
    pub fn end(&mut self) {
        *self = Self::default();
    }

    /// Stop the jobs out (their answers are dropped).
    fn stop_jobs(&mut self) {
        self.job = None;
        self.seek = None;
    }

    /// The matches to highlight in result `rs`: those of the pattern typed so far while the
    /// prompt is open, else those of the search that is on.
    pub fn shown(&self, rs: &ResultSet) -> Option<&Matches> {
        let m = match &self.prompt {
            Some(p) => p.preview.as_ref(),
            None => self.active.as_ref().and_then(|a| a.found.as_ref()),
        };
        m.filter(|m| m.result == rs.id)
    }
}

/// What a job says.
#[derive(Debug)]
pub enum News {
    /// The first match from where it started; `wrapped`: past an end of the rows.
    First { cell: CellIdx, wrapped: bool },
    /// Every match (`None`: it was told to stop), or why the rows could not be read.
    Done(Result<Option<(Vec<CellIdx>, bool)>, Fault>),
}

/// An answer of job `job` of tab `tab`.
#[derive(Debug)]
pub struct Answer {
    pub job: u64,
    pub tab: TabId,
    pub news: News,
}

/// Where answers go: the event loop, or (headless) the app's own channel.
#[derive(Clone)]
enum Out {
    App(UnboundedSender<AppEvent>),
    Local(mpsc::Sender<AppEvent>),
}

impl Out {
    fn send(&self, a: Answer) {
        let ev = AppEvent::GridSearch(a);
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

/// A job's way back: its last answer ([`News::Done`]) is sent even when the job fails inside.
struct Reply {
    out: Out,
    job: u64,
    tab: TabId,
    done: bool,
}

impl Reply {
    fn send(&self, news: News) {
        self.out.send(Answer { job: self.job, tab: self.tab, news });
    }

    fn done(mut self, r: Result<Option<(Vec<CellIdx>, bool)>, Fault>) {
        self.done = true;
        self.send(News::Done(r));
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.done {
            self.send(News::Done(Err(Fault::other("the search stopped inside"))));
        }
    }
}

/// The app's side of the jobs.
#[derive(Default)]
pub struct GridJobs {
    next: u64,
    /// Headless jobs whose last answer was not handed to the app yet.
    pending: Arc<AtomicUsize>,
    local: Option<(mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>)>,
    /// The pattern of the last search (an empty pattern takes it).
    last: Option<String>,
    /// Cells the frames looked up for the highlight since the work was last taken.
    lookups: usize,
    /// Jobs that read every row for every match, since the app started.
    scans: u64,
    /// The most matches kept (tests make it small); [`MAX_MATCHES`] when not set.
    cap: Option<usize>,
}

/// The index of cell (row, col) of a result of `ncols` columns.
fn cell(row: usize, col: usize, ncols: usize) -> CellIdx {
    row as CellIdx * ncols as CellIdx + col as CellIdx
}

/// Search `snap` (rows of `ncols` columns) for the cells `re` matches, in row-major order.
/// `first` is told the first match from cell `origin` in the search's direction (after it when
/// `forward`, before it otherwise; around the ends when there is none that way, `wrapped`) as
/// soon as it is known. Returns the first [`MAX_MATCHES`] matches (`cap` in tests) and whether
/// there were more, or `None` when `stop` was set. NULL never matches.
#[allow(clippy::too_many_arguments)]
pub fn scan(
    snap: &RowsSnapshot,
    ncols: usize,
    re: &Regex,
    origin: CellIdx,
    forward: bool,
    cap: usize,
    stop: &AtomicBool,
    mut first: impl FnMut(CellIdx, bool),
) -> Result<Option<(Vec<CellIdx>, bool)>, Fault> {
    let mut cells = Vec::new();
    let mut capped = false;
    let mut told = false;
    let (mut first_any, mut last_any, mut last_before) = (None, None, None);
    let mut stopped = false;
    let mut row = 0usize;
    snap.for_each_chunk(CHUNK, |chunk| {
        if stop.load(Ordering::Relaxed) {
            stopped = true;
            return false;
        }
        for r in chunk {
            for (c, v) in r.iter().take(ncols).enumerate() {
                let Some(v) = v else { continue };
                if !re.is_match(v) {
                    continue;
                }
                let at = cell(row, c, ncols);
                if cells.len() < cap {
                    cells.push(at);
                } else {
                    capped = true;
                }
                first_any.get_or_insert(at);
                last_any = Some(at);
                if !told && forward && at > origin {
                    first(at, false);
                    told = true;
                } else if !forward && at < origin {
                    last_before = Some(at);
                }
            }
            row += 1;
            // Backward: past the origin's cell the match before it is known.
            if !told
                && !forward
                && cell(row, 0, ncols) > origin
                && let Some(at) = last_before
            {
                first(at, false);
                told = true;
            }
        }
        // Past the cap only the first match is still looked for.
        !(capped && told)
    })?;
    if stopped {
        return Ok(None);
    }
    if !told {
        let around = if forward {
            first_any.map(|a| (a, true))
        } else {
            last_before.map(|a| (a, false)).or(last_any.map(|a| (a, true)))
        };
        if let Some((at, wrapped)) = around {
            first(at, wrapped);
        }
    }
    Ok(Some((cells, capped)))
}

/// The first match from cell `origin` in the search's direction, without collecting the
/// others: forward, the cells after it to the end, then from the start up to it (`wrapped`);
/// backward, the cells before it back to the start, then from the end back to it. The rows are
/// read a chunk at a time from the cursor's row on (backward: chunks from the end of each part
/// back). `Ok(None)` when `stop` was set, `Ok(Some(None))` when nothing matches. NULL never
/// matches.
pub fn seek(
    snap: &RowsSnapshot,
    ncols: usize,
    re: &Regex,
    origin: CellIdx,
    forward: bool,
    stop: &AtomicBool,
) -> Result<Option<Option<(CellIdx, bool)>>, Fault> {
    let len = snap.len();
    if len == 0 || ncols == 0 {
        return Ok(Some(None));
    }
    let orow = ((origin / ncols as CellIdx) as usize).min(len - 1);
    let find = |range: Range<usize>, last: bool, keep: &dyn Fn(CellIdx) -> bool| {
        find_in(snap, ncols, re, stop, range, last, keep)
    };
    let first =
        if forward { find(orow..len, false, &|c| c > origin)? } else { find(0..orow + 1, true, &|c| c < origin)? };
    match first {
        None => Ok(None),
        Some(Some(c)) => Ok(Some(Some((c, false)))),
        Some(None) => {
            let around = if forward {
                find(0..orow + 1, false, &|c| c <= origin)?
            } else {
                find(orow..len, true, &|c| c >= origin)?
            };
            Ok(around.map(|f| f.map(|c| (c, true))))
        }
    }
}

/// The first matching cell of rows `range` that `keep` takes, or the last one (`last`: the
/// rows are read a chunk at a time from the end of the range back). `Ok(None)` when `stop` was
/// set.
fn find_in(
    snap: &RowsSnapshot,
    ncols: usize,
    re: &Regex,
    stop: &AtomicBool,
    range: Range<usize>,
    last: bool,
    keep: &dyn Fn(CellIdx) -> bool,
) -> Result<Option<Option<CellIdx>>, Fault> {
    // One part of the range: its first (or last) cell kept, and whether it was told to stop.
    let part = |range: Range<usize>| -> Result<(Option<CellIdx>, bool), Fault> {
        let (mut found, mut stopped) = (None, false);
        snap.for_each_chunk_in(range, CHUNK, |at, rows| {
            if stop.load(Ordering::Relaxed) {
                stopped = true;
                return false;
            }
            for (i, r) in rows.iter().enumerate() {
                for (c, v) in r.iter().take(ncols).enumerate() {
                    let at = cell(at + i, c, ncols);
                    if v.as_deref().is_some_and(|v| re.is_match(v)) && keep(at) {
                        found = Some(at);
                        if !last {
                            return false;
                        }
                    }
                }
            }
            true
        })?;
        Ok((found, stopped))
    };
    if !last {
        let (found, stopped) = part(range)?;
        return Ok((!stopped).then_some(found));
    }
    let mut end = range.end;
    while end > range.start {
        let start = end.saturating_sub(CHUNK).max(range.start);
        match part(start..end)? {
            (_, true) => return Ok(None),
            (Some(c), _) => return Ok(Some(Some(c))),
            (None, _) => end = start,
        }
    }
    Ok(Some(None))
}

/// Where `n` or `N` goes from cell `at` among `cells` (sorted): the next match in the
/// direction (`wrapped`: around an end), `Some(None)` when there is none at all, `None` when
/// the list cannot tell (it is capped and the answer may lie past its last cell).
pub fn step(cells: &[CellIdx], capped: bool, at: CellIdx, forward: bool) -> Option<Option<(CellIdx, bool)>> {
    let (Some(&first), Some(&last)) = (cells.first(), cells.last()) else {
        return (!capped).then_some(None);
    };
    if capped && at >= last {
        return None;
    }
    if forward {
        let i = cells.partition_point(|&c| c <= at);
        Some(Some(cells.get(i).map_or((first, true), |&c| (c, false))))
    } else {
        match cells.partition_point(|&c| c < at) {
            // Around the start: the last match is past the capped list.
            0 if capped => None,
            0 => Some(Some((last, true))),
            i => Some(Some((cells[i - 1], false))),
        }
    }
}

/// The grid's cursor on cell `at` of a result of `ncols` columns: on its page, the view
/// following it; a selected range goes when the page changes.
fn goto(g: &mut GridState, at: CellIdx, ncols: usize) {
    let (row, col) = ((at / ncols as CellIdx) as usize, (at % ncols as CellIdx) as usize);
    if g.page_size > 0 && row / g.page_size != g.page {
        g.page = row / g.page_size;
        g.anchor = None;
    }
    (g.row, g.col) = (row, col);
    g.detached = false;
}

/// The row result a tab shows in its grid, if it shows one.
fn shown_rows(t: &Tab) -> Option<&ResultSet> {
    match &t.results {
        Results::Rows(rs) if t.exec.view == super::tabs::ResultView::Rows => Some(rs),
        _ => None,
    }
}

/// A search of tab `t`'s grid is on: its prompt is closed and the search is of the rows shown.
pub(crate) fn search_on(t: &Tab) -> bool {
    t.find.prompt.is_none() && t.find.active.as_ref().zip(shown_rows(t)).is_some_and(|(a, rs)| a.result == rs.id)
}

/// What the line under the grid shows of a search.
pub struct Strip {
    /// `/` or `?`.
    pub mark: char,
    /// The pattern of the search that is on; `None` while the prompt is open (it draws its
    /// input).
    pub pattern: Option<String>,
    /// How wide the pattern (or what the prompt has) is drawn.
    pub text_width: usize,
    /// The cursor's match among the matches: `3/120`, `-/120` off a match, `…` while they are
    /// counted, `1,000,000+` past the cap.
    pub count: String,
    /// What is not searched (the rows still on the server), the longest first: the first that
    /// fits is drawn, none where none does.
    pub notes: Vec<String>,
}

impl App {
    /// The prompt of the grid's search is open in the active tab.
    pub fn grid_prompt_open(&self) -> bool {
        !self.tabs.is_empty() && self.tab().find.prompting()
    }

    /// A search of the active tab's grid is on (its `n` and `N` go to its matches).
    pub fn grid_search_on(&self) -> bool {
        !self.tabs.is_empty() && search_on(self.tab())
    }

    /// The pattern of the search that is on in the active tab's grid, if one is.
    pub fn grid_search_pattern(&self) -> Option<&str> {
        self.grid_search_on().then(|| self.tab().find.active.as_ref().map(|a| a.re.as_str())).flatten()
    }

    /// Cells the highlight looked up since the last call (the count starts again).
    pub fn take_grid_search_work(&mut self) -> usize {
        std::mem::take(&mut self.grid_jobs.lookups)
    }

    pub(crate) fn count_grid_lookups(&mut self, n: usize) {
        self.grid_jobs.lookups += n;
    }

    pub(super) fn grid_search_action(&mut self, s: SearchAction) {
        match s {
            SearchAction::Open { forward } => {
                // The prompt is drawn on the line under the grid: none where the grid has no
                // line to spare (it would take keys unseen).
                let room = self.layout.grid_area;
                if room.width > 0 && room.height <= 3 {
                    let key = self.key_for(Action::Panel(action::PanelAction::Maximize), Ctx::Grid);
                    return self.flash(Notice::new(Msg::GridSearchNoRoom { key }, Level::Info));
                }
                let t = self.tabs.active_mut();
                let Some(rs) = shown_rows(t) else { return };
                let result = rs.id;
                t.find.stop_jobs();
                t.find.prompt = Some(Prompt {
                    input: TextInput::default(),
                    forward,
                    result,
                    origin: Spot::of(&t.grid),
                    preview: None,
                });
            }
            SearchAction::Next { same } => self.grid_search_next(same),
            SearchAction::Clear => self.tabs.active_mut().find.end(),
        }
    }

    /// `n` (`same`) or `N`: the next match from the cursor. From the matches found while no
    /// row came since and the list can tell; past the cap of the list, or while every match is
    /// still being found for these rows, a job looks for the next one only (the one finding them
    /// goes on); after rows came, a job finds every match again from the cursor.
    fn grid_search_next(&mut self, same: bool) {
        let t = self.tabs.active_mut();
        let Some(rs) = shown_rows(t) else { return };
        let Some(a) = t.find.active.as_ref() else { return };
        let forward = a.forward == same;
        let ncols = rs.columns.len();
        if ncols == 0 {
            return;
        }
        let at = cell(t.grid.row, t.grid.col, ncols);
        let current = a.found.as_ref().filter(|m| m.current(rs));
        let known = current.and_then(|m| step(&m.cells, m.capped, at, forward));
        let finding = t
            .find
            .job
            .as_ref()
            .is_some_and(|j| j.purpose == Purpose::Search && j.result == rs.id && j.rows == rs.rows.len());
        let seek = current.is_some() || finding;
        match known {
            Some(Some((to, wrapped))) => {
                goto(&mut t.grid, to, ncols);
                if wrapped {
                    self.flash(super::update::search_notice(SearchNotice::Wrapped { bottom: forward }));
                }
            }
            Some(None) => {
                // Nothing matches: the search ends (`n` is the next page again).
                let pattern = a.re.as_str().to_string();
                t.find.end();
                self.flash(super::update::search_notice(SearchNotice::NotFound(pattern)));
            }
            None => {
                let re = a.re.clone();
                let purpose = if seek { Purpose::Seek } else { Purpose::Search };
                self.start_grid_job(purpose, re, forward, at);
            }
        }
    }

    /// A key while the prompt is open: `Enter` searches, `Esc` (or `Backspace` on an empty
    /// pattern) goes back to where the cursor was, `Ctrl+W` deletes the word before the cursor,
    /// any other key edits the pattern.
    pub(super) fn grid_prompt_key(&mut self, key: KeyEvent) {
        let Some(p) = self.tabs.active_mut().find.prompt.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.close_grid_prompt(),
            KeyCode::Backspace if p.input.text().is_empty() => self.close_grid_prompt(),
            KeyCode::Enter => self.grid_prompt_enter(),
            _ => {
                let ctrl_w = key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL);
                let r = if ctrl_w { p.input.delete_word_back() } else { p.input.handle_key(&key) };
                if r == InputResult::Changed {
                    self.grid_preview();
                }
            }
        }
    }

    /// Text pasted into the prompt.
    pub(super) fn grid_prompt_paste(&mut self, text: &str) {
        if let Some(p) = self.tabs.active_mut().find.prompt.as_mut() {
            p.input.insert_str(text);
            self.grid_preview();
        }
    }

    /// Close the prompt as `Esc` does: the cursor and the view go back, the search that was on
    /// before stays.
    pub(super) fn close_grid_prompt(&mut self) {
        let t = self.tabs.active_mut();
        if let Some(p) = t.find.prompt.take() {
            t.find.stop_jobs();
            p.origin.restore(&mut t.grid);
        }
    }

    /// The pattern typed so far: the cursor back where the prompt opened, then (a valid
    /// pattern) on its first match once a job finds it.
    fn grid_preview(&mut self) {
        let t = self.tabs.active_mut();
        let Some(p) = t.find.prompt.as_mut() else { return };
        t.find.job = None;
        t.find.seek = None;
        p.preview = None;
        p.origin.restore(&mut t.grid);
        let (forward, origin) = (p.forward, p.origin);
        let Some(re) = Some(p.input.text()).filter(|s| !s.is_empty()).and_then(|s| compile(s).ok()) else { return };
        let ncols = shown_rows(t).map_or(0, |rs| rs.columns.len());
        self.start_grid_job(Purpose::Preview, re, forward, cell(origin.row, origin.col, ncols));
    }

    /// `Enter` in the prompt: the pattern (an empty one: the last search's) becomes the search
    /// that is on, and the cursor goes to its first match from where the prompt opened.
    fn grid_prompt_enter(&mut self) {
        let t = self.tabs.active_mut();
        let Some(p) = t.find.prompt.take() else { return };
        t.find.stop_jobs();
        let pattern = match p.input.text() {
            "" => self.grid_jobs.last.clone().ok_or(SearchNotice::NoPrevious),
            s => Ok(s.to_string()),
        };
        let re = pattern.and_then(|s| compile(&s).map_err(SearchNotice::Invalid));
        let re = match re {
            Ok(re) => re,
            Err(n) => {
                p.origin.restore(&mut self.tabs.active_mut().grid);
                return self.flash(super::update::search_notice(n));
            }
        };
        self.grid_jobs.last = Some(re.as_str().to_string());
        let t = self.tabs.active_mut();
        let Some(rs) = shown_rows(t) else { return };
        let ncols = rs.columns.len();
        t.find.active = Some(Active { re: re.clone(), forward: p.forward, result: p.result, found: None });
        self.start_grid_job(Purpose::Search, re, p.forward, cell(p.origin.row, p.origin.col, ncols));
    }

    fn grid_out(&mut self) -> Out {
        match &self.tx {
            Some(tx) => Out::App(tx.clone()),
            None => Out::Local(self.grid_jobs.local.get_or_insert_with(mpsc::channel).0.clone()),
        }
    }

    /// A job over the rows the active tab's grid shows, as they are now, from cell `origin`
    /// (a job that finds every match replaces both jobs; one that seeks, only the other seek).
    fn start_grid_job(&mut self, purpose: Purpose, re: Regex, forward: bool, origin: CellIdx) {
        let out = self.grid_out();
        let tab = self.tab().id;
        let cap = self.grid_jobs.cap.unwrap_or(MAX_MATCHES);
        let t = self.tabs.active_mut();
        match purpose {
            Purpose::Seek => t.find.seek = None,
            _ => t.find.stop_jobs(),
        }
        let Some(rs) = shown_rows(t) else { return };
        let (result, rows, ncols) = (rs.id, rs.rows.len(), rs.columns.len());
        if ncols == 0 {
            return;
        }
        let snap = match rs.rows.snapshot(0..rows) {
            Ok(s) => s,
            Err(f) => return self.results_read_failed(&f),
        };
        self.grid_jobs.next += 1;
        let id = self.grid_jobs.next;
        if purpose != Purpose::Seek {
            self.grid_jobs.scans += 1;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let pattern = re.as_str().to_string();
        let t = self.tabs.active_mut();
        let from = (t.grid.row, t.grid.col);
        let job =
            Running { id, purpose, result, rows, ncols, forward, pattern, from, found_one: false, stop: stop.clone() };
        match purpose {
            Purpose::Seek => t.find.seek = Some(job),
            _ => t.find.job = Some(job),
        }
        if matches!(out, Out::Local(_)) {
            self.grid_jobs.pending.fetch_add(1, Ordering::SeqCst);
        }
        let reply = Reply { out, job: id, tab, done: false };
        std::thread::spawn(move || {
            let first = |cell, wrapped| reply.send(News::First { cell, wrapped });
            let r = match purpose {
                Purpose::Seek => seek(&snap, ncols, &re, origin, forward, &stop).map(|found| {
                    found.map(|f| {
                        if let Some((cell, wrapped)) = f {
                            first(cell, wrapped);
                        }
                        (Vec::new(), false)
                    })
                }),
                _ => scan(&snap, ncols, &re, origin, forward, cap, &stop, first),
            };
            reply.done(r);
        });
    }

    /// Jobs that read every row for every match since the app started (`n` past the cap only
    /// seeks).
    pub fn grid_search_scans(&self) -> u64 {
        self.grid_jobs.scans
    }

    /// Keep at most `cap` matches (tests; [`MAX_MATCHES`] otherwise).
    pub fn set_grid_search_cap(&mut self, cap: usize) {
        self.grid_jobs.cap = Some(cap);
    }

    /// Headless (without the event loop's channel): hand the answers of the grid's search jobs
    /// to the app, waiting up to `wait` while one is still out. How many came.
    pub fn pump_grid_search(&mut self, wait: Duration) -> usize {
        let deadline = Instant::now() + wait;
        let mut n = 0;
        while self.grid_jobs.pending.load(Ordering::SeqCst) > 0 {
            let Some((_, rx)) = &self.grid_jobs.local else { break };
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(ev) = rx.recv_timeout(left) else { break };
            if matches!(&ev, AppEvent::GridSearch(Answer { news: News::Done(_), .. })) {
                self.grid_jobs.pending.fetch_sub(1, Ordering::SeqCst);
            }
            n += 1;
            self.on_app_event(ev);
        }
        n
    }

    /// An answer of a job: for the tab that waits for it, about the result it still shows;
    /// any other is dropped. A first match moves the cursor only when it is still where it was
    /// when the job started.
    pub(super) fn on_grid_search_answer(&mut self, a: Answer) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == a.tab) else { return };
        let seeking = t.find.seek.as_ref().is_some_and(|j| j.id == a.job);
        let slot = if seeking { &t.find.seek } else { &t.find.job };
        let Some(result) = slot.as_ref().filter(|j| j.id == a.job).map(|j| j.result) else { return };
        if shown_rows(t).is_none_or(|rs| rs.id != result) {
            match seeking {
                true => t.find.seek = None,
                false => t.find.job = None,
            }
            return;
        }
        let slot = if seeking { &mut t.find.seek } else { &mut t.find.job };
        let notice = match a.news {
            News::First { cell, wrapped } => {
                let Some(job) = slot.as_mut() else { return };
                job.found_one = true;
                if (t.grid.row, t.grid.col) != job.from {
                    return;
                }
                let said = (wrapped && job.purpose != Purpose::Preview).then_some(job.forward);
                goto(&mut t.grid, cell, job.ncols);
                said.map(|bottom| SearchNotice::Wrapped { bottom })
            }
            News::Done(r) => {
                let Some(job) = slot.take() else { return };
                match r {
                    Ok(Some((cells, capped))) => {
                        let m = Matches { result: job.result, rows: job.rows, ncols: job.ncols, cells, capped };
                        match job.purpose {
                            Purpose::Preview => {
                                if let Some(p) = t.find.prompt.as_mut() {
                                    p.preview = Some(m);
                                }
                            }
                            Purpose::Search => {
                                if let Some(a) = t.find.active.as_mut() {
                                    a.found = Some(m);
                                }
                            }
                            Purpose::Seek => {}
                        }
                        // A search that matches nothing ends (`n` is the next page again).
                        let none = !job.found_one && job.purpose != Purpose::Preview;
                        if none {
                            t.find.end();
                        }
                        none.then(|| SearchNotice::NotFound(job.pattern.clone()))
                    }
                    Ok(None) => None,
                    Err(f) => return self.results_read_failed(&f),
                }
            }
        };
        if let Some(n) = notice {
            self.flash(super::update::search_notice(n));
        }
    }

    /// A tab whose grid no longer shows the result its search is about (another result tab,
    /// the Messages, a Plan or a Chart, a run's new rows) drops the search.
    pub(super) fn sweep_grid_search(&mut self) {
        for t in self.tabs.iter_mut() {
            let f = &t.find;
            let bound = f.prompt.as_ref().map(|p| p.result).or(f.active.as_ref().map(|a| a.result));
            let Some(result) = bound else {
                if (f.job.is_some() || f.seek.is_some()) && shown_rows(t).is_none() {
                    t.find.stop_jobs();
                }
                continue;
            };
            if shown_rows(t).is_none_or(|rs| rs.id != result) {
                t.find.end();
            }
        }
    }

    /// The line under the grid of the active tab, while its search has something to show.
    pub fn grid_search_strip(&self) -> Option<Strip> {
        if self.tabs.is_empty() {
            return None;
        }
        let t = self.tab();
        let rs = shown_rows(t)?;
        let (mark, pattern, matches) = match (&t.find.prompt, &t.find.active) {
            (Some(p), _) => (p.forward, None, p.preview.as_ref()),
            (None, Some(a)) if a.result == rs.id => (a.forward, Some(a.re.as_str().to_string()), a.found.as_ref()),
            _ => return None,
        };
        let typed = t.find.prompt.as_ref().is_some_and(|p| !p.input.text().is_empty());
        let counting = t.find.job.is_some();
        let at = (!rs.columns.is_empty()).then(|| cell(t.grid.row, t.grid.col, rs.columns.len()));
        let count = match matches.filter(|m| m.result == rs.id) {
            _ if counting => "\u{2026}".to_string(),
            Some(m) => {
                let k = at
                    .and_then(|c| m.cells.binary_search(&c).ok())
                    .map_or("-".to_string(), |i| datarig_core::i18n::fmt_count(i as u64 + 1));
                let total = datarig_core::i18n::fmt_count(m.cells.len() as u64);
                format!("{k}/{total}{}", if m.capped { "+" } else { "" })
            }
            None => String::new(),
        };
        let mut notes = Vec::new();
        if !count.is_empty() && (typed || pattern.is_some()) && rs.more {
            // The rows the matches are of (rows may have come since).
            let read = matches.filter(|m| m.result == rs.id && !counting).map_or(rs.rows.len(), |m| m.rows);
            let key = self.next_page_key(Ctx::GridMatches);
            notes.push(self.i18n.msg(&Msg::GridSearchMore { count: read as u64, key }).to_string());
            notes.push(self.i18n.label(Label::GridSearchMoreShort).to_string());
        }
        let text = match (&pattern, &t.find.prompt) {
            (Some(p), _) => p.as_str(),
            (None, Some(p)) => p.input.text(),
            _ => "",
        };
        let text_width = crate::text::width(text);
        Some(Strip { mark: if mark { '/' } else { '?' }, pattern, text_width, count, notes })
    }

    /// The key of the next page as `ctx` resolves it (`Space r n` while `n` is the search's).
    pub(crate) fn next_page_key(&self, ctx: Ctx) -> String {
        self.keymap
            .hint_keys(Action::PageNext, ctx, self.enhanced_keys)
            .map(|k| crate::keymap::keys::label(&k))
            .unwrap_or_default()
    }

    /// The grid's key context as it is now: `grid.matches` while a search is on.
    pub(crate) fn grid_ctx(&self) -> Ctx {
        if self.grid_search_on() { Ctx::GridMatches } else { Ctx::Grid }
    }

    /// Draw the open prompt's input into `area`; where its cursor is, and whether the start of
    /// the pattern is scrolled out of `area`.
    pub(crate) fn draw_grid_prompt(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        style: ratatui::style::Style,
    ) -> Option<(u16, bool)> {
        let p = self.tabs.active_mut().find.prompt.as_mut()?;
        let cx = p.input.render(area, buf, style, true, false, None);
        Some((cx, p.input.scrolled()))
    }
}

#[cfg(test)]
mod tests;
