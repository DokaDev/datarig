//! Asking for an Apply (`space d a`, `:apply`): the rows found by their values are counted
//! afresh on the tab's session first (the status bar says so; any key, a click or a paste stops
//! waiting and does nothing else, so none can be taken by what the answers bring), then the
//! changes go one of three ways. While any of them cannot be applied, nothing is asked: the
//! Changes tab shows them, the first selected. A choice among identical rows, an Apply whose
//! result is not known, or an environment that confirms every write opens the preview, the
//! statements on screen before `y` applies. Else one line in the status bar asks
//! ([`StatusAsk`]): only `y` applies; any other key, a click or a paste cancels and does
//! nothing else.
//!
//! The question is for the changes as they were asked about: an edit, another connection or
//! session, a transaction that began or ended, another tab or a run drops it, said.
//!
//! A question (or an Apply waiting for its counts) that ends without its answer, cancelled or
//! dropped, or a wait whose answers ask nothing, leaves [`ANSWER_GUARD`] in which the keys that
//! answer questions do nothing in the results but say they were ignored, with what was said: one
//! pressed as it went is never taken by the grid (where `y` copies over the clipboard).

use super::changes::{ChangesModel, Planned};
use super::*;
use datarig_core::driver::ApplyTx;
use datarig_core::edit::PlanError;
use datarig_core::edit::paste::Format;

/// How long after a question ended without its answer its answer keys are ignored.
pub const ANSWER_GUARD: Duration = Duration::from_secs(1);

/// The keys that answer a question of the status bar (as typed on QWERTY).
const ANSWER_KEYS: [char; 4] = ['y', 'n', 'c', '1'];

/// The key `key` types as QWERTY has it (a jamo typed with a Korean input source: its key),
/// without Ctrl, Alt or Super.
fn typed_key(key: &KeyEvent) -> Option<char> {
    if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) {
        return None;
    }
    let KeyCode::Char(c) = key.code else { return None };
    match crate::input::hangul::keys(c) {
        Some(k) => (k.len() == 1).then(|| k[0]),
        None => Some(c),
    }
}

/// A question in the status bar, in place of its segments: the first of `forms` (longest first)
/// that fits, in the tone of `tone`; what `y` does.
#[derive(Debug)]
pub struct StatusAsk {
    pub forms: Vec<String>,
    pub tone: Level,
    pub action: AskAction,
}

/// What `y` does to a question of the status bar.
#[derive(Debug)]
pub enum AskAction {
    /// Apply the changes as they were asked about.
    ApplyEdits(ApplyAsk),
    /// Paste many rows, as they were asked about.
    Paste(Box<super::PasteAsk>),
    /// Read text with commas as CSV (`c`) or one column (`1`).
    Csv(Box<super::CsvAsk>),
}

/// The changes an Apply was asked for: the tab, its result, what the rows were read under, the
/// revision of the changes and the statements, which `y` sends; how the tab stood to a
/// transaction then.
#[derive(Debug)]
pub struct ApplyAsk {
    tab: TabId,
    result: u64,
    bound: Bound,
    revision: u64,
    steps: Vec<edit::Step>,
    tx: (bool, u64, bool),
}

/// An Apply waiting for the counts of its rows found by their values.
#[derive(Debug)]
pub struct ApplyWait {
    tab: TabId,
    result: u64,
    bound: Bound,
    revision: u64,
    format: EditFormat,
    /// How many rows are counted.
    pub rows: u64,
}

impl App {
    /// How tab `t` stands to a transaction: in a block, which one, and whether manual commit
    /// opens one.
    fn tx_seen(&self, t: &Tab) -> (bool, u64, bool) {
        (t.exec.in_block, t.exec.tx_epoch, self.manual_commit(t.id))
    }

    /// `space d a`, `:apply`: the shown result's pending changes, to apply them. Refused as an
    /// edit key is, and with nothing pending; the rows found by their values are counted first.
    pub(super) fn apply_request(&mut self) {
        self.ask = None;
        self.apply_wait = None;
        let Some((_, format, bound)) = self.edit_gate() else { return };
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        if rs.edit.batch.is_empty() {
            return self.flash(Notice::new(Label::EditNothingPending, Level::Info));
        }
        if !matches!(rs.edit.target, Target::Ready(_)) {
            return self.say_edit_read_only(Label::EditWhyNotResolved);
        }
        if self.tab_busy(t.id) {
            return self.flash_busy();
        }
        let (tab, result, revision) = (t.id, rs.id, rs.edit.batch.revision());
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        // Counted afresh, whatever the Changes tab counted before.
        self.send_counts(tab, format);
        let rows = rows_of(self.tab()).map_or(0, |rs| rs.edit.checks.asked().len() as u64);
        if rows > 0 {
            self.apply_wait = Some(ApplyWait { tab, result, bound, revision, format, rows });
            return;
        }
        self.apply_decide(tab, result, bound, format, false);
    }

