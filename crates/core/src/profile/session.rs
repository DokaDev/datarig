//! Startup SQL presets (session presets in 0.31): named lists of settings a query session starts
//! with, kept next to the profiles in the config file as `[startup.<name>]` tables (their 0.31
//! name `[sessions.<name>]` is read too), for one kind of database each:
//!
//! ```toml
//! [startup.reporting]
//! dbms = "postgres"                 # "postgres" | "mysql"
//! sql = ["SET statement_timeout = '30s'", "SET work_mem = '64MB'"]
//!
//! [policy.prod]
//! startup = { postgres = "reporting", mysql = "my_reporting" }
//!
//! [[connections]]
//! name = "orders"
//! startup = "reporting"             # this profile's preset ("none": none, whatever its
//!                                   # environment maps)
//! startup_sql = ["SET work_mem = '256MB'"]   # its own statements, after the preset's
//! ```
//!
//! Presets are optional: a profile with none of these keys starts its sessions with the
//! database's defaults. What a query session starts with is its [`plan`], in order (a later
//! setting of the same name wins): the app's own limits (the intents `lock_wait`,
//! `idle_disconnect`, `tx_idle_rollback`, from the environment or the config), then the preset
//! (the profile's own `startup`, else the one its environment maps for its database), then the
//! profile's `startup_sql`. A preset name that is not there, or one for the other database, is an
//! error of that profile: it does not connect (never without the preset instead). Each
//! statement must be one setting of the session ([`crate::sql::risk::session_set`]); in a
//! read-only environment only the settings a read-only session may change. Metadata sessions
//! never get them.
//!
//! A statement may set the lock wait limit itself (PostgreSQL's `lock_timeout`, MySQL's
//! `lock_wait_timeout` and `innodb_lock_wait_timeout`): it overrides the environment's
//! `lock_wait` for those sessions, and the plan says so with the value the server then has, as
//! far as the app can read it ([`SessionPlan::lock_wait`]).
//!
//! A statement may set the server's idle limit too (PostgreSQL's `idle_session_timeout`, MySQL's
//! `wait_timeout`): that value is then the idle disconnect of those sessions in place of the
//! environment's `idle_disconnect` ([`SessionPlan::idle`]); the app's own timer follows it.
//! PostgreSQL's has no form for one transaction (the server counts it only outside one), so
//! behind a pooler it is not sent and the environment's limit stays.
//!
//! On PostgreSQL behind a pooler in transaction mode the session's settings would stay on server
//! connections other clients share: there each statement goes first in every transaction as
//! `SET LOCAL` instead ([`OnPooler`]), and one that has no such form is not sent (the profile
//! still connects, and says so).

use super::ConnectionConfig;
use crate::policy::Policy;
use crate::sql::dialect::{Dialect, Language, MySqlMode};
use crate::sql::risk::{Classifier, NotLocal, ReadOnlyBlock, SetProblem};
use serde::{Deserialize, Deserializer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// The rules of a preset's name are those of every named table of the file (see
// [`super::name`]).
pub use super::name::{NAME_MAX, NameProblem, name_problem, same_name};

/// The value of a profile's `session` that picks no preset ([`SessionRef::Off`]): never a
/// preset's name.
pub const NONE: &str = "none";

/// A preset's identity while the app runs (never written: the file knows it by its name). It
/// stays the same across a rename.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionPresetId(u64);

impl SessionPresetId {
    /// A new id, unique in this process.
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        SessionPresetId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for SessionPresetId {
    fn default() -> Self {
        Self::new()
    }
}

/// The kind of database a preset is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dbms {
    Postgres,
    MySql,
}

impl Dbms {
    pub const ALL: [Dbms; 2] = [Dbms::Postgres, Dbms::MySql];

    /// `dbms` as the file writes it: `"postgres"` or `"mysql"`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "postgres" => Some(Dbms::Postgres),
            "mysql" => Some(Dbms::MySql),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Dbms::Postgres => "postgres",
            Dbms::MySql => "mysql",
        }
    }

    /// Its name as people write it.
    pub fn display(self) -> &'static str {
        match self {
            Dbms::Postgres => "PostgreSQL",
            Dbms::MySql => "MySQL",
        }
    }

    /// The database of a profile's `driver` (`None`: a driver no preset is for).
    pub fn of_driver(driver: &str) -> Option<Self> {
        match driver.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Some(Dbms::Postgres),
            "mysql" | "mariadb" => Some(Dbms::MySql),
            _ => None,
        }
    }

    /// The language its statements are checked in (MySQL: the default sql mode; the check
    /// also reads them with `ANSI_QUOTES`).
    pub fn language(self) -> Language {
        match self {
            Dbms::Postgres => Language::Sql(Dialect::Postgres),
            Dbms::MySql => Language::Sql(Dialect::MySql(MySqlMode::default())),
        }
    }
}

