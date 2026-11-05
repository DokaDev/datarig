//! The summary of the grid's selected range on the status line: its size, then for numbers
//! (every selected column an integer, decimal or floating-point one) their sum, average, least
//! and greatest, else how many distinct values; and how many are NULL. While the result grid
//! has the focus and a range, rows or columns are selected, it takes the message's place.
//!
//! The values summed are those the grid shows: a pending change or what an Apply wrote over the
//! value as read, as a copy takes them; a cell whose value is not known yet (a pending DEFAULT, a
//! row the Apply changed that is read again later) is counted apart. Only rows read are summed:
//! new rows of the selection are left out, and whole columns are every fetched row (said when
//! the server has more).
//!
//! The rows are read on a worker, never on the UI thread: a job is bound to its tab, its result,
//! the selection and the values over it; a job whose selection changed is told to stop and its
//! answer is dropped. Integers and decimals are summed exactly (`i128` with a scale); past that
//! range, or with a floating-point value such as `NaN` or `1e300`, as `f64`.

use super::*;
use datarig_core::driver::ValueKind;
use datarig_core::fault::Fault;
use datarig_core::results::RowsSnapshot;
use std::cmp::Ordering as Cmp;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

/// The most distinct values counted; past them the count says `100,000+`.
pub const DISTINCT_CAP: usize = 100_000;

/// Rows read at a time by a job (it checks whether to stop between them).
const CHUNK: usize = 1024;

/// The values the grid shows over selected cells, by (row, column): `None` when the cell has no
/// value yet, `Some(None)` for NULL.
pub type Over = BTreeMap<(usize, usize), Option<Option<String>>>;

/// What a summary is of: the tab, its result, the rows read and the columns selected, and the
/// values over them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    tab: TabId,
    result: u64,
    rows: Range<usize>,
    cols: Range<usize>,
    over: Over,
    /// The selection has new rows too (left out).
    new_rows: bool,
    /// Whole columns of a result the server has more rows of.
    fetched_only: bool,
}

/// What the numbers or the values of a selection come to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Figures {
    /// As written (grouped by thousands, as read for least and greatest).
    Numbers {
        sum: String,
        avg: String,
        min: String,
        max: String,
    },
    Distinct {
        count: usize,
        capped: bool,
    },
}

/// A selection summed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// `None`: no value but NULL (or none known).
    pub figures: Option<Figures>,
    pub nulls: u64,
    /// Cells whose value is not known yet.
    pub unknown: u64,
}

enum State {
    Working {
        job: u64,
        stop: Arc<AtomicBool>,
    },
    Done(Summary),
    /// The rows could not be read (said once).
    Failed,
}

/// The app's side of the summary.
#[derive(Default)]
pub struct GridSummary {
    key: Option<Key>,
    state: Option<State>,
    next: u64,
    /// Jobs started since the app started (tests count them).
    pub jobs: u64,
}

/// A job's answer.
#[derive(Debug)]
pub struct SummaryAnswer {
    pub job: u64,
    pub result: Result<Option<Summary>, Fault>,
}

/// A job's way back: an answer is sent even when the job fails inside.
struct Reply {
    out: super::grid_search::Out,
    job: u64,
    sent: bool,
}

impl Reply {
    fn send(mut self, result: Result<Option<Summary>, Fault>) {
        self.sent = true;
        self.out.event(AppEvent::GridSummary(SummaryAnswer { job: self.job, result }));
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.sent {
            let result = Err(Fault::other("the summary stopped inside"));
            self.out.event(AppEvent::GridSummary(SummaryAnswer { job: self.job, result }));
        }
    }
}

/// An exact decimal: `m` × 10^-`s`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dec {
    m: i128,
    s: u32,
}

/// The most digits of a decimal kept exactly (an `i128` holds 38).
const MAX_DIGITS: usize = 36;

