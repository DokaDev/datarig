//! UI-independent core of datarig: everything a front end and a database driver share.
//!
//! * [`driver`] — the driver abstraction (base interface + capability flags, session channels).
//! * [`sql`] — tolerant lexer, statement splitter and completion engine.
//! * [`profile`] — connection profile model and `postgres://` DSN parsing.
//! * [`secret`] — password sources (OS keychain, secrets file, command, env, prompt), the
//!   in-memory store, the session cache and the deletion log.
//! * [`config`] — config file loading and format-preserving saving.
//! * [`policy`] — safety policies (`[policy.<name>]`) that profiles name.
//! * [`migrate`] — the launch-time config v2 / keychain migration.
//! * [`paths`] — the data and state directories; [`fsutil`] — atomic file writes.
//! * [`scripts`] — saved queries and their index; [`workspace`] — tab restore and the instance
//!   lock.
//! * [`results`] — the rows of a result: a window in memory, the rest in a spill file.
//! * [`chart`] — a result's rows as the numbers of a chart (columns, axes, sums).
//! * [`export`] — result rows as TSV, CSV, JSON, a Markdown table or SQL INSERT statements.
//! * [`transport`] — how a driver reaches a server through a tunnel (a [`transport::Dialer`]).
//! * [`theme`] — user theme files (`themes/<name>.toml`), read and checked.
//! * [`fault`] — failures as data (a kind the UI words, a detail for the error log).
//! * [`i18n`] — the embedded en/ko message catalogs.
//!
//! This crate must never depend on a UI crate (ratatui/crossterm) or on a concrete driver.

pub mod chart;
pub mod config;
pub mod driver;
pub mod export;
pub mod fault;
pub mod fsutil;
pub mod i18n;
pub mod migrate;
pub mod panics;
pub mod paths;
pub mod policy;
pub mod profile;
pub mod results;
pub mod scripts;
pub mod secret;
pub mod sql;
pub mod theme;
pub mod transport;
pub mod workspace;
