//! The transaction open on a tab's session: manual commit (an environment's `commit =
//! "manual"`), and committing or rolling back any open transaction (one the session opened for
//! manual commit, or the user's own `BEGIN`).
//!
//! With manual commit each run says, statement by statement, which may write
//! (`TxRule::Manual`): the session opens a transaction right before the first of them and it
//! stays open until the user commits or rolls it back (`tx.commit`, `tx.rollback`, or a
//! `COMMIT` they type). Plain reads before it run without one. The app never commits by itself.
//!
//! Before a run is sent, statements that cannot work with the open transaction are refused or
//! asked about ([`App::tx_check`]): PostgreSQL's statements that refuse a transaction block
//! (`VACUUM`, `CREATE INDEX CONCURRENTLY`, …) are refused while one is open, and with manual
//! commit and none open they ask (they run on their own and cannot be rolled back); MySQL's
//! statements that commit the open transaction first ask with manual commit, and `LOCK TABLES`
//! is refused there.
//!
//! What the open transaction did is kept per tab in memory ([`OpenTx`]: the statements that may
//! have written and succeeded, with their rows), for the status bar and the summary; a run the
//! session rolled back leaves it.

use super::action::Action;
use super::*;
use crate::keymap::Ctx;
use datarig_core::driver::{TxRule, TxStep as Step};
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::risk::{Danger, TxStep};
use std::collections::VecDeque;
use std::time::SystemTime;

/// The most statements a tab's transaction log keeps (the oldest go first; the counts go on).
pub const TX_LOG_CAP: usize = 200;

/// A statement the open transaction ran that may have written, and succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxEntry {
    /// The run it was part of.
    pub run: u64,
    /// Its first line (cut).
    pub sql: String,
    /// The rows it changed, as the server said (0 when it did not say).
    pub rows: u64,
    pub elapsed: Option<Duration>,
}

/// The transaction block open on a tab's session.
#[derive(Clone, Debug)]
pub struct OpenTx {
    /// The session opened it for manual commit (else the user's `BEGIN`, or a session without
    /// autocommit).
    pub manual: bool,
    /// When it began, by the app's clock (what counts time) and the wall clock (what is shown:
    /// the app's clock may stand still while the machine sleeps).
    pub began: Instant,
    pub began_wall: SystemTime,
    /// The last [`TX_LOG_CAP`] statements it ran that may have written, oldest first.
    pub log: VecDeque<TxEntry>,
    /// Every such statement, and the rows they changed (past the log's cap too).
    pub changes: u64,
    pub rows: u64,
    /// What the last run booked into it: its id, statements and rows (undone when the session
    /// rolls that run back).
    last_run: Option<(u64, u64, u64)>,
}

impl OpenTx {
    pub fn new(manual: bool, began: Instant, began_wall: SystemTime) -> Self {
        Self { manual, began, began_wall, log: VecDeque::new(), changes: 0, rows: 0, last_run: None }
    }

    fn push(&mut self, e: TxEntry) {
        self.changes += 1;
        self.rows += e.rows;
        match &mut self.last_run {
            Some((run, n, rows)) if *run == e.run => (*n, *rows) = (*n + 1, *rows + e.rows),
            last => *last = Some((e.run, 1, e.rows)),
        }
        if self.log.len() == TX_LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(e);
    }

    /// Run `run` was rolled back by the session: what it did is not in the transaction.
    pub fn undo_run(&mut self, run: u64) {
        if let Some((r, n, rows)) = self.last_run.take_if(|(r, ..)| *r == run) {
            self.changes = self.changes.saturating_sub(n);
            self.rows = self.rows.saturating_sub(rows);
            self.log.retain(|e| e.run != r);
        }
    }
}

/// A commit or rollback the app asked for (`DbCommand::EndTx`), not answered yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ending {
    pub id: u64,
    pub commit: bool,
}

/// Why a statement of a run asks before it runs, beyond its risk: what it does to the open
/// transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxReason {
    /// PostgreSQL refuses it inside a transaction block: with manual commit it runs on its own,
    /// at once, and cannot be rolled back.
    OutsideTx,
    /// MySQL commits the open transaction before it runs: what the transaction did so far.
    ImplicitCommit { statements: u64, rows: u64 },
    /// A procedure that commits by itself, to run once outside a transaction: it takes effect
    /// at once and cannot be rolled back.
    CommitsItself,
}

