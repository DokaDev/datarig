//! What a server is, as one connection found it: a fact is known, or refused or unknown with
//! a reason.

use super::{EngineId, Why};

/// An answer about a server. `No` and `Unknown` both refuse: a caller that wants a value
/// matches all three arms (there is no helper that turns `Unknown` into a value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fact<T> {
    Known(T),
    /// The server does not have it, or datarig does not do it there.
    No(Why),
    /// Not known on this connection.
    Unknown(Why),
}

/// Whether something is supported: a fact with no value.
pub type Support = Fact<()>;

/// A server version. Compared part by part in field order: major, then minor, then patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self { major, minor, patch }
    }

    /// The version a server's report starts with: `major.minor[.patch]`, each part one or more
    /// ASCII digits (no sign), then the end of the text or one of ` -+_(`. A part over
    /// `u16::MAX` or any other text is `None`.
    ///
    /// `17.2 (Debian 17.2-1)` is 17.2.0, `11.4.13-MariaDB-ubu2404` 11.4.13, `8.0.36-28` 8.0.36
    /// and `5.5.5-10.6.12-MariaDB` 5.5.5; `18beta1`, `17devel` and the empty text are `None`.
    pub fn parse(text: &str) -> Option<Version> {
        let bytes = text.as_bytes();
        let mut at = 0;
        let part = |at: &mut usize| -> Option<u16> {
            let start = *at;
            while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
                *at += 1;
            }
            if *at == start {
                return None;
            }
            text[start..*at].parse().ok()
        };
        let major = part(&mut at)?;
        if bytes.get(at) != Some(&b'.') {
            return None;
        }
        at += 1;
        let minor = part(&mut at)?;
        let patch = match bytes.get(at) {
            Some(b'.') => {
                at += 1;
                part(&mut at)?
            }
            _ => 0,
        };
        match bytes.get(at) {
            None | Some(b' ' | b'-' | b'+' | b'_' | b'(') => Some(Version::new(major, minor, patch)),
            Some(_) => None,
        }
    }
}

/// Whether a session reaches its server directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    /// Pinned to one server session.
    Direct,
    /// A proxy or pooler in between, not pinned to one server session.
    Pooled,
    /// The engine runs inside datarig's process.
    InProcess,
}

/// Who runs the server, as far as the server itself tells. It decides which statements stop a
/// session (a provider may have procedures of its own for it) and which privilege a refusal
/// names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
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

impl Provider {
    /// The provider's product name (a proper name, the same in every language), `None` for
    /// [`Provider::Standard`].
    pub fn product(self) -> Option<&'static str> {
        match self {
            Provider::Standard => None,
            Provider::AmazonRds => Some("Amazon RDS"),
            Provider::AmazonAurora => Some("Amazon Aurora"),
            Provider::GoogleCloudSql => Some("Cloud SQL"),
            Provider::GoogleAlloyDb => Some("AlloyDB"),
            Provider::AzureDatabase => Some("Azure Database"),
        }
    }
}

/// What one connection found its server to be. Fixed for the life of the connection: a
/// reconnect is a new connection with new facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFacts {
    /// The version the server reported, parsed; `Unknown(VersionUnknown)` when it does not
    /// parse.
    pub version: Fact<Version>,
    /// The server's report of its version as sent, when the connection read one.
    pub version_text: Option<String>,
    /// Who runs the server.
    pub provider: Fact<Provider>,
    /// Whether the session reaches the server directly, by the driver's own check at connect;
    /// `Unknown(NotChecked)` when the check did not run or failed.
    pub topology: Fact<Topology>,
    /// The engine and what only that engine has.
    pub detail: EngineFacts,
}

impl ServerFacts {
    /// The engine the server is: the variant of [`ServerFacts::detail`].
    pub fn engine(&self) -> EngineId {
        match &self.detail {
            EngineFacts::Postgres(_) => EngineId::Postgres,
            EngineFacts::MySql(_) => EngineId::MySql,
            EngineFacts::MariaDb(_) => EngineId::MariaDb,
        }
    }

    /// Whether the server's greeting names another engine than its `VERSION()` line (the
    /// engine of the facts): a proxy wrote the greeting, and either engine may be the one that
    /// runs the session's text.
    pub fn reports_disagree(&self) -> bool {
        match &self.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => my.greeting != self.engine(),
            EngineFacts::Postgres(_) => false,
        }
    }

    /// On which engines' rules the session's text is judged: both of the family when its
    /// reports disagree ([`ServerFacts::reports_disagree`]), else its own.
    pub fn judged(&self) -> crate::sql::risk::mysql::Judged {
        use crate::sql::risk::mysql::Judged;
        if self.reports_disagree() { Judged::WithSibling } else { Judged::Own }
    }
}

/// The engine of a server with its server-level facts. MySQL and MariaDB share a struct but
/// not a variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineFacts {
    Postgres(PgFacts),
    MySql(MyFacts),
    MariaDb(MyFacts),
}

/// What only a PostgreSQL server has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PgFacts {}

/// What only a MySQL or MariaDB server has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyFacts {
    /// The version number of the server's greeting.
    pub handshake: Version,
    /// The engine the server's greeting names, which may differ from the one its `VERSION()`
    /// line names (the engine of the facts) when a proxy wrote the greeting.
    pub greeting: EngineId,
    /// Whether the server's `performance_schema` is on (`No(FeatureOff(..))` when it is off,
    /// `Unknown` when it was not read).
    pub performance_schema: Support,
    /// Whether the server listed the session's marker on the session's own connection
    /// (`Known`), so a connection of its own can name the session to stop its statement;
    /// `No(MarkerUnconfirmed)` when it did not, `Unknown` when it was not read.
    pub marker: Support,
}

impl MyFacts {
    /// The facts of a greeting of `greeting` at `handshake`, `performance_schema` and the marker
    /// not read.
    pub fn new(handshake: Version, greeting: EngineId) -> MyFacts {
        MyFacts {
            handshake,
            greeting,
            performance_schema: Fact::Unknown(Why::NotChecked),
            marker: Fact::Unknown(Why::NotChecked),
        }
    }
}

/// What a test connection learns of the server: no more than its greeting and version tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingIdentity {
    pub engine: Fact<EngineId>,
    pub version: Fact<Version>,
}
