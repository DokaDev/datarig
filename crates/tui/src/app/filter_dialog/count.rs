//! `# count exactly`: the server counts the rows of each value of the dialog's selected column,
//! under the draft's other conditions ([`query::group_count`]), on the tab's own session as the
//! count of a table's rows goes (busy refused, a time limit where that count has one, cancelled
//! with the tab's cancel). The answer is kept with everything it depends on ([`Scope`]); a list
//! whose scope is no longer the dialog's is shown as an earlier count, and none of its values is
//! ticked until it is counted again.

use super::*;
use datarig_core::driver::{ColumnMeta, DbError, GroupCounts, ReadIn};
use datarig_core::profile::ProfileId;
use datarig_core::query::{self, GroupCount, GroupRefusal, TableName};

/// What a count of a column's values is of: the tab's profile, binding and session, the table,
/// the column, the other conditions' text as sent, and what the session reads values in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub(crate) profile: ProfileId,
    pub(crate) binding: u64,
    pub(crate) generation: u64,
    pub(crate) table: TableName,
    pub(crate) column: String,
    pub(crate) other: Option<String>,
    pub(crate) read_in: ReadIn,
}

impl Scope {
    /// Only the other conditions differ from `now`'s.
    pub(crate) fn others_changed(&self, now: &Scope) -> bool {
        self.other != now.other && Scope { other: now.other.clone(), ..self.clone() } == *now
    }
}

/// A count sent and not answered: its request and its scope.
#[derive(Clone, Debug)]
pub(crate) struct Counting {
    pub(crate) id: u64,
    pub(crate) scope: Scope,
    /// The column as the rows loaded describe it (its values are read as such).
    pub(crate) meta: ColumnMeta,
    /// An expression among the other conditions names the column.
    pub(crate) kept_expr: bool,
}

/// A count answered: its scope, the column as read and the values.
#[derive(Clone, Debug)]
pub(crate) struct Exact {
    pub(crate) scope: Scope,
    pub(crate) meta: ColumnMeta,
    pub(crate) counts: GroupCounts,
    pub(crate) kept_expr: bool,
}

impl App {
    /// The scope a count of column `col` of the open dialog's table would have now, with its
    /// draft's other conditions.
    pub(crate) fn filter_dialog_scope(&self, d: &FilterDialog, col: &str) -> Option<Scope> {
        let t = self.tabs.get(d.tab)?;
        Some(Scope {
            profile: t.profile?,
            binding: t.binding,
            generation: t.exec.generation,
            table: self.table_name(d.tab)?,
            column: col.to_string(),
            other: d.draft.where_.text_without(col),
            read_in: ReadIn::first(t.exec.values.as_ref()),
        })
    }

    /// `#` in the dialog: the server counts the selected column's values under the draft's
    /// other conditions, or the dialog says why not.
    pub(crate) fn filter_dialog_count(&mut self) {
        match self.filter_dialog_count_sql() {
            Ok(()) => {}
            Err(n) => self.say_over_filter_dialog(n),
        }
    }

    /// Sends the count of [`App::filter_dialog_count`]; `Err` with why it is not sent.
    fn filter_dialog_count_sql(&mut self) -> Result<(), Notice> {
        let warn = |m: Msg| Notice::new(m, Level::Warning);
        let refused = |why: String| Notice::new(Msg::TableDialogCountRefused { why }, Level::Warning);
        let Some(d) = self.filter_dialog() else { return Ok(()) };
        let tab = d.tab;
        let Some(c) = d.column().cloned() else { return Ok(()) };
        if self.tab_busy(tab) {
            let key = self.key_for(Action::CancelQuery, Ctx::Nav);
            return Err(warn(Msg::QueryBusy { key }));
        }
        if c.invisible {
            return Err(refused(self.i18n.label(Label::TableDialogValuesInvisible).to_string()));
        }
        let Some(meta) = c.meta.clone() else {
            return Err(refused(self.i18n.label(Label::TableDialogCountUnknown).to_string()));
        };
        let (Some(t), Some(target), Some(scope)) =
            (self.tabs.get(tab), self.table_target(tab), self.filter_dialog_scope(d, &c.name))
        else {
            return Ok(());
        };
        if t.exec.session.is_none() {
            return Err(refused(self.i18n.label(Label::TableDialogCountNoSession).to_string()));
        }
        let kept_expr = d.draft.where_.terms().iter().any(|term| {
            term.is_expr() && term.text.as_deref().is_some_and(|x| values::mentions(x, &c.name, d.dialect))
        });
        let pid = scope.profile;
        let read_only = self.read_only(pid);
        let classifier = self.classifier(tab);
        let g = GroupCount {
            table: &scope.table,
            target: &target,
            column: &meta,
            enum_known: c.enum_known,
            other: scope.other.as_deref(),
            classifier: &classifier,
            read_only,
        };
        let sql = query::group_count(&g).map_err(|r| refused(self.group_refusal_why(&r)))?;
        let statements = vec![sql.clone()];
        if let Some(n) = self.unsupported(tab, &statements).or_else(|| self.read_only_refusal(tab, pid, &statements)) {
            return Err(n);
        }
        if !self.dangerous(tab, pid, &statements).is_empty() {
            return Err(refused(self.i18n.label(Label::RepeatWrites).to_string()));
        }
        let (limit, unbounded) = self.count_limit(tab);
        self.query_seq += 1;
        let id = self.query_seq;
        let now = self.now();
        let Some(t) = self.tabs.get_mut(tab) else { return Ok(()) };
        // A count as far as the tab goes: the rows stay as they are.
        t.exec.running = Some(Running {
            id,
            started: now,
            fetch: false,
            count: true,
            estimate: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: false,
            apply: false,
        });
        let (column, excerpt) = (c.name.clone(), super::super::runlog::excerpt(&sql, 120));
        let sent = match unbounded {
            Some(_) => Msg::TableDialogCountSentUnbounded { column, sql: excerpt },
            None => Msg::TableDialogCountSent { column, sql: excerpt },
        };
        t.exec.run.notes.push(Notice::new(sent, Level::Info));
        self.send_tab(tab, DbCommand::GroupCount { id, sql, limit });
        if let Some(d) = self.filter_dialog_mut() {
            d.counting = Some(Counting { id, scope, meta, kept_expr });
            // Inside the user's transaction (or one that holds a result's rows) the count has no
            // time limit: said, it may wait.
            d.message = unbounded.map(|_| Notice::new(Label::TableDialogCountNoLimit, Level::Info));
        }
        Ok(())
    }

