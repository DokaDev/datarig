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
    /// The cells (row read, column) the Apply wrote: they, and the rows added, are shown as
    /// applied (a value a trigger changed is shown as it is now, not as written).
    pub written: BTreeSet<(usize, usize)>,
    pub deleted: BTreeSet<usize>,
    /// The rows added, each column's value (`Value::Default`: what the table filled in is not
    /// known).
    pub new: Vec<Vec<Value>>,
    /// Cells whose value the server returned in another form than the result shows (row read,
    /// column; row added by its place in `new`, column): a run shows them.
    pub rerun: BTreeSet<(usize, usize)>,
    pub rerun_new: BTreeSet<(usize, usize)>,
    /// The user's transaction it is in until that ends (not committed yet).
    pub tx: Option<u64>,
    /// The changes it applied, and the statements it sent for them.
    pub batch: EditBatch,
    pub sent: Vec<Step>,
}

/// Why an Apply applied nothing: what it said, the statement that failed (when one did), for
/// the changes at `revision`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyFailed {
    pub text: String,
    pub sql: Option<String>,
    pub revision: u64,
}

/// The value result column `c` of `rs` holds in row `row` the server returned for step `d`: by
/// the name of the table column it is. `Some(None)`: returned in another form than the result
/// shows.
fn returned(rs: &ResultSet, c: usize, d: &AppliedStep, row: &[Cell]) -> Option<Option<Value>> {
    let Target::Ready(t) = &rs.edit.target else { return None };
    let (columns, _) = d.returned.as_ref()?;
    let name = &t.columns.get(c)?.as_ref()?.column.name;
    let i = columns.iter().position(|m| &m.name == name)?;
    Some(match row.get(i)? {
        Some(_) if d.unshown.contains(&i) => None,
        Some(v) => Some(Value::Text(v.clone())),
        None => Some(Value::Null),
    })
}

/// The layer Apply `a` makes on `rs`, its steps answered by `done`: each changed row's values
/// as sent, or as the server returned the row; the rows added, with what the server returned
/// for them, else the number an `AUTO_INCREMENT` column took.
fn layer(rs: &ResultSet, a: &Applying, done: &[AppliedStep]) -> Applied {
    let mut out = Applied { tx: a.tx, batch: a.batch.clone(), sent: a.steps.clone(), ..Applied::default() };
    for (row, e) in a.batch.rows() {
        if e.delete {
            out.deleted.insert(row);
        } else {
            out.set.insert(row, e.set.clone());
            out.written.extend(e.set.keys().map(|&c| (row, c)));
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
        let back = d.returned.as_ref().and_then(|(_, rows)| rows.first());
        match s.origin {
            Origin::Row(r) if !out.deleted.contains(&r) => {
                let (Some(row), Some(e)) = (back, a.batch.row(r)) else { continue };
                let set = out.set.entry(r).or_default();
                for c in 0..rs.columns.len() {
                    // What the server stored where it differs from the value as read (a
                    // trigger's), and every value set. One in another form, set or not (a
                    // trigger may have changed it), is seen by a run.
                    let written = e.set.contains_key(&c);
                    match returned(rs, c, d, row) {
                        None => {}
                        Some(None) => {
                            set.remove(&c);
                            out.rerun.insert((r, c));
                        }
                        Some(Some(v)) => {
                            let read = match e.orig.get(c) {
                                Some(Some(t)) => Value::Text(t.clone()),
                                _ => Value::Null,
                            };
                            if v != read || written {
                                set.insert(c, v);
                            }
                        }
                    }
                }
            }
            Origin::New(id) => {
                let Some(cells) = new.get_mut(&id) else { continue };
                let at = a.batch.drafts().iter().position(|d| d.id == id).unwrap_or(usize::MAX);
                if let Some(row) = back {
                    for (c, cell) in cells.iter_mut().enumerate() {
                        match returned(rs, c, d, row) {
                            Some(Some(v)) => *cell = v,
                            Some(None) => {
                                out.rerun_new.insert((at, c));
                            }
                            None => {}
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
        let result_id = rs.id;
        let count = a.batch.counts().total() as u64;
        // Done: the layer, and the changes cleared with their history.
        let mut done = None;
        // Kept in a transaction: its number, the writes and the rows they changed.
        let mut booked = None;
        let stale = profile != Some(a.profile) || session != a.session || rs.edit.batch != a.batch;
        match &result {
            // Not for the changes as they are now: what it did is not known for them, and they
            // are applied again only once the user says so.
            Ok(_) if stale => rs.edit.apply_unknown = true,
            Err(f) if f.why == ApplyError::Db(DbError::CommitUnknown) => rs.edit.apply_unknown = true,
            _ if stale => return,
            Ok(steps) => {
                let applied = layer(rs, &a, steps);
                if let Some(epoch) = applied.tx {
                    let writes: Vec<u64> = (a.steps.iter().zip(steps))
                        .filter(|(s, _)| matches!(s.expect, Expect::Affected(_)))
                        .map(|(_, d)| d.count)
                        .collect();
                    booked = Some((epoch, writes.len() as u64, writes.iter().sum::<u64>()));
                }
                done = Some(if applied.tx.is_some() {
                    Msg::EditAppliedKept { count }
                } else {
                    Msg::EditApplied { count }
                });
                rs.edit.applied = Some(Box::new(applied));
                rs.edit.batch.clear();
            }
            Err(_) => {}
        }
        // What it wrote counts in the open transaction: one entry of its summary (the block an
        // Apply opened for manual commit is reported after its answer: then).
        if let Some((epoch, writes, rows)) = booked {
            let sql = self.i18n.msg(&Msg::EditTxEntry { count: writes }).to_string();
            let entry = crate::app::tx::TxEntry { run: qid, sql, rows, elapsed: None };
            if let Some(t) = self.tabs.get_mut(tab) {
                let open = t.exec.in_block && t.exec.tx_epoch == epoch;
                match t.exec.tx.as_mut() {
                    Some(tx) if open => tx.book_apply(entry, writes),
                    _ => t.exec.apply_tx = Some((epoch, entry, writes)),
                }
            }
        }
        let notice = match (&result, done) {
            _ if stale => Notice::new(Label::EditFailedUnknown, Level::Error),
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
                let sql = f.step.and_then(|i| a.steps.get(i)).map(|s| s.sql.clone());
                if let Some(rs) = self.tabs.get_mut(tab).and_then(|t| t.row_results_mut().find(|rs| rs.id == result_id))
                {
                    rs.edit.failed = Some(ApplyFailed { text, sql, revision: rs.edit.batch.revision() });
                    rs.edit.cursor.seek = true;
                }
                self.changes_body = None;
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

    /// The row of `origin` when Apply `a` was sent, named as the Changes tab's list names it
    /// (`id=5`, `row 31`, `new 1`).
    fn applied_row(&self, tab: TabId, result: u64, a: &Applying, origin: Origin) -> String {
        let t = self.tabs.get(tab);
        let rs = t.and_then(|t| {
            let shown = match &t.results {
                Results::Rows(rs) if rs.id == result => Some(rs.as_ref()),
                _ => None,
            };
            shown.or_else(|| t.exec.steps.values().map(|p| &p.rs).find(|rs| rs.id == result))
        });
        let Some(rs) = rs else { return "?".to_string() };
        let at = match origin {
            Origin::Row(r) => RowAt::Read(r),
            Origin::New(id) => RowAt::New(id),
        };
        self.row_name_in(rs, &a.batch, at)
    }
}