/// What [`App::tx_check`] found in a run.
#[derive(Default)]
pub(super) struct TxCheck {
    /// It does not run: why.
    pub refused: Option<Notice>,
    /// Statements that ask, by their place in the run.
    pub reasons: Vec<(usize, TxReason)>,
    /// What the run's Messages say once it is sent.
    pub notes: Vec<Notice>,
}

/// How `sql` stands to a transaction block by its first words: opens one (`BEGIN`, `START
/// TRANSACTION`), ends one (`COMMIT`, `END`, `ROLLBACK` but not `ROLLBACK TO`, `ABORT`, `PREPARE
/// TRANSACTION`; `… AND CHAIN` opens the next at once), or neither (`None`).
fn block_change(sql: &str, dialect: Dialect) -> Option<bool> {
    let words: Vec<String> = datarig_core::sql::lexer::lex_in(sql, dialect)
        .into_iter()
        .filter(|t| t.is_word())
        .take(4)
        .map(|t| t.text(sql).to_ascii_uppercase())
        .collect();
    let w = |i: usize| words.get(i).map(String::as_str);
    let chain = words.windows(2).any(|p| p[0] == "AND" && p[1] == "CHAIN");
    match w(0) {
        Some("BEGIN" | "START") => Some(true),
        Some("ROLLBACK") if w(1) == Some("TO") || w(2) == Some("TO") => None,
        Some("COMMIT" | "END" | "ROLLBACK" | "ABORT") => Some(chain),
        Some("PREPARE") if w(1) == Some("TRANSACTION") => Some(false),
        _ => None,
    }
}

/// `d` as a clock shows it: `4:12`, `1:02:03`.
pub fn fmt_clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl Tab {
    /// The statements of the last run that ended in the open transaction and may have written
    /// go into its log (once each).
    pub(super) fn book_tx(&mut self) {
        let e = &mut self.exec;
        let (Some(tx), Some(from)) = (e.tx.as_mut(), e.tx_from) else { return };
        let run = e.run_id;
        let start = if e.booked.0 == run { e.booked.1.max(from) } else { from };
        let mut next = start;
        for (i, s) in e.run.statements.iter().enumerate().skip(start) {
            use super::runlog::StatementOutcome as O;
            let rows = match &s.outcome {
                O::Waiting | O::Running => break,
                O::Affected(n) => Some(*n),
                O::Rows { .. } | O::Command(_) => Some(0),
                O::Failed(_) | O::Cancelled | O::NotRun => None,
            };
            next = i + 1;
            if let Some(rows) = rows
                && e.prepared.tx_step(&s.sql) == TxStep::Opens
            {
                tx.push(TxEntry { run, sql: super::runlog::excerpt(&s.sql, 80), rows, elapsed: s.elapsed });
            }
        }
        e.booked = (run, next);
    }
}

impl App {
    /// The current time of the wall clock (tests pass their own).
    pub fn wall_now(&self) -> SystemTime {
        (self.wall)()
    }

    /// How long the open transaction `tx` has been open: by the wall clock (it goes on while the
    /// machine sleeps), by the app's clock when the wall clock went back.
    pub fn tx_elapsed(&self, tx: &OpenTx) -> Duration {
        self.wall_now().duration_since(tx.began_wall).unwrap_or_else(|_| self.now().saturating_duration_since(tx.began))
    }

    /// Tab `id` commits by hand: its rules say so and its driver can (the session's word once it
    /// is open, else the driver's).
    pub fn manual_commit(&self, id: TabId) -> bool {
        let Some(t) = self.tabs.get(id) else { return false };
        let can = match &t.exec.session {
            Some(s) => s.caps.manual_commit,
            None => self.profile_capabilities(t.profile).is_some_and(|c| c.manual_commit),
        };
        can && self.tab_rules(t).commit.manual()
    }

