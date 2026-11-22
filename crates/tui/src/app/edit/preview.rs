//! The preview of a result's pending changes before an Apply that needs one ([`super::ask`]): the
//! statements that apply them, each with the count it must give, in a dialog that is also the
//! question whether to apply them. Cancel is the default: only `y` applies.
//!
//! It opens once the rows found by their values are counted (with plain reads on the tab's own
//! session: they see its open transaction, as the Apply will). What it shows is the changes'
//! model ([`ChangesModel`]); a change of the result, its rows' connection or its changes closes
//! the dialog.
//!
//! `y` sends the Apply on the tab's session: the dialog says it applies until the answer comes
//! (the query's cancel key cancels it), closes once the changes are applied, and says why when
//! nothing was.

use super::changes::{ChangesModel, Planned};
use super::*;
use crate::app::overlay::Buttons;
use datarig_core::driver::ApplyTx;
use datarig_core::edit::{Choice, PlanError};

pub(super) mod literals;

/// The open preview of tab `tab`'s result `result`: the revision of the changes, the target and
/// the format it was opened on.
pub struct EditPreview {
    pub tab: TabId,
    pub result: u64,
    bound: Bound,
    revision: u64,
    target: EditTarget,
    format: EditFormat,
    /// After an Apply whose result is not known, `y` was pressed once: the next applies again.
    again: bool,
    /// The first line of the body on screen.
    pub scroll: usize,
    pub buttons: Buttons,
    /// The body as last laid out (rebuilt when its width, the theme or its lines change).
    pub(crate) body: Option<PreviewBody>,
    /// How many times the body was laid out.
    pub body_builds: u64,
}

impl EditPreview {
    /// How many rows the body was last laid out in (`None`: not drawn yet).
    pub fn body_rows(&self) -> Option<usize> {
        self.body.as_ref().map(|b| b.rows.len())
    }
}

/// The preview's body laid out in rows of text and style, for the lines and width it was made for.
pub(crate) struct PreviewBody {
    pub(crate) width: usize,
    pub(crate) theme: std::sync::Arc<crate::theme::Theme>,
    pub(crate) lines: Vec<PreviewLine>,
    pub(crate) rows: Vec<Vec<(String, Style)>>,
}

/// A line of the preview's body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewLine {
    /// A section's name.
    Head(String),
    /// A statement and the count it must give; `risky`: it deletes, or changes several rows.
    Step {
        sql: String,
        expect: String,
        risky: bool,
    },
    /// The statement the last Apply failed at, with the count it had to give.
    Failed {
        sql: String,
        expect: String,
    },
    /// A row that matches several identical rows: which to change.
    Choose {
        text: String,
        keys: String,
    },
    /// The text values the statement before carries as their bytes, read back (`column =
    /// 'text'`, ...): shown under it, not SQL, never sent or copied.
    Values(String),
    Note(String),
    Problem(String),
}

/// The longest a read-back value is shown, in columns (the rest cut with `…`).
pub(super) const VALUE_W: usize = 40;

/// The preview as drawn.
pub struct PreviewView {
    pub title: Localized,
    pub summary: String,
    pub lines: Vec<PreviewLine>,
    /// How the Apply stands to a transaction.
    pub footer: String,
    /// `y` applies.
    pub apply: bool,
    /// The statements' dialect (their colors).
    pub dialect: Dialect,
}

impl App {
    /// The preview of the shown result's pending changes, as counted for them, before an Apply
    /// (`bound`: what the rows were read under, `format`: the edits the server takes).
    pub(super) fn edit_preview_push(&mut self, format: EditFormat, bound: Bound) {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let Target::Ready(target) = &rs.edit.target else { return };
        let (tab, result, revision, target) = (t.id, rs.id, rs.edit.batch.revision(), (**target).clone());
        let p = EditPreview {
            tab,
            result,
            bound,
            revision,
            target,
            format,
            again: false,
            scroll: 0,
            buttons: Buttons::default(),
            body: None,
            body_builds: 0,
        };
        self.overlays.push(Overlay::EditPreview(Box::new(p)));
    }

