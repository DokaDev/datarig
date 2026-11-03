//! After the Apply of a result's changes: its answer, bound to what it was sent for, becomes the
//! applied layer over the rows as read (the values written, the rows deleted, the rows added
//! with what the table stored for them when it said), or says why nothing was applied, naming
//! the row and the statement. Sent is not done: only the answer changes the result.
//!
//! Under manual commit, or inside an open transaction, the layer is not committed until that
//! transaction ends: a commit keeps it, a rollback (the user's, the app's for an idle
//! transaction, the server's, a lost connection) drops it ([`EditState::tx_ended`]).

use super::*;
use datarig_core::driver::{AppliedStep, ApplyError, ApplyFailure};
use datarig_core::edit::{Origin, Step};
use std::collections::{BTreeMap, BTreeSet};

/// An Apply sent and not answered yet: its id, what it was sent under, and what it applies.
#[derive(Debug)]
pub struct Applying {
    pub(super) id: u64,
    pub(super) profile: ProfileId,
    /// The tab's query session it was sent on.
    pub(super) session: u64,
    pub(super) batch: EditBatch,
    pub(super) steps: Vec<Step>,
    /// The user's transaction it runs in, or opens for manual commit; `None`: it commits.
    pub(super) tx: Option<u64>,
}

/// What an Apply changed in a result, shown over its rows as read until the query runs again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Rows read whose values changed: each column's value now.
    pub set: BTreeMap<usize, BTreeMap<usize, Value>>,
    pub deleted: BTreeSet<usize>,
    /// The rows added, each column's value (`Value::Default`: what the table filled in is not
    /// known).
    pub new: Vec<Vec<Value>>,
    /// The user's transaction it is in until that ends (not committed yet).
    pub tx: Option<u64>,
}

/// The value result column `c` of `rs` holds in a row the server returned (`columns`, `row`):
/// by the name of the table column it is.
fn returned(rs: &ResultSet, c: usize, columns: &[ColumnMeta], row: &[Cell]) -> Option<Value> {
    let Target::Ready(t) = &rs.edit.target else { return None };
    let name = &t.columns.get(c)?.as_ref()?.column.name;
    let i = columns.iter().position(|m| &m.name == name)?;
    Some(match row.get(i)? {
        Some(v) => Value::Text(v.clone()),
        None => Value::Null,
    })
}

/// The layer Apply `a` makes on `rs`, its steps answered by `done`: each changed row's values
/// as sent, or as the server returned the row; the rows added, with what the server returned
/// for them, else the number an `AUTO_INCREMENT` column took.
fn layer(rs: &ResultSet, a: &Applying, done: &[AppliedStep]) -> Applied {
    let mut out = Applied { tx: a.tx, ..Applied::default() };
    for (row, e) in a.batch.rows() {
        if e.delete {
            out.deleted.insert(row);
        } else {
            out.set.insert(row, e.set.clone());
        }
    }
    let mut new: BTreeMap<u64, Vec<Value>> = a.batch.drafts().iter().map(|d| (d.id, d.cells.clone())).collect();
    let auto = |c: usize| match &rs.edit.target {
        Target::Ready(t) => {
            t.columns.get(c).and_then(Option::as_ref).is_some_and(|c| c.column.fill == ColumnFill::AutoIncrement)
        }
        _ => false,
    };
    for (s, d) in a.steps.iter().zip(done) {
        if !matches!(s.expect, Expect::Affected(_)) {
            continue;
        }
        let back = d.returned.as_ref().and_then(|(cols, rows)| Some((cols, rows.first()?)));
        match s.origin {
            Origin::Row(r) if !out.deleted.contains(&r) => {
                let (Some((cols, row)), Some(e)) = (back, a.batch.row(r)) else { continue };
                let set = out.set.entry(r).or_default();
                for c in 0..rs.columns.len() {
                    // What the server stored where it differs from the value as read (a
                    // trigger's, a type's own form), and every value set.
                    let Some(v) = returned(rs, c, cols, row) else { continue };
                    let read = match e.orig.get(c) {
                        Some(Some(t)) => Value::Text(t.clone()),
                        _ => Value::Null,
                    };
                    if v != read || set.contains_key(&c) {
                        set.insert(c, v);
                    }
                }
            }
            Origin::New(id) => {
                let Some(cells) = new.get_mut(&id) else { continue };
                if let Some((cols, row)) = back {
                    for (c, cell) in cells.iter_mut().enumerate() {
                        if let Some(v) = returned(rs, c, cols, row) {
                            *cell = v;
                        }
                    }
                } else if let Some(n) = d.insert_id {
                    for (c, cell) in cells.iter_mut().enumerate() {
                        if *cell == Value::Default && auto(c) {
                            *cell = Value::Text(n.to_string());
                        }
                    }
                }
            }
            Origin::Row(_) => {}
        }
    }
    out.new = a.batch.drafts().iter().filter_map(|d| new.remove(&d.id)).collect();
    out
}

