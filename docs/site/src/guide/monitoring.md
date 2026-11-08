# Server monitor

The Monitor tab shows what a connection's server says about its load and its sessions:
PostgreSQL 12 or later and MySQL 8.0.17 or later, self-managed or a managed service. It only
reads: nothing it sends changes anything on the server.

Open it with `M` on a connection in the explorer, its menu's "Monitor" item, `Space m m`, or
`:monitor` (the explorer's connection or the active tab's; `:monitor <name>` for another one).
There is one Monitor tab per connection: opening it again goes to the one that is open.

## How it reads

The tab has a connection of its own, opened once the connection's profile is connected. It
never uses a tab's query session or the explorer's, and it never sends anything you typed.
Each read is one read-only transaction (PostgreSQL) or a few read-only statements (MySQL), with
a short lock wait and a statement limit, so it never waits behind other sessions or holds them
up.

- It reads only while the tab is the one shown. A Monitor tab in the background sends nothing,
  and when it is shown again it reads at once; its charts show a gap for the time it was not
  looking, never a line across it.
- One read at a time: the next one starts an interval after the answer to the last, so a slow
  server is never asked twice at once. The interval is `[monitor] interval` (2 seconds by
  default; 1, 2, 5, 10, 30 or 60 seconds); `+` and `-` change it for the tab.
- `p` pauses (what was read stays for reading), `r` reads at once (also while paused).
- After three failed reads in a row the delay doubles, up to a minute; the next good read goes
  back to the interval. A failed read says why; what is shown is from the read before.
- Rates are worked out on the server's own clock. A restart or a statistics reset is a gap,
  never a negative rate or a spike, and the monitor's own reads are taken out of the counts.

## Overview

The server's version, provider (when the server says it is a managed service), whether it
takes writes, its uptime and its connection limit; then a chart of each metric the server has
(PostgreSQL's commits, MySQL's statements, sessions, the page cache's hit share, rows read and
written, pages read from storage, and more), and the longest running sessions: those running a
statement, and those idle with a transaction open. A click on one shows it in the Sessions view.

## Sessions

Every session the account may see, the longest running statement first. `s` sorts by the next
column and `S` the other way (a click on a column's name sorts by it), `/` filters by user,
database, application, client, state or statement, `j`/`k` select. Idle sessions are not
listed but counted; `a` lists them too. Sessions waiting for a lock, and transactions idle for
over a minute, are marked; the monitor's own connection is dim.

`Enter` shows the selected session in full: its client, application, kind, what it waits for,
when its transaction and statement started, and its statement. The list holds the first 256
characters of a statement; the details read the rest of a longer one, once, by a read of its
own. The whole text is shown only while the session still runs that statement: PostgreSQL tells
it by the session's and the statement's start, to the microsecond; MySQL tells a statement's
start in whole seconds, so there a new run of the same statement started in the same second
cannot be told apart, and the details say so. When the session has ended, runs another
statement, or the account cannot tell, the details say which and show only the start. Where the
server itself may have cut a statement (PostgreSQL keeps only `track_activity_query_size`
bytes), that is said.

`o` opens the statement in a new console of the connection, in that session's database, without
running it (the other session's search path and settings are not known, and the console says
so). `y` copies it. Both take the whole text once the details have read it, else the start and
a note that it is only the start.

## Locks

`3` shows who waits for whom as a tree: each session or transaction on one line, under the one
it waits for, the roots (those that wait for nobody) first, the one with the most sessions
behind it at the top. A waiter's line says what it waits for (the lock mode and what it is
on: a table, a row, a transaction) and since when; a root's says what it holds and how long
its transaction has been open. A session waiting for more than one says which others it also
waits for. `j`/`k` or a click select a line, the selected line is shown in full below the tree
when it is longer than the screen, `b` goes to the root blocker of the selected wait, `Enter`
shows the session in full, and `o` and `y` take its statement as in the Sessions view.

The tree comes from what the server says:

- PostgreSQL names each waiter's blockers (`pg_blocking_pids`), asked for the 50 longest waits
  only; when more sessions wait, the view says how many are left out. What each waits for and
  what its blockers hold come from `pg_locks`, read only while the Locks view is shown and at
  most every 5 seconds (or every interval when it is longer), and not at all while no session
  the account sees waits: each read asks the server's lock manager for all its locks. The view
  says when that was last read.
- A prepared transaction (`PREPARE TRANSACTION`) holds its locks with no session: it is a root
  of its own, with its id, owner, database and age. Only `COMMIT PREPARED` or `ROLLBACK
  PREPARED` with its id ends it; the monitor does not offer them.
- MySQL names the blockers of row lock waits (`performance_schema.data_lock_waits`); what such
  a wait is on is not read (`data_locks` would make the server go through every lock it holds,
  millions in a large transaction), so the line says it waits for a row lock and the statement
  shows on which table. For
  metadata locks (an `ALTER TABLE` waiting for a long transaction, `FLUSH TABLES WITH READ
  LOCK`) it does not: the tree shows the sessions holding a lock on the same object that the
  request cannot be granted beside, as the server's lock manager decides it, and says that
  they may block it. With more than one such holder, `b` does not pick one. A request waiting
  behind another request rather than a granted lock is shown waiting, its holder not found.
  Both lock tables are read only while the Locks view is shown, at most every 5 seconds, and
  only when a session may be waiting for a lock.
- A wait in a cycle (a deadlock the server has not broken yet) is shown once, from its
  smallest session, and marked.

## What the account sees

What the account may not see is said in the tab, with what would show it, never shown as
nothing:

| Server | Without | The tab shows |
|---|---|---|
| PostgreSQL | `pg_read_all_stats` (or superuser) | other roles' sessions without their state, statement and times, and their lock waits out of the Locks view |
| MySQL | `PROCESS` | only the account's own sessions |
| MySQL | `PROCESS`, `SELECT` on `performance_schema`, or the metadata lock instrument (`wait/lock/metadata/sql/mdl`) | an idle session as `idle ?`: whether it holds a transaction open is not known |
| MySQL | enough `performance_schema` thread slots (`performance_schema_max_thread_instances`) | that the list may miss sessions |
| MySQL | `SELECT` on `performance_schema` | the Locks view without who blocks a row lock wait, and without metadata lock waits |
| MySQL | the metadata lock instrument (`wait/lock/metadata/sql/mdl`) | the Locks view without metadata lock waits |