    /// The rows of tab `tab`'s shown result's changes found by their values counted afresh,
    /// in format `format`, on the tab's session: the result's checks start again for the
    /// changes as they are (none are asked for changes that cannot be made anyway).
    pub(super) fn send_counts(&mut self, tab: TabId, format: EditFormat) {
        let Some(t) = self.tabs.get(tab) else { return };
        let Some(rs) = rows_of(t) else { return };
        let Target::Ready(target) = &rs.edit.target else { return };
        let not_whole = self.cells_not_whole(t, rs);
        let (revision, batch) = (rs.edit.batch.revision(), &rs.edit.batch);
        let counts = match edit::no_default(target, batch).is_empty() && edit::not_shown(target, batch).is_empty() {
            true => {
                let whole = |r: usize, c: usize| !not_whole.contains(&(r, c));
                edit::count_statements(target, batch, &*edit::format(format), &whole).unwrap_or_default()
            }
            false => Vec::new(),
        };
        let mut asked = Vec::with_capacity(counts.len());
        for (row, sql) in counts {
            self.query_seq += 1;
            let id = self.query_seq;
            self.send_tab(tab, DbCommand::Count { id, sql, limit: None });
            asked.push((id, row));
        }
        if let Some(rs) = self.tabs.get_mut(tab).and_then(rows_of_mut) {
            rs.edit.checks.restart(revision, asked);
        }
    }

    /// The count `qid` of tab `tab` came: one of a result's checks, when it asked it (`true`).
    /// It is recorded whether the preview is open or not (closed, it is there when the changes
    /// are shown again); an answer for changes that moved since it was asked is dropped.
    pub(in crate::app) fn edit_preview_counted(&mut self, tab: TabId, qid: u64, result: &Result<u64, DbError>) -> bool {
        let text = result.as_ref().err().map(|e| self.db_error_text(e));
        let Some(t) = self.tabs.get_mut(tab) else { return false };
        let asked = |rs: &&mut crate::widgets::grid::ResultSet| rs.edit.checks.asked().iter().any(|(id, _)| *id == qid);
        let Some(rs) = t.row_results_mut().find(asked) else { return false };
        if rs.edit.checks.for_revision() == rs.edit.batch.revision() {
            rs.edit.checks.answered(qid, result.as_ref().map(|n| *n).map_err(|_| text));
        }
        true
    }

    /// The open preview is still of the active tab's shown result, its changes and target as
    /// they were, its rows bound as they were, and the tab's session open (its counts are
    /// answered there).
    fn edit_preview_current(&self) -> bool {
        let Some(p) = self.overlays.edit_preview() else { return false };
        let active = |t: &&Tab| self.tab().id == t.id && t.exec.session.is_some();
        let Some(t) = self.tabs.get(p.tab).filter(active) else { return false };
        let Some(rs) = rows_of(t).filter(|rs| rs.id == p.result) else { return false };
        let ready = matches!(&rs.edit.target, Target::Ready(x) if **x == p.target);
        ready
            && rs.edit.batch.revision() == p.revision
            && rs.edit.bound.as_ref() == Some(&p.bound)
            && self.edit_bound(t) == Some(p.bound.clone())
    }

    /// After every event: the preview closes when it is no longer current, and says so.
    pub(super) fn sweep_edit_preview(&mut self) {
        if self.overlays.edit_preview().is_some() && !self.edit_preview_current() {
            self.overlays.close(OverlayKind::EditPreview);
            self.flash(Notice::new(Label::EditPreviewClosed, Level::Info));
        }
    }

