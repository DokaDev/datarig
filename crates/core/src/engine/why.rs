//! Why a fact about a server is not known, as data: the UI words each reason.

/// The reason a [`Fact`](super::Fact) is `No` or `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The connection has not checked it (yet, or the check failed).
    NotChecked,
    /// The server's report of its version did not parse as a version.
    VersionUnknown,
    /// The server's own reports of its version (a MySQL-family greeting and `VERSION()`) name
    /// different versions of the same engine: a proxy in between may have written one of them.
    VersionsDisagree,
    /// A MySQL-family greeting and the server's `VERSION()` name different engines (MySQL and
    /// MariaDB): a proxy in between may have written the greeting itself, so neither the version
    /// nor how the session reaches the server is known.
    EnginesDisagree,
    /// The engine has no such thing, on any version.
    NotOnEngine,
    /// The server has it switched off.
    FeatureOff(Feature),
}

/// A part of a server that may be switched off there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// MySQL's and MariaDB's `performance_schema`, where the sessions' connection attributes are
    /// listed (off by default on MariaDB).
    PerformanceSchema,
}

impl Feature {
    /// Its name as the server writes it.
    pub fn name(self) -> &'static str {
        match self {
            Feature::PerformanceSchema => "performance_schema",
        }
    }
}
