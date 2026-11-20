//! The engines by name, and what a profile names before any server answered.

/// A database engine datarig serves. On a connection it is decided from the server's own
/// reports, never from the profile alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EngineId {
    Postgres,
    MySql,
    MariaDb,
}

/// The engines a profile's driver name allows before any server answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expected {
    /// Only this engine serves the name.
    One(EngineId),
    /// Any engine of this family may answer (a MySQL profile may reach MariaDB, and the
    /// reverse): the server decides.
    Family(&'static [EngineId]),
    /// No engine serves the name, kept as written.
    Unknown(String),
}