    /// How an Apply in tab `tab` stands to a transaction now, in words: inside the open one,
    /// opening one for manual commit, or committed at its end.
    pub(super) fn preview_tx_text(&self, tab: TabId) -> String {
        let label = match self.tabs.get(tab) {
            Some(t) if t.exec.user_tx() => Label::EditPreviewTxInside,
            _ if self.manual_commit(tab) => Label::EditPreviewTxOpens,
            _ => Label::EditPreviewTxCommit,
        };
        self.i18n.label(label).to_string()
    }

    /// The model of the open preview's changes (`None`: none is open, or its result is not shown).
    fn edit_preview_model(&self) -> Option<std::sync::Arc<ChangesModel>> {
        let p = self.overlays.edit_preview()?;
        self.changes_model(p.tab, p.result, p.format)
    }

    /// What the open preview shows (`None`: none is open).
    pub fn edit_preview_view(&self) -> Option<PreviewView> {
        let p = self.overlays.edit_preview()?;
        let m = self.edit_preview_model()?;
        let i18n = &self.i18n;
        let mut apply = matches!(m.planned, Planned::Ready(_));
        // Sent and not answered yet; or why the last Apply applied nothing.
        let mut first = Vec::new();
        let rs = self.tabs.get(p.tab).and_then(rows_of).filter(|rs| rs.id == p.result);
        if rs.is_some_and(|rs| rs.edit.applying.is_some()) {
            first.push(PreviewLine::Note(i18n.label(Label::EditApplying).to_string()));
            apply = false;
        } else if let Some(e) = rs.and_then(|rs| rs.edit.failed.as_ref().filter(|f| f.revision == p.revision)) {
            first.push(PreviewLine::Problem(e.text.clone()));
        } else if rs.is_some_and(|rs| rs.edit.apply_unknown) {
            first.push(PreviewLine::Problem(i18n.label(Label::EditFailedUnknown).to_string()));
        }
        // Applied again only once the user says so, warned.
        if rs.is_some_and(|rs| rs.edit.apply_unknown) {
            if p.again && apply {
                first.push(PreviewLine::Problem(i18n.label(Label::EditPreviewAgain).to_string()));
            }
            apply &= p.again;
        }
        first.extend(m.sql.iter().cloned());
        let footer = self.preview_tx_text(p.tab);
        Some(PreviewView {
            title: m.title.clone(),
            summary: m.summary.clone(),
            lines: first,
            footer,
            apply,
            dialect: m.dialect,
        })
    }

    /// Tab `tab`'s result `result` is being applied.
    fn applying(&self, tab: TabId, result: u64) -> bool {
        self.tabs.get(tab).and_then(rows_of).is_some_and(|rs| rs.id == result && rs.edit.applying.is_some())
    }

    /// The last Apply of tab `tab`'s result `result` was not answered: it may be applied.
    fn apply_unknown(&self, tab: TabId, result: u64) -> bool {
        self.tabs.get(tab).and_then(rows_of).is_some_and(|rs| rs.id == result && rs.edit.apply_unknown)
    }

    /// The preview's buttons, Cancel first (it has the focus), each with the key it presses:
    /// Cancel, Copy SQL, and Apply while the changes can be applied.
    pub fn edit_preview_buttons(&self) -> Vec<(Label, KeyCode)> {
        let mut b = vec![(Label::DialogButtonCancel, KeyCode::Esc), (Label::EditPreviewCopy, KeyCode::Char('c'))];
        if self.edit_preview_view().is_some_and(|v| v.apply) {
            b.push((Label::EditPreviewApply, KeyCode::Char('y')));
        }
        b
    }

