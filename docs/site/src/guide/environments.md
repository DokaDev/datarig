# Environments and safety

## What datarig checks

- **What asks.** Every statement is parsed before it is sent, with PostgreSQL's own parser
  (libpg_query) for PostgreSQL and datarig's MySQL classifier for MySQL, never with a regular
  expression. These open a confirmation with Cancel focused: `DROP`, `TRUNCATE`, an `UPDATE` or
  `DELETE` without a real `WHERE`, `ALTER TABLE … DROP COLUMN`, `ALTER … TYPE`, `MERGE` that
  updates or deletes, `DO` and `CALL`, `COPY` to or from server files or programs, server-side
  built-ins that act on files or other backends, and any text the parser rejects. An
  environment with `confirm = "writes"` asks before every write too.
- **What is refused.** A read-only profile refuses anything that is not a read, transaction
  control or a safe session setting before sending it, and the server refuses the rest:
  every transaction of a read-only profile is opened `READ ONLY`, so even a function called
  from a `SELECT` cannot write. (MySQL works differently; see
  [MySQL](mysql.md#read-only-profiles).)
- **What it is not.** The confirmation is a guardrail against mistakes in what the text shows.
  Functions called from a query, triggers, views and rules are not inspected; a read-only
  environment is the guarantee.
- **Paging leaves nothing open by default**: datarig keeps no transaction, lock or snapshot
  open while you read a result, unless your environment asks for it (see
  [Paging and locks](#paging-and-locks)).
- `EXPLAIN ANALYZE` of a write is rolled back (see [Plans](plans.md)).

```text
{{#include ../screens/run-confirm.txt}}
```

## Environments

An environment is a named set of rules that profiles share: read-only or not, which
statements ask first, whether you commit by hand ([Manual commit](manual-commit.md)), how
results page, the spill limit, idle and lock limits, a [Startup SQL](startup-sql.md) per
database, and an optional color and badge (such as `PROD`). The badge shows on the profile's
tabs, in the explorer and in the status bar, and the profile's name takes the environment's
color in the explorer, the tab list, quick connect and the tab bar.

Pick one in a profile's form (Advanced section, `Enter` on Environment). The list has "None
(default rules)", every environment with its color, badge and rules, and "New environment…".

### The Environments section

The explorer's **Environments** section starts with **Default rules (no environment)**: the
rules of every profile without an environment. `e` edits them and `Enter` lists those
profiles; they cannot be renamed, copied or deleted, and have no color or badge.

Below it each environment shows its color, its badge and its rules in a few words: read-only in
blue, "asks before every write" and "manual commit" in the warning color, "holds locks while
paging" in the error color. `n` makes one, `e` edits, `c` copies, `R` renames, `d` deletes,
and `Enter` or `l` lists the profiles that use it. A rename takes those profiles along;
deleting one that profiles use asks first and names them. The command line also finds the
action `env.new`.

While there is no environment, the section offers **Add presets: dev, test, staging, prod**,
and the profile form's list offers the same four:

| Preset | Rules | Color | Badge |
|---|---|---|---|
| `dev` | the defaults | green | |
| `test` | the defaults | blue | |
| `staging` | every write asks, manual commit | orange | `STG` |
| `prod` | read-only, every write asks, manual commit | red | `PROD` |

Nothing is written on its own: presets are added only when you ask, and once written they are
ordinary environments. **No behaviour depends on an environment's name**; a preset only gives
the first values of its rules, color and badge.

### In the config file

An environment is a `[policy.<env>]` table, every key optional:

```toml
[policy.prod]
read_only = true
confirm = "writes"
commit = "manual"
color = "red"
badge = "PROD"
lock_wait = "5s"

[policy.local]
paging = "hold"
paging_idle_timeout = "10s"

[[connections]]
name = "prod-replica"
policy = "prod"
# ...
```

| Key | Values | Default |
|---|---|---|
| `read_only` | `true`, `false` | `false` |
| `confirm` | `"destructive"`, `"writes"` | `"destructive"` |
| `commit` | `"auto"`, `"manual"` | `"auto"` |
| `tx_idle_warning` | a duration of at least 10 s, or `"off"` | `"5m"` |
| `tx_idle_rollback` | a duration of at least 10 s, or `"off"` | `"30m"` |
| `paging` | `"no_hold"`, `"hold"` | `"no_hold"` |
| `paging_idle_timeout` | seconds, or `"90s"`, `"5m"`, `"1h"`, `"off"` | `"30s"` |
| `spill_limit` | bytes, or `"512MB"`, `"2GB"`, `"off"` | the top-level `spill_limit` |
| `idle_disconnect` | `"1m"` to `"24h"`, or `"off"` | the top-level `idle_disconnect` |
| `lock_wait` | `"1s"` to `"24h"`, or `"off"` | the top-level `lock_wait` |
| `startup` | `{ postgres = "<name>", mysql = "<name>" }`, either left out | none |
| `color` | `red`, `orange`, `yellow`, `green`, `teal`, `cyan`, `blue`, `indigo`, `purple`, `pink`, `brown`, `gray`, or `#rrggbb` | none |
| `badge` | up to 8 columns, no blanks around it | none |

The idle-transaction keys are explained on [Manual commit](manual-commit.md#settings), and
`startup` on [Startup SQL](startup-sql.md). Older names are still read: `tx_idle_timeout` for
`tx_idle_rollback`, and `session` for `startup`; a table with both names is an error.

- A wrong `color` or `badge` is reported when the file is read, like any other value.
- Saving from the app changes only what you changed. An untouched table, and an untouched
  value in an edited one, keep their text and comments; a value set back to its default is
  removed; a renamed table keeps its comments and its place. Environments written as dotted
  keys or as an inline table stay that way.
- `[policy.default]` holds the default rules. It is not an environment: it is the section's
  first row, and the app writes it only when you save a change in its form. A `color` or
  `badge` in it is not shown anywhere. Without it the built-in defaults apply: writable,
  dangerous statements ask, automatic commit, no held pages.

**A profile naming an environment that does not exist is an error of that profile**, not a
fallback to the default rules. Its explorer line shows the environment mark in the error color,
connecting is refused with a notice that names the missing environment, and a connected profile
whose environment was deleted or renamed away runs nothing until another one is picked.

## Paging and locks

A result with more rows than a page (`page_size`, 500 rows by default) can be paged two ways,
chosen per environment with `paging`. Inside your own `BEGIN` … `COMMIT` block both behave the
same: the result's cursor lives in your transaction and ends with it.

| | `paging = "no_hold"` (default) | `paging = "hold"` |
|---|---|---|
| After the first page | The cursor and its transaction end in the same round trip | The cursor stays open in a transaction of its own |
| Held on the server while you read | Nothing: no transaction, no lock, no snapshot | The transaction, its snapshot, and an `ACCESS SHARE` lock on every table the statement read |
| Next page (`n`) | Runs the statement again and skips the rows you have, only for a plain `SELECT` that is safe to repeat (said each time; the rows may differ if the data changed) | Read from the open cursor |
| A statement not safe to repeat (a write, a volatile function, a view, …) | You see the first page, and why; page it yourself with `LIMIT`/`OFFSET`, or use `"hold"` | Read from the open cursor |
| Held until | — | The result is read to its end, another statement runs in the tab, or `paging_idle_timeout` passes without a page |

Why nothing is held by default: an `ACCESS SHARE` lock held by an idle result makes a deploy's
`ALTER TABLE` wait, and every query after the `ALTER TABLE` waits behind it. A held
transaction also keeps vacuum from cleaning up. Use `"hold"` for a local database, or when you
walk a large result page by page (without it each page runs the statement again and skips the
rows before it, which costs more the deeper you go). A row count (`#`) never keeps anything
open. MySQL pages its own way; see [MySQL](mysql.md#paging-and-locks).

Rows already fetched stay in a memory window (`result_window_rows`, 10,000 rows by default) and
spill to a private temporary file past it, up to `spill_limit` (1 GB by default; an environment
may set its own).

## Idle connections and lock waits

Two settings say how long a connection may wait, the same for every database (datarig turns
them into each server's own settings). Both are off unless you set them: at the top level of
the config, on the settings screen (Connections group), with `:set idle_disconnect=5m` or
`:set lock_wait=5s` (any duration in range, or `off`), and per environment (its form's Idle
disconnect and Lock wait fields; empty takes the config's value, `"off"` turns it off there).

| Key | Values | What it does |
|---|---|---|
| `idle_disconnect` | `"1m"` to `"24h"`, or `"off"` | Closes a connection nothing used for that long |
| `lock_wait` | `"1s"` to `"24h"`, or `"off"` | A statement waits at most that long for a lock another session holds |

### Idle disconnect

- A tab's session closes once nothing ran in it (a run, a page, a commit) for that long, and
  the tab says so. Once the whole connection sat unused that long (the explorer's reads count
  too), its metadata sessions and its SSH tunnel close as well (a shared tunnel once no other
  connection uses it), and the explorer shows it.
- The next run or explorer action connects again. A password that would have to be asked for
  (the prompt, the keychain) is reused from the first connect; a password command, variable or
  secrets file is read again.
- A tab with an open transaction is warned a minute before (half the limit under two
  minutes), in the status bar and its Messages. When the session closes, the server rolls the
  transaction back, and datarig says so.
- Time asleep counts: a session left overnight closes at the first key, answer or timer after
  the machine wakes.
- The server gets its own limit for each query session, a minute longer, for when datarig
  cannot close the session itself (it was killed, the machine slept): PostgreSQL
  `idle_session_timeout` (version 14 or later, only on a direct connection; behind a pooler it
  would end server connections other clients share), MySQL `wait_timeout`. A session the server
  ended that way is reported as closed for being idle, not as a lost connection.
- A change of the setting applies to sessions opened after it; the app's own timer follows it
  at once.

### Lock waits

- For the query sessions only; the explorer's lookups keep their own two seconds.
- PostgreSQL: `lock_timeout`, set for the session on a direct connection. Behind a
  [pooler](poolers.md) it is `SET LOCAL` in each transaction instead. On a direct connection,
  a `RESET ALL` or `DISCARD ALL` you run clears it (back to the server's default) until the
  session connects again; behind a pooler the next transaction gets it again.
- MySQL: `lock_wait_timeout` (metadata locks) and `innodb_lock_wait_timeout` (row locks), in
  whole seconds.
- A statement that waited too long fails with "Waited longer than 5s for a lock another
  session holds, so the server cancelled the statement", followed by the server's own error.
- It does not change how commits work: with automatic commit only that statement failed; in a
  manual transaction on PostgreSQL only that run is undone (its savepoint) and the transaction
  keeps what earlier runs did; on MySQL the server undoes the statement only (unless
  `innodb_rollback_on_timeout` is on: then the whole transaction, and datarig says so).
- A [Startup SQL](startup-sql.md) or a profile's own `startup_sql` may set the lock wait itself
  (PostgreSQL `SET lock_timeout = '3s'`; MySQL
  `SET SESSION lock_wait_timeout = 3, innodb_lock_wait_timeout = 3`). It runs after datarig's
  own limit, so it overrides the environment's for those sessions, and the error names its
  value. PostgreSQL's `0` means no limit. When datarig cannot read the value (`DEFAULT`, an
  expression, a unit it does not read; on MySQL the two settings at different values), the
  error says the statement waited too long, without a time.
- `:session` shows the lock wait that applies and what it overrides.

## Idle transactions

With manual commit, an environment also says when an idle open transaction is warned about
(`tx_idle_warning`) and rolled back (`tx_idle_rollback`). See
[Manual commit](manual-commit.md#settings).
