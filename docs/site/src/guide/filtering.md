# Filtering and sorting a table

A table tab (opened from the explorer) has a line above its grid with a `WHERE` condition, an
`ORDER BY` and a `LIMIT`. They run on the server, over the whole table, and the rows you get are
the server's answer. A query's rows have no such line: filter and sort them in the statement.

![Filtering a table in the line above its grid, sorting it by two columns from the grid, then filtering it by the value of a cell](https://dokadev.github.io/datarig/demo/table-filter.gif)

```text
{{#include ../screens/table-sort.txt}}
```

## The line and its keys

`f` (or `Space r f`) edits the condition and `Space r o` the order; `Tab` and `Shift+Tab` move
between the three fields. In a field:

- `Enter` applies what the three fields hold and runs the table again; the keys go back to the
  grid, or stay on the field that was refused. `Ctrl+E` does the same.
- `Esc` puts back what was applied and goes back to the grid (with a list open, the first `Esc`
  closes the list).
- `Ctrl+Space` completes. In the condition: the table's columns, the keywords of a condition,
  functions, and inside a literal the values of an enumerated type or of a check, never values read
  from the table. In the order: the columns, then `ASC` and `DESC` (and `NULLS FIRST`, `NULLS LAST`
  on PostgreSQL). A list that opens by itself while you type does not take `Enter` or `Tab`: once
  you move through it (arrows, `Ctrl+N`, `Ctrl+P`) or ask for it with `Ctrl+Space`, they pick its
  entry. `Ctrl+E` always applies.
- `Ctrl+R` lists the [earlier filters](#history) of this table.
- `Ctrl+L` empties the field (applied with `Enter`), `Ctrl+W` deletes a word and `Ctrl+U` the
  text before the cursor. Text pasted into a field is cut at 16 KiB.

With the mouse, a click on a field puts the cursor there and a click on the mark at the end of
the line runs what is applied again.

`Space r x` clears the condition, the order and the limit and runs the table again. From the
command line, `:where` sets the condition and `:orderby` the order, each alone meaning none, and
each runs the table; `:filter clear` does what `Space r x` does.

The condition is SQL as it goes after `WHERE`. The order is column names, each optionally followed
by `ASC` or `DESC` (`ASC` when left out; and `NULLS FIRST` or `NULLS LAST` on PostgreSQL), and is
shown in its normal form, the direction always written, once applied.

`LIMIT` caps the rows the tab reads (`-` when there is none; a narrow line shows it as `L100`, or
leaves it out when there is none).
It takes a whole number of rows, written in digits, and a blank field means no cap; `0`, a
number above 9,223,372,036,854,775,807 or anything that is not a number is refused before
anything is sent. A new limit runs the table
again from its first page, and once the rows reach it the results title says `limit 100`.

The condition, order and limit you applied are kept with the workspace and with a closed tab you
bring back (`Space t u`): they return to the line without running, and are
[checked again](#what-is-refused-and-why) when the tab runs.

## What runs is what you see

The statement of a table tab is built from the line and nothing else:

```sql
SELECT * FROM "store"."orders" WHERE (status = 'shipped') ORDER BY "total" DESC, "id" ASC
```

The condition is sent exactly as you typed it, inside those parentheses. `Space r v` opens the
statement of the rows shown in a new console, and `Ctrl+G` hands it to your own editor.

When the table has a primary key and you set an order, those of its columns not already in your
order are added after it, ascending (shown dimmed as `+ id`), so that pages read again follow one
order. While the order is not known to be unique, the line says "order not unique": the table's keys
are read from the catalog when the tab opens, without locking the table, and a table may have none
that make it unique. An ordered query brought back with a tab waits for those keys before its first
run, connecting first when needed; that wait counts as a running query (`Ctrl+C` cancels it), and a
connection that fails ends it and says so.

The cap is not written into the statement the grid runs: the pages stop at it instead: the last
page is cut there, no page is fetched after it, and what the server held open for the result is
closed. A count (`#`), an export of every row and the statement `Space r v` opens have it
written (`… LIMIT 100`), so they see the same rows the pages would.

When the server rejects the condition, the line marks it failed, underlined where the server
says the error is. The rows of the earlier query stay, dimmed, and the line under the grid says
they are not the filter's answer; a cancelled run or a lost connection leaves them the same way.

## Sort from the grid

`s` (or `Space r s`) sorts by the cursor's column alone: the first press sorts going up, the
second going down, the third takes the order away. `S` (or `Space r S`) adds the column as one
more key, after the others; pressed again on a key it turns that key, then takes it out, and the
other keys stay where they are. A click on the last two columns of the first row of a column's
header (the cursor's column shows `^v` there, a glyph with icons on) sorts as `s` does; a click
elsewhere on a header selects the column, as on any grid.

A sort runs the new order like `Enter` does and writes it into the line once it is sent: it
starts from the order of the rows shown and keeps their condition and limit. While the line
holds changes you have not applied, in any field, it does not sort: apply them with `Enter` or
put them back with `Esc` first; nor while the line shows a query that failed or was cancelled
over the earlier rows. Over edits not saved yet, it asks first, as any run does.

The headers say how the rows shown are sorted: an arrow after the name (`^` or `v` with icons
off), with the key's level from the second key on (`v2`); the primary key columns added after
yours are not marked. A column the condition compares is marked `=` (a filter glyph with icons
on), as far as datarig reads the condition: a condition it does not read marks nothing, which
does not mean it compares no column.

## Filter by value

`F` (or `Space r F`, or "Table → filter by value" in the grid's right-click menu) filters by the
value of the cell under the cursor. The menu offers `=`, `<>`, `<`, `>`, `<=` and `>=`, and for
text also:

- "starts with" (`LIKE 'value%'`, its `%`, `_` and `!` escaped with `!`; on PostgreSQL before 18,
  which refuses `LIKE` on a nondeterministic collation, offered once datarig has read that the
  column's collation is deterministic);
- "contains (case-insensitive)" (`ILIKE` on PostgreSQL, offered once datarig has read that the
  column's collation is deterministic, which it does with the tab's runs; on MySQL `LIKE`, which
  a `_ci` collation compares without case, else `LOWER(…) LIKE LOWER(…)`).

With cells of one column selected (up to 1000), it offers `IN` and `NOT IN` of their values. An
equality on a floating-point column may find nothing, and how text compares follows the column's
collation on the server: the menu says so next to those entries.

A MySQL `TIMESTAMP` is shown in the session's time zone. Its value is written as the instant it
was read as, in the session's zone when the filter runs:
`CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)`. That is the same instant on
every MySQL and MariaDB, after the session's zone changes and when the filter runs again from the
history. A value read before the session's zone changed is not taken.

On MySQL a number compared with a number column is written as its exact decimal, an exponent
written out (`9007199254740993e0` as `9007199254740993`): MySQL compares a number with an exponent,
or one in quotes, as a double. A number with no exact decimal within a `DECIMAL` (65 digits, 30
after the point) is not written.

The condition is added to the filter of the rows shown with `AND`; a filter whose top level is
not a plain chain of `AND`s (an `OR`, for one) is put in parentheses first. The new text goes
into the filter line and runs as `Enter` runs it there, with the same checks and questions,
pending edits included. While the line holds changes you have not applied, nothing is added:
apply them or put them back with `Esc` first. Nor while the line shows a condition that failed
or was cancelled over the earlier rows: run it again, or change it, first. Nor while the tab is
running.

### NULL

SQL's `=` and `<>` never match NULL, so the menu writes what each entry means:

| Cells | Entry | Condition | Rows whose value is NULL |
|---|---|---|---|
| A NULL | `IS NULL`, `IS NOT NULL` | as written, never `= NULL` | found, left out |
| A value | `<>` | `col <> value` | left out (the menu says so) |
| A value | `<>` (NULLs included) | `col IS DISTINCT FROM value` on PostgreSQL, `NOT (col <=> value)` on MySQL | kept |
| Several, no NULL | `NOT IN` | `col NOT IN (…)` | left out (the menu says so) |
| Several, no NULL | `NOT IN` (NULLs included) | `(col NOT IN (…) OR col IS NULL)` | kept |
| Several, one NULL | `IN` | `(col IN (…) OR col IS NULL)` | found |
| Several, one NULL | `NOT IN` | `(col NOT IN (…) AND col IS NOT NULL)` | left out |

### When there is no entry

A value is taken only as the server holds it: an entry is dimmed, with the reason, for a value that
is not shown whole (as a copy as SQL judges it), binary data over 64 KiB, a type whose comparison
datarig does not know (`json`, MySQL's JSON, geometry, most types of your own (not an enum), arrays
of a type without equality such as `xml[]`; PostgreSQL's `jsonb` and arrays of built-in types with
equality offer equality only), or `IN` and `NOT IN` of more than 1000 cells. A new row, a cell with
an edit on it or cells of several columns give no menu. A column whose name holds a line break is
neither filtered by value nor sorted from the grid: the filter line holds one line, and would not
hold the name as it is.

## What is refused, and why

Before anything is sent, datarig checks the condition. The refused
part is underlined when it can be pointed to; the status line says why, and the line under the
grid repeats it in full while it stands. Nothing reaches the server.

| Refused | Why |
|---|---|
| A comment, in any form | It can hide text from the one line you see. On MySQL `/*! */` is also text the server runs. |
| `;`, MySQL's `\g` and `DELIMITER` | A filter is one condition, and these would end the statement or speak to a client. |
| A parenthesis left open, or closing one the condition did not open; a string or quoted name left open | The text would not stay inside the parentheses it is put in. |
| A backslash in a string the server may read another way | What would run depends on a server setting. |
| A row lock (`FOR UPDATE`, `FOR SHARE`), also in a subquery | It would hold up other sessions. |
| Another clause of the statement outside the condition's own parentheses (`ORDER BY`, `LIMIT`, `GROUP BY`, `HAVING`, `WINDOW`, `UNION` and the like; a subquery may have its own), `INTO` anywhere, MySQL's `:=` | Only a condition is taken; what the statement reads stays this table's rows. |
| Built-ins that wait, lock, change the session or a sequence, read the server's files or reach another server (`pg_sleep`, `pg_advisory_lock`, `set_config`, `nextval`, `dblink`, MySQL's `SLEEP`, `GET_LOCK`, `LOAD_FILE`), also in attribute notation (`(id::oid).lo_get`); MariaDB's `NEXT VALUE FOR s` and `s.nextval` | A filter runs for every row and must not disturb anything. |
| MySQL's `ROW_COUNT()` and `FOUND_ROWS()` | They read the outcome of the statement before, which for a table tab is one the app sends. |

An order is refused unless it is the table's columns, each once; a limit, as
[above](#the-line-and-its-keys).

### Functions datarig cannot check

A list of known built-ins per server that read only their arguments (`lower`, `length`, `now`
and the like; `date_trunc` and `to_char` on PostgreSQL, also written with `pg_catalog.`) passes
without a word. Any other call asks first ("This filter
calls functions datarig cannot check", `y` runs, `Enter`, `n` or `Esc` cancels), once per text in
a tab: it runs on the server for every row, and it may write or call out.

On a read-only connection such a condition is refused instead: a function may do on the server
what a read-only transaction does not stop.

The checks run again on every statement built from a condition: a page read again, a count, an
estimate, an export, a condition from the history and one brought back with a tab. As for the
editor, some things are not seen: a function of yours called in attribute notation (`t.f`), an
operator of your own, and code a view or a policy runs. MariaDB's reads of a sequence's last
value (`LASTVAL(s)`, `PREVIOUS VALUE FOR s`, `s.currval`) ask as such a call does.

### Reads that lock rows on MySQL

On MySQL, a plain read locks the rows it reads when the session's isolation level is SERIALIZABLE
and autocommit is off or a transaction is open (with manual commit, the one datarig opens for the
read counts too): other sessions cannot change those rows until the transaction ends. In that state,
and with autocommit off or a transaction open when the level is not known, a table tab asks before
it runs (`Enter`, `n` or `Esc` cancels, `y` runs), on the session the read is sent on: a read that
waits for its connection, or opens a new session, asks once it is about to be sent, and nothing is
sent before the answer. The answer holds for the tab until the session, its level, its autocommit or
its transaction changes, even if it later changes back (a transaction the answered run itself opens
or ends does not count). A page read again or a count of the tab is refused in that state until the
tab has run with that answer.

While a transaction is open, the level that counts is the one it started with: a
`SET SESSION TRANSACTION ISOLATION LEVEL` inside it applies to later transactions only. A level
set with `SET TRANSACTION ISOLATION LEVEL` without `SESSION` applies to the next transaction only
and the server does not report it, so datarig treats the level as not known until that
transaction ends; after a `CALL`, an `EXECUTE` or an executable comment it stays not known until
the session's level is set again.

## History

A condition and order that ran are kept per table (its connection, database, schema and name)
once the server answered them with rows: a run that was refused, failed or was cancelled is not
kept. `Ctrl+R` in the filter line (or `Space r H` from the grid) lists the table's earlier ones,
the most recent first, with both texts in full under the list; typing narrows the list. A picked
entry fills the condition and the order (the `LIMIT` field stays as it is: the history does not
keep limits) and runs only when you press `Enter`, with the same checks as a condition you type.
A condition or order holding a line break (from a workspace file edited by hand, say) is not
kept, and one in the file is not filled in, since the one-line field cannot hold it as it is.
Each table keeps its last 50 entries and the last 200 tables used are kept; a condition or order
longer than 4 KiB is not kept. The tables used longest ago are left out to keep the file under
1 MiB.

The history stays on this machine: it is the file `filter-history.toml` in the state directory,
readable by you only (mode 0600 on Unix), and never part of the workspace file. A condition can
hold personal data, such as an email address:

- `:set history.filters=off` (or `[history] filters = "off"` in the config file) stops keeping
  new ones; it does not delete the file.
- `:filter history clear` forgets this table's (asked first: `Enter` cancels, `y` clears).
- `:filter history clear all` forgets every table's.

When the file cannot be read (a file over 4 MiB, or a condition or order in it longer than
4 KiB, counts as that too),
or something else than a plain file is at its path (datarig never follows a symbolic link there),
the history is off for the session and says so once; datarig never writes over it, and a clear
is refused naming the file, for you to fix or delete. A second datarig running at the same time
reads the history but does not write it.

## Estimate or exact count

Filtered rows show `page 3 / ?` in the results title (`page 3 / ? · # count` where there is
room): nothing is counted by itself. `#` opens a
small choice on a filtered table tab (on any other result it counts at once, and the title of an
unfiltered table shows the pages the server's statistics suggest, `~12`, until then):

- **Estimate (planner)** asks the server's planner how many rows the filter matches, without
  reading any (`EXPLAIN`, never `EXPLAIN ANALYZE`). The title then reads `page 3 / ~12 (planner)`,
  about 12 pages: an estimate, which can be far off, not a count. An estimate of no rows, or of
  fewer than the tab holds, is not shown.
- **Exact count** counts the rows.

### When there is no estimate

The entry is dimmed, with the reason:

- when the filter calls a function outside the built-ins a filter may call freely, confirmed or
  not (planning may run a function);
- on MySQL, when the filter holds a subquery (MySQL may run one while it plans);
- while a transaction is open on the tab's session or a result is held open there (the locks
  planning takes would last until it ends);
- on MariaDB, and on MySQL before 8.0.16, which give no estimate;
- on PostgreSQL, when planning the table could run a function. The server is asked in the same
  read-only transaction as the plan: a table whose indexes, constraints, statistics, partition
  key or generated columns use something not built in, one with a column whose type is not one
  of the server's standard types (a domain, an enum or a composite type included), one with a
  rule or row-level security, or one that cannot be locked (a materialised view) gets no
  estimate, and the entry is then dimmed for those rows.

The server stops an estimate after 5 seconds and gives up on a lock after 2 seconds (1 second on
MySQL). On MySQL the statement runs in the session's usual transaction mode, in a read-only
transaction of its own only when autocommit is off.

### Limits and outcomes of a count

The server stops a count statement after 30 seconds (`[results] count_timeout` in the
[configuration](../reference/configuration.md), `"off"` for no limit). It also gives up on a single
lock another session holds on the table after waiting 2 seconds (1 second on MySQL), unless the
setting is `"off"`. These are the server's limits on the statement and on each lock wait, not on the
whole request: the time to reach the server, or to get a connection from a pooler, comes on top.
Neither is ever longer than one your session already has (PostgreSQL's `statement_timeout` and
`lock_timeout`, MySQL's `max_execution_time`; MySQL's 1 second lock wait is the shortest
`lock_wait_timeout` it takes). MariaDB's are `max_statement_time` and `lock_wait_timeout`.

On PostgreSQL the limit is set only while no transaction is open on the tab's session: inside your
own transaction, or while a result's cursor is held open, the count keeps to that transaction's
settings and has no limit (the status line says so); when the session cannot tell whether a
transaction is open, a count with a limit is refused. On MySQL the limit is part of the count
statement itself (also inside a transaction).

The outcomes are kept apart, and none of them ever reads as 0 rows:

| Outcome | The status line says |
|---|---|
| Counted | the number of rows, and whether it matches the pages you saw |
| Stopped at the time limit | stopped after that time, the number of rows is not known |
| Gave up on a lock | it waited too long for a lock another session holds, the number of rows is not known |
| You cancelled (`Ctrl+C`) | count cancelled |
| Stopped by the server for any other reason, or the connection lost | the server's own words, or the connection's error |

## Search is not a filter

`/` [searches](results.md#search) the rows the tab already holds. It sends nothing to the server
and filters nothing: on a filtered table tab the line under the grid says "searching held rows",
and rows the server has not sent yet are not looked at. To find rows anywhere in the table, use
the condition.
