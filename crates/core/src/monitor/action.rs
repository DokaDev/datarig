//! Acting on another session of the server: cancelling its statement or ending it, from the
//! monitor's session list.
//!
//! Everything here is pure: what may be offered for a row and how it is asked ([`decide`]), what
//! the command carries to tell the session and its statement apart from whatever has taken their
//! place since the row was read ([`target`]), which statements stop a session on each provider
//! and what a refusal means there ([`route`], [`denied`]), what the driver reports back
//! ([`ActionOutcome`]) and what a later read of the session shows ([`confirm`]).
//!
//! A signal sent is not a statement stopped. The server's answer is shown as it is; a session
//! ended is confirmed only when a later read no longer finds it, and a cancel is called
//! successful only when the server tells it interrupted that very statement.

use super::format::{Flavour, MonitorFormat};
use super::model::{Need, SessionKey, SessionKind, SessionRow, SessionState, Timestamp, Visibility};
use crate::driver::DbError;
use crate::engine::{MonitorRules, Why};

/// What to do to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// Stop the statement it runs; the session stays (PostgreSQL `pg_cancel_backend`, MySQL
    /// `KILL QUERY`).
    Cancel,
    /// End the session; its open transaction is rolled back (PostgreSQL
    /// `pg_terminate_backend`, MySQL `KILL`).
    Terminate,
}

/// The identity of the statement a session ran when its row was read: the driver acts only if
/// the session still runs that statement, and never looks the statement up again on its own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StatementStamp {
    /// PostgreSQL: `query_start` as the tick read it, in the server's microseconds (`None`: the
    /// session never ran a statement).
    Pg { query_start: Option<Timestamp> },
    /// MySQL with `performance_schema`: the running top-level statement of the thread
    /// (`events_statements_current` with no `END_EVENT_ID`, nesting level 0, on an instrumented
    /// thread with the consumer on).
    MySqlEvent { thread_id: u64, event_id: u64 },
    /// MySQL without a running statement event to go by: `COMMAND`, the server's MD5 of the
    /// whole `INFO`, and `TIME` in seconds. The same text may be a new run of it, so a match is
    /// never proof.
    MySqlWeak { command: String, info_md5: Option<String>, time: u64 },
}

impl StatementStamp {
    /// A match does not prove the same statement ([`StatementStamp::MySqlWeak`]).
    pub fn weak(&self) -> bool {
        matches!(self, StatementStamp::MySqlWeak { .. })
    }

    /// Whether `now` (a later read of the same session) is still the statement this stamp
    /// names.
    fn same(&self, now: Option<&StatementStamp>) -> Same {
        match (self, now) {
            (_, None) => Same::Other,
            (StatementStamp::Pg { query_start: a }, Some(StatementStamp::Pg { query_start: b })) => {
                if a == b {
                    Same::Yes
                } else {
                    Same::Other
                }
            }
            (
                StatementStamp::MySqlEvent { thread_id: t, event_id: e },
                Some(StatementStamp::MySqlEvent { thread_id: u, event_id: f }),
            ) => {
                if t == u && e == f {
                    Same::Yes
                } else {
                    Same::Other
                }
            }
            (
                StatementStamp::MySqlWeak { command: c, info_md5: m, time: t },
                Some(StatementStamp::MySqlWeak { command: d, info_md5: n, time: u }),
            ) => {
                if c == d && m == n && u >= t {
                    Same::Weak
                } else {
                    Same::Other
                }
            }
            // A stamp of one kind then and of another now (the statement events were switched
            // on or off meanwhile): nothing to compare.
            _ => Same::CannotTell,
        }
    }
}

/// How a later read of a session's statement compares with a stamp ([`StatementStamp::same`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Same {
    /// The same statement.
    Yes,
    /// Another statement, or none.
    Other,
    /// A weak stamp that matches: maybe the same, maybe a new run of the same text.
    Weak,
    /// The two cannot be compared.
    CannotTell,
}

