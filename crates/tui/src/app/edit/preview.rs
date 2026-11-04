//! The preview of a result's pending changes (`:changes`, `:apply`): the statements that apply
//! them, each with the count it must give, in a dialog that is also the question whether to
//! apply them. Cancel is the default: only `y` applies.
//!
//! Rows found by their values are counted first, with plain reads on the tab's own session (they
//! see its open transaction, as the Apply will): until every count came the dialog says it
//! counts, and a failure is shown instead of the statements. The plan is made from the changes
//! as the dialog opened on; a change of the result, its rows' connection or its changes closes
//! the dialog.
//!
//! `y` sends the Apply on the tab's session: the dialog says it applies until the answer comes
//! (the query's cancel key cancels it), closes once the changes are applied, and says why when
//! nothing was.

use super::*;
use crate::app::overlay::Buttons;
use datarig_core::driver::ApplyTx;
use datarig_core::edit::{Choice, Facts, Origin, Plan, PlanError};
use std::collections::{BTreeMap, BTreeSet};

mod literals;

/// The open preview of tab `tab`'s result `result`: the changes and the target it was opened
/// on, what was counted, and the user's choices.
pub struct EditPreview {
    pub tab: TabId,
    pub result: u64,
    bound: Bound,
    batch: EditBatch,
    target: EditTarget,
    format: EditFormat,
    /// Cells (row, column) whose value as read is not shown whole.
    not_whole: BTreeSet<(usize, usize)>,
    /// Counts asked and not answered yet: their id and the row each counts.
    asked: Vec<(u64, usize)>,
    matches: BTreeMap<usize, u64>,
    choices: BTreeMap<usize, Choice>,
    /// Why the rows could not be counted.
    failed: Option<String>,
    /// Why the last Apply applied nothing.
    pub(super) apply_failed: Option<String>,
    /// After an Apply whose result is not known, `y` was pressed once: the next applies again.
    again: bool,
    /// The first line of the body on screen.
    pub scroll: usize,
    pub buttons: Buttons,
    /// The body as last laid out (rebuilt when its width, the theme or its lines change).
    pub(crate) body: Option<PreviewBody>,
    /// How many times the body was laid out.
    pub body_builds: u64,
    /// The line of text values read back from each statement, by the language and the statement (statements are
    /// planned again for every frame; reading their values back is not).
    literals: std::cell::RefCell<Vec<(Lang, String, Option<String>)>>,
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

/// Where the preview's plan stands.
enum Planned {
    Counting,
    Failed(String),
    /// New rows' cells left to a default their column lacks (`PlanError::NoDefault`,
    /// `PlanError::NotShown`).
    NoDefault(Vec<PlanError>),
    Refused(PlanError),
    Ready(Plan),
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
const VALUE_W: usize = 40;

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
    /// `space d p` / `space d a`, `:changes` / `:apply`: the preview of the shown result's
    /// pending changes. Refused as an edit key is, and with nothing pending; the rows found by
    /// their values are counted first.
    pub(super) fn edit_preview_open(&mut self) {
        let Some((_, format, bound)) = self.edit_gate() else { return };
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        if rs.edit.batch.is_empty() {
            return self.flash(Notice::new(Label::EditNothingPending, Level::Info));
        }
        let Target::Ready(target) = &rs.edit.target else {
            return self.say_edit_read_only(Label::EditWhyNotResolved);
        };
        if self.tab_busy(t.id) {
            return self.flash_busy();
        }
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let mut not_whole = BTreeSet::new();
        for (row, e) in rs.edit.batch.rows() {
            for (c, v) in e.orig.iter().enumerate() {
                if let Some(v) = v
                    && c < rs.columns.len()
                    && !shown_whole(d, rs, c, v, zone)
                {
                    not_whole.insert((row, c));
                }
            }
        }
        let (tab, result, batch, target) = (t.id, rs.id, rs.edit.batch.clone(), (**target).clone());
        // Nothing is counted for changes that cannot be made anyway.
        let counts = match edit::no_default(&target, &batch).is_empty() && edit::not_shown(&target, &batch).is_empty() {
            true => {
                let whole = |r: usize, c: usize| !not_whole.contains(&(r, c));
                edit::count_statements(&target, &batch, &*edit::format(format), &whole).unwrap_or_default()
            }
            false => Vec::new(),
        };
        let mut asked = Vec::with_capacity(counts.len());
        for (row, sql) in counts {
            self.query_seq += 1;
            let id = self.query_seq;
            self.send_tab(tab, DbCommand::Count { id, sql });
            asked.push((id, row));
        }
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        let p = EditPreview {
            tab,
            result,
            bound,
            batch,
            target,
            format,
            not_whole,
            asked,
            matches: BTreeMap::new(),
            choices: BTreeMap::new(),
            failed: None,
            apply_failed: None,
            again: false,
            scroll: 0,
            buttons: Buttons::default(),
            body: None,
            body_builds: 0,
            literals: Default::default(),
        };
        self.overlays.push(Overlay::EditPreview(Box::new(p)));
    }

