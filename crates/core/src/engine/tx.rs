//! What a statement means for the transaction open on a session, as data: the engine's
//! [`TxRules`](super::TxRules) answer each statement of a run with a [`TxVerdict`], and the app
//! words it.

use crate::sql::risk::{Risk, TxStep};

/// One statement of a run, as the transaction checks before the run read it.
pub struct TxStatement<'a> {
    pub sql: &'a str,
    /// Its risk, as the tab's classifier read it (with what the run prepares before it).
    pub risk: &'a Risk,
    /// Where it runs on a session that commits by hand while no transaction is open.
    pub step: TxStep,
    /// A transaction is open on the session when it is reached.
    pub open: bool,
    /// The session commits by hand.
    pub manual: bool,
}

/// What a statement does to the transaction open on its session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxVerdict {
    /// It does not run, nor does the rest of the run: why.
    pub refused: Option<TxRefusal>,
    /// It runs only once the user said yes: why it asks.
    pub ask: Option<TxAsk>,
    /// What the run's Messages say of it once the run is sent, in order.
    pub notes: Vec<TxNote>,
    /// The transaction open before it ends when it runs (it commits that transaction first).
    pub ends_tx: bool,
}

/// Why a statement is refused before its run is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxRefusal {
    /// It cannot run in a transaction block, and one is open.
    NoTxBlock,
    /// `LOCK TABLES` with manual commit: the next write would start a transaction, which
    /// releases the locks.
    LockTables,
}

/// Why a statement asks before its run is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxAsk {
    /// It cannot run in a transaction block: with manual commit it runs on its own and cannot
    /// be rolled back.
    OutsideTx,
    /// It commits the open transaction before it runs: what the transaction did so far.
    ImplicitCommit,
}

/// What the run's Messages say of a statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxNote {
    /// DDL with manual commit and no transaction open: it commits by itself and cannot be
    /// rolled back.
    MysqlDdl,
    /// A value added to an enum in a transaction: it cannot be used before that transaction
    /// commits.
    EnumValue,
    /// DDL in a transaction holds this lock until the transaction ends.
    DdlLock(DdlLock),
}

/// What a DDL statement holds until its transaction ends, by its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdlLock {
    /// `ACCESS EXCLUSIVE` on the object named: other sessions' reads of it wait.
    Exclusive(String),
    /// `DROP INDEX`: `ACCESS EXCLUSIVE` on the index's table.
    DropIndex(String),
    /// `CREATE INDEX`: `SHARE` on the table: other sessions' writes to it wait.
    Share(String),
    /// Some lock on the object named, which mode this text does not tell for sure.
    Kept(String),
}

/// Whether the server may take a time limit for a count of rows while a transaction is open on
/// the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CountInTx {
    /// Always: the limit is for the statement alone.
    Limited,
    /// Not while a transaction is open: inside it the count keeps to the transaction's own
    /// settings.
    LeftOut,
}
