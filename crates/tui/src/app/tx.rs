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

/// From this long idle on, the status bar says for how long the open transaction is idle.
pub const IDLE_SHOWN: Duration = Duration::from_secs(60);

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
    /// When a statement last ran in it (or it began), by the app's clock: it is idle since.
    pub last_active: Instant,
    /// It stayed idle past the environment's `tx_idle_warning` since then (said once).
    pub warned: bool,
}

impl OpenTx {
    pub fn new(manual: bool, began: Instant, began_wall: SystemTime) -> Self {
        Self {
            manual,
            began,
            began_wall,
            log: VecDeque::new(),
            changes: 0,
            rows: 0,
            last_run: None,
            last_active: began,
            warned: false,
        }
    }

    /// A statement ran in it at `now`: it is not idle (a new idle period starts after).
    pub fn touch(&mut self, now: Instant) {
        self.last_active = now;
        self.warned = false;
    }

    /// How long it has been idle at `now` (nothing while a statement runs: `running`).
    pub fn idle(&self, now: Instant, running: bool) -> Duration {
        if running { Duration::ZERO } else { now.saturating_duration_since(self.last_active) }
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
            // DDL in a transaction holds its lock until the transaction ends: said, with what
            // waits for it.
            if !mysql
                && r.class == datarig_core::sql::risk::Class::Ddl
                && (open || (manual && step == TxStep::Opens))
                && let Some((exclusive, target)) = ddl_lock(sql)
            {
                let step = step_text(i);
                let m = match exclusive {
                    true => Msg::TxDdlLockExclusive { step, target },
                    false => Msg::TxDdlLockShare { step, target },
                };
                out.notes.push(Notice::new(m, Level::Warning));
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
        self.end_tx_in(id, commit);
    }

    /// End tab `id`'s open transaction ([`App::end_tx`]): whether the request went out.
    pub(super) fn end_tx_in(&mut self, id: TabId, commit: bool) -> bool {
        let Some(t) = self.tabs.get(id) else { return false };
        let refused = if !t.exec.user_tx() {
            Some(Notice::new(Label::TxNone, Level::Warning))
        } else if self.tab_busy(id) {
            let key = self.key_for(Action::CancelQuery, Ctx::Nav);
            Some(Notice::new(Msg::QueryBusy { key }, Level::Warning))
        } else if commit && t.exec.tx_aborted {
            let key = self.key_for(Action::TxRollback, Ctx::Nav);
            Some(Notice::new(Msg::TxOnlyRollback { key }, Level::Warning))
        } else if !t.exec.session.as_ref().is_some_and(|s| s.caps.manual_commit) {
            Some(Notice::new(Label::TxNotSupported, Level::Warning))
        } else {
            None
        };
        if let Some(n) = refused {
            if self.tab().id == id {
                self.flash(n);
            } else {
                self.tab_status(id, n);
            }
            return false;
        }
        self.query_seq += 1;
        let qid = self.query_seq;
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.ending = Some(Ending { id: qid, commit });
            t.exec.last_end = None;
        }
        let label = if commit { Label::TxCommitting } else { Label::TxRollingBack };
        self.tab_status(id, Notice::new(label, Level::Info));
        self.send_tab(id, DbCommand::EndTx { id: qid, commit });
        true
    }

    /// What the active tab's open transaction did: the summary dialog ([`TxSummary`]).
    ///
    /// [`TxSummary`]: super::overlay::TxSummary
    pub(super) fn tx_summary(&mut self) {
        if self.tabs.is_empty() || !self.tab().exec.user_tx() || self.tab().exec.tx.is_none() {
            return self.flash(Notice::new(Label::TxNone, Level::Info));
        }
        let tab = self.tab().id;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        let s = super::overlay::TxSummary { tab, scroll: 0, buttons: Default::default() };
        self.overlays.push(Overlay::TxSummary(s));
    }

    /// The summary's buttons, the one that changes nothing first: Close, Roll back, Commit (not
    /// for a failed transaction), each with the key it presses.
    pub fn tx_summary_buttons(&self) -> Vec<(Label, KeyCode)> {
        let aborted = self.overlays.tx_summary().and_then(|s| self.tabs.get(s.tab)).is_some_and(|t| t.exec.tx_aborted);
        let mut b = vec![(Label::DialogButtonClose, KeyCode::Esc), (Label::TxButtonRollbackNow, KeyCode::Char('r'))];
        if !aborted {
            b.push((Label::TxButtonCommitNow, KeyCode::Char('c')));
        }
        b
    }

