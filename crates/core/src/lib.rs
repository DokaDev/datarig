//! UI-independent core of datarig: everything a front end and a database driver share.
//!
//! * [`driver`] — the driver abstraction (base interface + capability flags, session channels).
//! * [`sql`] — tolerant lexer, statement splitter and completion engine.
//! * [`profile`] — connection profile model and `postgres://` DSN parsing.
//! * [`secret`] — password sources (OS keychain, secrets file, command, env, prompt), the
//!   in-memory store, the session cache and the deletion log.
//! * [`config`] — config file loading and format-preserving saving.
//! * [`policy`] — environments (`[policy.<name>]`): the safety rules, color and badge profiles name.
//! * [`migrate`] — the launch-time config v2 / keychain migration.
//! * [`paths`] — the data and state directories; [`fsutil`] — atomic file writes.
//! * [`scripts`] — saved queries and their index; [`workspace`] — tab restore and the instance
//!   lock.
//! * [`usage`] — how often actions ran by key, menu and command line (`usage.toml`), for tips.
//! * [`shown`] — the notices shown once (`shown.toml`), as opaque ids.
//! * [`update`]: the update check's facts (`update.toml`) and decisions, and the release notes
//!   of What's new.
//! * [`results`] — the rows of a result: a window in memory, the rest in a spill file.
//! * [`chart`] — a result's rows as the numbers of a chart (columns, axes, sums).
//! * [`export`] — result rows as TSV, CSV, JSON, a Markdown table or SQL INSERT statements.
//! * [`edit`] — editing a result's rows: the table they are, which columns change, how a row is found.
//! * [`query`]: a table's rows as a query (filter, order, cap) compiled to SQL, the filter text
//!   checked before it runs.
//! * [`monitor`]: server monitoring, one model for every database: a tick's rows read into it
//!   (the statements are each driver's), rates on the server's clock.
//! * [`transport`] — how a driver reaches a server through a tunnel (a [`transport::Dialer`]).
//! * [`theme`] — user theme files (`themes/<name>.toml`), read and checked.
//! * [`fault`] — failures as data (a kind the UI words, a detail for the error log).
//!
//! This crate must never depend on a UI crate (ratatui/crossterm), on a concrete driver or on
//! the UI text (`datarig-i18n`): it reports what happened as data and the UI words it.

pub mod chart;
pub mod config;
pub mod driver;
pub mod edit;
pub mod export;
pub mod fault;
pub mod fsutil;
pub mod migrate;
pub mod monitor;
pub mod panics;
pub mod paths;
pub mod policy;
pub mod profile;
pub mod query;
pub mod results;
pub mod scripts;
pub mod secret;
pub mod shown;
pub mod sql;
pub mod theme;
pub mod tls;
pub mod transport;
pub mod update;
pub mod usage;
pub mod workspace;