/// A session preset: a `[sessions.<name>]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPreset {
    pub id: SessionPresetId,
    /// The table's key, and what profiles and environments name.
    pub name: String,
    pub dbms: Dbms,
    /// One statement per item, in order.
    pub sql: Vec<String>,
    /// The name its table has in the file (a rename moves that table, comments and all).
    pub origin: Option<String>,
}

impl SessionPreset {
    /// A new preset `name` for `dbms` with `sql`, and a new id.
    pub fn new(name: &str, dbms: Dbms, sql: Vec<String>) -> Self {
        SessionPreset { id: SessionPresetId::new(), name: name.to_string(), dbms, sql, origin: None }
    }

    /// The first statement that cannot be a start-up setting, and why (`index` from 1).
    pub fn problem(&self) -> Option<(usize, SetProblem)> {
        self.sql
            .iter()
            .enumerate()
            .find_map(|(i, s)| Classifier::session_set(self.dbms.language(), s).err().map(|p| (i + 1, p)))
    }

    /// The first statement a read-only environment refuses (`index` from 1).
    pub fn read_only_refused(&self) -> Option<(usize, ReadOnlyBlock)> {
        self.sql.iter().enumerate().find_map(|(i, s)| match Classifier::session_set(self.dbms.language(), s) {
            Ok(set) => set.risk.read_only().err().map(|b| (i + 1, b)),
            Err(_) => None,
        })
    }
}

/// `[startup.<name>]` as the file has it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionSection {
    #[serde(default)]
    pub dbms: Option<String>,
    #[serde(default)]
    pub sql: Vec<String>,
}

/// A profile's `startup`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SessionRef {
    /// No key: the preset its environment maps for its database, if any.
    #[default]
    Inherit,
    /// `"none"`: no preset, whatever its environment maps.
    Off,
    /// The preset of this name.
    Named(String),
}

impl SessionRef {
    /// The value as written (`None`: no key).
    pub fn as_value(&self) -> Option<&str> {
        match self {
            SessionRef::Inherit => None,
            SessionRef::Off => Some(NONE),
            SessionRef::Named(n) => Some(n),
        }
    }

    /// A value as the file has it: `"none"` is [`SessionRef::Off`], another name a preset's.
    pub fn of(value: &str) -> Self {
        if value == NONE { SessionRef::Off } else { SessionRef::Named(value.to_string()) }
    }
}

impl<'de> Deserialize<'de> for SessionRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(SessionRef::of(&s))
    }
}

/// An environment's `startup`: the preset of each database its profiles start with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionMap {
    pub postgres: Option<String>,
    pub mysql: Option<String>,
}

impl SessionMap {
    /// The preset it maps for `dbms`.
    pub fn get(&self, dbms: Dbms) -> Option<&str> {
        match dbms {
            Dbms::Postgres => self.postgres.as_deref(),
            Dbms::MySql => self.mysql.as_deref(),
        }
    }

    /// Map `dbms` to `preset` (`None`: no preset).
    pub fn set(&mut self, dbms: Dbms, preset: Option<String>) {
        match dbms {
            Dbms::Postgres => self.postgres = preset,
            Dbms::MySql => self.mysql = preset,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.postgres.is_none() && self.mysql.is_none()
    }

    /// Every preset it names, with its database.
    pub fn iter(&self) -> impl Iterator<Item = (Dbms, &str)> {
        Dbms::ALL.into_iter().filter_map(|d| self.get(d).map(|n| (d, n)))
    }
}

/// Who picked a preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickedBy {
    /// The profile's environment (`None`: the default rules, `[policy.default]`).
    Environment(Option<String>),
    /// The profile's own `session`.
    Profile,
}

