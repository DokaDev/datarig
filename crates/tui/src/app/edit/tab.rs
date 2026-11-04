//! The Changes tab of a result (`:changes`, `space d p`, or its tab in the strip): its pending
//! changes as one line per changed cell, every reason they cannot be applied yet first; or, `s`,
//! the statements that apply them, as the preview shows them. `Enter` shows a line's row and
//! column in the grid, `c` copies the statements as they would be sent.
//!
//! The rows found by their values are counted for the tab on the tab's session once the changes
//! settle: [`COUNT_SETTLE`] after they last changed, while the tab is shown, once for each state
//! of the changes. Until then the statements say they are being counted; the list needs no
//! count.

use super::changes::{ChangesModel, Planned};
use super::*;
use crate::app::action::Action;
use datarig_core::config::ChangesView;
use datarig_core::edit::PlanError;

/// How long the changes stay as they are before the Changes tab counts their rows.
pub const COUNT_SETTLE: Duration = Duration::from_millis(500);

/// What a key does in the Changes tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangesAction {
    Down,
    Up,
    HalfDown,
    HalfUp,
    Top,
    Bottom,
    /// The selected line's row and column in the grid.
    Goto,
    /// The list or the statements, for this run.
    ToggleView,
    /// The statements to the clipboard, as they would be sent.
    CopySql,
}

/// Where a result's Changes tab is: the line selected in its list, the first row of the list and
/// of the statements on screen, and how many rows of them it showed last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangesCursor {
    pub line: usize,
    pub scroll: usize,
    pub sql_scroll: usize,
    pub page: usize,
    /// The statements scroll to the one that failed when they are next drawn.
    pub seek: bool,
}

/// The active tab's shown result and the revision of its changes, and since when they are so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    tab: TabId,
    result: u64,
    revision: u64,
    since: Instant,
}

/// What `Enter` on a line of the list did.
enum Went {
    /// The grid shows the line's row and column.
    There,
    /// The line points at no row (or the statements are shown).
    Nowhere,
    /// Its row is not in the grid any more.
    NotLoaded,
}

/// The row and the column line `k` of the list is about: the problems first, then the changed
/// cells.
fn entry_at(m: &ChangesModel, k: usize) -> Option<(RowAt, Option<usize>)> {
    match m.problems.get(k) {
        Some(p) => p.at,
        None => m.list.get(k - m.problems.len()).map(|l| (l.at, l.col)),
    }
}

impl App {
    /// The active tab shows the Changes tab of its shown result.
    pub fn changes_shown(&self) -> bool {
        !self.tabs.is_empty()
            && self.tab().exec.view == tabs::ResultView::Changes
            && self.tab().has_changes()
            && self.results_shown()
    }

    /// The Changes tab shows the statements, not the list.
    pub fn changes_sql(&self) -> bool {
        self.changes_view.unwrap_or(self.prefs.changes_view) == ChangesView::Sql
    }

    /// `:changes`, `space d p`: the Changes tab of the shown result, with the focus. With nothing
    /// pending, why not (as an edit key says it), else that nothing is.
    pub(super) fn changes_open(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        if !self.tab().has_changes() {
            if self.edit_gate().is_some() {
                self.flash(Notice::new(Label::EditNothingPending, Level::Info));
            }
            return;
        }
        self.overlays.close(OverlayKind::Commands);
        let t = self.tab_mut();
        t.exec.view = tabs::ResultView::Changes;
        t.pane.hidden = false;
        self.focus = Focus::Results;
    }

    /// Where the shown result's Changes tab is.
    pub(crate) fn changes_cursor(&self) -> Option<ChangesCursor> {
        rows_of(self.tab()).map(|rs| rs.edit.cursor)
    }

    /// Where the shown result's Changes tab is, to move it.
    pub(crate) fn changes_cursor_mut(&mut self) -> Option<&mut ChangesCursor> {
        rows_of_mut(self.tabs.active_mut()).map(|rs| &mut rs.edit.cursor)
    }