    /// Keys of the summary: `c` commits, `r` rolls back (both close it), `Esc`, `q` and `Enter`
    /// close it, `j`/`k` and the arrows scroll. Held keys only scroll.
    pub(super) fn tx_summary_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(s) = self.overlays.tx_summary_mut() else { return };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => s.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => s.scroll = s.scroll.saturating_sub(1),
            _ if repeat => {}
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlays.close(OverlayKind::TxSummary),
            KeyCode::Char(c @ ('c' | 'r')) => {
                let tab = s.tab;
                if c == 'c' && !self.tx_summary_buttons().iter().any(|b| b.1 == KeyCode::Char('c')) {
                    let key = self.key_for(Action::TxRollback, Ctx::Nav);
                    return self.flash(Notice::new(Msg::TxOnlyRollback { key }, Level::Warning));
                }
                self.overlays.close(OverlayKind::TxSummary);
                self.end_tx_in(tab, c == 'c');
            }
            _ => {}
        }
    }

    /// The status bar's words for the active tab's open transaction: `TX open · 3 changes ·
    /// 120 rows · 4:12` (`None`: none is open, or it failed: the bar says that instead).
    pub fn tx_status(&self) -> Option<String> {
        let e = &self.tab().exec;
        let tx = e.tx.as_ref().filter(|_| e.user_tx() && !e.tx_aborted)?;
        let mut parts = vec![
            self.i18n.label(Label::StatusTxOpen).to_string(),
            self.i18n.msg(&Msg::TxChanges { count: tx.changes }).to_string(),
            self.i18n.msg(&Msg::TxRows { count: tx.rows }).to_string(),
            fmt_clock(self.tx_elapsed(tx)),
        ];
        // Idle a minute or more: for how long; past the warning, that it holds its locks.
        let idle = tx.idle(self.now(), self.tab_busy(self.tab().id));
        if idle >= IDLE_SHOWN || tx.warned {
            parts.push(self.i18n.msg(&Msg::StatusTxIdle { time: fmt_clock(idle) }).to_string());
        }
        if tx.warned {
            parts.push(self.i18n.label(Label::StatusTxHoldingLocks).to_string());
        }
        Some(parts.join(" · "))
    }

    /// Tab `t`'s open transaction stayed idle past its environment's warning.
    pub fn tx_idle_warned(t: &Tab) -> bool {
        t.exec.user_tx() && t.exec.tx.as_ref().is_some_and(|x| x.warned)
    }

    /// When the next idle warning is due (the earliest over the tabs with an open, idle
    /// transaction not warned yet, under their environments' `tx_idle_warning`).
    pub(super) fn tx_idle_due(&self) -> Option<Instant> {
        self.tabs
            .iter()
            .filter(|t| t.exec.user_tx() && !self.tab_busy(t.id))
            .filter_map(|t| {
                let tx = t.exec.tx.as_ref().filter(|x| !x.warned)?;
                Some(tx.last_active + self.tab_rules(t).tx_idle_warning?)
            })
            .min()
    }

    /// The idle warnings due at `now`: one notice for each open transaction idle past its
    /// environment's `tx_idle_warning` (once per idle period). Nothing is ended.
    pub(super) fn tx_idle_tick(&mut self, now: Instant) {
        let due: Vec<(TabId, usize, Duration)> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.exec.user_tx() && !self.tab_busy(t.id))
            .filter_map(|(i, t)| {
                let tx = t.exec.tx.as_ref().filter(|x| !x.warned)?;
                let limit = self.tab_rules(t).tx_idle_warning?;
                let idle = tx.idle(now, false);
                (idle >= limit).then_some((t.id, i, idle))
            })
            .collect();
        for (id, i, idle) in due {
            if let Some(tx) = self.tabs.get_mut(id).and_then(|t| t.exec.tx.as_mut()) {
                tx.warned = true;
            }
            let (commit, rollback) = self.tx_keys();
            let m = Msg::TxIdleWarning { tab: (i + 1).to_string(), time: fmt_clock(idle), commit, rollback };
            let n = Notice::new(m, Level::Warning);
            if let Some(t) = self.tabs.get_mut(id) {
                t.exec.run.notes.push(n.clone());
            }
            // Said where the user looks: the tab's status, and the status bar from another tab.
            self.tab_status(id, n.clone());
            self.status = Some(n);
        }
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

/// A question's answer that goes on once the open transactions it ends were committed or
/// rolled back, one tab at a time ([`App::tx_choice`]).
pub(super) struct PendingEnd {
    pub action: ConfirmAction,
    pub commit: bool,
    /// The tabs still to end, in order.
    pub tabs: VecDeque<TabId>,
    /// The tab whose end the server is asked for now.
    pub current: Option<TabId>,
}