    /// The counts of an Apply came (or none was needed): the changes' problems in the Changes
    /// tab, the preview, or the question in the status bar. Not asked after a wait (`waited`),
    /// its answer keys are ignored a while: one typed for the question never lands elsewhere.
    fn apply_decide(&mut self, tab: TabId, result: u64, bound: Bound, format: EditFormat, waited: bool) {
        let Some(m) = self.changes_model(tab, result, format) else { return };
        let Some(t) = self.tabs.get(tab) else { return };
        let Some(rs) = rows_of(t).filter(|rs| rs.id == result) else { return };
        let count = m.problems.iter().filter(|p| !p.choice).count() as u64;
        if count > 0 {
            let first = m.entries.iter().position(|e| matches!(e, ListEntry::Problem(k) if !m.problems[*k].choice));
            self.changes_show_problems(first.unwrap_or(0));
            let said = Msg::EditAskProblems { count };
            return if waited { self.ask_ended(said) } else { self.flash(Notice::new(said, Level::Info)) };
        }
        let choose = matches!(m.planned, Planned::Refused(PlanError::Choose { .. }));
        // An environment that can't be read confirms every write.
        let writes = t.profile.is_some_and(|p| !self.policy_of(p).is_ok_and(|p| !p.confirm.writes()));
        if choose || writes || rs.edit.apply_unknown {
            return self.edit_preview_push(format, bound);
        }
        let Planned::Ready(plan) = &m.planned else {
            let reason = m.why_not.clone().unwrap_or_default();
            return if waited {
                self.ask_ended(Msg::EditPreviewCantApply { reason })
            } else {
                self.say_cant_apply(reason)
            };
        };
        let forms = self.apply_ask_forms(t, rs, &m);
        let action = AskAction::ApplyEdits(ApplyAsk {
            tab,
            result,
            bound,
            revision: rs.edit.batch.revision(),
            steps: plan.steps.clone(),
            tx: self.tx_seen(t),
        });
        self.ask = Some(StatusAsk { forms, tone: Level::Warning, action });
    }

    /// The question of an Apply of `rs`'s changes (`m`) in tab `t`, longest first: how many, to
    /// which table and of which kinds, how it stands to an open transaction or one manual commit
    /// opens, the keys; the same without the table, then with only the deletes of the kinds; how
    /// many, the deletes and, short, how it stands to a transaction.
    fn apply_ask_forms(&self, t: &Tab, rs: &ResultSet, m: &ChangesModel) -> Vec<String> {
        let i18n = &self.i18n;
        let c = rs.edit.batch.counts();
        let count = c.total() as u64;
        let table = match &rs.edit.target {
            Target::Ready(x) => format!("{}.{}", x.table.schema, x.table.name),
            _ => String::new(),
        };
        let summary = m.summary.clone();
        let tx = match () {
            _ if t.exec.user_tx() => Some(Label::EditAskTxInside),
            _ if self.manual_commit(t.id) => Some(Label::EditAskTxOpens),
            _ => None,
        };
        let long = |head: Msg| {
            let mut text = i18n.msg(&head).to_string();
            if tx.is_some() {
                text.push(' ');
                text.push_str(&self.preview_tx_text(t.id));
            }
            text.push_str("  ");
            text.push_str(&i18n.label(Label::EditAskKeys).to_string());
            text
        };
        let deletes =
            (c.deletes > 0).then(|| i18n.msg(&Msg::EditPreviewDeletes { count: c.deletes as u64 }).to_string());
        let mut forms = vec![
            long(Msg::EditAskApply { count, table, summary: summary.clone() }),
            long(Msg::EditAskApplyMid { count, summary: summary.clone() }),
        ];
        if let Some(d) = deletes.as_ref().filter(|d| **d != summary) {
            forms.push(long(Msg::EditAskApplyMid { count, summary: d.clone() }));
        }
        let short = match (deletes, tx.map(|l| i18n.label(l).to_string())) {
            (None, None) => Msg::EditAskApplyShort { count },
            (Some(deletes), None) => Msg::EditAskApplyShortDeletes { count, deletes },
            (None, Some(tx)) => Msg::EditAskApplyShortTx { count, tx },
            (Some(deletes), Some(tx)) => Msg::EditAskApplyShortDeletesTx { count, deletes, tx },
        };
        forms.push(i18n.msg(&short).to_string());
        forms
    }

