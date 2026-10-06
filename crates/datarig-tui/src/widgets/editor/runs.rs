//! Where runs are in the text: the statements of the run in progress (the one executing is
//! drawn apart) and, once a run ended, a hint after each statement's last line saying what it
//! did. The hints are not text: nothing yanks, saves, searches or formats them.
//!
//! Each statement is a [`Span`] of bytes that every change of the text moves
//! ([`Runs::adjust`], called by each splice): a change before it shifts it, a change after it
//! leaves it, and a change inside it (or text put right before its first character) marks it
//! edited. A change next to it or before it can also change what the statement is without
//! touching its bytes (text joined to a statement without `;`, a `;` deleted, a comment or a
//! string opened earlier): a span is checked against the text before its hint is drawn or
//! made ([`Editor::check_spans`]), and one that is no longer exactly a statement is edited
//! too. A span checked before changes only before it lexes the text from the first of them up
//! to its first token: it still starts the same statement when a `;` there is followed by
//! blanks and comments only, or when there is none and nothing changed before its previous
//! `;`. A hint goes with the first change of its statement; a run whose statement was edited
//! while it ran gets no hint for it (its answer is about text that is no longer there), and the
//! statement stays marked while it runs.
//!
//! A run is bound to the app's query id: the app stages the statements the user asked to run
//! ([`Editor::stage_run`]), binds them to the query id when it sends them
//! ([`Editor::start_run`]) and hands the outcomes back when that run ended
//! ([`Editor::finish_run`]). An editor replaced in its tab drops all of it.