/// Where an item of a plan comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The config's top-level setting (no environment says otherwise).
    Global,
    /// The default rules (`[policy.default]`, or their built-in defaults).
    DefaultRules,
    /// The profile's environment.
    Environment(String),
    /// A preset, and who picked it.
    Preset { name: String, by: PickedBy },
    /// The profile's own `session_sql`.
    Profile,
}

/// A limit the app sets in its own way on each driver (an environment's neutral setting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `lock_wait` (`None`: the server's setting).
    LockWait(Option<Duration>),
    /// `idle_disconnect` (`None`: never).
    IdleDisconnect(Option<Duration>),
    /// `tx_idle_rollback` (`None`: never).
    TxIdleRollback(Option<Duration>),
}

/// What an item of a plan is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Planned {
    Intent(Intent),
    /// A statement, sent as written.
    Sql(String),
    /// The environment maps this preset for the profile's database and the profile chose none
    /// (`session = "none"`): skipped.
    Skipped {
        preset: String,
    },
}

/// How a statement goes on PostgreSQL behind a pooler in transaction mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnPooler {
    /// First in every transaction, as this `SET LOCAL`.
    PerTransaction(String),
    /// Not sent: it has no form for one transaction.
    Refused(NotLocal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub what: Planned,
    pub source: Source,
    /// A PostgreSQL statement's way behind a pooler (`None`: not a PostgreSQL statement).
    pub pooled: Option<OnPooler>,
}

/// What a profile's query sessions start with, in order: a later setting of the same name wins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPlan {
    pub items: Vec<PlanItem>,
    /// The lock wait its statements set in place of the intent's `lock_wait` (`None`: none of
    /// them sets it).
    pub lock_wait: Option<LockWaitOverride>,
    /// The server's idle limit its statements set in place of the intent's `idle_disconnect`
    /// (`None`: none of them sets it).
    pub idle: Option<IdleOverride>,
}

/// An idle limit a plan's statements set, which overrides the environment's `idle_disconnect`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdleOverride {
    /// Where the last statement that sets it comes from.
    pub source: Source,
    /// The server's limit then on a direct connection ([`Wait::NoLimit`]: never).
    pub direct: Wait,
    /// The same behind a pooler in transaction mode, where a statement with no form for one
    /// transaction is not sent: the intent's limit when none is sent there.
    pub pooled: Wait,
}

/// A lock wait limit a plan's statements set, which overrides the environment's `lock_wait`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockWaitOverride {
    /// Where the last statement that sets it comes from.
    pub source: Source,
    /// What the server's limit then is.
    pub wait: Wait,
}

/// A limit (a lock wait, an idle time) as the app reads a statement's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// A statement waits this long at most.
    Limit(Duration),
    /// No limit (PostgreSQL's `0`).
    NoLimit,
    /// Not known: a value the app does not read (`DEFAULT`, the server's own; an expression, an
    /// odd unit), on MySQL two limits that differ or one the app does not know, or a value that
    /// differs between a direct connection and one behind a pooler (a statement not sent there).
    Unknown,
}

/// A statement of a plan as the driver gets it.
pub use crate::driver::SessionStatement;

impl SessionPlan {
    /// The statements to send, in order, each with its form behind a pooler.
    pub fn statements(&self) -> Vec<SessionStatement> {
        self.items
            .iter()
            .filter_map(|i| match (&i.what, &i.pooled) {
                (Planned::Sql(sql), pooled) => Some(SessionStatement {
                    sql: sql.clone(),
                    per_transaction: match pooled {
                        Some(OnPooler::PerTransaction(l)) => Some(l.clone()),
                        _ => None,
                    },
                }),
                _ => None,
            })
            .collect()
    }

    /// The source of statement `index` (from 0) of [`SessionPlan::statements`], and its number
    /// there (from 1).
    pub fn statement_source(&self, index: usize) -> Option<(&Source, usize)> {
        let sql: Vec<&PlanItem> = self.items.iter().filter(|i| matches!(i.what, Planned::Sql(_))).collect();
        let source = &sql.get(index)?.source;
        Some((source, sql[..index].iter().filter(|i| i.source == *source).count() + 1))
    }
}