    /// Keys of the preview: `y` applies, `c` copies the statements, `1` / `a` answer which of
    /// several identical rows a change is for, `Esc`, `Enter` and `q` close it, `j`/`k` and the
    /// arrows scroll. Held keys only scroll.
    pub(in crate::app) fn edit_preview_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(p) = self.overlays.edit_preview_mut() else { return };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => p.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => p.scroll = p.scroll.saturating_sub(1),
            _ if repeat => {}
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlays.close(OverlayKind::EditPreview),
            KeyCode::Char('c') => self.edit_preview_copy(),
            KeyCode::Char('y') => self.edit_preview_apply(),
            KeyCode::Char(k @ ('1' | 'a')) => {
                let Some(m) = self.edit_preview_model() else { return };
                let Planned::Refused(PlanError::Choose { row, .. }) = &m.planned else { return };
                let row = *row;
                let Some(p) = self.overlays.edit_preview() else { return };
                if let Some(rs) = self.tabs.get_mut(p.tab).and_then(rows_of_mut).filter(|rs| rs.id == p.result) {
                    rs.edit.checks.choose(row, if k == '1' { Choice::One } else { Choice::All });
                }
                self.clear_cant_apply();
            }
            _ => {}
        }
    }

    /// `c`: every statement of the plan, in order, to the clipboard.
    fn edit_preview_copy(&mut self) {
        let Some(m) = self.edit_preview_model() else { return };
        let Planned::Ready(plan) = &m.planned else {
            return self.flash(Notice::new(Label::EditPreviewNothingToCopy, Level::Info));
        };
        self.copy_steps(&plan.steps);
    }

    /// Statements `steps`, in order and whole, to the clipboard: each as it is sent, with `;`
    /// and a line break after it.
    pub(super) fn copy_steps(&mut self, steps: &[edit::Step]) {
        let text: String = steps.iter().map(|s| format!("{};\n", s.sql)).collect();
        let count = steps.len() as u64;
        let done = match self.deliver(&text) {
            Ok(m) => Notice::new(Msg::EditPreviewCopied { count, method: self.method_text(m) }, Level::Success),
            Err(msg) => Notice::new(msg, Level::Error),
        };
        self.show_status(done.clone());
        self.flash(done);
    }

    /// `y`: the plan's statements go to the tab's session, when the changes can be applied and
    /// the result may still be edited; why not, said. After an Apply whose result is not known,
    /// the first `y` warns, the second applies.
    fn edit_preview_apply(&mut self) {
        let Some(p) = self.overlays.edit_preview() else { return };
        if self.applying(p.tab, p.result) {
            return self.say_cant_apply(self.i18n.label(Label::EditApplying).to_string());
        }
        let Some(m) = self.edit_preview_model() else { return };
        let Planned::Ready(plan) = &m.planned else {
            return self.say_cant_apply(m.why_not.clone().unwrap_or_default());
        };
        let (tab, result, bound) = (p.tab, p.result, p.bound.clone());
        // A run, a statement waiting for its connection, a commit or rollback not answered (the
        // app's own for an idle transaction too): the Apply would land in what that leaves.
        if self.tab_busy(tab) {
            return self.flash_busy();
        }
        if self.apply_unknown(tab, result)
            && let Some(p) = self.overlays.edit_preview_mut().filter(|p| !p.again)
        {
            p.again = true;
            return;
        }
        // Asked again: the connection, its environment and the rows' binding.
        if self.edit_gate().is_none_or(|(_, _, b)| b != bound) || !self.edit_preview_current() {
            return self.overlays.close(OverlayKind::EditPreview);
        }
        let tx = match self.tabs.get(tab) {
            Some(t) if self.manual_commit(tab) => ApplyTx::Keep { idle_timeout: self.manual_idle_timeout(t) },
            _ => ApplyTx::Commit,
        };
        self.send_apply(tab, result, plan.steps.clone(), tx);
    }

    pub(super) fn say_cant_apply(&mut self, reason: String) {
        self.flash(Notice::new(Msg::EditPreviewCantApply { reason }, Level::Info));
    }

    /// Why the changes couldn't be applied, in the status bar, is over: a choice was made or
    /// the Apply sent.
    fn clear_cant_apply(&mut self) {
        if self.transient.as_ref().is_some_and(|(n, _)| matches!(n.msg, Msg::EditPreviewCantApply { .. })) {
            self.transient = None;
        }
    }

    /// The Apply of `steps` (the changes of tab `tab`'s result `result`) on the tab's session,
    /// in a transaction as `tx` says; bound to what it was sent for. A held result's portal is
    /// closed first (its rows stay). The preview stays open, saying it applies.
    pub(super) fn send_apply(&mut self, tab: TabId, result: u64, steps: Vec<edit::Step>, tx: ApplyTx) {
        self.clear_cant_apply();
        self.query_seq += 1;
        let id = self.query_seq;
        let now = self.now();
        let Some(t) = self.tabs.get_mut(tab) else { return };
        // Checked before anything changes: nothing leaves the tab busy with nothing sent.
        let Some(profile) = t.profile.filter(|_| rows_of(t).is_some_and(|rs| rs.id == result)) else { return };
        if matches!(t.exec.paging, Paging::Open { .. }) {
            t.exec.paging = Paging::Replaced;
            t.exec.rerun_ok = t.exec.prepared.repeatable(t.answer_sql()).is_ok();
            let qid = t.exec.query_id;
            self.send_tab(tab, DbCommand::ClosePortal { id: qid });
        }
        let Some(t) = self.tabs.get_mut(tab) else { return };
        // The user's transaction it runs in, or the one it opens for manual commit (the next).
        let epoch = match (t.exec.in_block, tx) {
            (true, _) => Some(t.exec.tx_epoch),
            (false, ApplyTx::Keep { .. }) => Some(t.exec.tx_epoch + 1),
            (false, ApplyTx::Commit) => None,
        };
        let session = t.exec.generation;
        let Some(rs) = rows_of_mut(t).filter(|rs| rs.id == result) else { return };
        let batch = rs.edit.batch.clone();
        rs.edit.failed = None;
        rs.edit.applying = Some(Box::new(Applying { id, profile, session, batch, steps: steps.clone(), tx: epoch }));
        t.exec.running = Some(Running {
            id,
            started: now,
            fetch: false,
            count: false,
            estimate: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: true,
        });
        if let Some(p) = self.overlays.edit_preview_mut() {
            p.again = false;
        }
        self.changes_body = None;
        self.send_tab(tab, DbCommand::ApplyEdits { id, steps, tx });
    }

    /// `space d x`, `:discard`: drop every pending change of the shown result, once the user
    /// says so (`Enter` keeps them, `y` drops them).
    pub(super) fn edit_discard_ask(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let count = rs.edit.batch.counts().total() as u64;
        if count == 0 {
            return self.flash(Notice::new(Label::EditNothingPending, Level::Info));
        }
        let (tab, result) = (t.id, rs.id);
        self.confirm(
            Label::EditDiscardTitle,
            Label::EditDiscardKeys,
            Label::EditDiscardKeys,
            ConfirmAction::DiscardEdits { tab, result },
        );
        if let Some(c) = self.overlays.confirm_mut() {
            c.text = Msg::EditDiscardText { count };
        }
    }

    /// Yes to dropping the pending changes of tab `tab`'s result `result`, while it is shown.
    pub(in crate::app) fn edit_discard(&mut self, tab: TabId, result: u64) {
        let Some(t) = self.tabs.get_mut(tab) else { return };
        let Some(rs) = rows_of_mut(t).filter(|rs| rs.id == result) else { return };
        // Asked before the Apply was sent: the changes being applied stay.
        if rs.edit.applying.is_some() {
            return self.say_refused(Label::EditWhyApplying.into());
        }
        let count = rs.edit.batch.counts().total() as u64;
        if !rs.edit.batch.discard() {
            return;
        }
        if self.tab().id == tab {
            self.edit_grid_sync();
        }
        self.flash(Notice::new(Msg::EditDiscarded { count }, Level::Info));
    }
}