/// Whether the session `row` shows is the one `target` names: `Some(false)` when another took
/// its id (a later start, another thread), `None` when what would tell is hidden now.
fn same_session(target: &ActionTarget, row: &SessionRow) -> Option<bool> {
    if row.key.id != target.key.id {
        return Some(false);
    }
    let mut known = true;
    if let Some(started) = target.key.started {
        match row.key.started {
            Some(now) if now != started => return Some(false),
            Some(_) => {}
            None => known = false,
        }
    }
    if let Some(thread) = target.thread {
        match row.thread {
            Some(now) if now != thread => return Some(false),
            Some(_) => {}
            None => known = false,
        }
    }
    known.then_some(true)
}

/// What an action command carries: the session (and its statement) as the row the user acted
/// on showed it. A target read before the monitor reconnected names nothing on the new
/// connection's server and is dropped.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActionTarget {
    /// The session: PostgreSQL pid and `backend_start`; MySQL connection id (ids wrap, so the
    /// thread below tells a later session of the same id apart where it is known).
    pub key: SessionKey,
    /// The account the session runs as.
    pub user: Option<String>,
    /// MySQL `performance_schema` `THREAD_ID`, when readable.
    pub thread: Option<u64>,
    /// The statement it ran; a terminate needs none (except behind a pooler).
    pub stamp: Option<StatementStamp>,
}

/// Why an action is not offered for a row (listed dim with the reason) or not sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The profile is read-only: both actions are refused.
    ReadOnly,
    /// The monitor's own connection.
    MonitorConnection,
    /// What tells the session (or its statement) apart is hidden from the account, and what
    /// would show it.
    Hidden(Need),
    /// A cancel of a session that runs no statement.
    NoStatement,
    /// One of the server's own processes, a replication connection or a parallel worker (its
    /// leader is the one to cancel).
    Background,
    /// Behind a pooler a server session can serve any client: ending one that runs no
    /// statement could end someone else's work.
    PooledIdle,
    /// PostgreSQL: the session shows no statement start (the server does not track its
    /// activity, or it never ran a statement), so a later client of the same server session (a
    /// pooler's) could not be told apart from the one the row showed.
    Untracked,
    /// MySQL: the monitor's connection could not be confirmed to reach the server directly
    /// ([`crate::monitor::MySqlMonitor::unconfirmed`]): its actions could reach another session.
    Unconfirmed,
    /// The server's engine has no monitor rules for the format ([`rules_for`]), or they do not
    /// read it.
    Engine(Why),
}

/// What the user is told before an action is sent, besides what the session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Warning {
    /// MySQL: stopping table maintenance (`ALTER`, `OPTIMIZE`, `REPAIR`, `CHECK`, `ANALYZE`,
    /// `LOAD`) can leave the table half done or needing repair.
    MySqlMaintenance,
    /// MySQL: not a plain read, or not one the text shows: a statement stopped part way may
    /// leave the changes it made to a non-transactional table (MyISAM) half done.
    MySqlMayLeaveHalfDone,
    /// MySQL: ending the session rolls back its transactional changes; non-transactional ones
    /// stay.
    MySqlTerminateKeepsNonTx,
    /// The monitor reaches the server through a pooler (or cannot tell it does not): the server
    /// session may serve another client by the time it ends.
    PoolerSharedBackend,
    /// The statement is told apart by its text and times only: the same text may be a new run
    /// of it.
    WeakIdentity,
}

/// How an action is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// A one-line ask on the status line.
    Line,
    /// A dialog naming the session, with what it warns of.
    Modal { warnings: Vec<Warning> },
}

/// What may be done for a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Offer(Ask),
    Refuse(Refusal),
}

/// The grant that shows a PostgreSQL session's start and statement times to an account.
const PG_READ_ALL_STATS: Need = Need::Grant("pg_read_all_stats");

