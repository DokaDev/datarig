# datarig

[![CI](https://github.com/DokaDev/datarig/actions/workflows/ci.yml/badge.svg)](https://github.com/DokaDev/datarig/actions/workflows/ci.yml)

A full-featured database client that lives in your terminal — light, fast, and safe by default.

datarig is written in Rust with [Ratatui](https://ratatui.rs). It keeps several connections
open side by side, each with its own tabs, a schema explorer, a SQL editor with vim keys and a
results grid that pages through large results explicitly instead of loading them whole. It is
careful by default: dangerous statements ask first, and read-only profiles are checked before
anything is sent and, on PostgreSQL, also enforced by the server.

**Documentation: <https://dokadev.github.io/datarig/>**

## Status

**Early, pre-1.0.** Releases have binaries for macOS, Linux and Windows and a Homebrew formula.
**PostgreSQL** is implemented (a table's structure, its DDL and the key marks need
PostgreSQL 12 or newer), and **MySQL** in part (see
[MySQL](https://dokadev.github.io/datarig/guide/mysql.html) for what it supports). The configuration format, key bindings and behaviour may still change between releases.

Platforms: developed and tested on macOS; CI runs the full test suite on Linux. On Windows CI
builds datarig, lints it and runs the tests that need no database, but running datarig there is
untested.

## Install

```sh
brew install dokadev/tap/datarig
```

Or download an archive for macOS, Linux or Windows from
[GitHub Releases](https://github.com/DokaDev/datarig/releases) (`SHA256SUMS` lists their
checksums), or build from source with Rust 1.93 and libclang:

```sh
git clone https://github.com/DokaDev/datarig
cd datarig
cargo install --locked --path crates/tui
```

Details: [Install](https://dokadev.github.io/datarig/install.html). A terminal of at least
80×24 is required; a Nerd Font is optional.

## Quick start

```sh
datarig                # open the workspace; the explorer offers "＋ New connection"
datarig my-profile     # connect to a profile and open a console
```

Paste a URL such as `postgres://me@127.0.0.1:5432/app` into the new-connection form.
`Ctrl+O` picks a connection, `Ctrl+E` runs the statement under the cursor, `Ctrl+C` cancels,
`:q` closes a tab and `:qa` quits. Press `Space` and wait to see what follows, or `F1` for the
keyboard help. Next: [Getting started](https://dokadev.github.io/datarig/getting-started.html).


## Development

| Path | What |
|---|---|
| `crates/core` | UI-free core: driver interface, SQL tooling and the safety classifier, profiles, secrets, config, i18n |
| `crates/drivers/postgres`, `crates/drivers/mysql` | the database drivers |
| `crates/ssh`, `crates/tls` | SSH tunnels, TLS (rustls) |
| `crates/tui` | the terminal UI and the `datarig` binary |
| `crates/bench` | benchmarks and the CI performance budgets |
| `locales/` | the UI text catalogs (`en.toml`, `ko.toml`) |
| `dev/` | Docker Compose with PostgreSQL, MySQL, PgBouncer and an SSH bastion for the tests; see [dev/README.md](https://github.com/DokaDev/datarig/blob/main/dev/README.md) |
| `vendor/` | patched tokio-postgres, mysql_async and ssh-key; see [vendor/README.md](https://github.com/DokaDev/datarig/blob/main/vendor/README.md) |
| `docs/` | [architecture](https://github.com/DokaDev/datarig/blob/main/docs/architecture.md), [performance](https://github.com/DokaDev/datarig/blob/main/docs/perf.md), the generated [key bindings](https://github.com/DokaDev/datarig/blob/main/docs/keybindings.md) and [theme tokens](https://github.com/DokaDev/datarig/blob/main/docs/themes.md), and the documentation site (`docs/site`) |

How to build, test and send a change:
[CONTRIBUTING.md](https://github.com/DokaDev/datarig/blob/main/CONTRIBUTING.md); the test
services: [dev/README.md](https://github.com/DokaDev/datarig/blob/main/dev/README.md).

## License

datarig is dual-licensed under either of

- the Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/DokaDev/datarig/blob/main/LICENSE-APACHE))
- the MIT license ([LICENSE-MIT](https://github.com/DokaDev/datarig/blob/main/LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in datarig by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.

Third-party code: `vendor/tokio-postgres`, `vendor/mysql_async` and `vendor/ssh-key` keep their
own MIT/Apache-2.0 licenses, and the C code built in through `pg_query` (libpg_query,
PostgreSQL, protobuf-c, xxHash) has its notices in `vendor/licenses/pg_query/`.
`scripts/third-party-notices.sh` (cargo-about) writes `THIRD-PARTY-NOTICES.html` with the
licenses of every crate in the binary; CI builds it as an artifact. `cargo deny check` keeps the
dependencies to permissive licenses.
