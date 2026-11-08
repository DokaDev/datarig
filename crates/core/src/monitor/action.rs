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

use super::format::{Flavour, MonitorFormat, Route};
use super::model::{Need, SessionKey, SessionKind, SessionRow, SessionState, Timestamp, Visibility};
use crate::driver::DbError;
use crate::sql::dialect::MySqlMode;
use crate::sql::risk::{Class, Explain};

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

/// What may be offered for `row` of a server read with `format`, on a profile that is
/// `read_only` or not. Whether the row is one of this app's own tabs is the caller's to tell
/// (it knows the tabs).
pub fn decide(format: &MonitorFormat, row: &SessionRow, read_only: bool, kind: ActionKind) -> Decision {
    if read_only {
        return Decision::Refuse(Refusal::ReadOnly);
    }
    match refusal(format, row, kind) {
        Some(why) => Decision::Refuse(why),
        None => Decision::Offer(ask(format, row, kind)),
    }
}

fn refusal(format: &MonitorFormat, row: &SessionRow, kind: ActionKind) -> Option<Refusal> {
    if row.monitor {
        return Some(Refusal::MonitorConnection);
    }
    let Some(target) = target(format, row) else {
        return Some(Refusal::Hidden(PG_READ_ALL_STATS));
    };
    if matches!(row.kind, SessionKind::Background(_) | SessionKind::Replication | SessionKind::ParallelWorker { .. }) {
        return Some(Refusal::Background);
    }
    let running = runs_statement(row, target.stamp.as_ref());
    let untracked = matches!(target.stamp, None | Some(StatementStamp::Pg { query_start: None }));
    match kind {
        ActionKind::Cancel if !running => Some(Refusal::NoStatement),
        ActionKind::Terminate if matches!(format, MonitorFormat::Postgres(_)) && untracked => Some(Refusal::Untracked),
        ActionKind::Terminate if pooled(format) && !running => Some(Refusal::PooledIdle),
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

/// The monitor is not known to reach the server directly.
fn pooled(format: &MonitorFormat) -> bool {
    matches!(format, MonitorFormat::Postgres(pg) if pg.route != Route::Direct)
}

fn ask(format: &MonitorFormat, row: &SessionRow, kind: ActionKind) -> Ask {
    match (format, kind) {
        (MonitorFormat::Postgres(_), ActionKind::Cancel) => Ask::Line,
        (MonitorFormat::Postgres(_), ActionKind::Terminate) => {
            Ask::Modal { warnings: if pooled(format) { vec![Warning::PoolerSharedBackend] } else { Vec::new() } }
        }
        (MonitorFormat::MySql(_), ActionKind::Cancel) => {
            let mut warnings = Vec::new();
            if row.statement.as_ref().is_some_and(StatementStamp::weak) {
                warnings.push(Warning::WeakIdentity);
            }
            let text = row.query.as_ref();
            if text.is_some_and(|q| maintenance(&q.text)) {
                warnings.push(Warning::MySqlMaintenance);
            } else if !text.is_some_and(|q| !q.shortened && plain_read(&q.text)) {
                warnings.push(Warning::MySqlMayLeaveHalfDone);
            }
            if warnings.is_empty() { Ask::Line } else { Ask::Modal { warnings } }
        }
        (MonitorFormat::MySql(_), ActionKind::Terminate) => {
            Ask::Modal { warnings: vec![Warning::MySqlTerminateKeepsNonTx] }
        }
    }
}

/// The first keywords of MySQL statements that maintain a table.
const MAINTENANCE: &[&str] = &["ALTER", "OPTIMIZE", "REPAIR", "CHECK", "ANALYZE", "LOAD"];

/// The statement's first word is one of [`MAINTENANCE`] (read from a text that may be cut).
fn maintenance(text: &str) -> bool {
    let word: String = text
        .trim_start_matches(|c: char| c.is_whitespace() || c == '(')
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    MAINTENANCE.iter().any(|k| word.eq_ignore_ascii_case(k))
}

/// The MySQL classifier calls the whole text a read with no effect beyond it: no function that
/// is not a built-in, nothing that runs code, no danger, no `EXPLAIN ANALYZE`. The session's
/// sql mode is not known here; the classifier also reads the text as the other modes would and
/// keeps the worse reading.
fn plain_read(text: &str) -> bool {
    let risk = crate::sql::risk::mysql::classify(text, MySqlMode::default());
    risk.class == Class::Read
        && risk.danger.is_none()
        && !risk.writes
        && !risk.unchecked_call
        && !risk.runs_code
        && risk.explain != Explain::Analyze
}

/// What an action on `row` carries, `None` when the account cannot tell the session apart
/// (PostgreSQL: its start is hidden without `pg_read_all_stats`).
pub fn target(format: &MonitorFormat, row: &SessionRow) -> Option<ActionTarget> {
    if let MonitorFormat::Postgres(_) = format
        && (row.visible == Visibility::Hidden || row.key.started.is_none())
    {
        return None;
    }
    Some(ActionTarget { key: row.key, user: row.user.clone(), thread: row.thread, stamp: row.statement.clone() })
}

/// The engine an [`ActionRoute`] is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    Postgres,
    MySql,
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

/// How actions are sent on one engine and provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionRoute {
    pub engine: Engine,
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

const PG_STANDARD: Guidance = Guidance { grants: &["pg_signal_backend"], service_users: &[] };
const MYSQL_STANDARD: Guidance = Guidance { grants: &["CONNECTION_ADMIN"], service_users: &[] };
const RDS_MYSQL: Fallback =
    Fallback { cancel: "mysql.rds_kill_query", terminate: "mysql.rds_kill", on: &[MYSQL_KILL_DENIED] };
const AZURE_MYSQL: Fallback =
    Fallback { cancel: "mysql.az_kill_query", terminate: "mysql.az_kill", on: &[MYSQL_KILL_DENIED] };

/// Every engine and provider, the standard one of each engine first (a provider not listed
/// for an engine takes it).
pub const ROUTES: &[ActionRoute] = &[
    ActionRoute { engine: Engine::Postgres, flavour: Flavour::Standard, fallback: None, guidance: PG_STANDARD },
    ActionRoute {
        engine: Engine::Postgres,
        flavour: Flavour::AmazonRds,
        fallback: None,
        guidance: Guidance { grants: &["rds_superuser", "pg_signal_backend"], service_users: &["rdsadmin"] },
    },
    ActionRoute {
        engine: Engine::Postgres,
        flavour: Flavour::AmazonAurora,
        fallback: None,
        guidance: Guidance { grants: &["rds_superuser", "pg_signal_backend"], service_users: &["rdsadmin"] },
    },
    ActionRoute {
        engine: Engine::Postgres,
        flavour: Flavour::GoogleCloudSql,
        fallback: None,
        guidance: Guidance { grants: &["cloudsqlsuperuser", "pg_signal_backend"], service_users: &["cloudsqladmin"] },
    },
    ActionRoute {
        engine: Engine::Postgres,
        flavour: Flavour::GoogleAlloyDb,
        fallback: None,
        guidance: Guidance { grants: &["alloydbsuperuser", "pg_signal_backend"], service_users: &[] },
    },
    ActionRoute {
        engine: Engine::Postgres,
        flavour: Flavour::AzureDatabase,
        fallback: None,
        guidance: Guidance { grants: &["azure_pg_admin", "pg_signal_backend"], service_users: &["azuresu"] },
    },
    ActionRoute { engine: Engine::MySql, flavour: Flavour::Standard, fallback: None, guidance: MYSQL_STANDARD },
    ActionRoute {
        engine: Engine::MySql,
        flavour: Flavour::AmazonRds,
        fallback: Some(RDS_MYSQL),
        guidance: Guidance { grants: MYSQL_STANDARD.grants, service_users: &["rdsadmin"] },
    },
    ActionRoute {
        engine: Engine::MySql,
        flavour: Flavour::AmazonAurora,
        fallback: Some(RDS_MYSQL),
        guidance: Guidance { grants: MYSQL_STANDARD.grants, service_users: &["rdsadmin"] },
    },
    ActionRoute {
        engine: Engine::MySql,
        flavour: Flavour::AzureDatabase,
        fallback: Some(AZURE_MYSQL),
        guidance: Guidance { grants: MYSQL_STANDARD.grants, service_users: &["azure_superuser"] },
    },
];

/// How actions are sent to the server of `format`.
pub fn route(format: &MonitorFormat) -> &'static ActionRoute {
    let engine = match format {
        MonitorFormat::Postgres(_) => Engine::Postgres,
        MonitorFormat::MySql(_) => Engine::MySql,
    };
    let flavour = format.flavour();
    ROUTES
        .iter()
        .find(|r| r.engine == engine && r.flavour == flavour)
        .or_else(|| ROUTES.iter().find(|r| r.engine == engine && r.flavour == Flavour::Standard))
        .expect("every engine has a standard route")
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

/// The last MySQL version of Amazon RDS whose kill procedures fail for long target user names.
const RDS_LONG_USER_FIXED_AFTER: u32 = 80032;

/// The longest target user name those procedures take.
const RDS_LONG_USER_CHARS: usize = 16;

/// What the server's refusal `error` of an action on `target`, sent `via`, means; `None` when
/// it is not a refusal (another failure).
pub fn denied(format: &MonitorFormat, target: &ActionTarget, via: Via, error: NativeError) -> Option<Denied> {
    let route = route(format);
    let service = target.user.as_deref().is_some_and(|u| route.guidance.service_users.contains(&u));
    match (format, error) {
        (MonitorFormat::Postgres(pg), NativeError::Pg { sqlstate }) if sqlstate == PG_INSUFFICIENT_PRIVILEGE => {
            Some(if service {
                Denied::ServiceSession
            } else if pg.signal_backend || pg.superuser {
                // The account may signal other roles' sessions: what is left is a superuser's.
                Denied::SuperuserTarget
            } else {
                Denied::Grant(route.guidance.grants)
            })
        }
        (MonitorFormat::MySql(my), NativeError::MySql { errno }) => {
            let long_user = target.user.as_deref().is_some_and(|u| u.chars().count() > RDS_LONG_USER_CHARS);
            match errno {
                _ if matches!(via, Via::Procedure(_))
                    && my.flavour == Flavour::AmazonRds
                    && my.version <= RDS_LONG_USER_FIXED_AFTER
                    && long_user
                    && [MYSQL_KILL_DENIED, MYSQL_NEEDS_PRIVILEGE].contains(&errno) =>
                {
                    Some(Denied::RdsLongUserName)
                }
                MYSQL_KILL_DENIED | MYSQL_NEEDS_PRIVILEGE if service => Some(Denied::ServiceSession),
                MYSQL_KILL_DENIED => Some(Denied::GrantOrSystemUser(route.guidance.grants)),
                MYSQL_NEEDS_PRIVILEGE => Some(Denied::SystemUserTarget),
                MYSQL_EXECUTE_DENIED => match via {
                    Via::Procedure(name) => Some(Denied::ProcedureDenied(name)),
                    Via::Standard => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
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
}

/// What a read of the target's id just before an action of `kind` on `target` shows of it
/// (`now`: the session of that id as the read found it, `None` when there is none), for a
/// driver that reads the session again before it acts rather than in the statement that acts:
/// why nothing may be sent, `None` when it may. A cancel needs the same statement still
/// running; a weak stamp that matches is taken for it (the user was warned it may not be).
pub fn recheck(kind: ActionKind, target: &ActionTarget, now: Option<&SessionRow>) -> Option<NotSent> {
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