impl App {
    /// The tabs a yes to `action` would end the session of.
    fn ended_by(&self, action: ConfirmAction) -> Vec<TabId> {
        let of_profile = |p| self.tabs.iter().filter(|t| t.profile == Some(p)).map(|t| t.id).collect();
        match action {
            ConfirmAction::CloseTab(t) | ConfirmAction::SetConnection(t) | ConfirmAction::SetContext(t) => vec![t],
            ConfirmAction::Disconnect { id, .. } | ConfirmAction::DeleteProfile(id) => of_profile(id),
            ConfirmAction::Quit => self.tabs.iter().map(|t| t.id).collect(),
            _ => Vec::new(),
        }
    }

    /// The open question (of `action`) offers to commit or roll back first when every
    /// transaction its answer would end is one the app can end (the user's block on a session
    /// that ends it on request) and nothing runs or waits in those tabs (a running statement
    /// keeps today's question: cancelling it comes first). Its lines say what each holds.
    pub(super) fn offer_tx_choices(&mut self) {
        let Some(action) = self.overlays.confirm().map(|c| c.action) else { return };
        let tabs = self.ended_by(action);
        let at_risk: Vec<TabId> =
            tabs.iter().copied().filter(|id| self.tabs.get(*id).is_some_and(|t| t.exec.tx_at_risk())).collect();
        if at_risk.is_empty() || tabs.iter().any(|id| self.tab_busy(*id)) {
            return;
        }
        let endable = |t: &Tab| t.exec.user_tx() && t.exec.session.as_ref().is_some_and(|s| s.caps.manual_commit);
        if !at_risk.iter().all(|id| self.tabs.get(*id).is_some_and(endable)) {
            return;
        }
        let commit = !at_risk.iter().any(|id| self.tabs.get(*id).is_some_and(|t| t.exec.tx_aborted));
        let several =
            matches!(action, ConfirmAction::Quit | ConfirmAction::Disconnect { .. } | ConfirmAction::DeleteProfile(_));
        let mut lines = Vec::new();
        for id in &at_risk {
            let Some((i, t)) = self.tabs.iter().enumerate().find(|(_, t)| t.id == *id) else { continue };
            let Some(tx) = t.exec.tx.as_ref() else { continue };
            let rows = self.i18n.msg(&Msg::TxRows { count: tx.rows }).to_string();
            let time = fmt_clock(self.tx_elapsed(tx));
            lines.push(match several {
                true => Msg::TxPendingTab { tab: (i + 1).to_string(), count: tx.changes, rows, time },
                false => Msg::TxPending { count: tx.changes, rows, time },
            });
        }
        if let Some(c) = self.overlays.confirm_mut() {
            c.tx = Some(super::overlay::TxChoices { commit });
            c.keys = if commit { Label::TxChoiceKeys } else { Label::TxChoiceKeysRollback };
            c.details.extend(lines);
        }
    }

    /// `c` (`commit`) or `r` on a question that offers it: end the open transactions its answer
    /// would end, one tab at a time, and go on with the answer once the server confirmed each.
    /// The first that does not end as asked (a failed commit, a rollback instead, an unknown
    /// outcome, a lost connection) stops it all: nothing more is ended and the answer does not
    /// go on; the tab says why.
    pub(super) fn tx_choice(&mut self, commit: bool) {
        let Some(action) = self.overlays.confirm().map(|c| c.action) else { return };
        self.overlays.close(OverlayKind::Confirm);
        let tabs = self
            .ended_by(action)
            .into_iter()
            .filter(|id| self.tabs.get(*id).is_some_and(|t| t.exec.user_tx()))
            .collect();
        self.pending_end = Some(PendingEnd { action, commit, tabs, current: None });
        self.end_next();
    }

    /// The next tab of the pending answer: its transaction ended next, or, with none left, the
    /// answer goes on.
    fn end_next(&mut self) {
        let Some(p) = self.pending_end.as_mut() else { return };
        let Some(id) = p.tabs.pop_front() else {
            let Some(p) = self.pending_end.take() else { return };
            // Whatever ends the sessions now: nothing open is lost.
            return self.confirmed(p.action, None, None);
        };
        let commit = p.commit;
        p.current = Some(id);
        if !self.tabs.get(id).is_some_and(|t| t.exec.user_tx()) {
            // Ended meanwhile (by the user, by the server): nothing to do there.
            return self.end_next();
        }
        if !self.end_tx_in(id, commit) {
            self.end_stopped(id);
        }
    }

