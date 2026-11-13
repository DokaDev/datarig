//! A table tab's query bar ([`crate::widgets::qbar`] lays it out and draws it): what it says,
//! built from the tab's query as applied and the rows shown; its keys (`Space r w` puts the keys
//! on its conditions, `Space r l` types the limit in it); and its clicks. Every change it makes
//! goes through the tab's query as the filter dialog's do ([`Draft`], [`App::table_run`]): a
//! condition taken out leaves the others' text as it was sent.

use super::filter_dialog::DialogFocus;
use super::filter_dialog::draft::{Draft, Where};
use super::paging::Paging;
use super::table_query::{Unapplied, filter_text};
use super::tabs::ResultView;
use super::*;
pub use crate::widgets::qbar::BarHit;
use crate::widgets::qbar::{Bar, ChipM, ColsM, Glyphs, LimitM, RowsM, StateM};
use datarig_core::query::KeyOrigin;
use datarig_i18n::fmt_count;
use ratatui::layout::Position;

/// What the keys of the bar are on, and for which tab and view: they leave the bar when any of
/// it changes (another tab, result or view, the focus elsewhere, a dialog opened).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BarOwner {
    tab: TabId,
    shown: Option<usize>,
    view: ResultView,
}

/// The keys on a part of the `WHERE` segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BarFocus {
    on: BarOwner,
    pub(crate) hit: BarHit,
}

/// The `LIMIT` field typed in.
#[derive(Clone, Debug)]
pub(crate) struct LimitEdit {
    on: BarOwner,
    pub(crate) input: TextInput,
}

/// The query bar's state.
#[derive(Debug, Default)]
pub(crate) struct QueryBar {
    pub(crate) focus: Option<BarFocus>,
    pub(crate) edit: Option<LimitEdit>,
    /// A press on a part of the bar: it acts on the release over the same part.
    pub(crate) press: overlay::Press<BarHit>,
    /// Where the parts were drawn by the frame before (the pointer's highlight is dropped when
    /// they move).
    pub(crate) last: Vec<(Rect, BarHit)>,
}

/// The keys of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarKey {
    /// The previous and the next part of the `WHERE` segment.
    Prev,
    Next,
    /// Open the part the keys are on.
    Open,
    /// Take the condition out, and run.
    Remove,
    /// Add a condition.
    Add,
    /// Back to the grid.
    Leave,
    /// The `LIMIT` field: apply what is typed, or leave it as it was.
    LimitApply,
    LimitCancel,
}

/// How many rows the result shown has, as far as known.
enum Total {
    Exact(u64),
    About(u64),
    AtLeast(u64),
}

impl App {
    /// The bar the keys may be on now: the active table tab's, its results focused, no dialog
    /// open.
    fn bar_owner(&self) -> Option<BarOwner> {
        if self.profiles.is_empty() || self.tabs.is_empty() || self.focus != Focus::Results || self.modal_open() {
            return None;
        }
        let t = self.tab();
        t.is_table().then_some(BarOwner { tab: t.id, shown: t.exec.shown, view: t.exec.view })
    }

    /// The keys are on the bar's `WHERE` segment.
    pub(crate) fn bar_focused(&self) -> bool {
        self.qbar.focus.is_some_and(|f| Some(f.on) == self.bar_owner())
    }

    /// The bar's `LIMIT` field is typed in.
    pub(crate) fn bar_editing(&self) -> bool {
        self.qbar.edit.as_ref().is_some_and(|e| Some(e.on) == self.bar_owner())
    }

    /// The bar's keys and field leave once what they were for is gone.
    pub(crate) fn bar_settle(&mut self) {
        if !self.bar_focused() {
            self.qbar.focus = None;
        }
        if !self.bar_editing() {
            self.qbar.edit = None;
        }
    }

