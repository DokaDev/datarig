//! Where runs are in the text: the statements of the run in progress (the one executing is
//! drawn apart) and, once a run ended, a hint after each statement's last line saying what it
//! did. The hints are not text: nothing yanks, saves, searches or formats them.
//!
//! Each statement is a [`Span`] of bytes that every change of the text moves
//! ([`Runs::adjust`], called by each splice): a change before it shifts it, a change after it
//! leaves it, and a change inside it marks it edited. A hint goes with the first change of its
//! statement's text; a run whose statement was edited while it ran gets no hint for it (its
//! answer is about text that is no longer there), and the statement stays marked while it runs.
//!
//! A run is bound to the app's query id: the app stages the statements the user asked to run
//! ([`Editor::stage_run`]), binds them to the query id when it sends them
//! ([`Editor::start_run`]) and hands the outcomes back when that run ended
//! ([`Editor::finish_run`]). An editor replaced in its tab drops all of it.

use super::{Editor, Sel};
use datarig_core::sql::split::split;

/// Hints kept at most; past it the oldest go.
const MAX_HINTS: usize = 256;

/// The bytes of one statement of a run in the text, as edits move them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    /// The end of its `;`, or of its last token without one.
    pub end: usize,
    /// It ends with `;`: text typed right after it is not part of it.
    closed: bool,
    /// Its own text changed since the run took it.
    edited: bool,
}

impl Span {
    pub fn new(start: usize, end: usize, closed: bool) -> Self {
        Self { start, end, closed, edited: false }
    }

    /// Bytes `a..b` were replaced with `ins` (`blank`: only blanks, put in without removing
    /// anything).
    fn adjust(&mut self, a: usize, b: usize, ins: usize, blank: bool) {
        let delta = ins as isize - (b - a) as isize;
        let shift = |x: usize| (x as isize + delta).max(0) as usize;
        if b <= self.start {
            // Before it (text typed right before its first character included).
            self.start = shift(self.start);
            self.end = shift(self.end);
        } else if a > self.end || (a == self.end && (self.closed || blank)) {
            // After it. Right after a statement without `;` text goes on it, except blanks (a
            // new line below it).
        } else {
            self.edited = true;
            self.start = self.start.min(a);
            self.end = if b >= self.end { a + ins } else { shift(self.end) };
        }
    }

    fn overlaps(&self, o: &Span) -> bool {
        self.start < o.end && o.start < self.end || self.start == o.start
    }
}

/// What a statement's run did, for the mark before its hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Ok,
    Failed,
    RolledBack,
    Cancelled,
}

impl HintKind {
    /// Nerd Font glyphs (nf-fa-check, nf-fa-xmark, nf-fa-undo, nf-fa-ban) with icons on, text
    /// marks with icons off; never an emoji.
    pub fn mark(self, icons: bool) -> &'static str {
        match (self, icons) {
            (HintKind::Ok, true) => CHECK,
            (HintKind::Failed, true) => XMARK,
            (HintKind::RolledBack, true) => UNDO,
            (HintKind::Cancelled, true) => BAN,
            (HintKind::Ok, false) => "\u{2713}",
            (HintKind::Failed, false) => "\u{2717}",
            (HintKind::RolledBack, false) => "\u{21ba}",
            (HintKind::Cancelled, false) => "\u{2298}",
        }
    }
}

/// nf-fa-check.
pub const CHECK: &str = "\u{f00c}";
/// nf-fa-xmark.
pub const XMARK: &str = "\u{f00d}";
/// nf-fa-undo.
pub const UNDO: &str = "\u{f0e2}";
/// nf-fa-ban.
pub const BAN: &str = "\u{f05e}";

/// What a statement's last run did, as its hint says it (the text after the mark).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunHint {
    pub kind: HintKind,
    pub text: String,
}

/// A statement's hint: where the statement is, what it says, and the run and statement it came
/// from.
#[derive(Clone, Debug)]
pub(super) struct Hinted {
    pub(super) span: Span,
    pub(super) hint: RunHint,
    query: u64,
    index: usize,
}

/// The runs of an editor's text.
#[derive(Clone, Debug, Default)]
pub(super) struct Runs {
    /// What the user asked to run, until the app sends it: the statements and their spans.
    staged: Option<(Vec<String>, Vec<Span>)>,
    /// The run in progress: its query id and its statements' spans.
    active: Option<(u64, Vec<Span>)>,
    /// The hints of statements whose run ended, oldest first.
    pub(super) hints: Vec<Hinted>,
    /// The query id of the last run that ended.
    last: Option<u64>,
}