impl App {
    /// The answer `result` to Apply `qid` of tab `tab`: taken only by the result it was sent
    /// for, while its tab is bound and its changes are as they were (another answer is
    /// dropped). Done: the applied layer, the changes cleared with their history, the preview
    /// closed. Not done: why, naming the row and its statement; the changes stay.
    pub(in crate::app) fn edit_applied(
        &mut self,
        tab: TabId,
        qid: u64,
        result: Result<Vec<AppliedStep>, ApplyFailure>,
    ) {
        let Some(t) = self.tabs.get_mut(tab) else { return };
        if t.exec.running.is_some_and(|r| r.apply && r.id == qid) {
            t.exec.running = None;
        }
        let (profile, session) = (t.profile, t.exec.generation);
        let Some(rs) = t.row_results_mut().find(|rs| rs.edit.applying.as_ref().is_some_and(|a| a.id == qid)) else {
            return;
        };
        let Some(a) = rs.edit.applying.take() else { return };
        if profile != Some(a.profile) || session != a.session || rs.edit.batch != a.batch {
            return;
        }
        let result_id = rs.id;
        let count = a.batch.counts().total() as u64;
        // Done: the layer, and the changes cleared with their history.
        let mut done = None;
        match &result {
            Ok(steps) => {
                let applied = layer(rs, &a, steps);
                done = Some(if applied.tx.is_some() {
                    Msg::EditAppliedKept { count }
                } else {
                    Msg::EditApplied { count }
                });
                rs.edit.applied = Some(Box::new(applied));
                rs.edit.batch.clear();
            }
            Err(f) if f.why == ApplyError::Db(DbError::CommitUnknown) => rs.edit.apply_unknown = true,
            Err(_) => {}
        }
        let notice = match (&result, done) {
            (_, Some(msg)) => {
                if self.overlays.edit_preview().is_some_and(|p| p.tab == tab && p.result == result_id) {
                    self.overlays.close(OverlayKind::EditPreview);
                }
                Notice::new(msg, Level::Success)
            }
            (Ok(_), None) => return,
            (Err(f), None) => {
                let msg = self.apply_failure(tab, result_id, &a, f);
                let level =
                    if matches!(f.why, ApplyError::Db(DbError::Cancelled)) { Level::Warning } else { Level::Error };
                let text = self.i18n.msg(&msg).to_string();
                if let Some(p) = self.overlays.edit_preview_mut().filter(|p| p.tab == tab && p.result == result_id) {
                    p.apply_failed = Some(text);
                }
                Notice::new(msg, level)
            }
        };
        if let Some(t) = self.tabs.get_mut(tab) {
            t.exec.run.notes.push(notice.clone());
        }
        if self.tab().id == tab {
            self.edit_grid_sync();
            self.flash(notice.clone());
        }
        self.tab_status(tab, notice);
    }

    /// Why Apply `a` of tab `tab`'s result `result` applied nothing: the row of the
    /// step that failed (its key as read, when it has one) and, for a server's error, the
    /// statement.
    fn apply_failure(&self, tab: TabId, result: u64, a: &Applying, f: &ApplyFailure) -> Msg {
        let step = f.step.and_then(|i| a.steps.get(i));
        let row = step.map(|s| self.applied_row(tab, result, a, s.origin));
        match (&f.why, row) {
            (ApplyError::Db(DbError::Cancelled), _) => Label::EditFailedCancelled.into(),
            (ApplyError::Db(DbError::CommitUnknown), _) => Label::EditFailedUnknown.into(),
            (ApplyError::Count { got: 0 }, Some(row)) => Msg::EditFailedChanged { row },
            (ApplyError::Count { got }, Some(row)) => {
                let expected = match step.map(|s| s.expect) {
                    Some(Expect::Rows(n) | Expect::Affected(n)) => n.to_string(),
                    None => String::new(),
                };
                Msg::EditFailedCount { row, count: *got, expected }
            }
            (ApplyError::Db(DbError::LockBusy(_)), Some(row)) => Msg::EditFailedLocked { row },
            (ApplyError::Db(e @ (DbError::ReadWriteRefused | DbError::NotSupported)), _) => {
                Msg::EditFailedPlain { error: self.db_error_text(e) }
            }
            (ApplyError::Db(e), Some(row)) => {
                let sql = step.map(|s| crate::app::runlog::excerpt(&s.sql, 200)).unwrap_or_default();
                Msg::EditFailedDb { row, error: self.db_error_text(e), sql }
            }
            (ApplyError::Db(e), None) => Msg::EditFailedPlain { error: self.db_error_text(e) },
            (ApplyError::Count { .. }, None) => Label::EditPreviewInternal.into(),
        }
    }

    /// The row the grid showed for `origin` when Apply `a` was sent, counted from 1, with its
    /// key as read (`12 (id = 5)`) when its rows are found by a key.
    fn applied_row(&self, tab: TabId, result: u64, a: &Applying, origin: Origin) -> String {
        let t = self.tabs.get(tab);
        let rs = t.and_then(|t| {
            let shown = match &t.results {
                Results::Rows(rs) if rs.id == result => Some(rs),
                _ => None,
            };
            shown.or_else(|| t.exec.steps.values().map(|p| &p.rs).find(|rs| rs.id == result))
        });
        let Some(rs) = rs else { return "?".to_string() };
        match origin {
            Origin::New(id) => {
                let len = rs.rows.len() + rs.edit.applied_new();
                a.batch
                    .drafts()
                    .iter()
                    .position(|d| d.id == id)
                    .map_or_else(|| "?".to_string(), |i| (len + i + 1).to_string())
            }
            Origin::Row(r) => {
                let key = match (&rs.edit.target, a.batch.row(r)) {
                    (Target::Ready(t), Some(e)) => match &t.identity {
                        Identity::Key { columns, .. } => {
                            let parts: Vec<String> = columns
                                .iter()
                                .filter_map(|&c| {
                                    let name = &rs.columns.get(c)?.meta.name;
                                    let v = e.orig.get(c)?.as_deref()?;
                                    Some(format!("{name} = {}", crate::text::clip(&crate::text::sanitize_cell(v), 40)))
                                })
                                .collect();
                            (parts.len() == columns.len()).then(|| parts.join(", "))
                        }
                        Identity::Values { .. } => None,
                    },
                    _ => None,
                };
                match key {
                    Some(k) => format!("{} ({k})", r + 1),
                    None => (r + 1).to_string(),
                }
            }
        }
    }
}
