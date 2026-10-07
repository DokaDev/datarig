# Vendored crates

## tokio-postgres 0.7.18

| | |
|---|---|
| Upstream | [`tokio-postgres`](https://crates.io/crates/tokio-postgres) 0.7.18 from crates.io, part of [sfackler/rust-postgres](https://github.com/sfackler/rust-postgres) |
| License | MIT OR Apache-2.0 (`tokio-postgres/LICENSE-MIT`, `tokio-postgres/LICENSE-APACHE`) |
| Package checksum | sha256 `a528f7d280f6d5b9cd149635c8705b0dd049754bc67d81d31fa25169a93809d3` (the crates.io index `cksum` of the `.crate` file) |
| Upstream commit | `f1cb6ec0d5766b136cbd68f3010d64142a5daa66`, path `tokio-postgres` (`tokio-postgres/.cargo_vcs_info.json`, as published) |
| Used through | `[patch.crates-io]` in `Cargo.toml`; the workspace `exclude`s `vendor`, so its lints and tests do not run here |

### Why it is vendored

tokio-postgres always binds a portal with one result format code, `1` (binary), for every
column, and does not let a caller choose. datarig's query session needs PostgreSQL's own text
output for the types it does not decode from binary (ranges, multiranges, geometry, bit strings,
text search, `money`, `"char"`, `reg*`, enums, composites, extension and unknown types), so that
the grid, every copy format and a copied `INSERT` show and round-trip those values exactly. The
driver picks a format per column (`datarig-driver-postgres/src/query.rs`, `format_code`) and
binds with it.

Upstream has no API for this in 0.7.18: the format is hard-coded (`Some(1)` in
`query::encode_bind_raw`), `InnerClient` is `pub(crate)`, and the simple-query path returns
names only, without types. The upstream pull request that adds text result formats,
[sfackler/rust-postgres#961](https://github.com/sfackler/rust-postgres/pull/961) ("Add text
format result support", open since 2023-11), has not been merged; see also
[issue #882](https://github.com/sfackler/rust-postgres/issues/882).

### What is changed

Every change in the source is marked with a `datarig:` comment. Two groups:

**Result formats**:

- `src/transaction.rs`: `Transaction::bind_with_formats(statement, params, result_formats)`, a
  new public method: like `bind`, with the result format code of each column (`0` text, `1`
  binary; a single code applies to every column).
- `src/bind.rs`: `bind_with_formats`, the new function behind it (a copy of `bind` that calls
  `encode_bind_with_formats`).
- `src/query.rs`:
  - `encode_bind_with_formats`, a new function: `encode_bind` with `result_formats`;
  - `encode_bind_raw` takes `result_formats: &[i16]` and passes it to the Bind message instead
    of the hard-coded `Some(1)`;
  - its three existing callers (`query_typed`, `execute_typed`, `encode_bind`) pass `&[1]`,
    so every existing API still asks for binary exactly as upstream does.

**Pipelined requests**: upstream
sends each step of the extended protocol (Parse, Bind, Execute) as a request of its own, ended
by `Sync`, and waits for its answer before the next one, so a portal's first page costs a round
trip per step. These additions put the steps of one request into one write with one `Sync`:

- `src/pipeline.rs` (new, all of it datarig's):
  - `encode_first_page` / `read_first_page`: `[BEGIN] + Bind + Describe (portal) +
    Execute(max_rows) + Sync`, answered with the first rows, whether the portal ran to its end,
    and the portal's current description (`FirstPage`). The `BEGIN` is Parse/Bind/Execute of
    the unnamed statement (a simple `Query` would end with a `ReadyForQuery` of its own, and the
    connection maps one `ReadyForQuery` to one request); run after the extended protocol's
    implicit transaction started, it turns that transaction into a block, so the portal bound
    after it outlives the `Sync`.
  - `execute_pipelined`: `[before] + Parse + Describe + Bind + Execute(max_rows) + [after] +
    Sync` of the unnamed statement (every column in text), for a statement expected to return
    no rows (`Executed`: rows affected, and the rows and description if it returned some after
    all). `before` and `after` are statements without rows sent as Parse/Bind/Execute in the
    same write (a read-only session's `BEGIN READ ONLY` and `COMMIT`).
  - The pipelined `BEGIN` of `encode_first_page` may be any statements without rows, sent in
    order (`BEGIN READ ONLY`, then `SET LOCAL search_path …`; or `SET TRANSACTION READ ONLY`
    first in the caller's block), and `FirstPage` carries the rows affected from the command
    tag. `execute_pipelined`'s `before` is likewise a list.
  - `prepare_wrapped`: `[before] + Parse + Describe (statement) + [after] +
    Sync` of a named statement, for a statement prepared in a transaction of the caller's
    (`BEGIN`, `SET LOCAL search_path …`, then `COMMIT`: the server resolves names when it
    parses, so a session whose search path is set per transaction behind a pooler parses with
    it). It answers a `Statement` like `Client::prepare`.
  - `encode_first_page_then`: `encode_first_page` with a statement sent after the Execute
    (`COMMIT` or `ROLLBACK`, as Parse/Bind/Execute of the unnamed statement), so the
    transaction, and the portal with it, ends in the round trip that read the first rows;
    `read_first_page` skips its three answers. `send_first_page` and `first_page_of` are its two
    halves (the response, then the page with its types looked up), so `bind_first_page_then`
    marks the transaction done as soon as the response says `COMMIT` succeeded. The portal of
    such a page is made with `Portal::ended` (`src/portal.rs`): it ended with its transaction,
    so dropping it sends no `Close` (which would be a request, and a round trip, of its own).
  - Both read the whole response up to its `ReadyForQuery` before looking up any type: a
    lookup is a request of its own, answered after this one.
- `src/transaction.rs`: a `begin` flag (`BEGIN` not sent yet) and the statements sent as that
  `BEGIN` (`begin_sql`, a list); `new_datarig`;
  `Transaction::bind_first_page` (sends the pending `BEGIN` with the first page) and
  `bind_first_page_then` (also ends the transaction in the same write; marks it done when that
  succeeded); `commit`,
  `rollback` and `Drop` send nothing while `BEGIN` has not been sent or for a handle on the
  caller's own block (`done`); `_savepoint` sets the new field.
- `src/client.rs`: `Client::transaction_pipelined` (a transaction whose `BEGIN` goes out with
  its first page) and `transaction_pipelined_with` (other statements as its `BEGIN`, e.g.
  `BEGIN READ ONLY` and `SET LOCAL …`), `Client::transaction_in_block` (a handle on the
  caller's open block that never commits or rolls it back) and `transaction_in_block_with`
  (with statements to send first in the block), `Client::execute_pipelined` and
  `execute_pipelined_wrapped` (with statements before and after it), `Client::prepare_wrapped`.
- `src/prepare.rs`: `next_name`, the next statement name, shared with
  `pipeline::prepare_wrapped` (`s<prefix>_<n>` with a random prefix per process,
  see below).

**Statements that live in one transaction** (a pooler in transaction mode
without prepared statement support, where the next transaction may run on another server
connection):

- `src/pipeline.rs`: `prepare_wrapped` takes `named`; `false` prepares the unnamed statement
  (`Statement::unnamed`, no `Close`, nothing left on the server).
- `src/client.rs`: `Client::prepare_unnamed_wrapped` (the unnamed statement after `before`,
  e.g. `BEGIN`), `Client::transaction_opened` (a transaction whose `BEGIN` the caller already
  sent: `new_datarig` with no `BEGIN` statements, ended as usual), and
  `Client::forget_typeinfo_statements` / `InnerClient::forget_typeinfo_statements` (drop the
  cached statements of type lookups so none outlives the caller's transaction; the looked-up
  types stay cached).
- `src/prepare.rs`: statement names are `s<prefix>_<n>`, the prefix drawn once per process
  (upstream: `s<n>`), so statements other clients left on a pooled server connection do not
  collide with this process's (`42P05`, prepared statement already exists).
- `src/lib.rs`: `mod pipeline` and `pub use pipeline::{Executed, FirstPage}`.

**Warnings on Windows**: `src/client.rs` imports `PathBuf` only on Unix, where
`Addr::Unix` uses it (upstream imports it on every platform, an unused import on Windows).

Every upstream API behaves as before; only callers of the new functions pipeline.

**Upstream shape.** The additions are shaped so they could be offered upstream as they are:
`bind_first_page` is `bind` + `query_portal` in one round trip on an existing `Transaction`,
and a deferred `BEGIN` is a property of the transaction (`transaction_pipelined`), not of the
call. A fuller upstream design would let every `Transaction` method send the pending `BEGIN`
with its first request (here only `bind_first_page` does, and the others must not be used
first), and would expose a general pipeline builder rather than these two fixed shapes.

Packaging:

- `Cargo.toml`: cargo's generated header comment is replaced by a note about this copy; the
  `[[test]]` and `[[bench]]` targets and the `[dev-dependencies]` are removed (not built here).
  `Cargo.toml.orig` is upstream's, unchanged.
- `tests/`, `benches/` and `Cargo.lock` of the published package are left out.
- `rustfmt.toml` (empty) is added, so the workspace's formatting settings do not rewrite
  upstream's code.
- `CHANGELOG.md`, `README.md`, the licenses and `.cargo_vcs_info.json` are as published.

### Re-syncing with upstream

1. Fetch the new release: `cargo fetch` after bumping the version, or download
   `https://static.crates.io/crates/tokio-postgres/tokio-postgres-<version>.crate`, and check
   its sha256 against the `cksum` in the crates.io index.
2. Unpack it over `tokio-postgres/`, keeping `rustfmt.toml`.
3. Remove `tests/`, `benches/` and `Cargo.lock`; in `Cargo.toml` remove the `[[test]]` and
   `[[bench]]` targets and the `[dev-dependencies]`, and put the note back in place of the
   generated header.
4. Reapply the `datarig:` changes listed above (`git diff` of this directory against the
   previous release shows them; `src/pipeline.rs` is ours and carries over as it is, unless the
   `Row::new`, `Portal::new`, `Column` or `Statement::unnamed` internals it uses changed).
5. Update the version, checksum and commit in the table above, then run the workspace checks:
   `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
   `cargo test --workspace` with `DATARIG_TEST_PG_URL` set (the driver's PG tests compare
   every type with `format('%s', …)` and round-trip it through SQL INSERT, and count the round
   trips of each kind of statement behind a latency proxy).

### When to drop it

Drop this copy and the `[patch.crates-io]` entry as soon as a tokio-postgres release lets a
caller choose the result format of each column when binding a portal (for example once #961 or
an equivalent API is released) **and** bind a portal and fetch its first rows in one round trip
after a pipelined `BEGIN` (or an equivalent pipeline API), and switch
`datarig-driver-postgres` to those APIs. Without the second, the first page of a result costs
three more round trips again. Until then,
follow each upstream release that fixes a bug or a security issue by re-syncing as above.

## mysql_async 0.37.1

| | |
|---|---|
| Upstream | [`mysql_async`](https://crates.io/crates/mysql_async) 0.37.1 from crates.io ([blackbeam/mysql_async](https://github.com/blackbeam/mysql_async)) |
| License | MIT OR Apache-2.0 (`mysql_async/LICENSE-MIT`, `mysql_async/LICENSE-APACHE`) |
| Package checksum | sha256 `40d11da0e2d9fad4640c9f9198ee431c6d68444568f83ef1f10f3367270071e4` (the crates.io index `cksum` of the `.crate` file) |
| Upstream commit | `ce4b27698c50fb945d8c9ff8c40a2a646be50b12` (`mysql_async/.cargo_vcs_info.json`, as published) |
| Used through | `[patch.crates-io]` in `Cargo.toml`, with `default-features = false` and `minimal-rust` (no TLS, no C compression library, no value conversion features); the workspace `exclude`s `vendor` |

### Why it is vendored

mysql_async connects only over a socket of its own: a TCP connection it opens, or a Unix
socket. Its `Endpoint` is crate-private, so a caller cannot hand it a stream it opened itself.
datarig's MySQL driver reaches a server through an SSH tunnel as the PostgreSQL driver does:
over a channel of the tunnel (`transport::Dialer`), never through a listener on a local port
(another local user could connect to that port first, and mysql_async takes a Unix socket for
a secure line and would send a `caching_sha2_password` password over it in clear).

It also always asks the server for multi-statements and `LOAD DATA LOCAL`, which datarig must
not have: with multi-statements one request could run a text the server reads as several
statements, and with `LOAD DATA LOCAL` a server could ask for any file of the client.

### What is changed

Every change in the source is marked with a `datarig:` comment.

- `src/io/mod.rs`: the trait `CustomStream` (any `AsyncRead + AsyncWrite + Send + Unpin`), the
  endpoint `Endpoint::Custom` over one (neither a socket nor secure, so a full
  `caching_sha2_password` login sends the password encrypted with the server's public key;
  its liveness check is left to the stream's owner), and `Stream::custom`. `src/io/tls/rustls_io.rs` and `native_tls_io.rs` refuse TLS over such a
  stream (not built here: no TLS feature is on).
- `src/conn/mod.rs`: `Conn::connect_with_stream(opts, stream)`, a connection over the
  caller's stream (handshake, authentication, settings, init and setup commands as `Conn::new`
  does them, which now share them in `Conn::open`; it never moves to the server's socket file),
  and `Conn::is_mariadb`. `continue_caching_sha2_password_auth`: a full login over a
  connection that is neither secure nor a socket encrypts the password with the key of the
  options (`server_public_key`) when there is one and then never asks the server for its key;
  without one it asks (`0x02`) only when `public_key_retrieval` is on, and otherwise fails
  with `DriverError::PublicKeyRetrievalDisabled` before it sends anything (the key the server
  sends comes over the same unencrypted connection, so anyone in between could send theirs).
- `src/opts/mod.rs`: the client capabilities never include `CLIENT_MULTI_STATEMENTS` nor
  `CLIENT_LOCAL_FILES`, and include `CLIENT_SESSION_TRACK`, so the server reports changes of the
  session (the variables it tracks, the current database) in its OK packets. `MysqlOpts`'s
  `Debug` is written out instead of derived, so it never prints the password. Two options:
  `server_public_key` (the server's RSA public key in PEM, `None` by default) and
  `public_key_retrieval` (whether the server may be asked for its key, `true` by default as
  upstream does), with their `OptsBuilder` setters and `Opts` getters.
- `src/error/mod.rs`: `DriverError::PublicKeyRetrievalDisabled`.
- `src/lib.rs`: `pub use self::io::CustomStream`.

Packaging: `Cargo.toml`'s generated header comment is replaced by a note about this copy; its
`[[test]]` targets, `[dev-dependencies]` and `[profile.bench]` are removed (not built here).
`Cargo.toml.orig` is upstream's, unchanged. `tests/` and the `Cargo.lock` of the published
package are left out; `rustfmt.toml` (empty) is added, so the workspace's formatting settings do
not rewrite upstream's code. Everything else is as published.

### Re-syncing with upstream

1. Fetch the new release (`cargo fetch` after bumping the version, or
   `https://static.crates.io/crates/mysql_async/mysql_async-<version>.crate`) and check its
   sha256 against the `cksum` in the crates.io index.
2. Unpack it over `mysql_async/`, keeping `rustfmt.toml`; remove `tests/` and `Cargo.lock`;
   in `Cargo.toml` remove the `[[test]]` targets, the `[dev-dependencies]` and
   `[profile.bench]`, and put the note back in place of the generated header.
3. Reapply the `datarig:` changes listed above (`git diff` of this directory against the
   previous release shows them).
4. Update the version, checksum and commit in the table above, then run the workspace checks
   with `DATARIG_TEST_MYSQL_URL` set (and once more with `DATARIG_TEST_DIAL=tcp`).

### When to drop it

Drop this copy and its `[patch.crates-io]` entry once a mysql_async release can connect over a
stream the caller opened **and** lets the caller leave multi-statements and `LOAD DATA LOCAL`
off, and switch `datarig-driver-mysql` to those APIs. Until then, follow each upstream release
that fixes a bug or a security issue by re-syncing as above.

## Third-party code built into the binary: pg_query 6.2.0

Not vendored (a normal crates.io dependency of `datarig-core`), but it compiles C code into
the binary, so its licenses are kept here, in `licenses/pg_query/`. The published crate does
not include them.

| | |
|---|---|
| Crate | [`pg_query`](https://crates.io/crates/pg_query) 6.2.0 ([pganalyze/pg_query.rs](https://github.com/pganalyze/pg_query.rs)), MIT (`LICENSE-pg_query.rs`) |
| C library | libpg_query 17-6.2 ([pganalyze/libpg_query](https://github.com/pganalyze/libpg_query)), BSD-3-Clause (`LICENSE-libpg_query`): PostgreSQL 17.7's parser and scanner extracted as a library |
| PostgreSQL source in it | PostgreSQL License (`COPYRIGHT-PostgreSQL`, from `REL_17_STABLE`) |
| Also compiled in | protobuf-c (BSD-2-Clause, `LICENSE-protobuf-c`) and xxHash (BSD-2-Clause, `LICENSE-xxhash`), from `libpg_query/vendor` |
| Used for | the safety classifier (`datarig-core/src/sql/risk.rs`) and the splitter's differential tests (`sql/split/tests.rs`) |
| Build needs | a C compiler (`cc`) and libclang (`bindgen`); both are on GitHub's hosted runners (Linux, macOS, Windows) and come with Xcode or LLVM. `protoc` is not needed (the crate ships its generated protobuf code and regenerates it only when `protoc` is found) |
| Binary size | about +3.6 MB in the release build |

All five licenses are permissive and ask only that the copyright notices and license texts go
with the source and binary distributions: ship `licenses/pg_query/` with a release.