/// The monitor rules of the engine whose probe gave `format`, checked to read it: why there are
/// none ([`Why::EnginesDisagree`] when they do not read it). Every question of an action asks
/// them so, first.
pub fn rules_for(format: &MonitorFormat) -> Result<&'static dyn MonitorRules, Why> {
    rules_for_with(format.rules(), format)
}

/// [`rules_for`] of the registry's answer `lookup` for `format`: the rules, when they read it.
pub fn rules_for_with(
    lookup: Result<&'static dyn MonitorRules, Why>,
    format: &MonitorFormat,
) -> Result<&'static dyn MonitorRules, Why> {
    let rules = lookup?;
    match rules.reads(format) {
        true => Ok(rules),
        false => Err(Why::EnginesDisagree),
    }
}

/// What may be offered for `row` of a server read with `format`, on a profile that is
/// `read_only` or not. Whether the row is one of this app's own tabs is the caller's to tell
/// (it knows the tabs).
pub fn decide(format: &MonitorFormat, row: &SessionRow, read_only: bool, kind: ActionKind) -> Decision {
    decide_with(MonitorFormat::rules, format, row, read_only, kind)
}

/// [`decide`] with the engine's rules looked up by `lookup` (checked by [`rules_for_with`]),
/// which is called only for a profile that may act: read-only is refused first.
pub fn decide_with(
    lookup: impl FnOnce(&MonitorFormat) -> Result<&'static dyn MonitorRules, Why>,
    format: &MonitorFormat,
    row: &SessionRow,
    read_only: bool,
    kind: ActionKind,
) -> Decision {
    if read_only {
        return Decision::Refuse(Refusal::ReadOnly);
    }
    let rules = match rules_for_with(lookup(format), format) {
        Ok(rules) => rules,
        Err(why) => return Decision::Refuse(Refusal::Engine(why)),
    };
    match refusal(rules, format, row, kind) {
        Some(why) => Decision::Refuse(why),
        None => Decision::Offer(rules.ask(format, row, kind)),
    }
}

fn refusal(rules: &dyn MonitorRules, format: &MonitorFormat, row: &SessionRow, kind: ActionKind) -> Option<Refusal> {
    if row.monitor {
        return Some(Refusal::MonitorConnection);
    }
    if rules.unconfirmed(format) {
        return Some(Refusal::Unconfirmed);
    }
    let Some(target) = identified(rules, format, row) else {
        return Some(Refusal::Hidden(PG_READ_ALL_STATS));
    };
    if matches!(row.kind, SessionKind::Background(_) | SessionKind::Replication | SessionKind::ParallelWorker { .. }) {
        return Some(Refusal::Background);
    }
    let running = runs_statement(row, target.stamp.as_ref());
    let untracked = matches!(target.stamp, None | Some(StatementStamp::Pg { query_start: None }));
    match kind {
        ActionKind::Cancel if !running => Some(Refusal::NoStatement),
        ActionKind::Terminate if rules.end_needs_start() && untracked => Some(Refusal::Untracked),
        ActionKind::Terminate if rules.pooled(format) && !running => Some(Refusal::PooledIdle),
        _ => None,
    }
}

/// The row runs a statement that `stamp` names.
fn runs_statement(row: &SessionRow, stamp: Option<&StatementStamp>) -> bool {
    let idle = matches!(row.state, SessionState::Idle | SessionState::IdleInTx | SessionState::IdleInTxAborted);
    let stamped = match stamp {
        None | Some(StatementStamp::Pg { query_start: None }) => false,
        Some(_) => true,
    };
    stamped && !idle
}

/// What an action on `row` carries, `None` when the account cannot tell the session apart
/// (PostgreSQL: its start is hidden without `pg_read_all_stats`), or the format's engine has no
/// monitor rules that read it.
pub fn target(format: &MonitorFormat, row: &SessionRow) -> Option<ActionTarget> {
    target_with(format.rules(), format, row)
}