/// The app's own limits outside any environment (the config's top-level settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    pub lock_wait: Option<Duration>,
    pub idle_disconnect: Option<Duration>,
}

/// Why a profile's sessions cannot start with what it says: it does not connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    /// No preset has this name (the profile's `session`, or its environment's mapping).
    NotFound { preset: String, by: PickedBy },
    /// The preset is for another database than the profile's.
    WrongDbms { preset: String, by: PickedBy, dbms: Dbms, driver: String },
    /// Statement `index` (from 1) of `source` is not a start-up setting.
    Statement { source: Source, index: usize, statement: String, problem: SetProblem },
    /// Statement `index` (from 1) of `source` is one a read-only environment refuses.
    ReadOnly { source: Source, index: usize, statement: String, block: ReadOnlyBlock },
    /// The profile has session statements and a driver no preset is for (its name).
    Driver(String),
}

/// The plan of profile `c` with its rules `rules` (its environment's, or the default rules),
/// the config's `defaults` and the presets `presets` (see the module docs). Pure: no I/O.
pub fn plan(
    c: &ConnectionConfig,
    rules: &Policy,
    defaults: Defaults,
    presets: &[SessionPreset],
) -> Result<SessionPlan, SessionError> {
    let env = c.policy.as_deref().filter(|n| *n != crate::policy::DEFAULT);
    let rules_source = || match env {
        Some(n) => Source::Environment(n.to_string()),
        None => Source::DefaultRules,
    };
    let mut items = Vec::new();
    let mut intent =
        |i: Intent, source: Source| items.push(PlanItem { what: Planned::Intent(i), source, pooled: None });
    match rules.lock_wait {
        Some(d) => intent(Intent::LockWait(d), rules_source()),
        None => intent(Intent::LockWait(defaults.lock_wait), Source::Global),
    }
    match rules.idle_disconnect {
        Some(d) => intent(Intent::IdleDisconnect(d), rules_source()),
        None => intent(Intent::IdleDisconnect(defaults.idle_disconnect), Source::Global),
    }
    intent(Intent::TxIdleRollback(rules.tx_idle_rollback), rules_source());
    let Some(dbms) = Dbms::of_driver(&c.driver) else {
        if c.session != SessionRef::Inherit || !c.session_sql.is_empty() {
            return Err(SessionError::Driver(c.driver.clone()));
        }
        return Ok(SessionPlan { items, lock_wait: None, idle: None });
    };
    let mut waits = LockWaits::new(dbms, rules.lock_wait.unwrap_or(defaults.lock_wait));
    let mut idle = IdleLimits::new(dbms, rules.idle_disconnect.unwrap_or(defaults.idle_disconnect));
    let mapped = rules.session.get(dbms);
    let picked = match &c.session {
        SessionRef::Off => {
            if let Some(p) = mapped {
                items.push(PlanItem {
                    what: Planned::Skipped { preset: p.to_string() },
                    source: Source::Profile,
                    pooled: None,
                });
            }
            None
        }
        SessionRef::Named(n) => Some((n.as_str(), PickedBy::Profile)),
        SessionRef::Inherit => mapped.map(|p| (p, PickedBy::Environment(env.map(str::to_string)))),
    };
    if let Some((name, by)) = picked {
        let Some(p) = presets.iter().find(|p| p.name == name) else {
            return Err(SessionError::NotFound { preset: name.to_string(), by });
        };
        if p.dbms != dbms {
            return Err(SessionError::WrongDbms { preset: p.name.clone(), by, dbms: p.dbms, driver: c.driver.clone() });
        }
        let source = Source::Preset { name: p.name.clone(), by };
        for (i, s) in p.sql.iter().enumerate() {
            items.push(statement(dbms, rules, &source, i + 1, s, &mut waits, &mut idle)?);
        }
    }
    for (i, s) in c.session_sql.iter().enumerate() {
        items.push(statement(dbms, rules, &Source::Profile, i + 1, s, &mut waits, &mut idle)?);
    }
    Ok(SessionPlan { items, lock_wait: waits.overridden(), idle: idle.overridden() })
}

