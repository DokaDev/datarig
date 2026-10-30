# Poolers

datarig works through PgBouncer and other PostgreSQL connection poolers, including transaction
pooling, where consecutive transactions of one client may run on different server
connections.

## How a pooler is detected

When a query session connects, datarig checks in the same round trip whether the server
process answering is the one the connection started with. If it is not, the connection is
treated as **pooled** for the rest of the session. MySQL connections are never treated as
pooled; a proxy that multiplexes them (such as ProxySQL) is not detected.

## Prepared statements

- Nothing outside the query session relies on prepared statements, and statement names are
  unique per process.
- The query session keeps prepared statements between transactions (the server-side
  statement cache). Turn it off for a pooler in transaction mode that does not support them
  (PgBouncer before 1.22, or one with `max_prepared_statements = 0`):

  ```toml
  [[connections]]
  name = "app-via-pgbouncer"
  statement_cache = false
  ```

- When the server keeps losing prepared statements, the session turns the cache off by itself
  and says so.

## What changes behind a pooler

Settings that would outlive a transaction would stay on a server connection other clients
share, so behind a pooler datarig keeps them inside each transaction:

- **`lock_wait`** is sent as `SET LOCAL lock_timeout` in each transaction instead of once for
  the session (see [Environments and safety](environments.md#lock-waits)).
- **`idle_disconnect`** gets no server-side `idle_session_timeout` behind a pooler; datarig's
  own timer still closes the tab's session.
- **Startup SQL** statements go first in every transaction as `SET LOCAL`; statements that have
  no such form are not sent, and the tab names them (see
  [Startup SQL](startup-sql.md#behind-a-connection-pooler)).
- Statements that cannot run in a transaction (`VACUUM`, `CREATE INDEX CONCURRENTLY`, …),
  `CALL`, `DO`, and your own `SET` and `RESET` run without these per-transaction settings.

In datarig, `SHOW lock_timeout` and `current_setting('lock_timeout')` show the limit in effect.
Another client of the same pooled server connection sees the server's value, because nothing
stays on it.

## Manual commit and pooling

- In **transaction pooling**, from the first write to the commit one server connection is
  yours, and the open transaction holds that pool slot for as long as it stays open.
- **Statement pooling** (PgBouncer's `pool_mode = statement`) refuses transactions of several
  statements, so [manual commit](manual-commit.md) does not work through it.
- The server-side limit of `tx_idle_rollback` is set with `SET LOCAL`, so it ends with the
  transaction and nothing stays on the pooled connection.
