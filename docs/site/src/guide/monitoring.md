# Server monitor

The Monitor tab shows what a connection's server says about its load and its sessions:
PostgreSQL 12 or later and MySQL 8.0.17 or later, self-managed or a managed service. It reads,
and acts on the server only when you ask it to: `c` cancels a session's statement and `X` ends
a session (see [Cancelling and ending sessions](#cancelling-and-ending-sessions)).

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
  shows on which table. The blocking transaction's session is the one that has it now (from
  `INNODB_TRX`, which needs `PROCESS`): a prepared XA transaction whose session went on to
  other work is shown as a transaction with no session, and without `PROCESS` only the
  transaction is named. For
  metadata locks (an `ALTER TABLE` waiting for a long transaction, `FLUSH TABLES WITH READ
  LOCK`) it does not: the tree shows the sessions holding a lock on the same object that the
  request cannot be granted beside, as the server's lock manager decides it, and says that
  they may block it. With more than one such holder, `b` does not pick one. A request waiting
  behind another request rather than a granted lock is shown waiting, its holder not found.
  Both lock tables are read only while the Locks view is shown, at most every 5 seconds, and
  only when a session may be waiting for a lock.
- A wait in a cycle (a deadlock the server has not broken yet) is shown once, from its
  smallest session, and marked.
## Replication

`4` shows what the server says about its replication. It is read when the view is shown, with
`r`, and then at most every 10 seconds (the interval when it is longer) while the view stays
shown, on a read of its own: when it fails, the view says why and keeps the last good one, and
the other views read on. `j`/`k` scroll.

- **Role**: a replica (a PostgreSQL standby in recovery, a MySQL server with replication
  channels or a secondary of its replication group), a PostgreSQL logical subscriber, a primary (servers take changes from it
  and it takes none, or its group's primary), or not replicating. When the account may not
  read what would tell, the view says it cannot tell, never "not replicating".
- **What it takes changes from**: on a PostgreSQL standby, where its WAL receiver streams from
  and through which slot, whether replay runs or is paused, the log positions received and
  replayed, and when the source was last heard from. Without a WAL receiver the standby replays
  only what it has, or reads from an archive, and the view says so. On a MySQL replica, each
  channel's source, whether its receiver (I/O thread) and applier (SQL thread) run and what
  they are doing, the last heartbeat, the last transaction fetched and applied, the GTID sets
  received and executed, a delay set on purpose, and the last error of each side.
- **The servers that take changes from it**: a PostgreSQL primary lists its WAL senders: the
  standby's name and address, its state and sync state, how much it has not replayed yet, the
  write, flush and replay lags while they are measured, and its last reply. A MySQL source
  lists the replicas connected to it (their address, account and what the sending thread
  does); their lag is read on each replica.
- **Slots** (PostgreSQL): each replication slot, whether its consumer is connected, how much log
  it keeps, and its `wal_status` (from 13). A slot whose consumer is away keeps log that
  grows, and is marked.
- **Subscriptions** (PostgreSQL logical replication): each subscription of the server, whether
  its apply worker runs, the position it received and when the publisher was last heard from.
- **Group replication** (MySQL): the members of the group, their state and role.

How far behind is said only as far as the server can tell:

- PostgreSQL: a standby that has replayed all it received says so; else how many bytes it has
  received and not replayed yet. The age of the last replayed transaction is shown apart, on
  the primary's clock, because it grows on a primary with nothing to write too: it is never
  given as the lag.
- MySQL: `Seconds_Behind_Source` only measures the applier against what the receiver fetched,
  so 0 can hide a receiver that is behind or stalled. The view shows the receiver's state and
  its last heartbeat next to it (a source sends one only while it has nothing else to send),
  and says when the receiver does not run. A channel has applied all it received only where the
  receiver's and the applier's places in the source's binary log are the same (shown as
  "source log read to ... applied to ..."): `Seconds_Behind_Source` is 0 also while events of
  the same second wait. Where every transaction carries a GTID (`gtid_mode` `ON`) it counts the
  transactions received and not applied yet from the GTID sets, which never prove that nothing
  is left (a channel may take transactions without GTIDs); a NULL is never shown as 0.

On Amazon Aurora the cluster's own replicas share its storage and are not listed by these
views; the view says so.

## The server in full

`i` shows what the server is, over any view: its whole version line, its provider (read from
the server, never guessed from its host name), whether it takes writes, when it started, its
connections now and its limit, the size of its page cache, and the settings that decide what
the monitor can read (PostgreSQL `track_activities`, `track_counts`,
`track_activity_query_size`, `track_io_timing` and `wal_level`; MySQL `performance_schema`,
`server_id`, `gtid_mode`, `log_bin` and `super_read_only`). A setting that leaves the monitor
less to read is marked with what it hides (`track_activities` and `track_counts` are read as the monitor's own connection has them: another session's may differ). `i` or `Esc` closes it.

## Cancelling and ending sessions

In the Sessions view, `c` cancels the selected session's statement (the session stays) and `X`
ends the session (its open transaction is rolled back). The action menu (`Space Space`, a right
click) lists both, dimmed with the reason when they cannot be sent.

- A cancel on PostgreSQL asks one line on the status line: only `y` sends it, any other key
  sends nothing. Ending a session, and a MySQL cancel of anything but a plain read, opens a
  dialog that names the session, its client, state, transaction and statement, and what the
  action can leave behind. `Enter` keeps the session as it is; only `y` sends.
- Both are refused on a read-only environment, and never sent to the monitor's own connection.
- When the session is one of your own tabs, the question names the tab; the cancel is the same
  guarded cancel, and the tab shows its statement cancelled by the server. The name is a hint
  from the session id the tab's connection reported, checked against the row's user, database
  and application name; where the server does not list the application, as MySQL, the tab is
  not named. Ending your own
  tab's session closes that tab's session, and the dialog says so.
- Nothing is sent to a session other than the one you acted on. The action carries the session
  and its statement as the row showed them, and is sent only if they are still there:
  PostgreSQL checks the session's start and the statement's start; MySQL checks the statement's
  event in `performance_schema`, or, without it, the statement's text and times (a weak check:
  the same text may be a new run of it, and the dialog says so). An action asked for before the
  monitor reconnected is dropped.