    /// The Changes tab of the shown result with the focus, its line `line` (a problem) selected.
    fn changes_show_problems(&mut self, line: usize) {
        let t = self.tab_mut();
        t.exec.view = tabs::ResultView::Changes;
        t.pane.hidden = false;
        self.focus = Focus::Results;
        if let Some(c) = self.changes_cursor_mut() {
            c.line = line;
            c.scroll = 0;
        }
    }

    /// The question of the status bar is still about the active tab's shown result as it was
    /// asked: the same changes, statements, binding and session, the tab idle and its transaction
    /// as it was.
    fn ask_current(&self, a: &ApplyAsk) -> bool {
        if self.tabs.is_empty() || self.modal_open() {
            return false;
        }
        let t = self.tab();
        let Some(rs) = rows_of(t).filter(|rs| rs.id == a.result) else { return false };
        let same = t.id == a.tab
            && t.exec.session.is_some()
            && !self.tab_busy(t.id)
            && rs.edit.batch.revision() == a.revision
            && rs.edit.applying.is_none()
            && self.tx_seen(t) == a.tx
            && self.edit_check().is_ok_and(|(_, _, b)| b == a.bound);
        let format = self.tab_edit_format(t.id);
        same && format.and_then(|f| self.changes_model(t.id, rs.id, f)).is_some_and(|m| match &m.planned {
            Planned::Ready(plan) => plan.steps == a.steps,
            _ => false,
        })
    }

    /// The Apply waiting for its counts is still for the active tab's shown result as it was,
    /// no dialog open and the tab idle.
    fn wait_current(&self, w: &ApplyWait) -> bool {
        if self.tabs.is_empty() || self.modal_open() {
            return false;
        }
        let t = self.tab();
        t.id == w.tab
            && t.exec.session.is_some()
            && !self.tab_busy(t.id)
            && rows_of(t).is_some_and(|rs| rs.id == w.result && rs.edit.bound.as_ref() == Some(&w.bound))
            && self.edit_bound(t).as_ref() == Some(&w.bound)
    }

    /// A key while the status bar asks: `y` (`c` or `1` for text with commas) answers, any
    /// other key cancels; neither does anything else. A held key does nothing.
    pub(in crate::app) fn status_ask_key(&mut self, key: KeyEvent, repeat: bool) {
        if repeat {
            return;
        }
        let typed = typed_key(&key);
        if let Some(StatusAsk { action: AskAction::Csv(a), .. }) = &self.ask {
            let format = match typed {
                Some('c') => a.csv.map(|_| Format::Csv),
                Some('1') => a.plain,
                _ => None,
            };
            if let Some(f) = format
                && let Some(StatusAsk { action: AskAction::Csv(a), .. }) = self.ask.take()
            {
                return self.csv_ask_answer(*a, f);
            }
            return self.status_ask_cancel();
        }
        if typed == Some('y') { self.status_ask_yes() } else { self.status_ask_cancel() }
    }

    /// A question (or an Apply waiting for its counts) ended without its answer: `said` is
    /// said, and its answer keys are ignored for [`ANSWER_GUARD`].
    pub(in crate::app) fn ask_ended(&mut self, said: impl Into<Msg>) {
        let said = said.into();
        self.ask_gone = Some((self.now() + ANSWER_GUARD, said.clone()));
        self.flash(Notice::new(said, Level::Info));
    }

    /// `key` is an answer key pressed in the results within [`ANSWER_GUARD`] of a question that
    /// ended without its answer: taken, said, doing nothing else.
    pub(in crate::app) fn answer_swallowed(&mut self, key: &KeyEvent) -> bool {
        let Some((until, said)) = &self.ask_gone else { return false };
        if self.now() >= *until {
            self.ask_gone = None;
            return false;
        }
        // The inspector's keys copy too (a plan's text, a chart's data).
        let results = match self.focus {
            Focus::Results => !self.cell_edit_open() && !self.grid_prompt_open(),
            Focus::Inspector => true,
            Focus::Tree | Focus::Editor => false,
        };
        let Some(k) = typed_key(key).filter(|k| results && ANSWER_KEYS.contains(k)) else { return false };
        let said = self.i18n.msg(said).to_string();
        self.flash(Notice::new(Msg::EditAskIgnored { said, key: k.to_string() }, Level::Info));
        true
    }

