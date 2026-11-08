//! What a server offers the monitor ([`MonitorFormat`]), learnt from a probe on the monitor
//! connection: the version (which columns and statements exist), what the account may read,
//! and the flavour of the server.
//!
//! The flavour is read from the server (a provider's own roles, functions or settings), never
//! guessed from the host name: a managed service behind a custom name is still told, and a
//! self-managed server whose name looks like a provider's is not taken for one. Each provider is
//! a row of [`PG_MARKERS`] or [`MYSQL_MARKERS`], so a new one is data, not a special case.

/// The statements a monitor connection reads with, and what they may read, for one server
/// (`Capabilities::monitor`, refined once the monitor connection has run its probe).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorFormat {
    Postgres(PgMonitor),
    MySql(MySqlMonitor),
}

impl MonitorFormat {
    /// The provider the server belongs to.
    pub fn flavour(&self) -> Flavour {
        match self {
            MonitorFormat::Postgres(pg) => pg.flavour,
            MonitorFormat::MySql(my) => my.flavour,
        }
    }
}

/// The oldest PostgreSQL the monitor reads (`server_version_num`).
pub const PG_MIN_VERSION: u32 = 120000;

/// [`PG_MIN_VERSION`] as users read it.
pub const PG_OLDEST: &str = "PostgreSQL 12";

/// A PostgreSQL server, as its probe found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgMonitor {
    /// `server_version_num` (170011 for 17.11).
    pub version: u32,
    /// The account is a superuser.
    pub superuser: bool,
    /// The account has the privileges of `pg_read_all_stats` (also through `pg_monitor`, or as
    /// a superuser): it sees every session's state, statement and times.
    pub read_all_stats: bool,
    /// The account has the privileges of `pg_signal_backend`: it may cancel and end other
    /// roles' sessions (never a superuser's).
    pub signal_backend: bool,
    pub flavour: Flavour,
    /// How the monitor connection reaches the server, as the driver verified it.
    pub route: Route,
}

/// How a monitor connection reaches its server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Route {
    /// Straight to the server: its server session is the connection's own.
    Direct,
    /// Through a pooler: a server session may serve one client now and another next.
    Pooled,
    /// Not verified either way.
    #[default]
    Unknown,
}

impl PgMonitor {
    /// `pg_stat_activity.leader_pid` (13 on): parallel workers name their leader.
    pub fn leader_pid(&self) -> bool {
        self.version >= 130000
    }

    /// `pg_locks.waitstart` (14 on): when a lock wait started.
    pub fn waitstart(&self) -> bool {
        self.version >= 140000
    }
}

/// The oldest MySQL the monitor reads (8.0.17), as `major * 10000 + minor * 100 + patch`.
pub const MYSQL_MIN_VERSION: u32 = 80017;

/// [`MYSQL_MIN_VERSION`] as users read it.
pub const MYSQL_OLDEST: &str = "MySQL 8.0.17";

/// A MySQL server, as its probe found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MySqlMonitor {
    /// `major * 10000 + minor * 100 + patch` (80411 for 8.4.11).
    pub version: u32,
    /// `@@performance_schema` is on.
    pub performance_schema: bool,
    /// The account's user name (`CURRENT_USER()` before the `@`): without `PROCESS` it sees
    /// only its own sessions.
    pub user: String,
    /// The account holds `PROCESS` by a direct grant: it sees every session (`SUPER` does not
    /// show them). A grant through a role is not seen here, so "no" means "cannot tell", never
    /// "may not".
    pub process: Readable,
    /// `information_schema.INNODB_TRX` (needs `PROCESS`): whether an idle session holds an
    /// InnoDB transaction open, and since when.
    pub innodb_trx: Readable,
    /// `performance_schema.metadata_locks` with `threads` (needs `SELECT` on them, and the
    /// metadata lock instrument on): which sessions hold locks past their statements, in a
    /// transaction or not. Without it a session idle with no InnoDB transaction may still hold
    /// one (a transaction that read only MyISAM tables, `LOCK TABLES`): not known.
    pub metadata_locks: Readable,
    /// `performance_schema.events_statements_current` with `threads` (needs `SELECT` on them),
    /// its consumer on with those above it: which statement each instrumented thread runs, by
    /// event id. Without it a statement is told apart by its text and times only.
    pub statements: Readable,
    pub flavour: Flavour,
}