    /// How the statements of a run in tab `id` stand to a transaction: `TxRule::Auto` unless the
    /// tab commits by hand; then each statement's step as the tab's classifier sees it (with
    /// what the run prepares before it), and PostgreSQL's idle limit for the transaction.
    /// `as_is`: every statement runs as it is (a procedure the user chose to run outside a
    /// transaction).
    pub(super) fn tx_rule(&self, id: TabId, statements: &[String], as_is: bool) -> TxRule {
        if !self.manual_commit(id) {
            return TxRule::Auto;
        }
        let Some(t) = self.tabs.get(id) else { return TxRule::Auto };
        let mut c = t.exec.prepared.clone();
        let steps = statements
            .iter()
            .map(|sql| {
                let step = match c.tx_step(sql) {
                    _ if as_is => Step::AsIs,
                    TxStep::Opens => Step::Open,
                    TxStep::ReadOutside => Step::ReadOnly,
                    TxStep::Outside => Step::AsIs,
                };
                c.classify(sql);
                step
            })
            .collect();
        let pg = t.exec.prepared.language().dialect() == Dialect::Postgres;
        let idle_timeout = self.tab_rules(t).tx_idle_timeout.filter(|_| pg);
        TxRule::Manual { steps, idle_timeout }
    }

    /// What `statements` do to the transaction of tab `id`, statement by statement, as the tab
    /// follows it while the run goes on (a statement that opens a transaction with manual
    /// commit, a `BEGIN`, a `COMMIT`, MySQL's implicit commit): refused (PostgreSQL's
    /// statements that refuse a transaction block, while one is open; MySQL's `LOCK TABLES`
    /// with manual commit), asked about (see [`TxReason`]), or noted in the run's Messages (an
    /// enum value added in a transaction; MySQL's DDL with manual commit and none open).
    pub(super) fn tx_check(&self, id: TabId, statements: &[String]) -> TxCheck {
        let mut out = TxCheck::default();
        let Some(t) = self.tabs.get(id) else { return out };
        let manual = self.manual_commit(id);
        let dialect = t.exec.prepared.language().dialect();
        let mysql = matches!(dialect, Dialect::MySql(_));
        let mut open = t.exec.user_tx();
        // What the open transaction did before the run; once a statement of the run ends it, none.
        let (mut changes, mut rows) = t.exec.tx.as_ref().map_or((0, 0), |x| (x.changes, x.rows));
        // Statements of this run that may write in the open transaction before a later one.
        let mut pending = 0u64;
        let mut c = self.classifier(id);
        let step_text = |i: usize| (i + 1).to_string();
        for (i, sql) in statements.iter().enumerate() {
            let step = c.tx_step(sql);
            let r = c.classify(sql);
            if !mysql && r.no_tx_block {
                if open {
                    let (commit, rollback) = self.tx_keys();
                    let sql = super::runlog::excerpt(sql, 40);
                    let m = Msg::TxNoTxBlockRefused { sql, commit, rollback };
                    out.refused = Some(Notice::new(m, Level::Error));
                    return out;
                }
                if manual {
                    out.reasons.push((i, TxReason::OutsideTx));
                }
            }
            if mysql && manual && r.danger == Some(Danger::Locks) && r.implicit_commit {
                out.refused = Some(Notice::new(Label::TxLockTablesRefused, Level::Error));
                return out;
            }
            if mysql && r.implicit_commit {
                if open && manual {
                    let statements = changes + pending;
                    out.reasons.push((i, TxReason::ImplicitCommit { statements, rows }));
                } else if manual && r.class == datarig_core::sql::risk::Class::Ddl {
                    out.notes.push(Notice::new(Msg::TxMysqlDdl { step: step_text(i) }, Level::Warning));
                }
                open = false;
                (changes, rows, pending) = (0, 0, 0);
            }
            if !mysql && r.adds_enum_value && (open || (manual && step == TxStep::Opens)) {
                out.notes.push(Notice::new(Msg::TxEnumValue { step: step_text(i) }, Level::Warning));
            }
            match block_change(sql, dialect) {
                Some(o) => {
                    open = o;
                    (changes, rows, pending) = (0, 0, 0);
                }
                None if step == TxStep::Opens && (open || manual) => {
                    open = true;
                    pending += 1;
                }
                None => {}
            }
        }
        out
    }

    /// The keys that commit and roll back, as the messages name them.
    pub(super) fn tx_keys(&self) -> (String, String) {
        (self.key_for(Action::TxCommit, Ctx::Nav), self.key_for(Action::TxRollback, Ctx::Nav))
    }

