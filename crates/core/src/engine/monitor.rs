//! Which engine a monitor's server is: the format its probe gave, by engine ([`MonitorFormat`]),
//! and the engine's monitor rules for it.

use super::{EngineId, Fact, MonitorRules, Why};
use crate::monitor::{Flavour, MonitorFormat, PgMonitor};

impl From<PgMonitor> for MonitorFormat {
    /// The format a PostgreSQL probe gave.
    fn from(pg: PgMonitor) -> Self {
        MonitorFormat::Postgres(pg)
    }
}

impl MonitorFormat {
    /// The engine whose probe gave the format.
    pub fn engine(&self) -> EngineId {
        match self {
            MonitorFormat::Postgres(_) => EngineId::Postgres,
            MonitorFormat::MySql(_) => EngineId::MySql,
        }
    }

    /// The provider the server belongs to.
    pub fn flavour(&self) -> Flavour {
        match self {
            MonitorFormat::Postgres(pg) => pg.flavour,
            MonitorFormat::MySql(my) => my.flavour,
        }
    }

    /// The monitor rules of the format's engine, or why it has none. Unchecked: an action asks
    /// them through [`crate::monitor::action::rules_for`], which also checks they read the
    /// format.
    pub fn rules(&self) -> Result<&'static dyn MonitorRules, Why> {
        match &self.engine().spec().monitor {
            Fact::Known(rules) => Ok(*rules),
            Fact::No(why) | Fact::Unknown(why) => Err(why.clone()),
        }
    }
}