    /// Tab `id`'s session said something: when the pending answer waits for its transaction to
    /// end, go on (it ended as asked) or stop (it did not).
    pub(super) fn end_answered(&mut self, id: TabId) {
        if self.pending_end.as_ref().is_none_or(|p| p.current != Some(id)) {
            return;
        }
        let Some(t) = self.tabs.get(id) else { return self.end_stopped(id) };
        match t.exec.last_end {
            // Once the session says no transaction is open any more (`TxOpen(false)` after the
            // block's end): what goes on then sees nothing open, nothing rolled back.
            Some(true) if !t.exec.tx_at_risk() => self.end_next(),
            Some(true) => {}
            Some(false) => self.end_stopped(id),
            // Its session ended without an answer (closed, lost).
            None if t.exec.ending.is_none() && !t.exec.user_tx() => self.end_stopped(id),
            None => {}
        }
    }

    /// The pending answer stops at tab `id`: nothing more ends, and nothing goes on.
    fn end_stopped(&mut self, id: TabId) {
        self.pending_end = None;
        let tab = self.tabs.iter().position(|t| t.id == id).map_or_else(String::new, |i| (i + 1).to_string());
        self.status = Some(Notice::new(Msg::TxEndStopped { tab }, Level::Error));
    }
}

impl App {
    /// The rules changed from `before`: a tab whose transaction was opened for manual commit
    /// while its rules now commit by themselves keeps that transaction (later statements join
    /// it; the user ends it), and says so. Whether one did.
    pub(super) fn commit_rule_changed(&mut self, before: &datarig_core::policy::Policies) -> bool {
        let tabs: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.exec.user_tx() && t.exec.tx.as_ref().is_some_and(|x| x.manual))
            .filter(|t| {
                let name = t.profile.and_then(|p| self.profile(p)).and_then(|p| p.policy.clone());
                let was = before.get(name.as_deref()).is_ok_and(|p| p.commit.manual());
                was && !self.tab_rules(t).commit.manual()
            })
            .map(|t| t.id)
            .collect();
        for id in &tabs {
            let n = Notice::new(Label::TxRuleChanged, Level::Warning);
            if let Some(t) = self.tabs.get_mut(*id) {
                t.exec.run.notes.push(n.clone());
            }
            self.tab_status(*id, n);
        }
        if !tabs.is_empty() {
            self.flash(Notice::new(Label::TxRuleChanged, Level::Warning));
        }
        !tabs.is_empty()
    }
}

/// The lock PostgreSQL's DDL statement `sql` takes on what it changes, by its kind (no catalog
/// is read), and what it names: `true` an `ACCESS EXCLUSIVE` lock (others' reads of it wait),
/// `false` a `SHARE` lock (others' writes wait: `CREATE INDEX`); `None` when that is not known
/// here.
fn ddl_lock(sql: &str) -> Option<(bool, String)> {
    use datarig_core::sql::lexer::Tok;
    let toks: Vec<_> =
        datarig_core::sql::lexer::lex_in(sql, Dialect::Postgres).into_iter().filter(|t| !t.is_trivia()).collect();
    let up = |i: usize| toks.get(i).filter(|t| t.is_word()).map(|t| t.text(sql).to_ascii_uppercase());
    let words: Vec<String> = (0..toks.len().min(4)).map_while(up).collect();
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let (exclusive, from) = match w.as_slice() {
        // `CONCURRENTLY` never runs in a transaction (refused before it is sent).
        ["CREATE", "INDEX" | "UNIQUE", ..] if !w.contains(&"CONCURRENTLY") => {
            (false, toks.iter().position(|t| t.is_word() && t.text(sql).eq_ignore_ascii_case("ON"))? + 1)
        }
        ["ALTER" | "DROP", "MATERIALIZED", ..] => (true, 3),
        ["ALTER" | "DROP", "TABLE" | "INDEX" | "VIEW" | "SEQUENCE", ..] => (true, 2),
        ["TRUNCATE" | "REINDEX" | "CLUSTER", ..] => (true, 1),
        _ => return None,
    };
    // The name: past `IF EXISTS`, `ONLY`, `TABLE`/`INDEX`, the words and dots that touch.
    let skip = ["IF", "EXISTS", "ONLY", "TABLE", "INDEX"];
    let mut k = from;
    while up(k).is_some_and(|w| skip.contains(&w.as_str())) {
        k += 1;
    }
    let mut name = String::new();
    let mut end = None;
    for t in toks.iter().skip(k) {
        let part = t.is_word() || matches!(t.kind, Tok::QuotedIdent | Tok::Dot);
        if !part || end.is_some_and(|e| e != t.start) {
            break;
        }
        name.push_str(t.text(sql));
        end = Some(t.end);
    }
    (!name.is_empty()).then_some((exclusive, name))
}