    /// What the active tab's bar says, when it is a table tab.
    pub(crate) fn bar_model(&self) -> Option<Bar> {
        let t = self.tab();
        if !t.is_table() {
            return None;
        }
        let tq = t.doc.table_query.as_ref()?;
        let icons = self.icons_on();
        let q = tq.applied();
        let chips: Vec<ChipM> = match filter_text(q) {
            None => Vec::new(),
            Some(text) => match self.table_target(t.id) {
                Some(target) => Where::from_text(text, &target)
                    .chips()
                    .iter()
                    .map(|c| ChipM { term: c.index, text: crate::text::escape_controls(c.text.trim()), expr: c.expr })
                    .collect(),
                None => vec![ChipM { term: 0, text: crate::text::escape_controls(text.trim()), expr: true }],
            },
        };
        let conditions = self.i18n.msg(&Msg::TableBarConditions { count: chips.len() as u64 }).to_string();
        let order = q
            .order
            .iter()
            .filter(|k| k.origin == KeyOrigin::User)
            .map(|k| (crate::text::escape_controls(&k.col), k.dir))
            .collect();
        let limit = match self.qbar.edit.as_ref().filter(|_| self.bar_editing()) {
            Some(e) => {
                let text = e.input.text().to_string();
                let cursor = crate::text::width(&text[..e.input.cursor_byte().min(text.len())]);
                LimitM::Editing { text, cursor }
            }
            None => match q.limit {
                Some(n) => LimitM::Cap(n),
                None => LimitM::None(self.i18n.label(Label::TableBarNone).to_string()),
            },
        };
        let (cols, rows) = match &t.results {
            Results::Rows(rs) => {
                let total = rs.columns.len();
                let hidden = t.grid.hidden.iter().filter(|h| **h).count().min(total);
                let shown = total - hidden;
                let full = match hidden {
                    0 => self.i18n.msg(&Msg::TableBarCols { count: total as u64 }).to_string(),
                    _ => self
                        .i18n
                        .msg(&Msg::TableBarColsHidden { shown: shown.to_string(), total: total.to_string() })
                        .to_string(),
                };
                (Some(ColsM { full, short: format!("{shown}/{total}"), hidden }), Some(self.bar_rows(t, rs)))
            }
            _ => (None, None),
        };
        let (prev, next) = crate::icons::page_arrows(icons);
        Some(Bar {
            state: self.bar_state(t, tq),
            chips,
            conditions,
            order,
            limit,
            cols,
            rows,
            advanced: self.i18n.label(Label::TableBarAdvanced).to_string(),
            focus: self.qbar.focus.filter(|_| self.bar_focused()).map(|f| f.hit),
            glyphs: Glyphs { filter: crate::icons::filter_status(icons), prev, next },
            backgrounds: crate::theme::cur().has_backgrounds(),
        })
    }

    /// The state of tab `t`'s query: its run on its way (the spinner), the run that brought no
    /// rows (failed, cancelled, not applied), or what the user wrote refused before it ran.
    fn bar_state(&self, t: &Tab, tq: &table_query::TableQuery) -> Option<StateM> {
        let running = t
            .exec
            .running
            .filter(|r| !r.count && !r.fetch && tq.requested.as_ref().is_some_and(|(id, _)| *id == r.id))
            .filter(|_| !tq.shows_applied())
            .map(|r| crate::widgets::spinner_at(r.started, self.now()).to_string());
        let word = match tq.failure() {
            Some(f) => Some(match f.how {
                Unapplied::Refused => Label::TableStatusFailed,
                Unapplied::Cancelled => Label::TableStatusCancelled,
                Unapplied::Ended => Label::TableStatusUnapplied,
            }),
            None if tq.refusal.is_some() => Some(Label::TableBarRefused),
            None if running.is_none() && !tq.shows_applied() => Some(Label::TableStatusUnapplied),
            None => None,
        };
        if running.is_none() && word.is_none() {
            return None;
        }
        Some(StateM {
            spinner: running,
            word: word.map(|w| self.i18n.label(w).to_string()),
            mark: crate::icons::warning(self.icons_on()).to_string(),
        })
    }