    /// The active tab can commit or roll back now: a transaction is open on a session that can
    /// end it, and nothing runs there.
    pub fn tx_endable(&self) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let t = self.tab();
        t.exec.user_tx() && t.exec.session.as_ref().is_some_and(|s| s.caps.manual_commit) && !self.tab_busy(t.id)
    }

    /// Commit (`commit`) or roll back the active tab's open transaction. Refused while nothing
    /// is open, while something runs, and (a commit) when the transaction failed.
    pub(super) fn end_tx(&mut self, commit: bool) {
        if self.tabs.is_empty() {
            return;
        }
        let id = self.tab().id;
        if !self.tab().exec.user_tx() {
            return self.flash(Notice::new(Label::TxNone, Level::Warning));
        }
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        if commit && self.tab().exec.tx_aborted {
            let key = self.key_for(Action::TxRollback, Ctx::Nav);
            return self.flash(Notice::new(Msg::TxOnlyRollback { key }, Level::Warning));
        }
        if !self.tab().exec.session.as_ref().is_some_and(|s| s.caps.manual_commit) {
            return self.flash(Notice::new(Label::TxNotSupported, Level::Warning));
        }
        self.query_seq += 1;
        let qid = self.query_seq;
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.ending = Some(Ending { id: qid, commit });
        }
        let label = if commit { Label::TxCommitting } else { Label::TxRollingBack };
        self.tab_status(id, Notice::new(label, Level::Info));
        self.send_tab(id, DbCommand::EndTx { id: qid, commit });
    }

    /// What the active tab's open transaction did, in a line.
    pub(super) fn tx_summary(&mut self) {
        let Some(tx) =
            self.tabs.get(self.tab().id).and_then(|t| t.exec.tx.clone()).filter(|_| self.tab().exec.user_tx())
        else {
            return self.flash(Notice::new(Label::TxNone, Level::Info));
        };
        let how = self.i18n.label(if tx.manual { Label::TxHowManual } else { Label::TxHowUser }).to_string();
        let rows = self.i18n.msg(&Msg::TxRows { count: tx.rows }).to_string();
        let time = fmt_clock(self.tx_elapsed(&tx));
        let id = self.tab().id;
        self.tab_status(id, Notice::new(Msg::TxSummary { how, count: tx.changes, rows, time }, Level::Info));
    }

    /// The status bar's words for the active tab's open transaction: `TX open · 3 changes ·
    /// 120 rows · 4:12` (`None`: none is open, or it failed: the bar says that instead).
    pub fn tx_status(&self) -> Option<String> {
        let e = &self.tab().exec;
        let tx = e.tx.as_ref().filter(|_| e.user_tx() && !e.tx_aborted)?;
        let parts = [
            self.i18n.label(Label::StatusTxOpen).to_string(),
            self.i18n.msg(&Msg::TxChanges { count: tx.changes }).to_string(),
            self.i18n.msg(&Msg::TxRows { count: tx.rows }).to_string(),
            fmt_clock(self.tx_elapsed(tx)),
        ];
        Some(parts.join(" · "))
    }

    /// When the active tab's open transaction's time shown next changes (its next whole
    /// second), by the app's clock; `None` when the status bar shows none.
    pub(super) fn tx_tick_due(&self, now: Instant) -> Option<Instant> {
        if self.tabs.is_empty() {
            return None;
        }
        let e = &self.tab().exec;
        let tx = e.tx.as_ref().filter(|_| e.user_tx() && !e.tx_aborted)?;
        let open = now.saturating_duration_since(tx.began);
        let frac = Duration::from_nanos(u64::from(open.subsec_nanos()));
        Some(now + (Duration::from_secs(1) - frac))
    }

    /// Ask whether to run the procedure that commits by itself, which failed as the first
    /// statement of the transaction opened for it in tab `id`, once outside a transaction: the
    /// run confirmation (the connection, its environment, the statement), bound to the tab, its
    /// binding and its session; `y` runs it as it is, anything else leaves it unrun.
    pub(super) fn offer_outside(&mut self, id: TabId) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let Some(sql) = t.exec.outside_offer.take() else { return };
        let Some(pid) = t.profile else { return };
        let item = super::safety::Dangerous {
            index: 0,
            why: None,
            tx: Some(TxReason::CommitsItself),
            no_where: None,
            explain: false,
            target: None,
            sql: sql.clone(),
        };
        self.ask_before_run_as(id, pid, vec![sql], vec![item], true);
    }
}