    /// Why a count of a column's values is not written, in words.
    fn group_refusal_why(&self, r: &GroupRefusal) -> String {
        let label = |l: Label| self.i18n.label(l).to_string();
        match r {
            GroupRefusal::UnknownColumn => label(Label::TableDialogCountUnknownColumn),
            GroupRefusal::NoEquality => label(Label::TableDialogOpOffNoEquality),
            GroupRefusal::Filter(b) => super::super::table_query::refusal_why(&self.i18n, b),
            GroupRefusal::Calls(calls) => {
                self.i18n.msg(&Msg::TableDialogCountCalls { calls: calls.join(", ") }).to_string()
            }
            GroupRefusal::NotRead { .. } => label(Label::TableDialogCountNotRead),
            GroupRefusal::NotRepeatable(n) => self.i18n.msg(&super::super::pages::why(n)).to_string(),
        }
    }

    /// The answer `result` to count `qid` of tab `id`'s values: the tab's count ends; the open
    /// dialog that sent it keeps the values with their scope, or says what came instead. An
    /// answer of a count no longer waited for (another one sent since, the session or binding
    /// changed) is dropped.
    pub(crate) fn on_group_counted(&mut self, id: TabId, qid: u64, result: Result<GroupCounts, DbError>) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        if !t.exec.running.is_some_and(|r| r.id == qid && r.count && !r.estimate) {
            return;
        }
        t.exec.running = None;
        let text = match &result {
            Err(e) => self.db_error_text(e),
            Ok(_) => String::new(),
        };
        let note = match &result {
            Ok(c) => Notice::new(Msg::TableDialogCountDone { count: c.values.len() as u64 }, Level::Success),
            Err(DbError::Cancelled) => Notice::new(Label::TableDialogCountCancelled, Level::Warning),
            // Stopped at its time limit or behind another session's lock: not known.
            Err(DbError::TimedOut(d)) => Notice::new(
                Msg::TableDialogCountTimedOut { limit: super::super::estimate::fmt_limit(*d) },
                Level::Warning,
            ),
            Err(DbError::LockTimeout(_) | DbError::ServerLockTimeout(_)) => {
                Notice::new(Label::TableDialogCountLockTimeout, Level::Warning)
            }
            Err(DbError::LockingRead) => Notice::new(Label::TableDialogCountLockingRead, Level::Warning),
            Err(DbError::SchemaChanged) => Notice::new(Label::TableDialogCountSchemaChanged, Level::Warning),
            Err(DbError::NotRepeatable(r)) => {
                let why = self.i18n.msg(&super::super::pages::why(r)).to_string();
                Notice::new(Msg::TableDialogCountRefused { why }, Level::Warning)
            }
            Err(_) => Notice::new(Msg::TableDialogCountFailed { error: text }, Level::Error),
        };
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.exec.run.notes.push(note.clone());
        let (profile, binding, generation) = (t.profile, t.binding, t.exec.generation);
        let Some(d) = self.filter_dialog_mut().filter(|d| d.tab == id) else { return };
        let Some(c) = d.counting.take_if(|c| c.id == qid) else { return };
        let s = &c.scope;
        if Some(s.profile) != profile || s.binding != binding || s.generation != generation {
            return;
        }
        // What came instead of values is said; values are listed (the list says what it holds).
        d.message = result.is_err().then_some(note);
        if let Ok(counts) = result {
            let column = c.scope.column.clone();
            d.exact.insert(column, Exact { scope: c.scope, meta: c.meta, counts, kept_expr: c.kept_expr });
        }
    }

    /// `Esc` in the dialog while its count runs: the count is cancelled (the dialog stays).
    /// `false` when no count of the dialog runs.
    pub(crate) fn filter_dialog_cancel_count(&mut self) -> bool {
        let Some(d) = self.filter_dialog() else { return false };
        let tab = d.tab;
        let Some(id) = d.counting.as_ref().map(|c| c.id) else { return false };
        let runs = self.tabs.get(tab).is_some_and(|t| t.exec.running.is_some_and(|r| r.id == id));
        if !runs {
            if let Some(d) = self.filter_dialog_mut() {
                d.counting = None;
            }
            return false;
        }
        self.cancel_in(tab);
        true
    }
}
