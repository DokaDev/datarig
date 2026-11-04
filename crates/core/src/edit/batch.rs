//! The pending changes of one result: cells set in its rows, rows to delete and new rows, on top
//! of the rows as read (which are never changed), with undo and redo by user action.
//!
//! Rows are named by their index in the result's row store, which only grows, so an index
//! stays the row's while the result is paged. A row's values as read are kept with its first
//! change, so they need not be read again (from the spill file) when the change is applied.

use super::sql::Value;
use crate::driver::Cell;
use std::collections::BTreeMap;

/// The changes of one row of the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowEdit {
    /// Its values as read, one per result column.
    pub orig: Vec<Cell>,
    /// The cells set, by result column; none equals its value as read.
    pub set: BTreeMap<usize, Value>,
    /// It is to be deleted (then nothing is set).
    pub delete: bool,
}

/// A new row, shown after row `after` of the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// Its name, which stays its own (undo and redo bring back the same one).
    pub id: u64,
    /// The row of the result it is shown after.
    pub after: usize,
    /// One per result column.
    pub cells: Vec<Value>,
}

/// How many statements the changes make, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub updates: usize,
    pub inserts: usize,
    pub deletes: usize,
}

impl Counts {
    pub fn total(self) -> usize {
        self.updates + self.inserts + self.deletes
    }
}

/// What one user action changed, to undo it: the touched rows as they were, and the new rows
/// as they were when it touched any.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Step {
    rows: Vec<(usize, Option<RowEdit>)>,
    drafts: Option<Vec<Draft>>,
}

/// The pending changes of one result. Two batches are equal when their changes and their history
/// are: the revision says only how often a batch moved.
#[derive(Clone, Debug, Default, Eq)]
pub struct EditBatch {
    rows: BTreeMap<usize, RowEdit>,
    drafts: Vec<Draft>,
    next_draft: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    revision: u64,
}

impl PartialEq for EditBatch {
    fn eq(&self, o: &Self) -> bool {
        (&self.rows, &self.drafts, self.next_draft, &self.undo, &self.redo)
            == (&o.rows, &o.drafts, o.next_draft, &o.undo, &o.redo)
    }
}