use super::lexing::REGION_LINES;
use super::{Editor, Sel};
use datarig_core::sql::lexer::{Tok, lex};
use datarig_core::sql::split::split;
use std::collections::{HashMap, HashSet};

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
    /// It was found to be a statement of the text, and no change since could make it another
    /// (a change after its `;` cannot).
    checked: bool,
    /// Since it was checked, the text changed before it from this byte on (only before it).
    moved_from: Option<usize>,
    /// Where its statement's lead starts: the end of the `;` before it, 0 at the text's start,
    /// `usize::MAX` when not known (as of its last check, moved with the text).
    bound: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, closed: bool) -> Self {
        Self { start, end, closed, edited: false, checked: false, moved_from: None, bound: usize::MAX }
    }

    /// Bytes `a..b` were replaced with `ins` (`blank`: only blanks, put in without removing
    /// anything).
    fn adjust(&mut self, a: usize, b: usize, ins: usize, blank: bool) {
        let delta = ins as isize - (b - a) as isize;
        let shift = |x: usize| (x as isize + delta).max(0) as usize;
        if b < self.start || (b == self.start && (a < b || blank)) {
            // Before it; blanks typed right before its first character too (text there joins
            // it).
            self.start = shift(self.start);
            self.end = shift(self.end);
            // What comes before it may make it another statement (a `;`, a comment opened):
            // checked again from here.
            if self.checked {
                self.moved_from = Some(self.moved_from.map_or(a, |m| m.min(a)));
                self.bound = match self.bound {
                    usize::MAX => usize::MAX,
                    x if b <= x => shift(x),
                    x if a < x => usize::MAX,
                    x => x,
                };
            }
        } else if a > self.end || (a == self.end && (self.closed || blank)) {
            // After it. Right after a statement without `;` text goes on it, except blanks (a
            // new line below it); it is checked again. After its `;` nothing can change it:
            // the text is lexed from left to right.
            self.checked &= self.closed;
            if !self.checked {
                self.moved_from = None;
            }
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

    /// What was staged is not run (refused, dropped at the confirmation or while it waited):
    /// nothing a later run could take is left.
    pub fn unstage_run(&mut self) {
        self.runs.staged = None;
    }

    /// Statements staged for a run that was not sent yet.
    pub fn has_staged_run(&self) -> bool {
        self.runs.staged.is_some()
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
        // Only the statements that get a hint are checked, and only as many as are kept.
        if let Some((_, spans)) = self.runs.active.as_mut() {
            let mut kept = 0;
            for (s, h) in spans.iter_mut().zip(&hints).rev() {
                if h.is_none() || kept == MAX_HINTS {
                    s.edited = true;
                } else {
                    kept += 1;
                }
            }
        }
        self.check_spans(true, |_| false);
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

    /// Bytes the checks of run spans against the text lexed since the last call (the benchmark).
    pub fn take_check_work(&mut self) -> usize {
        std::mem::take(&mut self.check_work)
    }

    /// What the hint of statement `index` of run `query` says, if it has one.
    pub fn run_hint_kind(&self, query: u64, index: usize) -> Option<HintKind> {
        self.runs.hints.iter().find(|h| h.query == query && h.index == index).map(|h| h.hint.kind)
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

    /// The hints, oldest first, each checked against the text (tests, the benchmark).
    pub fn run_hints(&mut self) -> impl Iterator<Item = (&Span, &RunHint)> {
        self.check_spans(false, |_| true);
        self.runs.hints.iter().map(|h| (&h.span, &h.hint))
    }

    /// Whether span `s`, a statement when last checked, still starts that statement after
    /// changes only before it (from `moved_from` on): lexed from where the lexer's state is
    /// known on the line of the first change, up to and with the span's first token. Its own
    /// text is unchanged, so starting it is being it.
    fn still_starts_its_statement(&mut self, s: &Span) -> bool {
        let Some(from) = s.moved_from else { return false };
        // The line before the first change: its state is known lexing up to that change's line
        // only (never the lines after it, which may be the long statement itself).
        let r = self.pos_bytes(from).0.saturating_sub(1);
        self.ensure_states(r);
        let (line, byte) = self.restart_of(r);
        let r0 = self.line_start(line) + byte;
        let (sl, sb) = self.pos_bytes(s.start);
        if r0 > s.start || sb > self.lines[sl].len() {
            return false;
        }
        // Its first token, lexed alone (its text is unchanged): it must come out whole after
        // the text before it, or that text swallowed it (a comment or a string not closed).
        let rest = &self.lines[sl][sb..];
        let mut cut = rest.len().min(64);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let Some(head) = lex(&rest[..cut]).first().copied() else { return false };
        if head.is_trivia() || (head.end == cut && cut < rest.len()) {
            return false;
        }
        let head_text = &rest[..head.end];
        let mut text = self.slice((line, byte), (sl, sb));
        let at = text.len();
        text.push_str(head_text);
        self.check_work += text.len();
        let toks = lex(&text);
        if !toks.iter().any(|t| t.start == at && t.end == text.len() && t.kind == head.kind) {
            return false;
        }
        let before: Vec<_> = toks.iter().take_while(|t| t.end <= at).collect();
        let semi = before.iter().rposition(|t| t.kind == Tok::Semi);
        let lead = &before[semi.map_or(0, |i| i + 1)..];
        // After a `;` lexed here, or (none) after the one before it, which did not change.
        lead.iter().all(|t| t.is_trivia()) && (semi.is_some() || (s.bound != usize::MAX && s.bound <= r0))
    }

    /// Check the spans of the hints `pick` takes (and of the run in progress with `active`)
    /// that a change may have touched since they were last checked: one that is no longer a
    /// statement of the text is edited, and its hint goes. A span that is a statement's text
    /// without its `;` (a selection that stopped before it) holds, and takes the `;` from then
    /// on. The lines around them are split as the whole text would be: from where the lexer's
    /// state is known, a statement counted only past the first `;` (unless from the text's
    /// start) and up to the last `;` (unless to its end); more lines when a span is not inside
    /// that.
    pub(super) fn check_spans(&mut self, active: bool, pick: impl Fn(&Span) -> bool) {
        // Spans that only moved: checked up to their first token.
        let moved = |s: &Span| !s.edited && s.checked && s.moved_from.is_some();
        let mut quick: Vec<(usize, Span)> = Vec::new();
        for (i, h) in self.runs.hints.iter().enumerate() {
            if moved(&h.span) && pick(&h.span) {
                quick.push((i, h.span));
            }
        }
        let n_hints = self.runs.hints.len();
        if active && let Some((_, spans)) = &self.runs.active {
            quick.extend(spans.iter().enumerate().filter(|(_, s)| moved(s)).map(|(i, s)| (n_hints + i, *s)));
        }
        for (i, s) in quick {
            let holds = self.still_starts_its_statement(&s);
            let span = match self.runs.active.as_mut() {
                Some((_, spans)) if i >= n_hints => &mut spans[i - n_hints],
                _ => &mut self.runs.hints[i].span,
            };
            span.moved_from = None;
            span.checked = holds;
        }
        let unchecked = |s: &Span| !s.edited && !s.checked;
        let run = self.runs.active.iter().filter(|_| active).flat_map(|(_, s)| s.iter());
        let hinted = self.runs.hints.iter().map(|h| &h.span).filter(|s| pick(s));
        let mut todo: Vec<(usize, usize)> =
            hinted.chain(run).filter(|s| unchecked(s)).map(|s| (s.start, s.end)).collect();
        if todo.is_empty() {
            return;
        }
        let picked: HashSet<(usize, usize)> = todo.iter().copied().collect();
        let lo = todo.iter().map(|s| s.0).min().unwrap_or(0);
        let hi = todo.iter().map(|s| s.1).max().unwrap_or(0);
        let n = self.lines.len();
        let (lo_line, hi_line) = (self.pos_bytes(lo).0, self.pos_bytes(hi).0);
        let mut k = REGION_LINES;
        // The spans that hold, with the end and `;` of their statement.
        let mut holds: HashMap<(usize, usize), (usize, bool, usize)> = HashMap::new();
        loop {
            let (first, last) = (lo_line.saturating_sub(k), (hi_line + k + 1).min(n));
            let (base, region) = self.region_text(first, last);
            self.check_work += region.len();
            let toks = lex(&region);
            let mut semis = toks.iter().filter(|t| t.kind == Tok::Semi);
            let from = if base == 0 { 0 } else { semis.next().map_or(usize::MAX, |t| base + t.end) };
            let to = if last == n {
                usize::MAX
            } else {
                toks.iter().rfind(|t| t.kind == Tok::Semi).map_or(0, |t| base + t.end)
            };
            // Each statement by its start: its body's end and its end; and their ends in order.
            let split = split(&region);
            let stmts: HashMap<usize, (usize, usize)> =
                split.iter().map(|st| (base + st.start, (base + st.body_end, base + st.end))).collect();
            let ends: Vec<usize> = split.iter().map(|st| base + st.end).collect();
            let whole = base == 0 && last == n;
            todo.retain(|&(a, b)| {
                if a >= from && b <= to {
                    match stmts.get(&a) {
                        Some(&(body, end)) if b == end || b == body => {
                            // The `;` before it: the end of the statement before it, or the
                            // text's start.
                            let before = ends.partition_point(|&e| e <= a);
                            let bound = match before {
                                0 if base == 0 => 0,
                                0 => usize::MAX,
                                i => ends[i - 1],
                            };
                            holds.insert((a, b), (end, end > body, bound));
                        }
                        _ => {}
                    }
                    false
                } else {
                    !whole
                }
            });
            if todo.is_empty() {
                break;
            }
            k = k.saturating_mul(4);
        }
        let hints = self.runs.hints.iter_mut().map(|h| &mut h.span);
        for s in hints.chain(self.runs.active.iter_mut().flat_map(|(_, s)| s.iter_mut())) {
            if !unchecked(s) || !picked.contains(&(s.start, s.end)) {
                continue;
            }
            match holds.get(&(s.start, s.end)) {
                Some(&(end, closed, bound)) => {
                    (s.end, s.closed, s.checked, s.bound, s.moved_from) = (end, closed, true, bound, None);
                }
                None => s.edited = true,
            }
        }
        self.runs.hints.retain(|h| !h.span.edited);
    }
}

#[cfg(test)]
mod tests;