/// [`target`] with the engine's rules looked up as `lookup` (checked by [`rules_for_with`]).
pub fn target_with(
    lookup: Result<&'static dyn MonitorRules, Why>,
    format: &MonitorFormat,
    row: &SessionRow,
) -> Option<ActionTarget> {
    identified(rules_for_with(lookup, format).ok()?, format, row)
}

/// [`target`] by `rules`.
fn identified(rules: &dyn MonitorRules, format: &MonitorFormat, row: &SessionRow) -> Option<ActionTarget> {
    if !rules.identifies(format, row) {
        return None;
    }
    Some(ActionTarget { key: row.key, user: row.user.clone(), thread: row.thread, stamp: row.statement.clone() })
}

/// The provider's own procedures that stop a session when the standard statement is refused
/// (a managed service gives its main account no privilege to `KILL` other accounts' sessions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fallback {
    /// Called with the session's id to cancel its statement.
    pub cancel: &'static str,
    /// Called with the session's id to end it.
    pub terminate: &'static str,
    /// The server's error codes of the standard statement that call for it.
    pub on: &'static [u16],
}

impl Fallback {
    /// The procedure for `kind`.
    pub fn procedure(&self, kind: ActionKind) -> &'static str {
        match kind {
            ActionKind::Cancel => self.cancel,
            ActionKind::Terminate => self.terminate,
        }
    }
}

/// What to name when the server refuses an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guidance {
    /// The grants or roles that allow the action on other accounts' sessions, by their server
    /// names.
    pub grants: &'static [&'static str],
    /// Accounts whose sessions are the provider's own: no customer's grant reaches them.
    pub service_users: &'static [&'static str],
}

/// How actions are sent on one engine and provider (an engine's are its
/// [`MonitorRules::routes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionRoute {
    pub flavour: Flavour,
    pub fallback: Option<Fallback>,
    pub guidance: Guidance,
}

/// The MySQL error of a `KILL` of another account's session without the privilege
/// (`ER_KILL_DENIED_ERROR`).
pub const MYSQL_KILL_DENIED: u16 = 1095;

/// The MySQL error of a statement that needs a privilege the account lacks
/// (`ER_SPECIFIC_ACCESS_DENIED_ERROR`): a `KILL` of a session holding `SYSTEM_USER`.
pub const MYSQL_NEEDS_PRIVILEGE: u16 = 1227;

/// The MySQL error of a `CALL` the account may not run (`ER_PROCACCESS_DENIED_ERROR`).
pub const MYSQL_EXECUTE_DENIED: u16 = 1370;

/// PostgreSQL's SQLSTATE for a missing privilege (`insufficient_privilege`).
pub const PG_INSUFFICIENT_PRIVILEGE: &str = "42501";

/// How actions are sent to the server of `format`: its engine's route for its provider, else the
/// engine's standard one; why there is none when the engine has no monitor rules that read it.
pub fn route(format: &MonitorFormat) -> Result<&'static ActionRoute, Why> {
    route_with(format.rules(), format)
}

/// [`route`] with the engine's rules looked up as `lookup` (checked by [`rules_for_with`]).
pub fn route_with(
    lookup: Result<&'static dyn MonitorRules, Why>,
    format: &MonitorFormat,
) -> Result<&'static ActionRoute, Why> {
    Ok(rules_for_with(lookup, format)?.routes().of(format.flavour()))
}

/// How the action was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The engine's own statement or function.
    Standard,
    /// The provider's procedure ([`Fallback`]).
    Procedure(&'static str),
}

/// Why the target no longer is what the user acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// No session of that id.
    SessionGone,
    /// A session of that id that started later (or another thread): not the one acted on.
    SessionReplaced,
    /// The session runs another statement, or none.
    StatementChanged,
    /// What tells the session or its statement apart cannot be compared now (hidden from the
    /// account since, or the statement events switched on or off): nothing is sent, the row
    /// should be read again.
    CannotTell,
}