/// The server's idle limit of a session as a plan's statements set it, one after another.
struct IdleLimits {
    dbms: Dbms,
    /// On a direct connection, and behind a pooler in transaction mode.
    values: [Wait; 2],
    /// Where the last statement that set it comes from.
    source: Option<Source>,
}

impl IdleLimits {
    /// The limit the app keeps before the statements (`idle_disconnect`; `None`: never).
    fn new(dbms: Dbms, idle_disconnect: Option<Duration>) -> Self {
        let first = idle_disconnect.map_or(Wait::NoLimit, Wait::Limit);
        IdleLimits { dbms, values: [first; 2], source: None }
    }

    /// A statement of `source` that sets `settings`; `pooled` its way behind a pooler.
    fn set(&mut self, source: &Source, settings: &[(String, String)], pooled: Option<&OnPooler>) {
        for (name, value) in settings {
            if !matches!(
                (self.dbms, name.as_str()),
                (Dbms::Postgres, "idle_session_timeout") | (Dbms::MySql, "wait_timeout")
            ) {
                continue;
            }
            let wait = wait_of(self.dbms, value);
            self.values[0] = wait;
            if !matches!(pooled, Some(OnPooler::Refused(_))) {
                self.values[1] = wait;
            }
            self.source = Some(source.clone());
        }
    }

    fn overridden(self) -> Option<IdleOverride> {
        let [direct, pooled] = self.values;
        self.source.map(|source| IdleOverride { source, direct, pooled })
    }
}

/// The lock wait limits of a session as a plan's statements set them, one after another.
struct LockWaits {
    dbms: Dbms,
    /// PostgreSQL's `lock_timeout` (twice); MySQL's `lock_wait_timeout` and
    /// `innodb_lock_wait_timeout`: on a direct connection, and behind a pooler in transaction mode
    /// (where a PostgreSQL statement with no form for one transaction is not sent).
    values: [[Wait; 2]; 2],
    /// Where the last statement that set one comes from.
    source: Option<Source>,
}

impl LockWaits {
    /// The limits the app sets before the statements (`lock_wait`; `None`: the server's).
    fn new(dbms: Dbms, lock_wait: Option<Duration>) -> Self {
        let first = lock_wait.map_or(Wait::Unknown, Wait::Limit);
        LockWaits { dbms, values: [[first; 2]; 2], source: None }
    }

    /// A statement of `source` that sets `settings`; `pooled` its way behind a pooler (one that
    /// is not sent there sets nothing there).
    fn set(&mut self, source: &Source, settings: &[(String, String)], pooled: Option<&OnPooler>) {
        for (name, value) in settings {
            let at = match (self.dbms, name.as_str()) {
                (Dbms::Postgres, "lock_timeout") => 0..2,
                (Dbms::MySql, "lock_wait_timeout") => 0..1,
                (Dbms::MySql, "innodb_lock_wait_timeout") => 1..2,
                _ => continue,
            };
            let wait = wait_of(self.dbms, value);
            self.values[0][at.clone()].fill(wait);
            if !matches!(pooled, Some(OnPooler::Refused(_))) {
                self.values[1][at].fill(wait);
            }
            self.source = Some(source.clone());
        }
    }

    /// The override, if a statement set a limit: known when both limits are the same, and the
    /// same on a direct connection and behind a pooler (whether a connection is behind one is
    /// known only once it connects).
    fn overridden(self) -> Option<LockWaitOverride> {
        let [[a, b], [c, d]] = self.values;
        let wait = if a == b && a == c && c == d { a } else { Wait::Unknown };
        self.source.map(|source| LockWaitOverride { source, wait })
    }
}

