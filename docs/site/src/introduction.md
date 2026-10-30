# Introduction

A full-featured database client that lives in your terminal — light, fast, and safe by default.

datarig is written in Rust with [Ratatui](https://ratatui.rs). It keeps several connections
open side by side, each with its own tabs, a schema explorer, a SQL editor with vim keys, and a
results grid that pages through large results explicitly instead of loading them whole.

It is careful by default: statements that may do harm ask before they run, read-only profiles
are checked before anything is sent and, on PostgreSQL, also enforced by the server, and
paging a result leaves no transaction, lock or snapshot open on the server unless you ask for
it.

## Status

datarig is early and pre-1.0. The configuration format, key bindings and behaviour may still
change between releases.

- **PostgreSQL** is implemented. CI tests it against PostgreSQL 17. Not supported before
  PostgreSQL 12: a table's structure, its DDL, the key marks and copying rows as `INSERT`
  (datarig says the server is too old).
- **MySQL** is implemented in part; [MySQL](guide/mysql.md) says what it supports and what not.
  MariaDB is a best effort.

**Platforms.** datarig is developed and tested on macOS. CI runs the full test suite on Linux.
On Windows, CI builds datarig, lints it and runs the tests that need no database, but running
datarig on Windows is untested: wherever this manual mentions Windows, it describes how
datarig is meant to behave there, not something anyone has tried.

## Performance

Performance is held by budgets that CI enforces
([crates/bench/budgets.toml](https://github.com/DokaDev/datarig/blob/main/crates/bench/budgets.toml),
explained in [docs/perf.md](https://github.com/DokaDev/datarig/blob/main/docs/perf.md)):

- One round trip for the first page of a prepared `SELECT`, one for each later page and one for
  an `INSERT`, also through an SSH tunnel.
- Paging through all 4,000,000 rows of the test table stays under 96 MiB of resident memory,
  growing less than 16 MiB after the in-memory window is full.
- Keystroke to frame under 25 ms (p95) in a 5 MB SQL file, in a plan of hundreds of nodes and in
  a chart of 100,000 fetched rows.
- Idle: under 48 MiB and 1% CPU, and fewer than 0.2 wakeups a second when nothing is waiting.
- First frame under 250 ms (p95); the release binary is at most 24 MiB.


## Where to go next

- [Install](install.md) datarig, then follow [Getting started](getting-started.md).
- The [user guide](guide/connections.md) explains each part of the workspace, task by task.
- The reference pages list every [key](reference/keybindings.md), [`:`
  command](reference/commands.md), [configuration key](reference/configuration.md) and
  [command-line option](reference/command-line.md). They are generated from the code, and the
  site is published from each release.

Source code, issues and releases are on GitHub:
<https://github.com/DokaDev/datarig>.
