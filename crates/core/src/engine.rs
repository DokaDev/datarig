//! Engine identity: the engines by name ([`EngineId`]), what a profile's driver name expects
//! before a server answered ([`Expected`], from [`expected`], the one reader of driver names),
//! what one connection found its server to be ([`ServerFacts`], each answer a [`Fact`] with a
//! [`Why`] when it is not known) and what is fixed per engine ([`EngineSpec`], one row per
//! engine in the registry [`EngineId::spec`]).
//!
//! `id`, `why` and `facts` are plain vocabulary: they import nothing else of this crate.

mod facts;
mod id;
mod mariadb;
mod mysql;
mod postgres;
mod spec;
mod why;

pub use facts::{EngineFacts, Fact, MyFacts, PgFacts, PingIdentity, Provider, ServerFacts, Support, Topology, Version};
pub use id::{EngineId, Expected};
pub use spec::{EngineSpec, Transport};
pub use why::Why;

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