impl Runs {
    /// Bytes `a..b` of the text were replaced with `ins`: the spans follow, and a hint whose
    /// statement changed goes.
    pub(super) fn adjust(&mut self, a: usize, b: usize, ins: &str) {
        let blank = a == b && ins.chars().all(char::is_whitespace);
        let ins = ins.len();
        let spans = self.staged.iter_mut().flat_map(|(_, s)| s.iter_mut());
        for s in spans.chain(self.active.iter_mut().flat_map(|(_, s)| s.iter_mut())) {
            s.adjust(a, b, ins, blank);
        }
        if self.hints.is_empty() {
            return;
        }
        for h in &mut self.hints {
            h.span.adjust(a, b, ins, blank);
        }
        self.hints.retain(|h| !h.span.edited);
    }
}

impl Editor {
    /// What a run takes (`Ctrl+E`): the statements of the Visual selection (which ends), else
    /// the statement under the cursor; each with its place in the text. A block's pieces are
    /// not one stretch of the text: they have no places.
    pub fn run_statements(&mut self) -> (Vec<String>, Vec<Span>) {
        let Some(sel) = self.selection() else {
            return match self.current_statement() {
                Some((a, b, body)) => {
                    let closed = a + body.len() < b;
                    (vec![body], vec![Span::new(a, b, closed)])
                }
                None => (Vec::new(), Vec::new()),
            };
        };
        let base = (self.sel != Sel::Block).then(|| self.visual_bounds().0);
        self.exit_visual();
        let stmts = split(&sel);
        let spans = match base {
            Some(base) => stmts.iter().map(|s| Span::new(base + s.start, base + s.end, s.end > s.body_end)).collect(),
            None => Vec::new(),
        };
        (stmts.iter().map(|s| s.body(&sel).to_string()).collect(), spans)
    }

    /// The user asked to run `statements`, found at `spans` (none, or one each): kept until the
    /// app sends them ([`Editor::start_run`]).
    pub fn stage_run(&mut self, statements: &[String], spans: Vec<Span>) {
        self.runs.staged = (spans.len() == statements.len()).then(|| (statements.to_vec(), spans));
    }

    /// The app sends `statements` as run `query`. When they are what was staged, the run is
    /// marked on their text and their old hints go; a run of anything else is not marked.
    pub fn start_run(&mut self, query: u64, statements: &[String]) {
        self.runs.active = None;
        let Some((staged, spans)) = self.runs.staged.take() else { return };
        if staged != statements {
            return;
        }
        self.runs.hints.retain(|h| !spans.iter().any(|s| s.overlaps(&h.span)));
        self.runs.active = Some((query, spans));
    }

    /// The query id of the run marked on the text, while it runs.
    pub fn active_run(&self) -> Option<u64> {
        self.runs.active.as_ref().map(|(q, _)| *q)
    }

    /// Run `query` ended; `hints` are what each of its statements did (`None`: nothing to say,
    /// it did not run). A statement edited since gets none. Anything but the marked run is
    /// ignored.
    pub fn finish_run(&mut self, query: u64, hints: Vec<Option<RunHint>>) {
        if self.active_run() != Some(query) {
            return;
        }
        let Some((_, spans)) = self.runs.active.take() else { return };
        for (index, (span, hint)) in spans.into_iter().zip(hints).enumerate() {
            if let Some(hint) = hint.filter(|_| !span.edited) {
                self.runs.hints.push(Hinted { span, hint, query, index });
            }
        }
        self.runs.last = Some(query);
        let over = self.runs.hints.len().saturating_sub(MAX_HINTS);
        self.runs.hints.drain(..over);
    }

    /// The query id of the last run whose statements got their hints.
    pub fn last_run(&self) -> Option<u64> {
        self.runs.last
    }

    /// What statement `index` of run `query` did, as the app learned after the run ended (the
    /// driver says a transaction rolled back after the statement's answer): its hint, if it
    /// still has one, says that now.
    pub fn amend_run_hint(&mut self, query: u64, index: usize, hint: RunHint) {
        if let Some(h) = self.runs.hints.iter_mut().find(|h| h.query == query && h.index == index) {
            h.hint = hint;
        }
    }

    /// The statement of the marked run that runs now (its index), and the spinner frame its
    /// gutter shows; `None` when nothing runs. Set by the app before each render.
    pub fn set_running(&mut self, running: Option<(usize, &'static str)>) {
        self.running = running;
    }

    /// The span of the marked run's statement `i`.
    pub(super) fn running_span(&self) -> Option<(Span, &'static str)> {
        let (i, frame) = self.running?;
        let (_, spans) = self.runs.active.as_ref()?;
        spans.get(i).map(|s| (*s, frame))
    }

    /// The hints, oldest first (tests).
    pub fn run_hints(&self) -> impl Iterator<Item = (&Span, &RunHint)> {
        self.runs.hints.iter().map(|h| (&h.span, &h.hint))
    }
}

#[cfg(test)]
mod tests;