    pub(in crate::app) fn changes_action(&mut self, a: ChangesAction) {
        use ChangesAction as A;
        match a {
            A::Goto => {
                self.changes_point();
                return;
            }
            A::ToggleView => {
                self.changes_view = Some(if self.changes_sql() { ChangesView::List } else { ChangesView::Sql });
                return;
            }
            A::CopySql => return self.changes_copy(),
            _ => {}
        }
        let sql = self.changes_sql();
        let Some(m) = self.shown_changes() else { return };
        let last = (m.problems.len() + m.list.len()).saturating_sub(1);
        let Some(c) = self.changes_cursor_mut() else { return };
        let half = (c.page / 2).max(1);
        // The statements scroll (their end is kept on screen as they are drawn); the list moves
        // its selection.
        let at = if sql { &mut c.sql_scroll } else { &mut c.line };
        *at = match a {
            A::Down => at.saturating_add(1),
            A::Up => at.saturating_sub(1),
            A::HalfDown => at.saturating_add(half),
            A::HalfUp => at.saturating_sub(half),
            A::Top => 0,
            _ => usize::MAX / 2,
        };
        if !sql {
            *at = (*at).min(last);
        }
    }

    /// `Enter`: the grid at the selected line's row and column (the row alone for a whole
    /// row); a row the grid no longer has is said.
    fn changes_point(&mut self) -> Went {
        let Some(m) = self.shown_changes() else { return Went::Nowhere };
        let Some(c) = self.changes_cursor() else { return Went::Nowhere };
        let at = if self.changes_sql() { None } else { entry_at(&m, c.line) };
        let Some((at, col)) = at else { return Went::Nowhere };
        let Some(rs) = rows_of(self.tab()) else { return Went::Nowhere };
        if shown_row(rs, at).is_none() {
            self.flash(Notice::new(Label::ChangesRowNotLoaded, Level::Info));
            return Went::NotLoaded;
        }
        let col = col.unwrap_or(self.tab().grid.col);
        self.tab_mut().exec.view = tabs::ResultView::Rows;
        self.edit_cursor_to(at, col);
        Went::There
    }

    /// An edit key for the cursor's cell pressed in the Changes tab: the grid, at the selected
    /// line's row and column. `false`: the key does nothing more, as the line has no row (or the
    /// statements are shown: the grid, as it was) or its row is not in the grid any more.
    pub(super) fn changes_to_grid(&mut self) -> bool {
        match self.changes_point() {
            Went::There => true,
            Went::NotLoaded => false,
            Went::Nowhere => {
                self.tab_mut().exec.view = tabs::ResultView::Rows;
                false
            }
        }
    }

    /// `c`: the statements of the changes, whole and in order, to the clipboard; refused while
    /// they are not final (problems, rows still to be counted).
    fn changes_copy(&mut self) {
        let Some(m) = self.shown_changes() else { return };
        let n = match &m.planned {
            Planned::Ready(plan) => return self.copy_steps(&plan.steps),
            Planned::Sent(steps) => return self.copy_steps(steps),
            _ => m.problems.len() as u64,
        };
        let msg = match n {
            0 => Label::ChangesCopyCounting.into(),
            count => Msg::ChangesCopyNotFinal { count },
        };
        self.flash(Notice::new(msg, Level::Info));
    }