impl EditBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// No change is pending.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.drafts.is_empty()
    }

    pub fn counts(&self) -> Counts {
        let deletes = self.rows.values().filter(|r| r.delete).count();
        Counts { updates: self.rows.len() - deletes, inserts: self.drafts.len(), deletes }
    }

    /// The changes of row `row`.
    pub fn row(&self, row: usize) -> Option<&RowEdit> {
        self.rows.get(&row)
    }

    /// Every changed row, in order.
    pub fn rows(&self) -> impl Iterator<Item = (usize, &RowEdit)> {
        self.rows.iter().map(|(i, r)| (*i, r))
    }

    /// What cell (`row`, `col`) is set to, when it is.
    pub fn value(&self, row: usize, col: usize) -> Option<&Value> {
        self.rows.get(&row)?.set.get(&col)
    }

    /// Row `row` is to be deleted.
    pub fn deleted(&self, row: usize) -> bool {
        self.rows.get(&row).is_some_and(|r| r.delete)
    }

    /// The new rows, in the order they are shown.
    pub fn drafts(&self) -> &[Draft] {
        &self.drafts
    }

    /// A number that moves on every change of the batch or its history, so what is derived from
    /// it can be kept while it stays.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Set cells `cells` (row, column) of the result's rows to `v`, as one action; `orig` gives a
    /// row's values as read. A cell set to its value as read is no longer changed. A row to be
    /// deleted is left as it is. Whether anything changed.
    pub fn set(&mut self, cells: &[(usize, usize)], v: &Value, orig: impl Fn(usize) -> Vec<Cell>) -> bool {
        let rows: Vec<usize> = cells.iter().map(|(r, _)| *r).collect();
        self.act(&rows, false, |b| {
            for &(row, col) in cells {
                if b.deleted(row) {
                    continue;
                }
                let e = b.rows.entry(row).or_insert_with(|| RowEdit {
                    orig: orig(row),
                    set: BTreeMap::new(),
                    delete: false,
                });
                if same(v, e.orig.get(col)) {
                    e.set.remove(&col);
                } else {
                    e.set.insert(col, v.clone());
                }
                if e.set.is_empty() {
                    b.rows.remove(&row);
                }
            }
        })
    }

    /// Mark rows `rows` to be deleted, or, when every one of them already is, unmark them, as
    /// one action. A row marked loses the cells set in it.
    pub fn delete(&mut self, rows: &[usize], orig: impl Fn(usize) -> Vec<Cell>) -> bool {
        self.delete_with_new(rows, &[], orig)
    }

    /// Mark (or unmark, as [`EditBatch::delete`]) rows `rows` and remove new rows `ids`, as one
    /// action.
    pub fn delete_with_new(&mut self, rows: &[usize], ids: &[u64], orig: impl Fn(usize) -> Vec<Cell>) -> bool {
        let unmark = !rows.is_empty() && rows.iter().all(|r| self.deleted(*r));
        self.act(rows, !ids.is_empty(), |b| {
            b.drafts.retain(|d| !ids.contains(&d.id));
            for &row in rows {
                if unmark {
                    b.rows.remove(&row);
                } else {
                    let orig = b.rows.remove(&row).map(|e| e.orig).unwrap_or_else(|| orig(row));
                    b.rows.insert(row, RowEdit { orig, set: BTreeMap::new(), delete: true });
                }
            }
        })
    }

    /// Add a new row with `cells`, shown after row `after` of the result, after the others shown
    /// there; or, when `next_to` is a new row, right after it (and after its row). Its id.
    pub fn add(&mut self, after: usize, next_to: Option<u64>, cells: Vec<Value>) -> u64 {
        let id = self.next_draft;
        self.next_draft += 1;
        self.act(&[], true, |b| {
            let (at, after) = match next_to.and_then(|n| b.drafts.iter().position(|d| d.id == n)) {
                Some(i) => (i + 1, b.drafts[i].after),
                None => (b.drafts.iter().rposition(|d| d.after <= after).map_or(0, |i| i + 1), after),
            };
            b.drafts.insert(at, Draft { id, after, cells });
        });
        id
    }

    /// Set cells `cells` (new row, column) of new rows to `v`, as one action.
    pub fn set_new(&mut self, cells: &[(u64, usize)], v: &Value) -> bool {
        self.act(&[], true, |b| {
            for &(id, col) in cells {
                if let Some(c) = b.drafts.iter_mut().find(|d| d.id == id).and_then(|d| d.cells.get_mut(col)) {
                    *c = v.clone();
                }
            }
        })
    }

    /// Remove new rows `ids`, as one action.
    pub fn remove_new(&mut self, ids: &[u64]) -> bool {
        self.act(&[], true, |b| b.drafts.retain(|d| !ids.contains(&d.id)))
    }

    /// Drop the changes of cells `cells` (row, column) and of new rows' cells `new` (new row,
    /// column), as one action: a cell set goes back to its value as read, a row marked to be
    /// deleted is no longer marked, and a new row's cell goes back to its default.
    pub fn revert(&mut self, cells: &[(usize, usize)], new: &[(u64, usize)]) -> bool {
        let rows: Vec<usize> = cells.iter().map(|(r, _)| *r).collect();
        self.act(&rows, !new.is_empty(), |b| {
            for &(row, col) in cells {
                let Some(e) = b.rows.get_mut(&row) else { continue };
                if e.delete {
                    b.rows.remove(&row);
                    continue;
                }
                e.set.remove(&col);
                if e.set.is_empty() {
                    b.rows.remove(&row);
                }
            }
            for &(id, col) in new {
                if let Some(c) = b.drafts.iter_mut().find(|d| d.id == id).and_then(|d| d.cells.get_mut(col)) {
                    *c = Value::Default;
                }
            }
        })
    }

    /// Drop every pending change, as one action.
    pub fn discard(&mut self) -> bool {
        let rows: Vec<usize> = self.rows.keys().copied().collect();
        self.act(&rows, true, |b| {
            b.rows.clear();
            b.drafts.clear();
        })
    }

    /// Forget every change and its history: after the changes were applied.
    pub fn clear(&mut self) {
        let (next_draft, revision) = (self.next_draft, self.revision + 1);
        *self = Self { next_draft, revision, ..Self::default() };
    }

    /// Undo the last action; whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        let back = self.swap(step);
        self.redo.push(back);
        self.revision += 1;
        true
    }

    /// Redo the last action undone; whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        let back = self.swap(step);
        self.undo.push(back);
        self.revision += 1;
        true
    }

    /// Put back what `step` holds, and return what was there instead.
    fn swap(&mut self, step: Step) -> Step {
        let mut back = Step::default();
        for (row, was) in step.rows {
            let now = match was {
                Some(e) => self.rows.insert(row, e),
                None => self.rows.remove(&row),
            };
            back.rows.push((row, now));
        }
        if let Some(drafts) = step.drafts {
            back.drafts = Some(std::mem::replace(&mut self.drafts, drafts));
        }
        back
    }

    /// Run user action `f`, which touches rows `rows` (and the new rows when `drafts`), as one
    /// step of the history when it changed anything. Whether it did.
    fn act(&mut self, rows: &[usize], drafts: bool, f: impl FnOnce(&mut Self)) -> bool {
        let mut keys: Vec<usize> = rows.to_vec();
        keys.sort_unstable();
        keys.dedup();
        let before = Step {
            rows: keys.iter().map(|r| (*r, self.rows.get(r).cloned())).collect(),
            drafts: drafts.then(|| self.drafts.clone()),
        };
        f(self);
        let changed = before.rows.iter().any(|(r, was)| self.rows.get(r) != was.as_ref())
            || before.drafts.as_ref().is_some_and(|d| *d != self.drafts);
        if changed {
            self.undo.push(before);
            self.redo.clear();
            self.revision += 1;
        }
        changed
    }
}

/// `v` is the value `orig` (as read) already is.
fn same(v: &Value, orig: Option<&Cell>) -> bool {
    match (v, orig) {
        (Value::Text(t), Some(Some(o))) => t == o,
        (Value::Null, Some(None)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
