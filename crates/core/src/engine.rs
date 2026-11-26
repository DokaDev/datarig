//! Engine identity: the engines by name ([`EngineId`]), what a profile's driver name expects
//! before a server answered ([`Expected`], from [`expected`], the one reader of driver names),
//! what one connection found its server to be ([`ServerFacts`], each answer a [`Fact`] with a
//! [`Why`] when it is not known), what is fixed per engine ([`EngineSpec`], one row per
//! engine in the registry [`EngineId::spec`], with the rules its text and statements are read
//! and judged by, its statements stand to a transaction by and its sessions are given and open
//! by) and what the app knows of a tab's server ([`Rules`]).
//!
//! `id`, `why`, `facts` and `session` are plain vocabulary: they import nothing else of this
//! crate; `tx` imports only the risk of a statement.

mod facts;
mod id;
mod mariadb;
mod monitor;
mod mysql;
pub mod mysql_family;
mod postgres;
mod rules;
mod session;
mod spec;
mod tx;
mod why;

pub use facts::{EngineFacts, Fact, MyFacts, PgFacts, PingIdentity, Provider, ServerFacts, Support, Topology, Version};
pub use id::{EngineId, Expected};
pub use mysql_family::{MYSQL_FILTER_CALLS, MYSQL_REFUSED_CALLS};
pub use postgres::{PG_FILTER_CALLS, PG_REFUSED_CALLS};
pub use postgres::{PG_PLAN, TX_ROLLBACK_ANSWER_WAIT};
pub use rules::{Rules, UNBOUND_TAB_DIALECT, UNBOUND_TAB_PLAN};
pub use session::{OpenVerdict, READ_ONLY_NOT_PROVEN, ReadOnlyProof};
pub use spec::{
    ApplierName, CancelRules, CatalogueRules, ChannelWords, Charts, DdlOfIndex, DdlRules, DdlToConsole, EditRules,
    EngineSpec, ExportRules, FilterCalls, FilterShape, FilterSql, FilterSyntax, GroupParts, InputRules, LagBytes,
    LastMessage, Like, MonitorRules, MonitorTerms, NameMatch, NullSafe, PlanRules, ReceiverName, ReplicaWho,
    ReplicationTerms, ResultTables, RiskRules, RoleWords, Routes, SessionIdName, SessionRules, SortRules, TextRules,
    Transport, TxRules, ValueRules, VersionLine, WaitsAsked,
};
pub use tx::{CountInTx, DdlLock, TxAsk, TxNote, TxRefusal, TxStatement, TxVerdict};
pub use why::{Feature, Why};

use crate::driver::ExplainFormat;
use crate::profile::dsn::Scheme;
use crate::profile::session::Dbms;

/// The engines a MySQL or MariaDB profile may reach.
const MYSQL_FAMILY: &[EngineId] = &[EngineId::MySql, EngineId::MariaDb];

/// The URL form, port and TLS names the profile form keeps for a driver name no engine serves.
pub const LEGACY_FORM_SCHEME: Scheme = Scheme::Postgres;

impl EngineId {
    /// Every engine, in registry order.
    pub const ALL: &'static [EngineId] = &[EngineId::Postgres, EngineId::MySql, EngineId::MariaDb];

    /// The engine's row of the registry.
    pub fn spec(self) -> &'static EngineSpec {
        match self {
            EngineId::Postgres => &postgres::SPEC,
            EngineId::MySql => &mysql::SPEC,
            EngineId::MariaDb => &mariadb::SPEC,
        }
    }
}

/// What a profile's driver name expects: the engine whose key or alias it is (ASCII case
/// ignored), as its family when several engines may answer; [`Expected::Unknown`] with the name
/// as written when no engine serves it.
pub fn expected(driver: &str) -> Expected {
    let named = |spec: &EngineSpec| {
        spec.key.eq_ignore_ascii_case(driver) || spec.aliases.iter().any(|a| a.eq_ignore_ascii_case(driver))
    };
    match EngineId::ALL.iter().map(|id| id.spec()).find(|spec| named(spec)) {
        Some(spec) => match spec.family {
            [_] => Expected::One(spec.id),
            family => Expected::Family(family),
        },
        None => Expected::Unknown(driver.to_string()),
    }
}

/// The plan format the server of `f` gives, as the plan rules of the engine its greeting names
/// answer ([`PlanRules`]): a MySQL-family server's greeting decides, whichever engine its
/// version line names, as it decides its edits; an engine that gives no plan the app reads
/// answers why.
pub fn plan_of(f: &ServerFacts) -> Fact<ExplainFormat> {
    let engine = match &f.detail {
        EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => my.greeting,
        EngineFacts::Postgres(_) => EngineId::Postgres,
    };
    match &engine.spec().plan {
        Fact::Known(rules) => rules.format(f),
        Fact::No(why) => Fact::No(why.clone()),
        Fact::Unknown(why) => Fact::Unknown(why.clone()),
    }
}

impl Expected {
    /// The row that decides what a profile shows and applies before connect: the engine's, or
    /// the first of the family's (a family shares its form and presets).
    fn spec(&self) -> Option<&'static EngineSpec> {
        match self {
            Expected::One(id) => Some(id.spec()),
            Expected::Family(ids) => ids.first().map(|id| id.spec()),
            Expected::Unknown(_) => None,
        }
    }

    /// The URL form, default port and TLS names of the profile form.
    pub fn form_scheme(&self) -> Scheme {
        match self.spec().map(|s| s.transport) {
            Some(Transport::Network { scheme, .. }) => scheme,
            Some(Transport::File) | None => LEGACY_FORM_SCHEME,
        }
    }

    /// Whether the environment's read-only functions apply to the profile before a server
    /// answered ([`RiskRules::read_only_calls`] of the engine whose form and presets it
    /// shows); not on a name no engine serves.
    pub fn read_only_calls(&self) -> Support {
        self.spec().map_or(Fact::No(Why::NotOnEngine), |s| s.risk.read_only_calls())
    }

    /// The database whose session presets apply (`None`: no preset is for it).
    pub fn preset(&self) -> Option<Dbms> {
        self.spec().and_then(|s| Dbms::parse(s.preset_key))
    }

    /// The driver row of the profile form and its icon (`None`: no engine serves the name).
    pub fn form_key(&self) -> Option<&'static str> {
        self.spec().map(|s| s.form_key)
    }
}

#[cfg(test)]
mod tests;
