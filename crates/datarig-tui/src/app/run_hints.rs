//! The editor's marks of a run: the statement that runs now (drawn apart while the run goes
//! on) and, once the run ended, a hint after each statement's last line with what it did.
//!
//! The editor keeps the statements' places through edits ([`crate::widgets::editor::Span`]);
//! the app binds them to the run's query id when it sends the run and hands the outcomes back
//! once that run ended, from the tab's [`super::runlog::RunLog`]. Events of other runs never
//! reach here (a tab drops answers of another query id or session generation), a closed tab
//! takes its editor with it, and a statement edited while it ran gets no hint.

use super::runlog::{StatementOutcome, StatementRun};
use super::*;
use crate::widgets::editor::{HintKind, RunHint};
use datarig_core::sql::risk;

impl App {
    /// Tab `id`'s run marked on its editor ended: its statements get their hints.
    pub(super) fn settle_run_hints(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id) else { return };
        let Some(query) = t.editor.active_run() else {
            return self.amend_rolled_back(id);
        };
        if t.exec.running.is_some_and(|r| r.id == query) {
            return;
        }
        let time = self.time_of_day();
        let plan = t.exec.plan.as_ref().filter(|p| p.query == query).map(|p| p.index);
        let hints: Vec<Option<RunHint>> =
            t.exec.run.statements.iter().enumerate().map(|(i, s)| self.run_hint(s, plan == Some(i), &time)).collect();
        if let Some(t) = self.tabs.get_mut(id) {
            t.editor.finish_run(query, hints);
        }
    }

    /// The driver says the user's transaction ended after the answer of the statement that
    /// ended it: once it says it rolled back, that statement's hint says so.
    fn amend_rolled_back(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id) else { return };
        let query = t.exec.query_id;
        if t.editor.last_run() != Some(query) || !t.exec.run.statements.iter().any(|s| s.rolled_back) {
            return;
        }
        let time = &self.time_of_day();
        let amend: Vec<(usize, RunHint)> = (t.exec.run.statements.iter().enumerate())
            .filter(|(_, s)| s.rolled_back)
            .filter_map(|(i, s)| Some((i, self.run_hint(s, false, time)?)))
            .collect();
        if let Some(t) = self.tabs.get_mut(id) {
            for (i, hint) in amend {
                t.editor.amend_run_hint(query, i, hint);
            }
        }
    }

    /// What statement `s` of a run that ended did, as its hint says it; `None` when it did not
    /// run. `plan`: its rows are the plan the results show.
    fn run_hint(&self, s: &StatementRun, plan: bool, time: &str) -> Option<RunHint> {
        let elapsed = s.elapsed.unwrap_or_default();
        let time = time.to_string();
        // `EXPLAIN ANALYZE` of a change ran it in a transaction the driver rolled back.
        let rolled_back = s.rolled_back || risk::classify(&s.sql).rollback_matters();
        let (kind, msg) = match &s.outcome {
            StatementOutcome::Waiting | StatementOutcome::Running | StatementOutcome::NotRun => return None,
            StatementOutcome::Failed(e) => {
                let line = e.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
                return Some(RunHint { kind: HintKind::Failed, text: line.to_string() });
            }
            StatementOutcome::Cancelled => (HintKind::Cancelled, Msg::Label(Label::EditorHintCancelled)),
            _ if rolled_back => (HintKind::RolledBack, Msg::EditorHintRolledBack { elapsed, time }),
            _ if plan => (HintKind::Ok, Msg::EditorHintPlan { elapsed, time }),
            StatementOutcome::Rows { count, more: false } => {
                (HintKind::Ok, Msg::EditorHintRows { count: *count, elapsed, time })
            }
            StatementOutcome::Rows { count, more: true } => {
                (HintKind::Ok, Msg::EditorHintMoreRows { count: *count, elapsed, time })
            }
            StatementOutcome::Affected(count) => {
                (HintKind::Ok, Msg::EditorHintAffected { count: *count, elapsed, time })
            }
            StatementOutcome::Command(tag) => {
                (HintKind::Ok, Msg::EditorHintCommand { tag: tag.clone(), elapsed, time })
            }
        };
        Some(RunHint { kind, text: self.i18n.msg(&msg).to_string() })
    }
}
