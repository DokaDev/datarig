# MySQL

**Versions.** MySQL 8.0, 8.4 and 9.x. CI runs the driver's tests against MySQL 8.4. MySQL 5.7
and older are refused with their version. MariaDB 10.6 and newer is a best effort: it connects
and runs statements, and is tested only lightly.

A MySQL profile has `driver = "mysql"`, and a URL starts with `mysql://` (or `mariadb://`).

## What works

- **Connecting** directly or through an [SSH tunnel](ssh-tunnels.md) or tunnel preset (the
  driver talks over the tunnel's channel; no local port is opened), with
  `caching_sha2_password` and `mysql_native_password`, and the test connection.
- **Running statements**, one per request: datarig splits scripts itself, `DELIMITER`
  included. The client never asks the server to run several statements from one request, and
  `LOAD DATA LOCAL` is off. The editor and the risk checks read the text in the session's sql
  mode (`ANSI_QUOTES`, `NO_BACKSLASH_ESCAPES`).
- **Values** as the server writes them: `DECIMAL` exact, unsigned integers in full, `BIT` as
  `b'…'`, binary values as `0x…`, `JSON`, dates and times with their fractions, `TIME` as an
  interval (negative, or over 24 hours), zero dates (`0000-00-00`) as text, `ENUM`/`SET` as
  text, `GEOMETRY` as bytes.
- **Messages**: rows affected, the last insert id, warnings and the server's info line; errors
  as the `mysql` client shows them (`ERROR 1146 (42S02): …`).
- **Transactions** as the server reports them after each statement. A statement that commits
  your open transaction by itself (DDL and others) is noted in Messages, and with manual commit
  asks first ([Manual commit](manual-commit.md)).
- **The explorer**: the server's databases right under the profile (a MySQL database is the
  schema), their tables and views with the server's estimates, and a table's structure
  (columns, keys, indexes, constraints, triggers); completion; `:use db` and the database
  picker (`Space c d`).
- The row count (`#`), key marks, run hints, [charts](charts.md), [export](export.md),
  [plans](plans.md) (MySQL 8.0.16 and later), and copying rows in every format.

### Cancel

`Ctrl+C` sends `KILL QUERY` from a connection of its own, never `KILL`. A `SLEEP()` that is
stopped returns 1, as in the `mysql` client. A cancel the server takes is waited for, however
long the statement then takes to roll back.

A statement that runs as another account (in a `SQL SECURITY DEFINER` routine; stopping it
needs `CONNECTION_ADMIN`) refuses the cancel. If it sends no rows it runs on to its end whatever
datarig does, and the tab says so and keeps waiting (ask a DBA or the routine's owner to `KILL`
it). datarig closes the tab's connection instead of reading on, and the next run connects again,
when:

- the server refuses the cancel and the statement keeps sending rows;
- the cancel cannot reach the server (its connection does not open);
- the server takes the cancel and the statement still sends rows more than 10 seconds later.

Whether the statement then stops is the server's: it may run on, and outside a transaction
block commit what it changes. An open transaction is rolled back once the server ends the
session.

### Show DDL

`D` on a table, view or trigger (or its menu), or `:ddl [kind] name` (`name` or `db.name`, in
the tab's database; `kind` is `table`, `view`, `procedure`, `function`, `trigger` or `event`,
needed only when one name names several), opens the server's own `SHOW CREATE` text.

- MySQL has no DDL of an index of its own: `D` on an index row shows its table's.
- A trigger's, routine's or event's text is wrapped in delimiter statements
  (`DELIMITER ;;` … `;;` / `DELIMITER ;`), so it runs again as one statement.
- An object made under another `sql_mode` than the connection's default starts with a
  `-- sql_mode: …` line: its body is read in that mode.
- The server writes the strings of a table's or view's text with backslash escapes whatever
  the mode, and says so in a first line when there is a backslash. `o` opens the text in a
  console in the object's database with every string written as that console's `sql_mode`
  reads it, so it makes the same object there; the DDL tab keeps the server's text.
- The `DEFINER` is kept, so running it again as another user needs `SET_USER_ID` (MySQL 8.0)
  or `SET_ANY_DEFINER` (8.2 and later). A routine you may run but not see
  says its definition is hidden.
- Reading waits at most 2 s on another session: an object whose table is locked shows
  "locked", and `r` reads it again.

### Copy as SQL

`INSERT` and `UPDATE` use backtick names and MySQL's literals (bytes `X'…'`, bits `b'…'`), and
leave generated columns out. A string with a backslash or a NUL is written as its UTF-8 bytes
(`_utf8mb4 X'…'`), so the statements read the same in any sql mode; a `TIMESTAMP` is written
with the offset of the session's time zone. `UPDATE` needs the table's primary key in the
result.

