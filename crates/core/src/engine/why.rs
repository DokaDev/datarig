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
}
