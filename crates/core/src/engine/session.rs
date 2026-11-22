//! How a session of a read-only profile stands to read-only, as data: the driver says what its
//! connection proved ([`ReadOnlyProof`]), the engine's [`SessionRules`](super::SessionRules)
//! answer whether the session opens ([`OpenVerdict`]), and the driver acts on the answer.

/// What a session's connection proved of read-only before the session opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnlyProof {
    /// The server answered that the session is read-only.
    Confirmed,
    /// The session's default is not read-only, and every transaction it opens is.
    PerTransaction,
    /// The server did not make the session read-only: what it reported and answered.
    Unconfirmed(String),
    /// Nothing was asked: every read of the session is read-only by itself.
    NotAsked,
}

/// Whether a session of a read-only profile opens, by what its connection proved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenVerdict {
    /// It opens.
    Open,
    /// It opens, and the user is told that every transaction is made read-only instead of the
    /// session.
    OpenPerTransaction,
    /// It opens, and the user is told what the server answered.
    OpenUnconfirmed(String),
    /// It does not open: why.
    Refused(String),
}

/// Why a session does not open when its connection proved nothing its role may open on.
pub const READ_ONLY_NOT_PROVEN: &str = "the session's read-only setting was not confirmed";