A copy is refused, naming the column, where a value would not be written whole: a `FLOAT` (the
server sends it rounded to about six digits), a `TIMESTAMP` while the session's time zone is not
a fixed offset (`SET time_zone = '+00:00'` first), text while the session's results are not
`utf8mb4`, and an `AUTO_INCREMENT` value of 0. A whole result is refused when it was read while
a procedure, an `EXECUTE` or an executable comment ran, before the session said its values, or
when a `SET time_zone` or `SET NAMES` ran while it was read: run the statement again.

The keys this needs are read for every database when the connection opens (and again after DDL
run in datarig), so a table created elsewhere later needs `r` on the connection in the explorer
before its rows can be copied as SQL. Column names compare without case, and database and table
names too when `lower_case_table_names` is 1 or 2. A temporary table that shadows a table of the
same name is not told apart from it.

## Paging and locks

As on PostgreSQL, nothing is held open while you read a result by default. The query session
keeps `sql_select_limit` at a page and one row, so the first page is read to the end of the
statement and its metadata lock is released; the next page runs the statement again and skips
the rows you have (only for a statement that is safe to repeat; one that names a view is checked
on the server first). `SHOW` and `DESCRIBE` are read whole.

With `paging = "hold"`, and inside your own transaction, a result is read whole into datarig's
local spill (up to its limit) and the server is free at once. So is a locking read
(`FOR UPDATE`, `FOR SHARE`, `LOCK IN SHARE MODE`), which under the page limit would lock fewer
rows than it asks for.

The explorer and completion read in autocommit with `lock_wait_timeout = 2` and a 10-second
`max_execution_time`: behind a waiting `ALTER TABLE` they give up after two seconds instead of
queueing behind it. A read that opens a table's definition (`SHOW CREATE TABLE`) takes a
high-priority shared metadata lock for the moment it reads, which MySQL grants past a waiting
`ALTER TABLE`.

## Read-only profiles

Two layers, as on PostgreSQL. datarig refuses everything that is not a read before it is sent,
including `CALL`, executable comments (`/*! … */`), functions that are not built in, and what
MySQL's own read-only mode still allows (`FOR SHARE`, `LOCK TABLES`, `GET_LOCK()`,
`SET GLOBAL`, …). And the session runs with `transaction_read_only = ON`, checked after every
statement (a session that loses it is closed).

Unlike PostgreSQL, MySQL lets a statement turn its read-only off, so the server's layer depends
on datarig's: for a guarantee that does not, use an account with `SELECT` privileges only.
Views, triggers, generated columns and `DEFAULT` expressions are not inspected; the server
still refuses their writes.

## Passwords on a direct connection

A MySQL profile uses TLS when the server offers it (`PREFERRED`; see
[TLS](tls.md)). A `caching_sha2_password` login the server has not cached (the first one,
or after a restart) needs the password itself:

- **Over TLS** it goes inside TLS, but to another machine only when the server's certificate
  was checked (`VERIFY_CA`, `VERIFY_IDENTITY`), or the connection goes through an SSH tunnel, or
  the profile allows key retrieval (below). Over `PREFERRED` or `REQUIRED` without a CA file the
  certificate is not checked, so the login fails with an explanation before anything about the
  password is sent.
- **Unencrypted** (a server without TLS, or `DISABLED`), the password goes encrypted with the
  server's RSA public key. Asked for over the same plain connection, that key could come from
  anyone in between, so on a direct connection to another machine datarig does not ask for it.
  Name the server's key file in the profile (`server_public_key_file`, the PEM of
  `SHOW STATUS LIKE 'Caching_sha2_password_rsa_public_key'`), or allow asking for it
  (`allow_public_key_retrieval = true`). A profile that names a key file and no TLS mode stays
  unencrypted (`DISABLED`).

A server on your own machine, or one reached through an SSH tunnel, gets the password either
way. A `mysql_native_password` login over plain TCP can be cracked offline from a capture, and
so can a weak password from any captured login exchange: use `VERIFY_IDENTITY` (or an SSH
tunnel) for any server that is not on your own machine.

`ERROR 1045` may also mean the account requires TLS (`REQUIRE SSL`) on a connection without it.
After a typed password is refused, datarig shows the error instead of asking again.

## What does not work yet

- The plan view on MariaDB: the plan actions are not offered there, and `EXPLAIN` runs and
  shows its text. MariaDB as a driver of its own is planned.
- A `USE` typed in a console is not followed by the tab's header and completion; use `:use`.
- The estimates are the server's (`TABLE_ROWS`), which can be far off for InnoDB.
- A view whose table was dropped is listed without columns, and a database that does not exist
  or that you may not read shows as empty.
- On a server with tens of thousands of tables, the explorer's catalog and key reads may hit
  their 10-second limit; copy as SQL then says the keys could not be read.
- Check constraints need MySQL 8.0.16 or later. A user with grants on some columns only sees
  those columns. Triggers are listed only with the `TRIGGER` privilege (without it the group
  says they are not known). `lower_case_table_names = 2` (macOS, Windows) is not tested.