    /// The count `qid` of tab `tab` came: the preview's, when it asked it (`true`).
    pub(in crate::app) fn edit_preview_counted(&mut self, tab: TabId, qid: u64, result: &Result<u64, DbError>) -> bool {
        let text = result.as_ref().err().map(|e| self.db_error_text(e));
        let Some(p) = self.overlays.edit_preview_mut().filter(|p| p.tab == tab) else { return false };
        let Some(k) = p.asked.iter().position(|(id, _)| *id == qid) else { return false };
        let (_, row) = p.asked.remove(k);
        match (result, text) {
            (Ok(n), _) => {
                p.matches.insert(row, *n);
            }
            // The other counts are not waited for.
            (Err(_), text) => {
                p.failed = text;
                p.asked.clear();
            }
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
            && rs.edit.batch == p.batch
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

    fn edit_planned(&self, p: &EditPreview) -> Planned {
        let missing: Vec<PlanError> = edit::no_default(&p.target, &p.batch)
            .into_iter()
            .map(|(id, column)| PlanError::NoDefault { origin: Origin::New(id), column })
            .chain(
                edit::not_shown(&p.target, &p.batch)
                    .into_iter()
                    .map(|(id, column)| PlanError::NotShown { origin: Origin::New(id), column }),
            )
            .collect();
        if !missing.is_empty() {
            return Planned::NoDefault(missing);
        }
        if let Some(e) = &p.failed {
            return Planned::Failed(e.clone());
        }
        if !p.asked.is_empty() {
            return Planned::Counting;
        }
        let whole = |r: usize, c: usize| !p.not_whole.contains(&(r, c));
        let facts = Facts { whole: &whole, matches: &p.matches, choices: &p.choices };
        match edit::plan(&p.target, &p.batch, &*edit::format(p.format), &facts) {
            Ok(plan) => Planned::Ready(plan),
            Err(e) => Planned::Refused(e),
        }
    }

    /// The row the grid shows for `origin`, counted from 1, as text.
    fn origin_row(&self, p: &EditPreview, origin: Origin) -> String {
        let shown = match origin {
            Origin::Row(r) => Some(r),
            Origin::New(id) => {
                let len = self.tabs.get(p.tab).and_then(rows_of).map_or(0, |rs| rs.rows.len() + rs.edit.applied_new());
                p.batch.drafts().iter().position(|d| d.id == id).map(|i| len + i)
            }
        };
        shown.map_or_else(|| "?".to_string(), |r| (r + 1).to_string())
    }

    /// The name of result column `c` of the preview's result.
    fn preview_column(&self, p: &EditPreview, c: usize) -> String {
        let rs = self.tabs.get(p.tab).and_then(rows_of);
        rs.and_then(|rs| rs.columns.get(c)).map(|c| c.meta.name.clone()).unwrap_or_default()
    }

    /// Why the plan was refused, in words.
    fn plan_error_text(&self, p: &EditPreview, e: &PlanError) -> String {
        let row = |r: usize| self.origin_row(p, Origin::Row(r));
        let msg = match e {
            PlanError::Column { origin, column, why } => {
                let reason = self.i18n.msg(&column_reason(*why, self.preview_column(p, *column))).to_string();
                Msg::EditPreviewColumn { row: self.origin_row(p, *origin), reason }
            }
            PlanError::NotWhole { row: r, column } => {
                Msg::EditPreviewNotWhole { row: row(*r), column: self.preview_column(p, *column) }
            }
            PlanError::NotCounted { row: r } => Msg::EditPreviewNotCounted { row: row(*r) },
            PlanError::Gone { row: r } => Msg::EditPreviewGone { row: row(*r) },
            PlanError::Ambiguous { row: r, matches } => Msg::EditPreviewAmbiguous { row: row(*r), count: *matches },
            PlanError::Choose { row: r, matches } => Msg::EditPreviewChoose { row: row(*r), count: *matches },
            PlanError::NoOneOfK { row: r } => {
                let count = p.matches.get(r).copied().unwrap_or(0);
                Msg::EditPreviewNoOneOfK { row: row(*r), count }
            }
            PlanError::NoDefault { origin, column } => {
                Msg::EditPreviewNoDefault { row: self.origin_row(p, *origin), column: self.preview_column(p, *column) }
            }
            PlanError::NotShown { origin, column } => {
                Msg::EditPreviewNotShown { row: self.origin_row(p, *origin), column: column.clone() }
            }
            PlanError::Internal(_) => Label::EditPreviewInternal.into(),
        };
        self.i18n.msg(&msg).to_string()
    }

    /// How an Apply in tab `tab` stands to a transaction now, in words: inside the open one,
    /// opening one for manual commit, or committed at its end.
    fn preview_tx_text(&self, tab: TabId) -> String {
        let label = match self.tabs.get(tab) {
            Some(t) if t.exec.user_tx() => Label::EditPreviewTxInside,
            _ if self.manual_commit(tab) => Label::EditPreviewTxOpens,
            _ => Label::EditPreviewTxCommit,
        };
        self.i18n.label(label).to_string()
    }

    /// What the open preview shows (`None`: none is open).
    pub fn edit_preview_view(&self) -> Option<PreviewView> {
        let p = self.overlays.edit_preview()?;
        let i18n = &self.i18n;
        let table = format!("{}.{}", p.target.table.schema, p.target.table.name);
        let title = i18n.msg(&Msg::EditPreviewTitle { table });
        let c = p.batch.counts();
        let summary = [
            (c.updates > 0).then_some(Msg::EditPreviewUpdates { count: c.updates as u64 }),
            (c.inserts > 0).then_some(Msg::EditPreviewInserts { count: c.inserts as u64 }),
            (c.deletes > 0).then_some(Msg::EditPreviewDeletes { count: c.deletes as u64 }),
        ]
        .into_iter()
        .flatten()
        .map(|m| i18n.msg(&m).to_string())
        .collect::<Vec<_>>()
        .join(" · ");
        let dialect = edit::format(p.format).language().dialect();
        let mut lines = Vec::new();
        let mut apply = false;
        match self.edit_planned(p) {
            Planned::Counting => lines.push(PreviewLine::Note(i18n.label(Label::EditPreviewCounting).to_string())),
            Planned::Failed(error) => {
                lines.push(PreviewLine::Problem(i18n.msg(&Msg::EditPreviewCountFailed { error }).to_string()))
            }
            Planned::NoDefault(errors) => {
                for e in errors {
                    lines.push(PreviewLine::Problem(self.plan_error_text(p, &e)));
                }
            }
            Planned::Refused(e @ PlanError::Choose { matches, .. }) => {
                let keys = i18n.msg(&Msg::EditPreviewChooseKeys { count: matches }).to_string();
                lines.push(PreviewLine::Choose { text: self.plan_error_text(p, &e), keys });
            }
            Planned::Refused(e) => lines.push(PreviewLine::Problem(self.plan_error_text(p, &e))),
            Planned::Ready(plan) => {
                apply = true;
                let checks: Vec<&edit::Step> =
                    plan.steps.iter().filter(|s| matches!(s.expect, Expect::Rows(_))).collect();
                if !checks.is_empty() {
                    let nowait = checks.iter().all(|s| s.sql.ends_with(" NOWAIT"));
                    let head = if nowait { Label::EditPreviewChecks } else { Label::EditPreviewChecksWait };
                    lines.push(PreviewLine::Head(i18n.label(head).to_string()));
                }
                let mut writes = false;
                for s in &plan.steps {
                    let n = match s.expect {
                        Expect::Rows(n) => n,
                        Expect::Affected(n) => {
                            if !writes {
                                writes = true;
                                lines.push(PreviewLine::Head(i18n.label(Label::EditPreviewWrites).to_string()));
                            }
                            n
                        }
                    };
                    let delete = matches!(s.expect, Expect::Affected(_)) && s.sql.starts_with("DELETE ");
                    lines.push(PreviewLine::Step {
                        sql: s.sql.clone(),
                        expect: i18n.msg(&Msg::EditPreviewExpect { count: n }).to_string(),
                        risky: delete || n > 1,
                    });
                    if let Some(text) = self.read_back(p, &s.sql, dialect) {
                        lines.push(PreviewLine::Values(text));
                    }
                }
            }
        }
        // Sent and not answered yet; or why the last Apply applied nothing.
        let mut first = Vec::new();
        let rs = self.tabs.get(p.tab).and_then(rows_of).filter(|rs| rs.id == p.result);
        if rs.is_some_and(|rs| rs.edit.applying.is_some()) {
            first.push(PreviewLine::Note(i18n.label(Label::EditApplying).to_string()));
            apply = false;
        } else if let Some(e) = &p.apply_failed {
            first.push(PreviewLine::Problem(e.clone()));
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
        first.extend(lines);
        let footer = self.preview_tx_text(p.tab);
        Some(PreviewView { title, summary, lines: first, footer, apply, dialect })
    }

    /// Read-back values as the preview says them, by where they are in their statement (`set
    /// note = 'text', … · where name = 'text'`): `column = 'text'` (a line break as one
    /// character, a long text cut), or how many bytes a value that is not text has.
    fn values_text(&self, values: &[(literals::Part, String, literals::Decoded)]) -> String {
        let one = |column: &str, v: &literals::Decoded| match v {
            literals::Decoded::Text(t) => literals::text_pair(column, t, VALUE_W),
            literals::Decoded::Bytes(n) => {
                let not_text = self.i18n.msg(&Msg::EditPreviewNotText { count: *n as u64 });
                format!("{} = {not_text}", crate::text::sanitize_cell(column))
            }
        };
        let mut groups: Vec<(literals::Part, Vec<String>)> = Vec::new();
        for (part, column, v) in values {
            match groups.last_mut() {
                Some((p, pairs)) if p == part => pairs.push(one(column, v)),
                _ => groups.push((*part, vec![one(column, v)])),
            }
        }
        let label = |p: literals::Part| {
            self.i18n.label(match p {
                literals::Part::Set => Label::EditPreviewPartSet,
                literals::Part::Where => Label::EditPreviewPartWhere,
                literals::Part::Values => Label::EditPreviewPartValues,
            })
        };
        groups
            .iter()
            .map(|(p, pairs)| format!("{} {}", label(*p), pairs.join(", ")))
            .collect::<Vec<_>>()
            .join(" \u{B7} ")
    }

    /// The text values statement `sql` of preview `p` carries as bytes, read back as the
    /// preview says them (`None`: it carries none); worked out once per statement.
    fn read_back(&self, p: &EditPreview, sql: &str, d: Dialect) -> Option<String> {
        let mut cache = p.literals.borrow_mut();
        let lang = self.i18n.lang;
        if let Some((_, _, text)) = cache.iter().find(|(l, s, _)| *l == lang && s == sql) {
            return text.clone();
        }
        let values = literals::text_literals(sql, d);
        let text = (!values.is_empty()).then(|| self.values_text(&values));
        cache.push((lang, sql.to_string(), text.clone()));
        text
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
                let Some(p) = self.overlays.edit_preview() else { return };
                if let Planned::Refused(PlanError::Choose { row, .. }) = self.edit_planned(p)
                    && let Some(p) = self.overlays.edit_preview_mut()
                {
                    p.choices.insert(row, if k == '1' { Choice::One } else { Choice::All });
                }
            }
            _ => {}
        }
    }

    /// `c`: every statement of the plan, in order, to the clipboard.
    fn edit_preview_copy(&mut self) {
        let Some(p) = self.overlays.edit_preview() else { return };
        let Planned::Ready(plan) = self.edit_planned(p) else {
            return self.flash(Notice::new(Label::EditPreviewNothingToCopy, Level::Info));
        };
        let text: String = plan.steps.iter().map(|s| format!("{};\n", s.sql)).collect();
        let count = plan.steps.len() as u64;
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
        let plan = match self.edit_planned(p) {
            Planned::Ready(plan) => plan,
            Planned::Counting => return self.say_cant_apply(self.i18n.label(Label::EditPreviewCounting).to_string()),
            Planned::Failed(error) => {
                return self.say_cant_apply(self.i18n.msg(&Msg::EditPreviewCountFailed { error }).to_string());
            }
            Planned::NoDefault(errors) => return self.say_cant_apply(self.plan_error_text(p, &errors[0])),
            Planned::Refused(e) => return self.say_cant_apply(self.plan_error_text(p, &e)),
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
        self.send_apply(tab, result, plan.steps, tx);
    }

    fn say_cant_apply(&mut self, reason: String) {
        self.flash(Notice::new(Msg::EditPreviewCantApply { reason }, Level::Info));
    }

    /// The Apply of `steps` (the changes of tab `tab`'s result `result`) on the tab's session,
    /// in a transaction as `tx` says; bound to what it was sent for. A held result's portal is
    /// closed first (its rows stay). The preview stays open, saying it applies.
    fn send_apply(&mut self, tab: TabId, result: u64, steps: Vec<edit::Step>, tx: ApplyTx) {
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
        rs.edit.applying = Some(Box::new(Applying { id, profile, session, batch, steps: steps.clone(), tx: epoch }));
        t.exec.running = Some(Running {
            id,
            started: now,
            fetch: false,
            count: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: true,
        });
        if let Some(p) = self.overlays.edit_preview_mut() {
            p.apply_failed = None;
            p.again = false;
        }
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