    /// The rows of the result `rs` shown in tab `t`: the page shown and how many there are
    /// (exact once complete or counted; else about as many as the explorer's statistics or the
    /// planner say, both no more than the query's cap; else the rows read and more), the state of
    /// its paging, and whether the previous and the next page can be shown.
    fn bar_rows(&self, t: &Tab, rs: &crate::widgets::grid::ResultSet) -> RowsM {
        let size = self.page_size.max(1);
        let len = rs.rows.len();
        let win = t.grid.window(len);
        let paged = rs.more || len > size;
        let cap = self.table_query(t.id).and_then(|q| q.shown.as_ref()).and_then(|q| q.limit);
        let capped = |n: u64| cap.map_or(n, |c| n.min(c));
        let estimate = self.table_estimate(t).filter(|&n| n > 0 && n >= len as u64);
        let planner = rs.planner.map(capped).filter(|&n| n > 0 && n >= len as u64);
        let total = match (rs.total(), estimate.or(planner)) {
            (Some(n), _) => Total::Exact(n),
            (None, Some(n)) => Total::About(n),
            (None, None) => Total::AtLeast(len as u64),
        };
        let (long, short) = match total {
            Total::Exact(n) => (fmt_count(n), crate::widgets::qbar::compact(n)),
            Total::About(n) => (format!("~{}", fmt_count(n)), format!("~{}", crate::widgets::qbar::compact(n))),
            Total::AtLeast(n) => (format!("{}+", fmt_count(n)), format!("{}+", crate::widgets::qbar::compact(n))),
        };
        let mid = match total {
            Total::Exact(n) => self.i18n.msg(&Msg::TableBarRows { count: n }).to_string(),
            _ => self.i18n.msg(&Msg::TableBarRowsText { rows: long.clone() }).to_string(),
        };
        let short = match total {
            Total::Exact(n) if n < 1000 => mid.clone(),
            _ => self.i18n.msg(&Msg::TableBarRowsText { rows: short }).to_string(),
        };
        let full = if paged && !win.is_empty() {
            let (from, to) = (fmt_count(win.start as u64 + 1), fmt_count(win.end.min(len) as u64));
            self.i18n.msg(&Msg::TableBarRowsRange { from, to, total: long }).to_string()
        } else {
            mid.clone()
        };
        // The portal belongs to the last statement of the rows' run.
        let answer = t.exec.shown.is_some() && t.exec.shown == t.answer_index();
        let paging = if answer { t.exec.paging } else { Paging::None };
        let label = |l: Label| self.i18n.label(l).to_string();
        let mark = if t.exec.running.is_some_and(|r| r.estimate) {
            Some((label(Label::ResultsTitleEstimating), false))
        } else if t.exec.running.is_some_and(|r| r.count) {
            Some((label(Label::ResultsTitleCounting), false))
        } else {
            match (paging, self.paging_left()) {
                (Paging::Open { in_block: true, .. }, _) => Some((label(Label::ResultsTitleInTx), false)),
                (Paging::Open { .. }, Some(d)) => Some((super::paging::fmt_left(d), true)),
                (Paging::Open { .. }, None) => Some((label(Label::ResultsTitlePaging), false)),
                (Paging::Released, _) if rs.more && !self.can_run_again() => {
                    Some((label(Label::ResultsTitleFirstPageOnly), true))
                }
                (Paging::ClosedIdle | Paging::Replaced | Paging::Interrupted, _) if rs.more => {
                    Some((label(Label::ResultsTitlePagingClosed), true))
                }
                (Paging::Stopped, _) => Some((label(Label::ResultsTitlePagingStopped), true)),
                _ => None,
            }
        };
        // The next page: one read already, or one the paging state can bring.
        let more = rs.more
            && t.exec.running.is_none()
            && match paging {
                Paging::Open { .. } => self.answer_here(),
                p if p.closed() => {
                    rs.edit.applied.is_none() && (self.answer_here() || self.kept_answer_here()) && self.can_run_again()
                }
                _ => false,
            };
        let arrows = paged.then_some((win.start > 0, win.end < len || more));
        RowsM { full, mid, short, mark, arrows }
    }

    /// `Space r w`: the keys on the bar's conditions (the first one, else its `+`).
    pub(super) fn bar_where(&mut self) {
        self.focus = Focus::Results;
        self.qbar.edit = None;
        let Some(on) = self.bar_owner() else { return };
        let first = self.layout.qbar.iter().map(|(_, h)| *h).find(|h| h.in_where());
        let hit = first.unwrap_or_else(|| match self.bar_model().and_then(|m| m.chips.first().map(|c| c.term)) {
            Some(t) => BarHit::Chip(t),
            None => BarHit::Add,
        });
        self.qbar.focus = Some(BarFocus { on, hit });
    }

