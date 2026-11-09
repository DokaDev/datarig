//! SQLite driver for datarig (rusqlite, with SQLite compiled in).
//!
//! * A profile names one database file (`path`, absolute). Its sessions open that file and
//!   nothing else: a missing file is an error, never created, and `ATTACH`/`DETACH` are refused
//!   by the engine (the authorizer), so a statement cannot reach another file.
//! * A read-only profile opens the file read-only (`SQLITE_OPEN_READ_ONLY`) and sets
//!   `query_only`, so the engine refuses every write whatever the text says. A read-write
//!   profile opens it read-write.
//! * SQLite has no server: each session is one connection, served by a thread of its own
//!   (rusqlite is synchronous). Cancelling a statement interrupts it (`sqlite3_interrupt`); a
//!   session whose handle was dropped interrupts its statement too.
//! * Results are read in one piece (no server-side cursor): a result is one page.

mod connect;
mod meta;
mod open;
mod query;
mod session;
mod values;

pub use connect::SqliteDriver;