    /// Any input but `y` while the status bar asks (another key, a click, the wheel, a paste):
    /// it cancels.
    pub(in crate::app) fn status_ask_cancel(&mut self) {
        let said = match self.ask.take().map(|a| a.action) {
            Some(AskAction::ApplyEdits(_)) => Label::EditAskCancelled,
            Some(AskAction::Paste(p)) => {
                self.paste_ask_dropped(*p);
                Label::EditPasteCancelled
            }
            Some(AskAction::Csv(a)) => {
                self.csv_ask_dropped(*a);
                Label::EditPasteCancelled
            }
            None => return,
        };
        self.ask_ended(said);
    }

    /// `y`: the statements asked about, still those of the changes, go to the tab's session.
    fn status_ask_yes(&mut self) {
        let Some(ask) = self.ask.take() else { return };
        let a = match ask.action {
            AskAction::ApplyEdits(a) => a,
            AskAction::Paste(p) => return self.paste_ask_yes(*p),
            // Answered by `c` or `1` only.
            AskAction::Csv(_) => return,
        };
        if !self.ask_current(&a) {
            return self.ask_ended(Label::EditAskMoved);
        }
        let tx = match self.tabs.get(a.tab) {
            Some(t) if self.manual_commit(a.tab) => ApplyTx::Keep { idle_timeout: self.manual_idle_timeout(t) },
            _ => ApplyTx::Commit,
        };
        self.send_apply(a.tab, a.result, a.steps, tx);
    }

    /// Any key (`Esc` too), a click, the wheel or a paste while an Apply waits for its counts:
    /// it waits no more, said.
    pub(in crate::app) fn apply_wait_cancel(&mut self) {
        if self.apply_wait.take().is_some() {
            self.ask_ended(Label::EditAskCancelled);
        }
    }

    /// After every event and timer: a question about changes that moved is dropped, said; an
    /// Apply whose counts all came goes on, one whose changes moved is dropped, said, and one
    /// whose tab changed, ran or has a dialog open is cancelled.
    pub(in crate::app) fn sweep_status_ask(&mut self) {
        if let Some(StatusAsk { action: AskAction::ApplyEdits(a), .. }) = &self.ask
            && !self.ask_current(a)
        {
            self.ask = None;
            self.ask_ended(Label::EditAskMoved);
        }
        if let Some(StatusAsk { action: AskAction::Paste(a), .. }) = &self.ask
            && !self.paste_current(a)
        {
            if let Some(StatusAsk { action: AskAction::Paste(a), .. }) = self.ask.take() {
                self.paste_ask_dropped(*a);
            }
            self.ask_ended(Label::EditPasteMoved);
        }
        if let Some(StatusAsk { action: AskAction::Csv(a), .. }) = &self.ask
            && !self.csv_current(a)
        {
            if let Some(StatusAsk { action: AskAction::Csv(a), .. }) = self.ask.take() {
                self.csv_ask_dropped(*a);
            }
            self.ask_ended(Label::EditPasteMoved);
        }
        let Some(w) = &self.apply_wait else { return };
        if !self.wait_current(w) {
            self.apply_wait = None;
            return self.ask_ended(Label::EditAskCancelled);
        }
        let Some(rs) = rows_of(self.tab()) else { return };
        // The counts are those of this revision: the Changes tab may have counted it afresh
        // meanwhile, and the Apply then takes those answers.
        if rs.edit.batch.revision() != w.revision || rs.edit.checks.for_revision() != w.revision {
            self.apply_wait = None;
            let keys = self.apply_keys();
            return self.ask_ended(Msg::EditAskChanged { keys });
        }
        if !rs.edit.checks.asked().is_empty() {
            return;
        }
        let Some(w) = self.apply_wait.take() else { return };
        self.apply_decide(w.tab, w.result, w.bound, w.format, true);
    }

    /// The keys of `edit.apply` in the grid, as the hints say them.
    fn apply_keys(&self) -> String {
        let keys =
            self.keymap.hint_keys(crate::app::action::Action::Edit(EditAction::Apply), Ctx::Grid, self.enhanced_keys);
        keys.map(|k| crate::keymap::keys::label(&k)).unwrap_or_default()
    }

    /// What the status bar says in place of its segments while it asks: its forms, longest
    /// first, and its tone.
    pub fn status_ask_line(&self) -> Option<(Vec<String>, Level)> {
        self.ask.as_ref().map(|a| (a.forms.clone(), a.tone))
    }

    /// What the status bar says while an Apply waits for its counts.
    pub fn apply_wait_text(&self) -> Option<String> {
        let w = self.apply_wait.as_ref()?;
        Some(self.i18n.msg(&Msg::EditAskChecking { count: w.rows }).to_string())
    }
}
