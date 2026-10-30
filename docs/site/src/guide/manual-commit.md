# Manual commit

With `commit = "manual"` in an [environment](environments.md), what you change waits for you:
datarig opens a transaction right before the first statement that may write, and it stays open
until you commit or roll it back. **datarig never sends a `COMMIT` on its own**; on MySQL,
statements that commit by themselves are covered [below](#limits). The default,
`commit = "auto"`, commits each statement that runs outside your own `BEGIN` block by itself.

```toml
[policy.staging]
commit = "manual"
tx_idle_warning = "5m"
tx_idle_rollback = "30m"
```

The `staging` and `prod` presets commit by hand in a new setup. Environments you already have
(and any without the key) stay automatic until you change them.

## When the transaction opens

- **Reads before the first write keep nothing open.** A plain read (a `SELECT` the paging
  allowlist takes, `SHOW`, …) runs as it does with automatic commit.
- Anything not known to be a plain read opens the transaction: writes, DDL (PostgreSQL),
  `SELECT … FOR UPDATE`, a `SELECT` that calls a function of yours or a volatile one, `CALL`,
  `DO`, an `EXECUTE` of a prepared write, text the parser rejects. Session settings,
  transaction control and `EXPLAIN ANALYZE` (rolled back anyway) never open it.
- Once open, every statement of the tab runs in it, reads too: in
  `SELECT …; UPDATE …; SELECT …` the transaction opens before the `UPDATE`, and the last
  `SELECT` sees the update. Results read in it are labelled uncommitted.
- Your own `BEGIN` still works as before, with the server's rules.
- With `read_only = true` no transaction ever opens, because every write is refused first.

**PostgreSQL.** DDL is transactional, so `CREATE`, `ALTER` and `DROP` roll back with the rest.
Until then they hold their locks, and Messages says so: the lock mode for the common cases
(`ALTER TABLE … ADD COLUMN`, `DROP TABLE` and `TRUNCATE` hold an `ACCESS EXCLUSIVE` lock, so
other sessions' reads of the table wait; `CREATE INDEX` a `SHARE` lock, so their writes wait).
Opening the transaction goes out with the statement, without an extra round trip.

**MySQL.** DDL (and `LOCK TABLES`, `START TRANSACTION`, …) commits the open transaction by
itself and cannot be rolled back. It never opens a transaction. With changes pending it asks
first, saying what it would commit ("first commits the open transaction: 3 statements (120
rows)"; `Enter` leaves it unrun, `y` runs it). `START TRANSACTION` costs one more round trip, on
the first write of each transaction only.

## Commit, roll back, look

| Keys | Command | What it does |
|---|---|---|
| `Space x c` | `:commit` | Commit |
| `Space x r` | `:rollback` (`:rb`) | Roll back |
| `Space x s` | `:tx` | What the open transaction did: its statements with their rows and times, how long it has been open and idle; `c` commits, `r` rolls back, `Esc` closes |

They work for your own `BEGIN` too, and are in the action menu. They are refused while a
statement runs in the tab, and a transaction that failed can only be rolled back.

While a transaction is open the tab shows `◆`, the explorer shows `◆` after its profile, and the
status bar shows something like `TX open · 3 changes · 120 rows · 4:12`: the statements that may
have written, the rows they changed and the time since it opened (with `idle 1:30` once it has
sat idle a minute).

```text
{{#include ../screens/tx-uncommitted.txt}}
```

## Settings

Per environment, in its form (Commit, Idle warning, Idle rollback) or in `[policy.<env>]`:

| Key | Values | Default |
|---|---|---|
| `commit` | `"auto"`, `"manual"` | `"auto"` |
| `tx_idle_warning` | a duration (`"90s"`, `"5m"`, `"1h"`; at least 10 s) or `"off"` | `"5m"` |
| `tx_idle_rollback` | a duration (at least 10 s) or `"off"` | `"30m"` |

- **`tx_idle_warning`**: once an open transaction has sat idle that long, datarig warns once
  (again only after it ran something and sat idle again), its `◆` turns red in the tab bar and
  the explorer, and the status bar adds `idle 5:00 · holding locks` in the error color. It never
  ends anything.
- **`tx_idle_rollback`**: a transaction opened for manual commit that sat idle that long is
  rolled back, and the connection stays. datarig sends `ROLLBACK` on the tab's session, which
  releases its locks, and once the server confirms it says how many changes were undone. A
  minute before (half the limit under two minutes) datarig warns, unless `tx_idle_warning`
  already did. The limit is the one the transaction began with. Time asleep counts once the
  machine wakes. With a shorter `idle_disconnect`, that closes the session first. Your own
  `BEGIN` is not covered.
  - If the `ROLLBACK` fails or gets no answer in time (10 s; 60 s on MySQL, whose rollback can
    take about as long as the writes did), datarig closes the tab's session instead and says
    so; the server rolls back once it sees the session end.
  - PostgreSQL also gets a server-side limit 30 s later than datarig's
    (`SET LOCAL idle_in_transaction_session_timeout` with the `BEGIN`, so `"30m"` sets 1830 s).
    It ends with the transaction, so nothing stays on a pooled connection, and acts only when
    datarig could not: the laptop asleep, datarig killed or stuck, the tunnel gone. `"off"`
    leaves the server's setting.
  - MySQL has no server-side limit for a transaction, so there the rule holds only while
    datarig runs and the machine is awake.
- The rule is read at each run. Switched to `"auto"` while a transaction is open, that
  transaction stays until you end it, and datarig says so.

## When a run fails

- **PostgreSQL**: each run in the open transaction goes under a savepoint of its own (no extra
  round trip). A run that fails or is cancelled is undone, and the transaction keeps what
  earlier runs did; it does not become aborted. Your own `BEGIN` block has no such savepoint: a
  failure aborts it, as PostgreSQL does.
- **MySQL**: the server undoes only the statement that failed. A deadlock (or a lock wait
  timeout with `innodb_rollback_on_timeout`) rolls back the whole transaction, and datarig says
  how many changes are gone.
- A run that opened the transaction and then fails rolls it back at once: nothing of it is
  kept.

**Statements that cannot run in a transaction** (PostgreSQL `VACUUM`,
`CREATE INDEX CONCURRENTLY`, `CREATE DATABASE`, `ALTER SYSTEM`, …) are refused while a
transaction is open ("Commit (c) or roll back (r) first"). With manual commit and none open they ask
first: they run at once and cannot be rolled back. A procedure or `DO` block that ends
transactions itself fails inside the transaction; when it was the first write, datarig rolls
back and asks whether to run it once outside one.

**How a commit ends**: committed, with the count, rows and time in Messages; or refused by the
server (a deferred constraint, a serialization failure), which then rolls back, with its error.
A PostgreSQL transaction that had failed is reported as rolled back, never as committed. When
the connection ends while a commit is on its way, whether it happened is not known, and
datarig says so: check the data.

## Closing, disconnecting, quitting

Closing a tab, disconnecting, quitting, switching a tab's connection or database with `:use`,
or deleting the profile with a transaction open asks first, with what it holds:

- `c` commits first, `r` rolls back first, `y` goes on anyway (the server rolls back), `Enter`
  or `n` stays. A failed transaction offers only `r` and `y`.
- After `c` or `r` it goes on only once the server confirmed; one that fails stops there.
- Quitting with several tabs ends them one at a time and stops at the first that fails.

A signal (SIGTERM, SIGHUP, the terminal gone), a crash or a kill cannot ask: the connection
closes and the server rolls back. No transaction comes back with a restored workspace.

## Limits

- MySQL cannot run a plain read outside a transaction read-only without an extra round trip,
  so it does not: a write hidden behind a read (a view that calls a stored function that
  writes) commits by itself. PostgreSQL runs those reads read-only, so such a write fails.
- MySQL: `LOCK TABLES` is refused with manual commit. A `CALL` of a procedure that runs DDL
  commits the transaction without asking (datarig says so afterwards). A `SET autocommit = 0`
  of yours shows "autocommit off (session)".
- Poolers: see [Poolers](poolers.md#manual-commit-and-pooling).
- SSH tunnels: when the tunnel is lost, the link from the bastion to the database may stay
  open, and the server may hold the transaction and its locks until it notices. On PostgreSQL
  the server-side part of `tx_idle_rollback` is the safety net; on MySQL there is none.
- PostgreSQL: each run that writes under its savepoint is a subtransaction, kept until the
  transaction ends. Past about 64 in one transaction, other sessions' snapshots get slower:
  commit long transactions of many runs.
- PostgreSQL: `tx_idle_rollback` needs PostgreSQL 9.6 or later; older servers refuse it and
  the first write fails. Set it to `"off"` there. Savepoint names starting with `datarig_` are
  datarig's own and are refused in a transaction opened for manual commit.