impl MySqlMonitor {
    /// `performance_schema.processlist` (8.0.22 on, with the performance schema on): the
    /// session list without the server's global thread list mutex.
    pub fn ps_processlist(&self) -> bool {
        self.version >= 80022 && self.performance_schema
    }
}

/// Whether the account may read something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readable {
    Yes,
    /// The server refused it for lack of a privilege.
    Refused,
    /// Not known: the probe failed for another reason, or found no direct grant (a role may
    /// hold it).
    Unknown,
}

/// Who runs the server, as far as the server itself tells. It decides which statements stop a
/// session (a provider may have procedures of its own for it) and which privilege a refusal
/// names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flavour {
    /// No provider's marker found: a self-managed server, or a provider not listed.
    Standard,
    /// Amazon RDS (PostgreSQL or MySQL).
    AmazonRds,
    /// Amazon Aurora (PostgreSQL or MySQL).
    AmazonAurora,
    /// Google Cloud SQL.
    GoogleCloudSql,
    /// Google AlloyDB.
    GoogleAlloyDb,
    /// Azure Database.
    AzureDatabase,
}

impl Flavour {
    /// The provider's product name (a proper name, the same in every language), `None` for
    /// [`Flavour::Standard`].
    pub fn product(self) -> Option<&'static str> {
        match self {
            Flavour::Standard => None,
            Flavour::AmazonRds => Some("Amazon RDS"),
            Flavour::AmazonAurora => Some("Amazon Aurora"),
            Flavour::GoogleCloudSql => Some("Cloud SQL"),
            Flavour::GoogleAlloyDb => Some("AlloyDB"),
            Flavour::AzureDatabase => Some("Azure Database"),
        }
    }
}

/// What on a server shows it belongs to a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// A role of this name exists (PostgreSQL `pg_roles`, readable by every account).
    Role(&'static str),
    /// A function of this name exists (PostgreSQL `to_regproc`).
    Function(&'static str),
    /// A global variable of this name exists (MySQL `SHOW GLOBAL VARIABLES`).
    Variable(&'static str),
    /// The server's `basedir` starts with this (MySQL).
    Basedir(&'static str),
}

/// PostgreSQL providers, the first match wins (Aurora and AlloyDB also have the roles of the
/// service they grew from).
pub const PG_MARKERS: &[(Marker, Flavour)] = &[
    (Marker::Function("aurora_version"), Flavour::AmazonAurora),
    (Marker::Role("alloydbsuperuser"), Flavour::GoogleAlloyDb),
    (Marker::Role("rds_superuser"), Flavour::AmazonRds),
    (Marker::Role("cloudsqlsuperuser"), Flavour::GoogleCloudSql),
    (Marker::Role("azure_pg_admin"), Flavour::AzureDatabase),
];

/// MySQL providers, the first match wins.
pub const MYSQL_MARKERS: &[(Marker, Flavour)] =
    &[(Marker::Variable("aurora_version"), Flavour::AmazonAurora), (Marker::Basedir("/rdsdbbin/"), Flavour::AmazonRds)];

/// What a probe found on a server, by marker: the facts [`flavour`] judges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// Names of roles, functions and variables found.
    pub names: Vec<String>,
    /// MySQL `@@basedir`.
    pub basedir: Option<String>,
}

/// The flavour a probe shows, by the first marker of `markers` it found.
pub fn flavour(markers: &[(Marker, Flavour)], probe: &Probe) -> Flavour {
    let has = |name: &str| probe.names.iter().any(|n| n == name);
    markers
        .iter()
        .find(|(marker, _)| match marker {
            Marker::Role(name) | Marker::Function(name) | Marker::Variable(name) => has(name),
            Marker::Basedir(prefix) => probe.basedir.as_deref().is_some_and(|b| b.starts_with(prefix)),
        })
        .map_or(Flavour::Standard, |(_, flavour)| *flavour)
}
