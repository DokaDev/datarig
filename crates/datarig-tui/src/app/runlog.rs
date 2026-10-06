//! The statements of a tab's last run, in order, with what each one did.
//!
//! A run is one or more statements (`Ctrl+E` on a selection that holds several) sent to the
//! tab's session at once: the driver runs them one after the other, stops at the first that
//! fails or when the user cancels, and answers the run with the last statement's result. On
//! the way it says when each statement starts and what each one before the last did
//! (`DbEvent::Started` / `DbEvent::Finished`). [`RunLog`] keeps that per statement, so the
//! screen can say which statement failed. The results pane shows each
//! statement's row result in a tab of its own and every outcome in the Messages tab.

use datarig_core::driver::Outcome;
use std::time::Duration;

/// What one statement of a run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatementOutcome {
    /// Not started yet.
    Waiting,
    Running,
    /// Rows: how many are fetched, and whether the server has more. Each row result has its
    /// own result tab; a statement before the last has every row read (up to its limit).
    Rows {
        count: u64,
        more: bool,
    },
    Affected(u64),
    /// Its command tag (`CREATE TABLE`, `SELECT` for rows not kept).
    Command(String),
    /// It failed: the error as the UI words it. The statements after it did not run.
    Failed(String),
    /// The run was cancelled while it ran, or before it started.
    Cancelled,
    /// It did not run: an earlier statement failed or the run was cancelled.
    NotRun,
}

impl StatementOutcome {
    fn ended(&self) -> bool {
        !matches!(self, StatementOutcome::Waiting | StatementOutcome::Running)
    }
}

/// One statement of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatementRun {
    pub sql: String,
    pub outcome: StatementOutcome,
    /// How long it ran (once it ended).
    pub elapsed: Option<Duration>,
    /// It ended the user's transaction, and that rolled back.
    pub rolled_back: bool,
}

/// The statements of a run, in order, and what the app says about the run beyond them (a count
/// it ran, a page it fetched by running a statement again).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunLog {
    pub statements: Vec<StatementRun>,
    pub notes: Vec<super::Notice>,
    /// The tab's binding when the run was sent (its profile, database and schema): what its
    /// results were read from.
    pub binding: u64,
    /// The generation of the tab's session it ran on.
    pub generation: u64,
}

impl RunLog {
    /// A run of `statements`, none started yet.
    pub fn new(statements: &[String]) -> Self {
        let statements = statements
            .iter()
            .map(|sql| StatementRun {
                sql: sql.clone(),
                outcome: StatementOutcome::Waiting,
                elapsed: None,
                rolled_back: false,
            })
            .collect();
        Self { statements, notes: Vec::new(), binding: 0, generation: 0 }
    }

    pub fn len(&self) -> usize {
        self.statements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// The run has more than one statement (its progress and failures name the statement).
    pub fn several(&self) -> bool {
        self.statements.len() > 1
    }

    /// The statement running now, if the driver said which.
    pub fn running(&self) -> Option<usize> {
        self.statements.iter().position(|s| s.outcome == StatementOutcome::Running)
    }

    /// The statement the run is at: the running one, else the first that did not end (one not
    /// started yet: the driver says when a run of several moves on).
    pub fn current(&self) -> Option<usize> {
        self.running().or_else(|| self.statements.iter().position(|s| !s.outcome.ended()))
    }

    /// Statement `index` ended the user's transaction by rolling it back.
    pub fn rolled_back(&mut self, index: usize) {
        if let Some(s) = self.statements.get_mut(index) {
            s.rolled_back = true;
        }
    }

    /// Statement `index` started.
    pub fn started(&mut self, index: usize) {
        if let Some(s) = self.statements.get_mut(index) {
            s.outcome = StatementOutcome::Running;
        }
    }

    /// Statement `index` (before the last) ended well. One whose rows came keeps their count.
    pub fn finished(&mut self, index: usize, outcome: &Outcome, elapsed: Duration) {
        if let Some(s) = self.statements.get_mut(index) {
            s.outcome = match (&s.outcome, outcome) {
                (StatementOutcome::Rows { count, .. }, _) => StatementOutcome::Rows { count: *count, more: false },
                (_, Outcome::Affected(n)) => StatementOutcome::Affected(*n),
                (_, Outcome::Command(tag)) => StatementOutcome::Command(tag.clone()),
            };
            s.elapsed = Some(elapsed);
        }
    }

    /// Statement `index` (before the last) has `count` rows so far; `more` follow.
    pub fn step_rows(&mut self, index: usize, count: u64, more: bool) {
        if let Some(s) = self.statements.get_mut(index) {
            s.outcome = StatementOutcome::Rows { count, more };
        }
    }

    /// The run's answer: what the statement it belongs to did (the running one, else the first
    /// that did not end). After a failure or a cancel the statements after it did not run.
    /// Returns that statement's index.
    pub fn answered(&mut self, outcome: StatementOutcome, elapsed: Option<Duration>) -> Option<usize> {
        let i = self.current()?;
        let stop = matches!(outcome, StatementOutcome::Failed(_) | StatementOutcome::Cancelled);
        let s = &mut self.statements[i];
        s.outcome = outcome;
        s.elapsed = elapsed.or(s.elapsed);
        if stop {
            for later in &mut self.statements[i + 1..] {
                later.outcome = StatementOutcome::NotRun;
            }
        }
        Some(i)
    }

    /// The run was cancelled between two statements (none was running): the next one and
    /// the rest did not run. The index of the first that did not, if any.
    pub fn cancelled_between(&mut self) -> Option<usize> {
        if self.running().is_some() {
            return None;
        }
        let i = self.statements.iter().position(|s| !s.outcome.ended())?;
        for s in &mut self.statements[i..] {
            s.outcome = StatementOutcome::NotRun;
        }
        Some(i)
    }

    /// The last statement's rows grew to `count` (paging).
    pub fn rows_fetched(&mut self, count: u64, more: bool) {
        if let Some(s) = self.statements.last_mut()
            && matches!(s.outcome, StatementOutcome::Rows { .. })
        {
            s.outcome = StatementOutcome::Rows { count, more };
        }
    }

    /// The statement that failed, if one did: its index and error.
    pub fn failed(&self) -> Option<(usize, &str)> {
        self.statements.iter().enumerate().find_map(|(i, s)| match &s.outcome {
            StatementOutcome::Failed(e) => Some((i, e.as_str())),
            _ => None,
        })
    }
}

/// The first line of `sql`, at most `max` characters (`…` when cut), to name a statement.
pub fn excerpt(sql: &str, max: usize) -> String {
    let line = sql.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let more = sql.trim().lines().count() > 1;
    let cut: String = line.chars().take(max).collect();
    if cut.chars().count() < line.chars().count() || more { format!("{cut}…") } else { cut }
}

#[cfg(test)]
mod tests;