impl Dec {
    /// `[-+]digits[.digits]`; `None` for anything else (an exponent, `NaN`, a currency sign) or
    /// more than [`MAX_DIGITS`] digits.
    fn parse(t: &str) -> Option<Dec> {
        let t = t.trim();
        let (neg, d) = match t.as_bytes().first()? {
            b'-' => (true, &t[1..]),
            b'+' => (false, &t[1..]),
            _ => (false, t),
        };
        let (int, frac) = d.split_once('.').unwrap_or((d, ""));
        if int.is_empty() && frac.is_empty() || int.len() + frac.len() > MAX_DIGITS {
            return None;
        }
        if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits = format!("{int}{frac}");
        let m: i128 = if digits.is_empty() { 0 } else { digits.parse().ok()? };
        Some(Dec { m: if neg { -m } else { m }, s: frac.len() as u32 })
    }

    /// `m` at scale `s` (at least its own).
    fn at(self, s: u32) -> Option<i128> {
        self.m.checked_mul(10i128.checked_pow(s.checked_sub(self.s)?)?)
    }

    fn add(self, o: Dec) -> Option<Dec> {
        let s = self.s.max(o.s);
        Some(Dec { m: self.at(s)?.checked_add(o.at(s)?)?, s })
    }

    fn cmp(self, o: Dec) -> Option<Cmp> {
        let s = self.s.max(o.s);
        Some(self.at(s)?.cmp(&o.at(s)?))
    }

    /// Divided by `n`, rounded half away from zero at scale `s` (at least its own).
    fn div(self, n: u64, s: u32) -> Option<Dec> {
        let m = self.at(s)?;
        let n = i128::from(n);
        let (q, r) = (m / n, m % n);
        let q = if r.abs() * 2 >= n { q + m.signum() } else { q };
        Some(Dec { m: q, s })
    }

    /// Grouped by thousands; `trim`: without the zeros at the end of the fraction.
    fn text(self, trim: bool) -> String {
        let digits = self.m.unsigned_abs().to_string();
        let s = self.s as usize;
        let digits = if digits.len() <= s { format!("{}{digits}", "0".repeat(s + 1 - digits.len())) } else { digits };
        let (int, frac) = digits.split_at(digits.len() - s);
        let frac = if trim { frac.trim_end_matches('0') } else { frac };
        let sign = if self.m < 0 { "-" } else { "" };
        let int = group(int);
        if frac.is_empty() { format!("{sign}{int}") } else { format!("{sign}{int}.{frac}") }
    }
}