    /// The statements the Changes tab shows: the preview's, rows still to be counted said so,
    /// a choice among identical rows said to be made when applying, the statement the last
    /// Apply failed at marked.
    pub(crate) fn changes_sql_lines(&self, m: &ChangesModel) -> Vec<PreviewLine> {
        if let Planned::Refused(PlanError::NotCounted { .. }) = &m.planned {
            return vec![PreviewLine::Note(self.i18n.label(Label::EditPreviewCounting).to_string())];
        }
        let failed = rows_of(self.tab())
            .and_then(|rs| rs.edit.failed.as_ref().filter(|f| f.revision == rs.edit.batch.revision()))
            .and_then(|f| f.sql.clone());
        let mut marked = false;
        m.sql
            .iter()
            .map(|l| match l {
                PreviewLine::Choose { text, .. } => PreviewLine::Choose {
                    text: text.clone(),
                    keys: self.i18n.label(Label::ChangesChooseOnApply).to_string(),
                },
                PreviewLine::Step { sql, expect, .. } if !marked && failed.as_deref() == Some(sql.as_str()) => {
                    marked = true;
                    PreviewLine::Failed { sql: sql.clone(), expect: expect.clone() }
                }
                l => l.clone(),
            })
            .collect()
    }

    /// The Changes tab's first line: which result and table (`Changes — Result 1 · shop.users`),
    /// how many changes of each kind, and how many problems there are.
    pub(crate) fn changes_head(&self, m: &ChangesModel) -> (String, String, Option<String>) {
        let t = self.tab();
        let result = match t.exec.shown {
            Some(i) => self.i18n.msg(&Msg::ResultsTabResult { n: (i + 1).to_string() }).to_string(),
            None => self.i18n.label(Label::ResultsTabRows).to_string(),
        };
        let table = match rows_of(t).map(|rs| &rs.edit.target) {
            Some(Target::Ready(x)) => format!("{}.{}", x.table.schema, x.table.name),
            _ => String::new(),
        };
        let title = self.i18n.msg(&Msg::ChangesTitle { result, table }).to_string();
        let problems = (!m.problems.is_empty())
            .then(|| self.i18n.msg(&Msg::ChangesProblems { count: m.problems.len() as u64 }).to_string());
        (title, m.summary.clone(), problems)
    }

    /// Where the Apply of the shown result's changes stands, when it says something, in its
    /// tone: being applied; applied (committed or not); gone with its transaction; not known to
    /// have been; why the last one applied nothing.
    pub(crate) fn changes_status(&self) -> Option<(String, Level)> {
        let rs = rows_of(self.tab())?;
        let i18n = &self.i18n;
        if rs.edit.applying.is_some() {
            let count = rs.edit.batch.counts().total() as u64;
            return Some((i18n.msg(&Msg::ChangesApplying { count }).to_string(), Level::Info));
        }
        if let Some(a) = rs.edit.applied.as_ref().filter(|_| rs.edit.batch.is_empty()) {
            let label = if a.tx.is_some() { Label::ChangesAppliedKept } else { Label::ChangesApplied };
            return Some((i18n.label(label).to_string(), Level::Success));
        }
        if let Some(label) = rs.edit.gone.filter(|_| rs.edit.batch.is_empty()) {
            return Some((i18n.label(label).to_string(), Level::Warning));
        }
        if rs.edit.apply_unknown {
            return Some((i18n.label(Label::EditFailedUnknown).to_string(), Level::Error));
        }
        let f = rs.edit.failed.as_ref().filter(|f| f.revision == rs.edit.batch.revision())?;
        Some((f.text.clone(), Level::Error))
    }

    /// Another result of the active tab has pending changes too: which and how many.
    pub(crate) fn changes_note(&self) -> Option<String> {
        let t = self.tab();
        let (i, p) = t.exec.steps.iter().find(|(_, p)| !p.rs.edit.batch.is_empty())?;
        let result = self.i18n.msg(&Msg::ResultsTabResult { n: (i + 1).to_string() }).to_string();
        let count = p.rs.edit.batch.counts().total() as u64;
        Some(self.i18n.msg(&Msg::ChangesOtherPending { result, count }).to_string())
    }