    /// `Space r l` (a click on `LIMIT`): the limit typed in the bar, from the one applied.
    pub(super) fn bar_limit(&mut self) {
        self.focus = Focus::Results;
        self.qbar.focus = None;
        let Some(on) = self.bar_owner() else { return };
        let text = self.table_query(on.tab).and_then(|tq| tq.applied().limit).map(|n| n.to_string());
        self.qbar.edit = Some(LimitEdit { on, input: TextInput::new(text.as_deref().unwrap_or("")) });
    }

    /// The filter dialog of the active table tab on column `column` (else the grid's), with its
    /// keys on `focus` when given; `order`: on the order.
    fn bar_dialog(&mut self, column: Option<String>, order: bool, focus: Option<DialogFocus>) {
        let id = self.tab().id;
        let Some(q) = self.table_query(id).map(|tq| tq.draft.clone()) else { return };
        let column = column.or_else(|| match &self.tab().results {
            Results::Rows(rs) => rs.columns.get(self.tab().grid.col).map(|c| c.meta.name.clone()),
            _ => None,
        });
        self.open_filter_dialog_with(id, &q, column.as_deref(), order);
        if let (Some(f), Some(d)) = (focus, self.filter_dialog_mut()) {
            d.focus_on(f);
        }
    }

    /// What a part of the bar does when it is clicked, or opened by its keys.
    pub(super) fn bar_hit(&mut self, hit: BarHit) {
        match hit {
            BarHit::Where | BarHit::Add => self.bar_dialog(None, false, Some(DialogFocus::Add)),
            BarHit::Chip(term) => {
                let id = self.tab().id;
                let target = self.table_target(id);
                let applied = self.table_query(id).map(|tq| tq.applied().clone());
                let (Some(target), Some(applied)) = (target, applied) else { return };
                let w = Draft::from_query(&applied, &target).where_;
                match w.terms().get(term).and_then(|t| t.row.as_ref()) {
                    Some(row) => self.bar_dialog(Some(row.col.clone()), false, None),
                    // An expression: its chip in the dialog's footer.
                    None => {
                        let at = w.chips().iter().position(|c| c.index == term).unwrap_or(0);
                        self.bar_dialog(None, false, Some(DialogFocus::Chips(at)))
                    }
                }
            }
            BarHit::Remove(term) => self.bar_remove(term),
            BarHit::More => self.bar_dialog(None, false, Some(DialogFocus::Chips(0))),
            BarHit::Order => self.bar_dialog(None, true, None),
            BarHit::Cols => self.bar_dialog(None, false, Some(DialogFocus::Columns)),
            BarHit::Advanced => self.bar_dialog(None, false, None),
            BarHit::Limit => self.bar_limit(),
            BarHit::Rows => self.count_rows(),
            BarHit::Prev => self.page_prev(),
            BarHit::Next => self.page_next(),
        }
    }

    /// Term `term` of the active table tab's condition (as applied) taken out, and the query run:
    /// the other terms are sent as they were written.
    pub(super) fn bar_remove(&mut self, term: usize) {
        let id = self.tab().id;
        // Refused before the draft changes: a busy tab keeps what it would run next.
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        let Some(target) = self.table_target(id) else { return };
        let Some(base) = self.table_query(id).map(|tq| tq.applied().clone()) else { return };
        let mut d = Draft::from_query(&base, &target);
        if d.where_.remove(term).is_none() {
            return;
        }
        let Ok(q) = d.to_query(&base, &target) else { return };
        let Some(tq) = self.table_query_mut(id) else { return };
        tq.draft = q.clone();
        self.table_run(id, q);
    }