Sent is not done. The answer says what the server said: refused (with the privilege it needs),
nothing sent (the session ended, another session took its id, it runs another statement),
sent, or sent with its answer lost. A sent action is then confirmed by reads of the session 1, 3
and 10 seconds after the answer, whether the tab is shown or not. A session ended is confirmed
when a read no longer finds it. A statement is called cancelled only when the server says so
(MySQL error 1317 for that statement); otherwise the read says the statement ended after the
cancel was sent, since it may have finished on its own.

On managed services:

| Server | Used when `KILL` is refused | To act on other accounts' sessions |
|---|---|---|
| PostgreSQL | none | `pg_signal_backend` |
| Amazon RDS and Aurora PostgreSQL | none | `rds_superuser` or `pg_signal_backend` |
| Google Cloud SQL for PostgreSQL | none | `cloudsqlsuperuser` or `pg_signal_backend` |
| Google AlloyDB | none | `alloydbsuperuser` or `pg_signal_backend` |
| Azure Database for PostgreSQL | none | `azure_pg_admin` or `pg_signal_backend` |
| MySQL | none | `CONNECTION_ADMIN` |
| Amazon RDS and Aurora MySQL | `mysql.rds_kill_query`, `mysql.rds_kill` | `CONNECTION_ADMIN` |
| Azure Database for MySQL | `mysql.az_kill_query`, `mysql.az_kill` | `CONNECTION_ADMIN` |

A session of the provider's own service accounts is refused, and said so. On PostgreSQL a
session of your own role can be signalled without these, and a superuser's session only by a
superuser.

Behind a pooler, a server session can serve any client: ending one is refused unless it runs a
statement, and the dialog warns that it may serve another client by the time it ends.

`L` shows the action list under the sessions: when each action was sent, what it was, the
session and its statement, the server's answer and what the reads after it showed. It is kept
for the tab's life and never written to disk.

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
| MySQL | 8.0.38 or later in 8.0, or 8.4 or later | the Locks view without who blocks a row lock wait (on older versions reading it can stall a busy server, so it is not read) |
| PostgreSQL | `pg_read_all_stats` | WAL senders and the WAL receiver without their state and positions (a standby's received and replayed positions still show) |
| MySQL | `REPLICATION CLIENT` | that whether the server takes changes from another is not known |
| MySQL | `PROCESS` | that the servers taking changes from it are not known (only the account's own sessions are listed) |
| MySQL | `SELECT` on `performance_schema` | that each channel's last heartbeat and last transaction, and a group's members, are not known |
