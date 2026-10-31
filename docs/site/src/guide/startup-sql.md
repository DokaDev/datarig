# Startup SQL

A Startup SQL is a named list of settings that a profile's query sessions (each tab's) start
with, for one kind of database. It is optional: a profile without any of these keys starts its
sessions with the database's own defaults.

```toml
[startup.reporting]
dbms = "postgres"                  # postgres | mysql
sql = ["SET statement_timeout = '30s'", "SET work_mem = '64MB'"]   # one statement per item

[policy.prod]
startup = { postgres = "reporting", mysql = "my_reporting" }   # per database, for its profiles

[[connections]]
name = "orders"
startup = "reporting"              # this profile's own choice, instead of its environment's
# startup = "none"                 # none, even when its environment maps one
startup_sql = ["SET work_mem = '256MB'"]   # its own statements, after the Startup SQL's
```

`none` is not a preset name: `[startup.none]` is an error of the file. The keys were named
differently in older versions and are still read: `[sessions.<name>]` for `[startup.<name>]`, an
environment's `session` for `startup`, and a profile's `session` and `session_sql` for
`startup` and `startup_sql`. A place that has a key and its old name is an error. Saving from
the app renames the old names in place once what they say changes.

## Manage presets

The explorer's **Startup SQL** section lists the presets with their database and how many
statements and profiles each has (the statements in the warning color when one changes a setting
a read-only environment refuses, such as MySQL's `foreign_key_checks = 0`), and under each
preset the first statement that is not allowed and why. `n` makes one, `e` edits, `c` copies,
`d` deletes, `t` tests, and `Enter` or `l` lists the profiles that use it (their own or through
their environment). The `/` filter finds a preset by its name or a statement. The command line
also finds `session.new`.

Its form has the name, the database and the statements, one per row:

- `Enter` on a row with a statement adds an empty row below; on an empty row it removes the
  row and moves to the next field.
- `Up`/`Down` go from row to row, `Tab` leaves the list, `Backspace` on an empty row or
  `Ctrl+D` on any row removes it.
- A paste of several statements makes a row of each: a `;` ends one, and so does a line break
  before a line that starts with `SET`. A row that still holds two statements
  (`SET a = 1 SET b = 2`) is refused.
- With the mouse, a click puts the cursor in a row, `×` removes it and **+ Add statement**
  adds one at the end.

Under each row that is not allowed the form says why, and under each one a read-only
environment would refuse, that it would. A rename takes the profiles and environments that name
the preset along; deleting one asks first and names them.

**Test** opens one session on a connected profile that uses the preset (else any connected
profile of its database; with several you pick one), runs the preset exactly as a tab's
session would, and closes the session again, saying what was applied or which statement the
server refused. A profile that is not connected is not connected for it.

In a profile's form (Advanced section, `Enter` on Startup SQL) you pick its environment's
preset (the default), **None**, a preset of its database, or "New Startup SQL…". Its own
statements are the **Own startup SQL** list below it. The environment form has one choice per
database.

## What a session starts with

In order, a later setting of the same name winning:

1. datarig's own limits: `lock_wait`, `idle_disconnect`, `tx_idle_rollback`
   ([Environments and safety](environments.md), [Manual commit](manual-commit.md));
2. the preset: the profile's `startup`, else the one its environment maps for its database
   (`[policy.default]` may map one too);
3. the profile's `startup_sql`.

So a preset's or the profile's own `SET` of the lock wait overrides the environment's
`lock_wait` for its sessions, and one of the server's idle limit (see
[Idle limit](#idle-limit)) its `idle_disconnect`. The explorer's lookups (the metadata
sessions) never get these settings.

### Idle limit

A statement may set the server's idle limit: PostgreSQL `SET idle_session_timeout = '10min'`
(version 14 or later), MySQL `SET SESSION wait_timeout = 600`. That value is then the idle
disconnect of those sessions instead of the environment's `idle_disconnect`:

- datarig's own timer follows it: a tab's session closes after that long unused, a tab with an
  open transaction is warned a minute before, and the message names that time. The server has
  the same limit, so either one may end the session first; a session the server ended is
  reported as closed for being idle.
- The connection's other sessions and its SSH tunnel stay open as long as its longest tab limit,
  so a tab never loses its tunnel before its own limit.
- PostgreSQL's `0` turns the limit off for those sessions.
- On PostgreSQL a value datarig does not read (`DEFAULT`, an expression, a unit it does not
  read) still goes to the server, and datarig keeps closing the session after the
  environment's `idle_disconnect` (the server does not count it inside a transaction). On MySQL
  `wait_timeout` must be a whole number of seconds from 1: the server counts it inside a
  transaction too, so another value (`DEFAULT`, `0`, an expression) is refused.
- Behind a pooler PostgreSQL's is not sent (see
  [Behind a connection pooler](#behind-a-connection-pooler)): the environment's
  `idle_disconnect` stays there.
- In a read-only environment PostgreSQL's is allowed; MySQL's `wait_timeout` is not (an idle
  limit is not among the settings a read-only session may change there).
- MySQL's `interactive_timeout` is refused: the server reads it only when a session starts, so
  it has no effect on an open one.

`:session` shows the limit that applies and what it overrides.

### Seeing what applies

`:session` (or `:startup`) shows what the query sessions of the explorer's selected profile
(else the active tab's) start with. `Ctrl+E` in a profile's form shows the same for the form's
values, saved or not. The view lists, in the order they are applied, the app's limits with
their values and where each comes from, then each statement under where it comes from. Under a
PostgreSQL statement a dim line says how it would go behind a pooler. A profile whose plan has
an error shows the words its connect would refuse with.

```text
{{#include ../screens/session-view.txt}}
```

## Rules for the statements

- **Only settings.** Each item is one statement that sets settings of the session and nothing
  else: PostgreSQL `SET [SESSION] name = value` (or `TO`, `TO DEFAULT`), `SET TIME ZONE …`,
  `SET SESSION CHARACTERISTICS AS TRANSACTION …`; MySQL `SET [SESSION | LOCAL] name = value[, …]`,
  `SET @var = …`, `SET SESSION TRANSACTION …`. Not `SET LOCAL`, a plain `SET TRANSACTION`,
  `SET GLOBAL` or `PERSIST`, `RESET`, `DISCARD`, a query or a call such as `set_config()`, nor
  several statements in one item. In a read-only environment only the settings a read-only
  session may change at all.
- **Not datarig's own settings.** What datarig manages itself is refused, with why and where to
  set it instead:
  - MySQL's `interactive_timeout`: it has no effect on an open session; set `wait_timeout`
    (see [Idle limit](#idle-limit)) or the environment's `idle_disconnect`.
  - `idle_in_transaction_session_timeout`: use `tx_idle_rollback`.
  - the read-only settings (`default_transaction_read_only`, `transaction_read_only`, MySQL's
    `tx_read_only`, `… TRANSACTION READ ONLY/WRITE`): use the environment's `read_only`.
  - MySQL's `autocommit` and `completion_type`: use the environment's `commit`.
  - `search_path`: the tab's schema sets it (`:use`).
  - what datarig reads results in, writes literals with or tells sessions apart by:
    `client_encoding`, MySQL's character sets, `collation_connection` and `SET NAMES`,
    `DateStyle`, `IntervalStyle`, `bytea_output`, `extra_float_digits`,
    `standard_conforming_strings`, `application_name`, MySQL's `sql_select_limit`,
    `resultset_metadata` and `session_track_*`.

  The lock wait settings, the server's idle limit (`idle_session_timeout`, MySQL's
  `wait_timeout`) and MySQL's `max_execution_time` are allowed. Settings of one
  transaction only are refused too, with where the session's default is: PostgreSQL's
  `transaction_isolation` and `transaction_deferrable` (use `default_transaction_isolation`, …),
  MySQL's `SET @@transaction_isolation` without a scope (use `SET SESSION …`).
- **MySQL in every sql mode.** A statement is read with and without `ANSI_QUOTES` and
  `NO_BACKSLASH_ESCAPES`; what any reading refuses is refused.
- **Who the session is.** `SET ROLE` is allowed in a read-only environment (every transaction
  stays `READ ONLY` whatever the role); `SET SESSION AUTHORIZATION` is not.
- **Errors are the profile's.** A preset name that does not exist, a preset for the other
  database, or a statement that is not allowed: the profile does not connect and says which
  preset or statement, from where, and why. It never connects without the preset instead.
  Statements are checked when a profile connects and before each tab's session opens, not
  when the file is read.
- **Sent is not applied.** Each statement runs alone, in order, when a tab's session opens (and
  when it reconnects). One the server refuses closes that session before anything else runs,
  and the tab quotes the server's error. The run that opened the session does not happen.

## Behind a connection pooler

On a PostgreSQL connection behind a pooler (see [Poolers](poolers.md)) nothing is set for the
session. Each `SET` goes first in every transaction as `SET LOCAL` instead, and the tab says so.
Statements that cannot run in a transaction, `CALL`, `DO`, and your own `SET` and `RESET` run
without them.

These are not sent there at all (the profile still connects, and the tab names what was not
sent): statements that change who the session is (`SET ROLE`, `SET SESSION AUTHORIZATION`:
a statement run outside a transaction would run as someone else), statements with no
`SET LOCAL` form (`SET SESSION CHARACTERISTICS …`), defaults of the session's transactions
(`default_transaction_isolation`, `default_transaction_deferrable`), and values that depend on
what the pooled connection did before (`temp_buffers`, PostgreSQL 17's `transaction_timeout`),
and `idle_session_timeout` (the server counts it only outside a transaction, and set for the
session it would end server connections other clients share).
Behind a pooler, set the isolation in your own block instead (`BEGIN ISOLATION LEVEL …`).

## Details

- On MySQL the classifier reads the session in the sql mode a preset set, and a read-only
  session is checked after its statements ran. A value with a function call or a subquery
  (`SET @x = (SELECT …)`) runs at connect like any other.
- A row shows its statement on one line, without its line comments, and a line break inside a
  string as `⏎`. A row you do not edit is saved as the file has it; one you edit is saved as
  its one line, so a comment in it is gone.