/// Digits grouped by thousands with commas.
fn group(int: &str) -> String {
    let mut out = String::with_capacity(int.len() + int.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A floating-point number as the summary writes it: grouped digits in a plain range, else in
/// scientific notation.
fn float_text(x: f64) -> String {
    if !x.is_finite() || x == 0.0 || (1e-4..1e15).contains(&x.abs()) {
        let t = format!("{x}");
        return match t.split_once('.') {
            Some((int, frac)) => format!("{}.{frac}", sign_group(int)),
            None => sign_group(&t),
        };
    }
    format!("{x:e}")
}

fn sign_group(int: &str) -> String {
    match int.strip_prefix('-') {
        Some(d) if d.bytes().all(|b| b.is_ascii_digit()) => format!("-{}", group(d)),
        _ if int.bytes().all(|b| b.is_ascii_digit()) => group(int),
        _ => int.to_string(),
    }
}

/// A number of the selection: exact when it is a plain decimal.
#[derive(Clone, Copy)]
struct Num {
    dec: Option<Dec>,
    f: f64,
}

impl Num {
    fn parse(t: &str) -> Option<Num> {
        match Dec::parse(t) {
            Some(d) => Some(Num { dec: Some(d), f: t.trim().parse().unwrap_or(f64::NAN) }),
            None => t.trim().parse::<f64>().ok().map(|f| Num { dec: None, f }),
        }
    }

    fn cmp(self, o: Num) -> Cmp {
        match (self.dec, o.dec) {
            (Some(a), Some(b)) => a.cmp(b).unwrap_or_else(|| self.f.total_cmp(&o.f)),
            _ => self.f.total_cmp(&o.f),
        }
    }
}

/// What a job counts as it reads.
struct Tally {
    numeric: bool,
    count: u64,
    exact: Option<Dec>,
    float: f64,
    min: Option<(Num, String)>,
    max: Option<(Num, String)>,
    distinct: HashSet<String>,
    capped: bool,
    nulls: u64,
    unknown: u64,
}

impl Tally {
    fn new(numeric: bool) -> Self {
        Tally {
            numeric,
            count: 0,
            exact: Some(Dec { m: 0, s: 0 }),
            float: 0.0,
            min: None,
            max: None,
            distinct: HashSet::new(),
            capped: false,
            nulls: 0,
            unknown: 0,
        }
    }

    /// One cell as the grid shows it (`None`: no value yet; `Some(None)`: NULL).
    fn take(&mut self, v: Option<Option<&str>>) {
        let v = match v {
            None => return self.unknown += 1,
            Some(None) => return self.nulls += 1,
            Some(Some(v)) => v,
        };
        if !self.capped && !self.distinct.contains(v) {
            if self.distinct.len() < DISTINCT_CAP {
                self.distinct.insert(v.to_string());
            } else {
                self.capped = true;
            }
        }
        if !self.numeric {
            return;
        }
        // A value of a numeric column that is no number (a currency sign): distinct values then.
        let Some(n) = Num::parse(v) else {
            self.numeric = false;
            return;
        };
        self.count += 1;
        self.float += n.f;
        self.exact = match (self.exact, n.dec) {
            (Some(a), Some(b)) => a.add(b),
            _ => None,
        };
        if self.min.as_ref().is_none_or(|(m, _)| n.cmp(*m) == Cmp::Less) {
            self.min = Some((n, v.trim().to_string()));
        }
        if self.max.as_ref().is_none_or(|(m, _)| n.cmp(*m) == Cmp::Greater) {
            self.max = Some((n, v.trim().to_string()));
        }
    }

    fn finish(self) -> Summary {
        let figures = if self.numeric {
            let (min, max) = (self.min.map(|m| m.1), self.max.map(|m| m.1));
            match (min, max) {
                (Some(min), Some(max)) => {
                    let (sum, avg) = match self.exact {
                        Some(e) => {
                            let avg = e.div(self.count, (e.s + 4).min(30)).map(|a| a.text(true));
                            (e.text(false), avg.unwrap_or_else(|| float_text(self.float / self.count as f64)))
                        }
                        None => (float_text(self.float), float_text(self.float / self.count as f64)),
                    };
                    Some(Figures::Numbers { sum, avg, min: num_text(&min), max: num_text(&max) })
                }
                _ => None,
            }
        } else if self.distinct.is_empty() {
            None
        } else {
            Some(Figures::Distinct { count: self.distinct.len(), capped: self.capped })
        };
        Summary { figures, nulls: self.nulls, unknown: self.unknown }
    }
}

/// A value read as the summary writes it: a plain decimal grouped by thousands, anything else as
/// it is.
fn num_text(v: &str) -> String {
    match Dec::parse(v) {
        Some(d) => d.text(false),
        None => v.to_string(),
    }
}

/// Whether a column of kind `k` holds numbers the summary adds up.
fn numeric(k: ValueKind) -> bool {
    matches!(k, ValueKind::Integer | ValueKind::Decimal | ValueKind::Float)
}

/// Sum rows `rows` (numbered from the snapshot's first row, `first` of the result) of columns
/// `cols` of `snap`, with `over` over them. `Ok(None)` when `stop` was set.
pub fn summarize(
    snap: &RowsSnapshot,
    first: usize,
    cols: &[usize],
    numbers: bool,
    over: &Over,
    stop: &AtomicBool,
) -> Result<Option<Summary>, Fault> {
    let mut tally = Tally::new(numbers);
    let mut stopped = false;
    snap.for_each_chunk_in(0..snap.len(), CHUNK, |at, chunk| {
        if stop.load(Ordering::Relaxed) {
            stopped = true;
            return false;
        }
        for (i, r) in chunk.iter().enumerate() {
            for &c in cols {
                let v = match over.get(&(first + at + i, c)) {
                    Some(o) => o.as_ref().map(|v| v.as_deref()),
                    None => Some(r.get(c).and_then(|v| v.as_deref())),
                };
                tally.take(v);
            }
        }
        true
    })?;
    Ok((!stopped).then(|| tally.finish()))
}

impl App {
    /// What the summary would be of now: the active tab's selected range of rows read, while
    /// the grid shows them and has the focus.
    fn summary_key(&self) -> Option<Key> {
        if self.tabs.is_empty() || !matches!(self.focus, Focus::Results | Focus::Inspector) {
            return None;
        }
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return None };
        if t.exec.view != super::tabs::ResultView::Rows || self.plan_shown() || self.chart_shown() {
            return None;
        }
        let (len, ncols) = (rs.rows.len(), rs.columns.len());
        let (sel_rows, cols) = t.grid.selection(len, ncols)?;
        let rows = sel_rows.start.min(len)..sel_rows.end.min(len);
        if rows.is_empty() || cols.is_empty() {
            return None;
        }
        let mut over = Over::new();
        let mut put = |r: usize, c: usize| {
            if rows.contains(&r) && cols.contains(&c) {
                let v = rs.over(r, c).map(|o| o.cell().map(|v| v.map(str::to_string)));
                if let Some(v) = v {
                    over.insert((r, c), v);
                }
            }
        };
        for (r, e) in rs.edit.batch.rows() {
            e.set.keys().for_each(|&c| put(r, c));
        }
        if let Some(a) = rs.edit.applied.as_deref() {
            a.set.iter().for_each(|(&r, m)| m.keys().for_each(|&c| put(r, c)));
            a.rerun.iter().for_each(|&(r, c)| put(r, c));
        }
        let more = rs.more || t.exec.paging.closed() || t.exec.paging == Paging::Stopped;
        Some(Key {
            tab: t.id,
            result: rs.id,
            new_rows: sel_rows.end > len,
            fetched_only: t.grid.shape == crate::widgets::grid::Shape::Cols && more,
            rows,
            cols,
            over,
        })
    }

    /// After an input or an event: a summary for the selection as it is now, a job started
    /// when it changed (the one before told to stop).
    pub(super) fn sync_grid_summary(&mut self) {
        let key = self.summary_key();
        if key == self.grid_summary.key {
            return;
        }
        if let Some(State::Working { stop, .. }) = &self.grid_summary.state {
            stop.store(true, Ordering::Relaxed);
        }
        self.grid_summary.state = None;
        self.grid_summary.key = key.clone();
        let Some(key) = key else { return };
        let Results::Rows(rs) = &self.tab().results else { return };
        let numbers = key.cols.clone().all(|c| numeric(rs.columns[c].meta.kind));
        let snap = match rs.rows.snapshot(key.rows.clone()) {
            Ok(s) => s,
            Err(f) => {
                self.grid_summary.state = Some(State::Failed);
                return self.results_read_failed(&f);
            }
        };
        self.grid_summary.next += 1;
        self.grid_summary.jobs += 1;
        let job = self.grid_summary.next;
        let stop = Arc::new(AtomicBool::new(false));
        self.grid_summary.state = Some(State::Working { job, stop: stop.clone() });
        let out = self.grid_job_out();
        let reply = Reply { out, job, sent: false };
        let (first, cols, over): (usize, Vec<usize>, Over) = (key.rows.start, key.cols.collect(), key.over);
        std::thread::spawn(move || {
            let r = summarize(&snap, first, &cols, numbers, &over, &stop);
            reply.send(r);
        });
    }

    /// A job answered: kept while the selection is the one it was started for.
    pub(super) fn on_grid_summary(&mut self, a: SummaryAnswer) {
        let current = matches!(&self.grid_summary.state, Some(State::Working { job, .. }) if *job == a.job);
        if !current {
            return;
        }
        self.grid_summary.state = match a.result {
            Ok(Some(s)) => Some(State::Done(s)),
            // Told to stop: another job took its place already.
            Ok(None) => return,
            Err(f) => {
                self.grid_summary.state = Some(State::Failed);
                return self.results_read_failed(&f);
            }
        };
    }

    /// The summary of the selection for the status line, while there is one.
    pub fn selection_summary(&self) -> Option<String> {
        self.selection_summary_forms().into_iter().next()
    }

    /// The summary of the selection, longest first: each shorter form leaves out the parts that
    /// matter least (the least and greatest values, then NULL, then the average, then the notes),
    /// down to its size and its sum or count. None while there is no selection to sum up.
    pub fn selection_summary_forms(&self) -> Vec<String> {
        let Some(key) = self.grid_summary.key.as_ref() else { return Vec::new() };
        let Some(state) = self.grid_summary.state.as_ref() else { return Vec::new() };
        let msg = |m: Msg| self.i18n.msg(&m).to_string();
        let label = |l: Label| self.i18n.label(l).to_string();
        // (how much it matters: 0 most, its text)
        let mut parts: Vec<(u8, String)> = vec![(
            0,
            format!(
                "{} \u{d7} {}",
                msg(Msg::GridSummaryRows { count: key.rows.len() as u64 }),
                msg(Msg::GridSummaryCols { count: key.cols.len() as u64 })
            ),
        )];
        match state {
            State::Working { .. } => parts.push((1, label(Label::GridSummaryWorking))),
            State::Failed => {}
            State::Done(s) => {
                match &s.figures {
                    Some(Figures::Numbers { sum, avg, min, max }) => {
                        parts.push((1, msg(Msg::GridSummarySum { value: sum.clone() })));
                        parts.push((3, msg(Msg::GridSummaryAvg { value: avg.clone() })));
                        parts.push((5, msg(Msg::GridSummaryMin { value: min.clone() })));
                        parts.push((5, msg(Msg::GridSummaryMax { value: max.clone() })));
                    }
                    Some(Figures::Distinct { count, capped: false }) => {
                        parts.push((1, msg(Msg::GridSummaryDistinct { count: *count as u64 })))
                    }
                    Some(Figures::Distinct { count, capped: true }) => {
                        parts.push((1, msg(Msg::GridSummaryDistinctMore { count: *count as u64 })))
                    }
                    None => {}
                }
                parts.push((4, msg(Msg::GridSummaryNulls { count: s.nulls })));
                if s.unknown > 0 {
                    parts.push((4, msg(Msg::GridSummaryUnknown { count: s.unknown })));
                }
            }
        }
        if key.new_rows {
            parts.push((2, label(Label::GridSummaryNewRows)));
        }
        if key.fetched_only {
            parts.push((2, label(Label::GridSummaryFetched)));
        }
        let form = |most: u8| parts.iter().filter(|(p, _)| *p <= most).map(|(_, t)| t.as_str()).collect::<Vec<_>>();
        let mut forms: Vec<String> = Vec::new();
        for most in [5, 4, 3, 2, 1] {
            let f = form(most).join(" \u{b7} ");
            if forms.last() != Some(&f) {
                forms.push(f);
            }
        }
        forms
    }

    /// Summary jobs started since the app started.
    pub fn grid_summary_jobs(&self) -> u64 {
        self.grid_summary.jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(numeric: bool, values: &[Option<&str>]) -> Summary {
        let mut t = Tally::new(numeric);
        values.iter().for_each(|v| t.take(Some(*v)));
        t.finish()
    }

    fn numbers(s: &Summary) -> (String, String, String, String) {
        match &s.figures {
            Some(Figures::Numbers { sum, avg, min, max }) => (sum.clone(), avg.clone(), min.clone(), max.clone()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn integers_and_decimals_add_up_exactly() {
        let s = tally(true, &[Some("1234567"), Some("1"), None, Some("-2")]);
        assert_eq!(numbers(&s), ("1,234,566".into(), "411,522".into(), "-2".into(), "1,234,567".into()));
        assert_eq!(s.nulls, 1);
        let s = tally(true, &[Some("0.1"), Some("0.2"), Some("10.25")]);
        assert_eq!(numbers(&s).0, "10.55", "no floating-point error");
        assert_eq!(numbers(&s).1, "3.516667", "rounded at four more places, zeros trimmed");
        let s = tally(true, &[Some("1"), Some("2")]);
        assert_eq!(numbers(&s).1, "1.5");
        let big = "9".repeat(36);
        let s = tally(true, &[Some(&big), Some("0.0001")]);
        assert_eq!(numbers(&s).0, "1e36", "past what an i128 holds at that scale: as f64");
    }

    #[test]
    fn floats_nan_and_exponents_fall_back_to_f64() {
        let s = tally(true, &[Some("1e3"), Some("2.5")]);
        assert_eq!(numbers(&s), ("1,002.5".into(), "501.25".into(), "2.5".into(), "1e3".into()));
        let s = tally(true, &[Some("NaN"), Some("1")]);
        assert_eq!(numbers(&s).0, "NaN");
        assert_eq!(float_text(1e300), "1e300");
        assert_eq!(float_text(-1234.5), "-1,234.5");
    }

    #[test]
    fn values_that_are_no_numbers_count_distinct_values() {
        let s = tally(true, &[Some("$1.00"), Some("$1.00"), Some("$2.00"), None]);
        assert_eq!(s.figures, Some(Figures::Distinct { count: 2, capped: false }));
        assert_eq!(s.nulls, 1);
        let s = tally(false, &[Some("a"), Some("b"), Some("a")]);
        assert_eq!(s.figures, Some(Figures::Distinct { count: 2, capped: false }));
        let s = tally(true, &[None, None]);
        assert_eq!((s.figures, s.nulls), (None, 2), "only NULL: no figures");
        let mut t = Tally::new(false);
        t.take(None);
        assert_eq!(t.finish().unknown, 1, "a value not known yet is counted apart");
    }

    #[test]
    fn decimals_parse_strictly() {
        assert_eq!(Dec::parse(" -12.50 "), Some(Dec { m: -1250, s: 2 }));
        assert_eq!(Dec::parse(".5"), Some(Dec { m: 5, s: 1 }));
        assert_eq!(Dec::parse("5."), Some(Dec { m: 5, s: 0 }));
        for bad in ["", "-", ".", "1e5", "1,000", "$1", "0x10", "1.2.3", "--1"] {
            assert_eq!(Dec::parse(bad), None, "{bad}");
        }
        assert_eq!(Dec { m: -1250, s: 2 }.text(false), "-12.50");
        assert_eq!(Dec { m: 5, s: 3 }.text(false), "0.005");
        assert_eq!(Dec { m: 1_000_000, s: 0 }.text(true), "1,000,000");
        assert_eq!(Dec { m: -7, s: 0 }.div(2, 1), Some(Dec { m: -35, s: 1 }));
        assert_eq!(Dec { m: -5, s: 0 }.div(3, 0), Some(Dec { m: -2, s: 0 }), "half away from zero");
    }

    #[test]
    fn a_job_reads_the_shown_values_and_stops_when_told() {
        let rows: Vec<Vec<Option<String>>> =
            (0..3000).map(|i| vec![Some(i.to_string()), Some(format!("t{}", i % 7))]).collect();
        let store = datarig_core::results::RowStore::in_memory(rows);
        let snap = store.snapshot(10..20).unwrap();
        let mut over = Over::new();
        over.insert((10, 0), Some(Some("1000".to_string())));
        over.insert((11, 0), Some(None));
        over.insert((12, 0), None);
        let s = summarize(&snap, 10, &[0], true, &over, &AtomicBool::new(false)).unwrap().unwrap();
        // 13..19 as read (112), 1000 pending over 10; 11 NULL; 12 not known yet.
        assert_eq!(numbers(&s).0, "1,112");
        assert_eq!((s.nulls, s.unknown), (1, 1));
        let all = store.snapshot(0..3000).unwrap();
        let s = summarize(&all, 0, &[1], false, &Over::new(), &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(s.figures, Some(Figures::Distinct { count: 7, capped: false }));
        assert_eq!(summarize(&all, 0, &[0], true, &Over::new(), &AtomicBool::new(true)).unwrap(), None);
    }
}