/// The limit `value` (as written) sets on `dbms`, as far as the app reads it (a lock wait, or
/// the server's idle limit: they take the same values). PostgreSQL's `lock_timeout` and
/// `idle_session_timeout`: a whole number of milliseconds, or one in quotes with one of
/// the units `ms`, `s`, `min`, `h`, `d` (`0`: no limit). MySQL's: a whole number of seconds, from
/// 1 up to a year (the largest `lock_wait_timeout` and `wait_timeout` take). Other forms the server takes too
/// (`' 3s'`, `'1.5s'`, `'3us'`, an expression) are `Unknown` on purpose: reading them as the
/// server does is not worth getting a value wrong. For a lock wait, unknown only drops the time
/// from a message; for the idle limit the app's timer keeps the environment's (MySQL's
/// `wait_timeout` must be one the app reads: `SetProblem::IdleValue`).
fn wait_of(dbms: Dbms, value: &str) -> Wait {
    let digits = |t: &str| !t.is_empty() && t.len() <= 12 && t.bytes().all(|b| b.is_ascii_digit());
    match dbms {
        Dbms::Postgres => {
            let quoted = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).filter(|v| !v.contains('\''));
            let text = quoted.unwrap_or(value);
            let at = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
            let (number, unit) = text.split_at(at);
            let unit = if quoted.is_some() { unit.trim() } else { unit };
            let ms: u64 = match unit {
                "" | "ms" => 1,
                "s" => 1_000,
                "min" => 60_000,
                "h" => 3_600_000,
                "d" => 86_400_000,
                _ => return Wait::Unknown,
            };
            match number.parse::<u64>().ok().filter(|_| digits(number)).and_then(|n| n.checked_mul(ms)) {
                Some(0) => Wait::NoLimit,
                Some(n) if n <= i32::MAX as u64 => Wait::Limit(Duration::from_millis(n)),
                _ => Wait::Unknown,
            }
        }
        Dbms::MySql => match value.parse::<u64>() {
            Ok(n) if digits(value) && (1..=31_536_000).contains(&n) => Wait::Limit(Duration::from_secs(n)),
            _ => Wait::Unknown,
        },
    }
}

/// Statement `index` (from 1) of `source`, checked; what it sets of the lock wait goes in
/// `waits`, of the server's idle limit in `idle`.
fn statement(
    dbms: Dbms,
    rules: &Policy,
    source: &Source,
    index: usize,
    sql: &str,
    waits: &mut LockWaits,
    idle: &mut IdleLimits,
) -> Result<PlanItem, SessionError> {
    let set = Classifier::session_set(dbms.language(), sql).map_err(|problem| SessionError::Statement {
        source: source.clone(),
        index,
        statement: sql.to_string(),
        problem,
    })?;
    if rules.read_only
        && let Err(block) = set.risk.read_only()
    {
        return Err(SessionError::ReadOnly { source: source.clone(), index, statement: sql.to_string(), block });
    }
    let pooled = set.per_transaction.map(|p| match p {
        Ok(local) => OnPooler::PerTransaction(local),
        Err(why) => OnPooler::Refused(why),
    });
    waits.set(source, &set.settings, pooled.as_ref());
    idle.set(source, &set.settings, pooled.as_ref());
    Ok(PlanItem { what: Planned::Sql(sql.to_string()), source: source.clone(), pooled })
}

/// The statements of `text` as one field holds them: split where `dbms` ends a statement, each
/// trimmed, the empty ones left out.
pub fn split_statements(dbms: Dbms, text: &str) -> Vec<String> {
    crate::sql::split::split_in(text, dbms.language().dialect())
        .iter()
        .map(|s| s.body(text).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Statements of `dbms` as one field shows them: joined by `; `, each without its line comments
/// (`-- …`, MySQL's `# …`), which would hide the statements after it on the one line, and with
/// its line breaks as blanks (outside strings they are blanks to the server too; a string that
/// holds one keeps it).
pub fn join_statements(dbms: Dbms, sql: &[String]) -> String {
    use crate::sql::lexer::{Tok, lex_in};
    let one = |s: &String| {
        let mut out = String::new();
        for t in lex_in(s, dbms.language().dialect()) {
            match t.kind {
                Tok::LineComment | Tok::Whitespace => {
                    if !out.is_empty() && !out.ends_with(' ') {
                        out.push(' ');
                    }
                }
                _ => out.push_str(t.text(s)),
            }
        }
        out.trim_end().to_string()
    };
    sql.iter().map(one).collect::<Vec<_>>().join("; ")
}

#[cfg(test)]
mod tests;