    /// How an Apply of the active tab's changes stands to a transaction, in words (nothing
    /// while none is pending).
    pub(crate) fn changes_tx_text(&self) -> String {
        match rows_of(self.tab()) {
            Some(rs) if !rs.edit.batch.is_empty() => self.preview_tx_text(self.tab().id),
            _ => String::new(),
        }
    }

    /// The keys of the Changes tab under it: each with what it does, or (`short`) the keys
    /// alone.
    pub(crate) fn changes_keys(&self, short: bool) -> String {
        let view = if self.changes_sql() { Label::HintChangesList } else { Label::HintChangesSql };
        let keys = [
            (Action::Changes(ChangesAction::ToggleView), view),
            (Action::Changes(ChangesAction::CopySql), Label::HintCopySql),
            (Action::Changes(ChangesAction::Goto), Label::HintGoto),
            (Action::Edit(EditAction::Apply), Label::HintApply),
        ];
        let bound = keys.iter().filter_map(|&(a, l)| {
            let k = self.keymap.hint_keys(a, Ctx::Changes, self.enhanced_keys)?;
            Some((crate::keymap::keys::label(&k), l))
        });
        if short {
            // The apply key is a sequence: only the tab's own keys.
            bound.take(3).map(|(k, _)| k).collect::<Vec<_>>().join(" ")
        } else {
            bound.map(|(k, l)| format!("{k} {}", self.i18n.label(l))).collect::<Vec<_>>().join(" \u{B7} ")
        }
    }

    /// When the rows of the shown Changes tab's changes found by their values are to be
    /// counted: [`COUNT_SETTLE`] after the changes last moved. `None` while nothing is to be
    /// counted now (another tab shown, counted for these changes already, rows found by their
    /// key, the tab's session busy or not open, the rows not editable as they are).
    pub(crate) fn changes_count_due(&self) -> Option<Instant> {
        if !self.changes_shown() {
            return None;
        }
        let seen = self.changes_seen?;
        let t = self.tab();
        let rs = rows_of(t)?;
        let revision = rs.edit.batch.revision();
        if (seen.tab, seen.result, seen.revision) != (t.id, rs.id, revision)
            || rs.edit.checks.for_revision() == revision
        {
            return None;
        }
        let Target::Ready(target) = &rs.edit.target else { return None };
        let found_by_values =
            matches!(target.identity, Identity::Values { .. }) && rs.edit.batch.rows().next().is_some();
        let idle = rs.edit.applying.is_none() && t.exec.session.is_some() && !self.tab_busy(t.id);
        (found_by_values && idle && self.edit_check().is_ok()).then_some(seen.since + COUNT_SETTLE)
    }

    /// The counts of the shown Changes tab go out once they are due.
    pub(crate) fn changes_count_tick(&mut self, now: Instant) {
        if self.changes_count_due().is_none_or(|due| now < due) {
            return;
        }
        let tab = self.tab().id;
        if let Some(format) = self.tab_edit_format(tab) {
            self.send_counts(tab, format);
        }
    }

    /// After every event: a Changes tab whose result has nothing pending any more gives way to
    /// the rows; the active tab's shown changes are seen as they are now (the time their counts
    /// wait from), and their counts go out once due.
    pub(super) fn sweep_changes(&mut self) {
        for t in self.tabs.iter_mut() {
            if t.exec.view == tabs::ResultView::Changes && !t.has_changes() {
                t.exec.view = tabs::ResultView::Rows;
            }
        }
        let now = self.now();
        let shown = (!self.tabs.is_empty()).then(|| self.tab()).and_then(|t| match &t.results {
            Results::Rows(rs) => Some((t.id, rs.id, rs.edit.batch.revision())),
            _ => None,
        });
        self.changes_seen = match (shown, self.changes_seen) {
            (Some(k), Some(s)) if k == (s.tab, s.result, s.revision) => Some(s),
            (Some((tab, result, revision)), _) => Some(Seen { tab, result, revision, since: now }),
            (None, _) => None,
        };
        self.changes_count_tick(now);
    }
}