/// What the server's refusal of an action means, from its native error code
/// ([`denied`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denied {
    /// The account lacks the privilege: one of these grants allows it (on PostgreSQL, being
    /// the target's role or a member of it does too).
    Grant(&'static [&'static str]),
    /// PostgreSQL: the target is a superuser's session, which only a superuser may signal.
    SuperuserTarget,
    /// MySQL: the target holds `SYSTEM_USER`, which only an account with it may stop.
    SystemUserTarget,
    /// MySQL `KILL` refused (1095): the account lacks one of these grants for another
    /// account's session, or the target's account holds `SYSTEM_USER` and this one does not
    /// (the server answers both alike).
    GrantOrSystemUser(&'static [&'static str]),
    /// The session is the provider's own (its account is one of the route's service users).
    ServiceSession,
    /// The account may not run the provider's procedure (`EXECUTE` on it).
    ProcedureDenied(&'static str),
    /// Amazon RDS for MySQL up to 8.0.32: its kill procedures fail for target user names
    /// longer than 16 characters; nothing the account is granted changes that.
    RdsLongUserName,
}

/// A server's native error code, before it becomes a [`DbError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeError<'a> {
    Pg { sqlstate: &'a str },
    MySql { errno: u16 },
}

/// What the server's refusal `error` of an action on `target`, sent `via`, means; `None` when
/// it is not a refusal (another failure), or the format's engine has no monitor rules that read
/// it.
pub fn denied(format: &MonitorFormat, target: &ActionTarget, via: Via, error: NativeError) -> Option<Denied> {
    denied_with(format.rules(), format, target, via, error)
}

/// [`denied`] with the engine's rules looked up as `lookup` (checked by [`rules_for_with`]).
pub fn denied_with(
    lookup: Result<&'static dyn MonitorRules, Why>,
    format: &MonitorFormat,
    target: &ActionTarget,
    via: Via,
    error: NativeError,
) -> Option<Denied> {
    rules_for_with(lookup, format).ok()?.denied(format, target, via, error)
}

/// Why nothing was sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotSent {
    /// The guard read found the target changed.
    Stale(Stale),
    /// The server refused it.
    Refused(Denied),
    /// The profile is read-only.
    ReadOnly,
    /// The target is the connection that would send it.
    OwnConnection,
    /// MySQL: the target carries neither its start nor its thread (`THREAD_ID`, read from
    /// `performance_schema.threads`), so it cannot be told apart from a later session given the
    /// same id (MySQL reuses ids while it runs). The account needs `SELECT` on
    /// `performance_schema.threads`, with the performance schema on.
    Unidentified,
    /// MySQL: the monitor's connection could not be confirmed to reach the server directly (its
    /// own marker on its own id, the handshake's), so the read before the action and the action
    /// could reach different sessions.
    Unconfirmed,
    /// The server's engine has no monitor rules for the format, or they do not read it
    /// ([`rules_for`]).
    Engine(Why),
}

/// What a read of the target's id just before an action of `kind` on `target` shows of it
/// (`now`: the session of that id as the read found it, `None` when there is none), for a
/// driver that reads the session again before it acts rather than in the statement that acts:
/// why nothing may be sent, `None` when it may. A cancel needs the same statement still
/// running; a weak stamp that matches is taken for it (the user was warned it may not be). A
/// target with neither its start nor its thread known is never acted on: the id alone may name
/// a later session.
pub fn recheck(kind: ActionKind, target: &ActionTarget, now: Option<&SessionRow>) -> Option<NotSent> {
    if target.key.started.is_none() && target.thread.is_none() {
        return Some(NotSent::Unidentified);
    }
    let Some(row) = now else {
        return Some(NotSent::Stale(Stale::SessionGone));
    };
    if row.monitor {
        return Some(NotSent::OwnConnection);
    }
    match same_session(target, row) {
        Some(true) => {}
        Some(false) => return Some(NotSent::Stale(Stale::SessionReplaced)),
        None => return Some(NotSent::Stale(Stale::CannotTell)),
    }
    // Another account shown: a later session, or the same one inside a `SQL SECURITY DEFINER`
    // routine (MySQL before 8.0.22 shows the definer then). Nothing is sent either way.
    if let (Some(then), Some(now)) = (&target.user, &row.user)
        && then != now
    {
        return Some(NotSent::Stale(Stale::CannotTell));
    }
    let changed = Some(NotSent::Stale(Stale::StatementChanged));
    match kind {
        ActionKind::Terminate => None,
        ActionKind::Cancel => {
            let Some(stamp) = &target.stamp else { return changed };
            if row.visible == Visibility::Hidden {
                return Some(NotSent::Stale(Stale::CannotTell));
            }
            if !runs_statement(row, row.statement.as_ref()) {
                return changed;
            }
            match stamp.same(row.statement.as_ref()) {
                Same::Other => changed,
                Same::CannotTell => Some(NotSent::Stale(Stale::CannotTell)),
                Same::Yes | Same::Weak => None,
            }
        }
    }
}