    /// A key of the bar.
    pub(super) fn bar_key(&mut self, k: BarKey) {
        match k {
            BarKey::Prev | BarKey::Next => {
                let Some(f) = self.qbar.focus.filter(|_| self.bar_focused()) else { return };
                let mut items: Vec<BarHit> = Vec::new();
                for (_, h) in &self.layout.qbar {
                    if h.in_where() && !items.contains(h) {
                        items.push(*h);
                    }
                }
                let at =
                    items.iter().position(|h| *h == f.hit).or_else(|| items.iter().position(|h| *h == BarHit::More));
                let to = match (at, k) {
                    (Some(i), BarKey::Prev) => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(items.len().saturating_sub(1)),
                    (None, _) => 0,
                };
                if let (Some(hit), Some(f)) = (items.get(to).copied(), self.qbar.focus.as_mut()) {
                    f.hit = hit;
                }
            }
            BarKey::Open => {
                if let Some(f) = self.qbar.focus.filter(|_| self.bar_focused()) {
                    self.bar_hit(f.hit);
                }
            }
            BarKey::Remove => {
                if let Some(BarHit::Chip(t)) = self.qbar.focus.filter(|_| self.bar_focused()).map(|f| f.hit) {
                    self.bar_remove(t);
                }
            }
            BarKey::Add => self.bar_hit(BarHit::Add),
            BarKey::Leave => self.qbar.focus = None,
            BarKey::LimitApply => self.bar_limit_apply(),
            BarKey::LimitCancel => self.qbar.edit = None,
        }
    }

    /// `Enter` in the `LIMIT` field: the tab's query with that cap (none when blank) runs. A text
    /// that is no cap is refused, said, and stays to be fixed.
    fn bar_limit_apply(&mut self) {
        let Some(e) = self.qbar.edit.as_ref().filter(|_| self.bar_editing()) else { return };
        let (id, text) = (e.on.tab, e.input.text().to_string());
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        let Some(target) = self.table_target(id) else { return };
        let Some(base) = self.table_query(id).map(|tq| tq.applied().clone()) else { return };
        let mut d = Draft::from_query(&base, &target);
        d.limit = text;
        match d.to_query(&base, &target) {
            Err((_, problem)) => {
                let why = table_query::limit_why(&self.i18n, problem);
                self.tab_status(id, Notice::new(Msg::TableLimitRefused { why }, Level::Warning));
            }
            Ok(q) => {
                self.qbar.edit = None;
                let Some(tq) = self.table_query_mut(id) else { return };
                tq.draft = q.clone();
                self.table_run(id, q);
            }
        }
    }

    /// A key typed in the `LIMIT` field: digits and the keys that move and delete in it.
    pub(super) fn bar_text_key(&mut self, key: KeyEvent) {
        let Some(e) = self.qbar.edit.as_mut() else { return };
        let plain = (key.modifiers - KeyModifiers::SHIFT).is_empty();
        match key.code {
            KeyCode::Char(c) if plain && !c.is_ascii_digit() => {}
            _ => {
                e.input.handle_key(&key);
            }
        }
    }

    /// Text pasted into the `LIMIT` field (checked when it is applied).
    pub(super) fn bar_paste(&mut self, text: &str) {
        if let Some(e) = self.qbar.edit.as_mut() {
            e.input.insert_str(text.trim());
        }
    }

    /// The part of the bar drawn at (x, y).
    pub(super) fn bar_hit_at(&self, x: u16, y: u16) -> Option<BarHit> {
        self.layout.qbar.iter().find(|(r, _)| r.contains(Position::new(x, y))).map(|(_, h)| *h)
    }

    /// A left press at (x, y): on a part of the bar, it is armed (and taken).
    pub(super) fn bar_press(&mut self, x: u16, y: u16) -> bool {
        let hit = self.bar_hit_at(x, y);
        // A press elsewhere leaves the bar's keys and its field.
        if hit != Some(BarHit::Limit) {
            self.qbar.edit = None;
        }
        if hit.is_none_or(|h| !h.in_where()) {
            self.qbar.focus = None;
        }
        let Some(hit) = hit else { return false };
        self.focus = Focus::Results;
        let now = self.now();
        self.qbar.press.press(MouseEventKind::Down(MouseButton::Left), Some(hit), now);
        true
    }

    /// A left release at (x, y): over the part a press armed, that part acts.
    pub(super) fn bar_release(&mut self, x: u16, y: u16) {
        let hit = if self.tabs.is_empty() { None } else { self.bar_hit_at(x, y) };
        let now = self.now();
        if let Some(h) = self.qbar.press.press(MouseEventKind::Up(MouseButton::Left), hit, now) {
            self.bar_hit(h);
        }
    }
}