/// What became of an action ([`crate::driver::DbEvent::MonitorActionDone`]). Only what the
/// server said is reported: an answer lost on the way is never taken for a yes or a no, and
/// the action is never sent again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Nothing reached the target.
    NotSent(NotSent),
    /// Sent, and the server said the signal did not reach a session (PostgreSQL `false`: no
    /// such backend by then, or the signal failed).
    NotDelivered,
    /// Sent, and the server took it: a statement stops or a session ends later, which a
    /// follow-up read shows ([`confirm`]).
    Acknowledged { via: Via },
    /// Sent, and its answer lost (a timeout, the connection lost): only a later read tells.
    OutcomeUnknown,
    /// Failed before anything was sent.
    Failed(DbError),
}

/// A read of one session after an action ([`crate::driver::DbEvent::MonitorFollowUp`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUp {
    /// The session of the target's id as it is now, `None` when there is none.
    pub row: Option<SessionRow>,
    /// Whether the server tells the target's statement was interrupted (MySQL
    /// `events_statements_history` error 1317 for that thread and event), `None` when it cannot
    /// tell.
    pub interrupted: Option<bool>,
}

/// What a follow-up read shows of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// The session is gone (or another took its id).
    Gone,
    /// The statement ended after the cancel was sent: it may have finished on its own.
    EndedAfterCancel,
    /// The server tells the statement was interrupted.
    Interrupted,
    /// The same statement still runs.
    StillRunning,
    /// The session is still there.
    StillThere,
    /// The read cannot tell.
    Unknown,
}

/// What `follow` shows of an action of `kind` on `target`.
pub fn confirm(kind: ActionKind, target: &ActionTarget, follow: &FollowUp) -> Confirmation {
    if kind == ActionKind::Cancel && follow.interrupted == Some(true) {
        return Confirmation::Interrupted;
    }
    let Some(row) = &follow.row else {
        return Confirmation::Gone;
    };
    match same_session(target, row) {
        Some(true) => {}
        Some(false) => return Confirmation::Gone,
        // A hidden start is no proof the session went.
        None => return Confirmation::Unknown,
    }
    match kind {
        ActionKind::Terminate => Confirmation::StillThere,
        ActionKind::Cancel => {
            if row.visible == Visibility::Hidden {
                return Confirmation::Unknown;
            }
            let Some(stamp) = &target.stamp else {
                return Confirmation::Unknown;
            };
            let running = runs_statement(row, row.statement.as_ref());
            match stamp.same(row.statement.as_ref()) {
                Same::Yes if running => Confirmation::StillRunning,
                Same::Yes | Same::Other => Confirmation::EndedAfterCancel,
                // Neither a weak stamp that matches nor one that cannot be compared is taken for
                // the same statement, nor for another while a statement runs.
                Same::Weak | Same::CannotTell if !running => Confirmation::EndedAfterCancel,
                Same::Weak | Same::CannotTell => Confirmation::Unknown,
            }
        }
    }
}
