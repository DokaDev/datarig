# Filtering and sorting a table

A table tab (opened from the explorer) reads its table with a `WHERE` condition, an `ORDER BY`
and a `LIMIT` that run on the server, over the whole table: the rows you get are the server's
answer. You build them in the filter dialog, or from the grid with a key on a cell or a column.
A query's rows have no such dialog: filter and sort them in the statement.

![Filtering a table by ticking a value in the filter dialog, keeping only the value of a cell, hiding a column, clearing the filter and bringing it back](https://dokadev.github.io/datarig/demo/table-filter.gif)

```text
{{#include ../screens/filter-dialog.txt}}
```

## The filter dialog

`f` (or `Space r f`) opens the dialog on the cursor's column; `Space r o` opens it on the order.
A click on a part of the [query bar](#the-query-bar-and-the-connection) (`⋯ Advanced`, a
condition, `ORDER BY`, the columns), `Space r A`, or "Filter and sort…" in a column's
[menu](#from-the-grid), opens it too. It covers the whole screen:

- the title names the table, says `edited` once what you changed differs from what is applied,
  and how many rows the tab holds;
- on the left, the table's columns;
- on the right, the selected column: its conditions, its values and its sort;
- at the bottom, the `Where` chips, the `Order`, the `Limit`, the `SQL` section and a line of
  hints for the part you are in.

`Tab` and `Shift+Tab` move from part to part. `Enter` applies what the dialog holds and runs the
table again; `Esc` closes it and drops your changes, without asking (an open list, a running
[`# count exactly`](#count-exactly), the search or the SQL section is closed or cancelled first,
and the dialog stays open). Nothing you change in the dialog is sent before `Enter`; only
[`# count exactly`](#count-exactly) asks the server something while it is open.

### Columns

The columns are listed in the table's order, as the catalogue has them, with their keys (`PK`,
`FK`, or a glyph with icons on), their type (when the window is wide enough), and a mark
when the column has a condition (`=`, a filter glyph with icons on) or a sort key (an arrow, with
its place once there are two keys or more). Until the catalogue has been read the list holds the
columns of the loaded rows and says `from the loaded result` (with the reason when reading it
failed). On MySQL, a column declared invisible says `not in SELECT *` in place of its type: it can
be filtered and sorted, but the loaded rows do not hold its values.

`↑` and `↓` (or `j` and `k`, `g` and `G`) select a column, `/` narrows the list by name, `s`
cycles the column's sort (up, down, none), `Space` or `C` [hides it](#hidden-columns) from the
grid or shows it again, and `→` goes to its conditions.

### Conditions

A column's conditions are rows of an operator and a value. `+ add condition` (`Space`, `→`, or a
click) adds one; `Space` on its operator opens the list of operators, `→` goes to its value, `x`
(or `Delete`) takes it out. A row without its value is `incomplete`: it makes no chip and is not
sent.

The operators, in the order listed: `=`, `between` (listed only for numbers, dates, times and
intervals), `≠`, `≠ or NULL`, `<`, `≤`, `>`, `≥`, `contains` and `starts with` (text), `in list`,
`not in list`, `is NULL`, `is not NULL`. Those the column cannot take are dimmed, with the reason:
once rows are loaded, a type without an equality datarig knows (`json`, MySQL's JSON, geometry), a
type that compares for equality only, patterns on a column that does not hold text or whose type is
not known before rows are loaded, and on PostgreSQL patterns on a collation the server refuses them
on (`ILIKE` on a nondeterministic collation, `LIKE` on one before PostgreSQL 18) or not known yet to
take them. On MySQL, `=`, `≠`, `≠ or NULL`, `in list` and `not in list` are dimmed on a `FLOAT`
column: "A MySQL FLOAT is held more precisely than it is shown: no value equals it exactly". `<`,
`≤`, `>`, `≥` and `between` still take it.

SQL's `=` and `<>` never match NULL, so the operators say what each does with it:

| Operator | Condition sent | Rows whose value is NULL |
|---|---|---|
| `=`, `<`, `≤`, `>`, `≥`, `between` | `col = value`, `col < value`, `col BETWEEN a AND b`, … | left out |
| `≠` | `col <> value` | left out |
| `≠ or NULL` | `col IS DISTINCT FROM value` on PostgreSQL, `NOT (col <=> value)` on MySQL | kept |
| `contains` | `col ILIKE '%value%' ESCAPE '!'` on PostgreSQL; on MySQL `col LIKE …` on a `_ci` collation, else `LOWER(col) LIKE LOWER(…)` | left out |
| `starts with` | `col LIKE 'value%' ESCAPE '!'` | left out |
| `in list` | `col IN (a, b)`, or `(col IN (a, b) OR col IS NULL)` with `NULL` in the list | found only with `NULL` in the list |
| `not in list` | `col NOT IN (a, b)`, or `(col NOT IN (a, b) AND col IS NOT NULL)` with `NULL` in the list | left out (kept by the `(col NOT IN (…) OR col IS NULL)` the grid's [`!`](#from-the-grid) writes) |
| `is NULL`, `is not NULL` | `col IS NULL`, `col IS NOT NULL` | found, left out |

How `contains` and `starts with` treat case depends on the server. On PostgreSQL, `contains` is
`ILIKE`, which ignores case, and `starts with` is `LIKE`, which follows the column's collation: case
counts on a deterministic collation (on PostgreSQL 18 and later `LIKE` is also written on a
nondeterministic one, which may ignore case). On MySQL and MariaDB both are `LIKE`, and the column's
collation decides: on a collation whose name ends in `_ci` case does not count, and accents count as
that collation says. `contains` on any other collation, or on a column whose collation datarig has
not read (yet), is written `LOWER(col) LIKE LOWER(…)`, so that it still ignores case; `starts with`
then follows the collation. The operator list shows the form of each one's SQL (`ILIKE '%…%'`,
`LIKE '%…%'`, `LIKE '…%'`); on MySQL `contains` shows `LIKE '%…%'` even where it is written with
`LOWER`.

In a pattern, `%`, `_` and `!` are escaped with `!`. A value is written as the column reads it:
bytes are typed in hexadecimal after `\x` or `0x`; text in single quotes is that text. On a number
column a plain number is sent as a number. On PostgreSQL anything else is sent as a string, for
the server to read or refuse, and on a `real` column a number is a string too (`'0.1'`): the
server reads it as a `real` and it compares exactly, where a bare `0.1` would be read as `double
precision`, and the `real` 0.1 is not that number. On MySQL a number is written as its exact
decimal, an exponent written out (`9007199254740993e0` as `9007199254740993`): MySQL compares a
number with an exponent, or one in quotes, as a double (a `FLOAT` takes the number as typed). Text
that is not a number is not written there, and its row says `not a number` (MySQL would read
`'abc'` as 0), nor is a number with no exact decimal within a `DECIMAL` (65 digits, 30 after the
point), whose row says `too many digits`.

In a list the values are separated by commas outside quotes; a bare `NULL` is SQL's NULL (in a
field of one value, `NULL` is the text `'NULL'`: use `is NULL`), and inside quotes `''` is a quote
and `\n`, `\r` and `\\` are a line feed, a carriage return and a backslash.

In a value, `←` at its start goes back to the operator (from the second value of `between`, to
the first), `↑` and `↓` to the row above or below
(`↓` into the values once there are some). Text pasted into a field becomes one line and is cut at
16 KiB.

### Values

Under the conditions, the column's values, each with how many rows hold it and a bar:

- They are counted in the rows the tab has loaded (`~ count in the 500 loaded rows`): an
  approximation of the table, not a count of it. With more than 20,000 rows in memory, the first
  20,000 are counted (`sample: 20,000/30,000`); rows written to disk are never read for it. NULL
  is listed as `NULL`. At most 200 values are listed, the commonest first (`more values than
  shown` when there are more); a value over 4 KiB is counted but not listed.
- A column of whole or decimal numbers whose values are mostly distinct (more than 80% of 50 or
more) shows its range instead, `12.50 .. 980.00   412 distinct`: use a condition (`between`, `≥`,
`≤`).
- Typing in the value of `=`, or the last value of `in list`, narrows the list
  (`containing “p”`).
- `Space` (or a click) ticks a value: it takes the place of what you typed, one value makes `=`,
  two or more `in list`, and a ticked `NULL` `is NULL` (with other values,
  `(col IN (…) OR col IS NULL)`). It goes to the condition you are typing, else to the column's
  first `=`, `in list` or `is NULL`, else a new `=` is added.
- Once the column has a condition of its own, the loaded rows hold only what it kept. The values
  this session saw in an earlier load of the table without a condition on that column come back
  under them, marked `earlier load`, so you can widen the condition. This memory stays in the
  session (at most 200 values per column, 64 columns per table, 1 MiB per table, 64 tables and
  4 MiB in all, the tables used longest ago left out first) and is forgotten when the explorer reads
  the schema again or a statement that changes the schema runs in datarig.

A value is ticked only as the server holds it, with the checks of the [grid's keys](#from-the-grid):
one not shown whole (a copy as SQL judges it), binary data over 64 KiB, a long value, one the
server writes otherwise than the grid, or one read as another type stays in the list, not
tickable, and says why.

A MySQL `TIMESTAMP` is shown in the session's time zone. A value ticked, or kept or excluded with
the grid's `=` and `!`, is written as the instant it was read as, worked out in UTC and converted
to the session's zone when the filter runs:
`CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)`. Every MySQL and MariaDB reads
it, and it is the same instant after the session's zone changes and when the filter runs again
from the [history](#history). A value typed with an offset (`2026-11-10 09:30:00+05:30`) is written
the same way. A value read before the session's zone changed is not taken, nor one read in a named
zone (such as `Europe/Berlin`, where a time shown in the hour the clocks go back is two instants):
only an offset such as `+09:00`, `UTC`, `GMT`, `Etc/UTC`, `Etc/GMT`, `Universal` or `Zulu` is
taken, and `SYSTEM` when the server's own zone is one of those. A typed value that is no
instant a `TIMESTAMP` holds (a date that does not exist, an offset no session has, a time outside
the type's range) says `no such instant` and is not written.

### Count exactly

`#` outside a text field (or a click on `# count exactly`, `# exact` in a narrow dialog) asks the
server how many rows hold each value of the
column. The count is of the whole table under the dialog's other conditions: the column's own
conditions are left out, an expression that names the column is kept (the list says so), and the
`Limit` does not apply. On PostgreSQL it sends:

```sql
SELECT CASE WHEN octet_length("status") <= 4096 THEN "status" END, octet_length("status"), count(*)
FROM "store"."orders" WHERE (total > 100) GROUP BY "status" ORDER BY count(*) DESC LIMIT 201
```

(on MySQL `IF(OCTET_LENGTH(c) <= 4096, c, NULL)` and `OCTET_LENGTH(c)`). The list then says
`exact · other conditions applied` (`exact · every row` when there are none) and shows the 200
commonest values; a value over 4 KiB is listed as `long value (N bytes)` and cannot be ticked.
When the other conditions change, or the session or the table does, the list says `earlier count`
and is not tickable until you count again; an answer to an earlier count is dropped.

The other conditions go through every [check](#what-is-refused-and-why) a run has, and a function
a run would ask about refuses the count instead (a count from the dialog asks nothing). It has the
limits of a [row count](#limits-and-outcomes-of-a-count): it stops after `[results]
count_timeout` (30 seconds unless set) and, unless that is `"off"`, gives up on a lock after 2
seconds (1 on MySQL). On
PostgreSQL it runs in a read-only transaction of its own; inside your own transaction, or while a
result is held open, it has no time limit and says so, and when the session cannot tell whether a
transaction is open it is refused. On MySQL it is refused while settings you gave for the next
transaction are waiting (no transaction open), and when its read would lock rows: a transaction is
open (or may be) and the isolation level is SERIALIZABLE or not known. With autocommit off and no
transaction open it reads in a read-only transaction of its own. `Esc` or `Ctrl+C` cancels it.
Stopped at the time limit, given up on a lock, cancelled or failed, it says so and never shows
counts.

`# count exactly` is dimmed for a column whose type has no equality datarig knows, or is not known
before rows are loaded. It is refused while the tab is running, and before the tab has a session
(run the table first).

### Sort

The last row of the right side sorts by the column: `(•) none  ( ) ↑ ascending  ( ) ↓ descending`
(`←` and `→`, or a click), with its place among the keys (`key 1 of 2`); `<` and `>` move the key
earlier or later in the order.

Once the column is sorted, the line also says where its NULLs go, when it fits: `NULLs last` or
`NULLs first`. The dialog leaves that to the server and writes no `NULLS FIRST` or `NULLS LAST` of
its own.
PostgreSQL puts NULLs last going up and first going down; MySQL and MariaDB put them first going
up and last going down. On PostgreSQL you can ask for another place in the
[`ORDER BY` field](#the-sql-section) (`total DESC NULLS LAST`), and the line says so; MySQL has no
such syntax, and it is refused there.

### Where, Order, Limit

The `Where` row shows the condition as chips, one per term of its top-level `AND`, in the order
they were added: `status = 'shipped'`, `total > 100`. Text the dialog cannot take apart (an `OR`
at the top, `XOR`, MySQL's `||`, `&&`, `|`, `&`, `!` or `:=`, a `NOT` that is not before a
parenthesised group or a comparison such as `NOT IN`, anything it cannot read) is one
`expr` chip, and so is a term that is not one condition on one column (`lower(name) = 'a'`). `←`
and `→` move along the chips, `Space` goes to the chip's column, `x` takes it out; chips that do
not fit are counted (`+2 more`). `undo` and `clear all` sit at its right.

What is sent is what the chips show. A condition you did not change in the dialog is sent byte for
byte as it was written. Once you add, change or remove a term, the condition is the terms' own
texts joined with ` AND `, each one you did not touch still as written, and a term with an `OR` of
its own in parentheses.

The `Order` row shows the keys (`total ↓`; `←` and `→` move along them, `<` and `>` move a key,
`x` takes it out, `Space` goes to its column's sort) and the key columns datarig adds after
yours, dimmed (`+ id`), or `order not unique` when the table has none that make it unique
(see [what runs](#what-runs-is-what-you-see)).

`Limit` caps the rows the tab reads: a whole number of rows, in digits; blank means no cap. `0`, a
number above 9,223,372,036,854,775,807 or anything that is not a number is refused when you
apply, the field underlined. A new limit runs the table again from its first page, and once the
rows reach it the query bar says `LIMIT 100` and the rows it holds. `Space r l` (or a click on
`LIMIT` in the query bar) types the limit in the bar itself: digits, `Enter` runs it, `Esc`
leaves it as it was, empty means no cap.

### The SQL section

`Q` (or `SQL ▸`) opens the statement as it would be sent, coloured, the key columns datarig adds
dimmed, above two fields, `WHERE` and `ORDER BY`, holding the dialog's condition and order:

```text
{{#include ../screens/filter-sql.txt}}
```

Type there for anything the chips do not build. What you typed is read back when you leave the
field, close the section (`Esc`, or `Q` outside its fields) or apply: the condition is taken apart
into chips again (an `OR` or a function makes an `expr` chip), and an order that is not the table's
columns is refused with the part underlined, the field keeping the keys. A field you did not type in
changes nothing. `Ctrl+Space` completes as `:where` and `:orderby` do: the table's columns, the
keywords of a condition, functions, and inside a literal the values of an enumerated type or of a
check, never values read from the table; in the order the columns, then `ASC` and `DESC` (and
`NULLS FIRST`, `NULLS LAST` on PostgreSQL). A list that opens by itself takes `Enter` and `Tab` only
once you move through it or ask for it with `Ctrl+Space`. A field is one line: text with a line
break inside a string, a quoted name or a comment is not read back once you type in it (undo the
typing, or open it in the editor).

`Open in editor` puts the dialog's statement, with its `LIMIT`, in a new console without running
it, after the checks of the condition and the limit a run has (a function datarig cannot check is
asked about first); as nothing runs, it asks nothing about locks or edits not saved yet. For
anything beyond this table's condition, order and limit, change it there.

### Undo and history

`Ctrl+Z` (or `u` outside a text field) undoes a step: each change in the dialog is one, typing in
a field until you leave it included. With no step left, it goes back once to what was applied
before the last change applied to the tab, from the dialog, a key on the grid, `:where` or any
other way: after `X` on the grid, `f`, `u`, `Enter` brings the filter back. Then it says "Nothing
to undo". `X` (or `clear all`) takes out every condition and the order and keeps the limit and the
hidden columns; nothing runs until `Enter`.

`Ctrl+R` lists this table's [earlier filters](#history); the one picked fills the condition and
the order (the limit stays) and runs only with `Enter`.

### Applying

`Enter` applies: the condition, order and limit run as any run of the tab does, with every check
and question (functions datarig cannot check, reads that lock rows on MySQL, edits not saved
yet). A refusal keeps the dialog open: the hint line says why and the refused chip, or part of a
field, is underlined. A question opens above the dialog, and a no keeps it with your changes. The
dialog closes once the statement is sent, and the query bar shows a spinner while it runs. A
change of the hidden columns alone runs nothing. While the tab is running, `Enter` is refused
(`Ctrl+C` cancels the run).

When the server rejects the condition, the query bar starts with `failed` and keeps the conditions
you asked for. The rows of the earlier query
stay, dimmed, and the line under the grid says they are not the filter's answer; a cancelled run
(`cancelled`) or one that brought no rows, such as on a lost connection (`unapplied`), leaves
them the same way. `f` opens the dialog on the request that failed, with the server's message in
its hint line and the chip where the server says the error is underlined.

The condition, order and limit you applied are kept with the workspace and with a closed tab you
bring back (`Space t u`): they return to the tab without running, and are
[checked again](#what-is-refused-and-why) when the tab runs.

## From the grid

On a table tab's grid:

- `=` (or `Space r =`) keeps only the cell's value, in place of the column's own conditions (the
  others stay as written). Cells selected in one column keep any of their values (`IN`), and a
  NULL cell keeps the rows whose value is NULL.
- `!` (or `Space r !`) keeps every row but that value, added to the conditions: it uses `≠ or
  NULL`, so the rows whose value is NULL stay, as "everything but this" reads.
- `X` (or `Space r X`) clears the conditions and the order, and keeps the limit and the hidden
  columns. With neither, it says so and runs nothing.
- `C` (or `Space r C`) [hides](#hidden-columns) the column.
- `s` and `S` [sort](#sort-from-the-grid).
- `F` (or `Space r F`, or a right click on a header) opens the column's menu.

| Cells | `=` sends | `!` sends |
|---|---|---|
| A value | `col = value` | `col IS DISTINCT FROM value` (MySQL `NOT (col <=> value)`) |
| A NULL | `col IS NULL` | `col IS NOT NULL` |
| Several values | `col IN (…)` | `(col NOT IN (…) OR col IS NULL)` |
| Several, NULL among them | `(col IN (…) OR col IS NULL)` | `(col NOT IN (…) AND col IS NOT NULL)` |

Each runs at once, with the checks and questions of the dialog's `Enter` (edits not saved yet
included), keeps the order and the limit of the rows shown, and can be undone in the dialog (`f`,
`u`, `Enter`). A value is written as the server reads it back, as in the dialog's
[conditions](#conditions): numbers bare (on PostgreSQL's `real` in quotes, on MySQL with any
exponent written out), bytes in hexadecimal, text with a backslash or a line break as an escape
string (`E'…'`) on PostgreSQL. On MySQL, text with a backslash, a NUL or a line break, or any text
when the session is not known to use `utf8mb4`, is written as its bytes, `_utf8mb4 X'…'`. Nothing
is taken while the tab is running or shows rows of an earlier query; and a value is refused, with
the reason, when it is not shown whole (as a copy as SQL judges it), is binary data over 64 KiB, is
of a type whose comparison datarig does not know (`json`, MySQL's JSON, geometry, most types of your
own), is in a new row or a cell with an edit on it, or when the cells are of several columns or more
than 1000. A column whose name holds a line break is neither filtered by value nor sorted from the
grid.

The column's menu (`F`) sorts it ascending or descending or takes it out of the order, opens the
dialog on it ("Filter and sort…"), keeps only or excludes the cursor's value (`=`, `!`), clears its
own conditions, or hides it. Its sorts say where the server puts NULLs in them ("Sort ascending,
NULLs last" on PostgreSQL, "NULLs first" on MySQL and MariaDB; see [Sort](#sort)). `j`, `k` and
the arrows move through it, `Enter` runs the selected entry and `Esc` closes it. An entry with a
key shown next to it runs with that key too: the grid's key for the same action, `s` for "Sort
ascending", `f`, `=`, `!` and `C` with the default keys. "Sort descending", "Clear sort" and
"Clear filter on this column" have no key of their own. An entry that cannot apply is dimmed, with
the reason in place of its key. A right click on a cell opens the grid's menu, whose entry
"Table → this column's menu (sort, filter, hide)" opens it.

```text
{{#include ../screens/column-menu.txt}}
```

The headers of the columns with a condition of their own, or in the order, are tinted. A header
also says how the rows shown are sorted: an arrow after the name (`^` or `v` with icons off), with
the key's level from the second key on (`v2`); the primary key columns added after yours are not
marked. A column the condition compares is marked `=` (a filter glyph with icons on), as far as
datarig reads the condition: a condition it does not read marks nothing, which does not mean it
compares no column.

### Sort from the grid

`s` (or `Space r s`) sorts by the cursor's column alone: going up, then down, then no order at all,
the next step following the column's direction in the order (on a column that is a later key
going down, it takes the whole order away). `S` (or `Space r S`) adds the column as one more key,
after the others; pressed again on a key it turns that key, then takes it out, and the other keys
stay where they are. A click on the last two columns of the first row of a column's header (the
cursor's column shows `^v` there, a glyph with icons on) sorts as `s` does; a click elsewhere on a
header selects the column, as on any grid.

```text
{{#include ../screens/table-sort.txt}}
```

A sort keeps the condition and the limit of the rows shown and runs at once. It does not sort while
the tab runs or shows rows of an earlier query (apply the filter again first); over edits not saved
yet it asks first, as any run does.

## Hidden columns

`C` on the grid, "Hide column" in the column's menu, or `Space` or `C` on a column in the dialog
hides a column from the grid; `Space` or `C` in the dialog shows it again, and the dialog's undo
takes a hide back. Hiding is a view only: the statement still reads every column, and nothing runs.

- The cursor, `h`, `l`, `0`, `$`, selections and a click step over hidden columns, and `/`
  [searches](results.md#search) the columns shown.
- A copy takes the cells shown; a copy as SQL `UPDATE` or `INSERT` keeps the table's key columns.
  A paste and a fill write the columns shown, and a paste waiting while the columns shown change is
  cancelled.
- An [export](export.md), the chart, the inspector and edits see every column; the export dialog
  says that hidden columns are included. Going to a change in a hidden column from the Changes tab
  (`Enter`, or an edit key) shows the column again and says so; the dialog's undo hides it again.
- The query bar says how many columns are shown (`6/7 cols`), and in the dialog the column
  is struck through (`hidden from the grid, still read`, followed by `(row key)` for a key column).
- With every column hidden, the grid says how to show one again, and no key reads or changes a
  cell until then.

Hidden columns are kept per table (its connection, database, schema and name) across restarts, in
the file `table-view.toml` of the state directory, readable by you only (mode 0600 on Unix), never
in the workspace file. A name the table no longer has is left out once the table's catalogue has
been read. When the file cannot be read, hidden columns are kept for the session only and it says
so once; datarig never writes over that file. The file keeps the last 500 tables and up to 1000
names per table.

## The query bar and the connection

A table tab's results pane has a query bar on its first line. It says what is applied, in the
order of the statement:

```text
WHERE [status = 'shipped' ×] [total > 1000 ×] + │ ORDER BY ↓ total + │ LIMIT 500 │ 6/7 cols │ 1–500 of ~50,001 rows ‹ ›  ⋯ Advanced
```

Each condition is a chip with the text that is sent; its `×` takes it out and runs the others as
they were written. A condition datarig does not read as one column's is in the warning colour.
`6/7 cols` says columns are hidden. The rows are those of the result shown (`~` an estimate,
`500+` while not known), with the page arrows. In a narrower window the bar says less in steps
(`⋯` alone, shorter rows, `+1` for the conditions and keys after the first, `2 conditions`,
`ORDER ↓2`), down to marks only (`▼2 ↓2 L ◌1 ⋯`); what is applied always keeps a mark. A spinner
goes first while the tab's query runs, and `failed`, `cancelled`, `unapplied` or `refused` when
the last run brought no rows or was refused.

Keys: `Space r w` puts the keys on the conditions (`←`/`→` move, `Enter` opens one in the dialog,
`x` takes it out and runs, `+` adds one, `Esc` goes back to the grid), `Space r l` types the
limit, `Space r c` opens the dialog on the columns, `Space r r` counts the rows (as `#`), and
`Space r A` opens the dialog. A click on a part does the same.

On a table tab the result strip (the result tabs and the connection,
`local-pg / datarig.shop.orders  127.0.0.1:55432`, shorter as the window narrows) is on the
results pane's top border.

## What runs is what you see

The statement of a table tab is built from what was applied and nothing else:

```sql
SELECT * FROM "store"."orders" WHERE (status = 'shipped') ORDER BY "total" DESC, "id" ASC
```

The condition is sent as the dialog shows it, inside those parentheses. `Space r v` opens the
statement of the rows shown in a new console, and `Ctrl+G` hands it to your own editor.
`Space r x` clears the condition, the order and the limit and runs the table again. From the
command line, `:where` sets the condition and `:orderby` the order, each alone meaning none, and
each runs the table; `:filter clear` does what `Space r x` does.

When you set an order, the columns of the table's primary key not already in it are added after
it, ascending (shown dimmed as `+ id`), so that pages read again follow one order; a table without
a primary key takes its shortest unique key whose columns are all `NOT NULL`. While the order is not
known to be unique, the dialog says "order not unique": the table's keys and columns are read from
the catalogue when the tab opens, without locking the table, and a table may have no key that makes
it unique. An ordered query brought back with a tab waits for those keys before its first run,
connecting first when needed; that wait counts as a running query (`Ctrl+C` cancels it), and a
connection that fails ends it and says so.

The limit is not written into the statement the grid runs: the pages stop at it instead: the last
page is cut there, no page is fetched after it, and what the server held open for the result is
closed. A count (`#`), an export of every row and the statement `Space r v` opens have it written
(`… LIMIT 100`), so they see the same rows the pages would.

## What is refused, and why

Before anything is sent, datarig checks the condition. The refused part is underlined when it can
be pointed to, the dialog says why, and nothing reaches the server.

| Refused | Why |
|---|---|
| A comment, in any form | It can hide text from what you see. On MySQL `/*! */` is also text the server runs. |
| `;`, MySQL's `\g` and `DELIMITER` | A filter is one condition, and these would end the statement or speak to a client. |
| A parenthesis left open, or closing one the condition did not open; a string or quoted name left open | The text would not stay inside the parentheses it is put in. |
| A backslash in a string the server may read another way | What would run depends on a server setting. |
| A row lock (`FOR UPDATE`, `FOR SHARE`), also in a subquery | It would hold up other sessions. |
| Another clause of the statement outside the condition's own parentheses (`ORDER BY`, `LIMIT`, `GROUP BY`, `HAVING`, `WINDOW`, `UNION` and the like; a subquery may have its own), `INTO` anywhere, MySQL's `:=` | Only a condition is taken; what the statement reads stays this table's rows. |
| Built-ins that wait, lock, change the session or a sequence, read, list or write the server's files or large objects, or reach another server (`pg_sleep`, `pg_advisory_lock`, `set_config`, `nextval`, `dblink`, `pg_read_file`, `lo_import`, `lo_get`, MySQL's `SLEEP`, `GET_LOCK`, `LOAD_FILE`), also in attribute notation (`(id::oid).lo_get`); MariaDB's `NEXT VALUE FOR s` and `s.nextval` | A filter runs for every row and must not disturb anything, on any connection. |
| MySQL's `ROW_COUNT()` and `FOUND_ROWS()` | They read the outcome of the statement before, which for a table tab is one the app sends. |

An order is refused unless it is the table's columns, each once; a limit, as
[above](#where-order-limit).

### Functions datarig cannot check

A list of known built-ins per server that read only their arguments (`lower`, `length`, `now`
and the like; `date_trunc` and `to_char` on PostgreSQL, also written with `pg_catalog.`) passes
without a word. Any other call asks first ("This filter calls functions datarig cannot check",
`y` runs, `Enter`, `n` or `Esc` cancels), once per text in a tab: it runs on the server for every
row, and it may write or call out.

On a read-only connection such a condition is refused instead: a function may do on the server
what a read-only transaction does not stop.

The checks run again on every statement built from a condition: a page read again, a count of the
rows or of a column's values, an estimate, an export, a condition from the history and one brought
back with a tab. As for the editor, some things are not seen: a function of yours called in
attribute notation (`t.f`), an operator of your own, and code a view or a policy runs. MariaDB's
reads of a sequence's last value (`LASTVAL(s)`, `PREVIOUS VALUE FOR s`, `s.currval`) ask as such a
call does.

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
tab has run with that answer; a count of a column's values is [refused](#count-exactly) outright.

While a transaction is open, the level that counts is the one it started with: a
`SET SESSION TRANSACTION ISOLATION LEVEL` inside it applies to later transactions only. A level
set with `SET TRANSACTION ISOLATION LEVEL` without `SESSION` applies to the next transaction only
and the server does not report it, so datarig treats the level as not known until that
transaction ends; after a `CALL`, an `EXECUTE` or an executable comment it stays not known until
the session's level is set again.

### Known limits

- A MySQL `TIMESTAMP` condition runs in whatever time zone the session has when it runs. Run again
  from the history, or typed with an offset, in a session zone with daylight saving, a value in
  the hour that repeats when the clocks go back may match the other instant of that hour: filter
  that hour by a range instead.
- On MySQL and MariaDB, a condition with an `INTERVAL` outside parentheses stays one `expr` chip:
  the interval's value runs up to its unit and may hold an `AND` of its own
  (`d + INTERVAL 1 AND 1 DAY`). Put the interval's term in parentheses to keep the others apart.
- A right click on a cell opens the grid's menu, not the column's: its entry "Table → this
  column's menu (sort, filter, hide)" (`F`) opens that one. A right click on a header opens the
  column's menu at once.

## History

A condition and order that ran are kept per table (its connection, database, schema and name)
once the server answered them with rows: a run that was refused, failed or was cancelled is not
kept. `Ctrl+R` in the filter dialog (or `Space r H` from the grid) lists the table's earlier ones,
the most recent first, with both texts under the list (up to six lines); typing narrows the list. A
picked entry fills the dialog's condition and order (opening the dialog from the grid; the limit
stays as it is: the history does not keep limits) and runs only when you press `Enter`, with the
same checks as a condition you type. A condition or order holding a line break (from a workspace
file edited by hand, say) is not kept, and one in the file is not filled in, since the dialog's
one-line fields cannot hold it as it is. Each table keeps its last 50 entries and the last 200
tables used are kept; a condition or order longer than 4 KiB is not kept. The tables used longest
ago are left out to keep the file under 1 MiB.

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

Filtered rows show `1,001–1,500 of 1,500+ rows` in the query bar: nothing is counted by
itself. `#` opens a
small choice on a filtered table tab whose rows are not all fetched (on any other result it counts
at once, and the query bar of an unfiltered table shows the rows the server's statistics
suggest, `~5,900 rows`, until then):

- **Estimate (planner)** asks the server's planner how many rows the filter matches, without
  reading any (`EXPLAIN`, never `EXPLAIN ANALYZE`). The query bar then reads `~5,900 rows`: an
  estimate, which can be far off, not a count. An estimate of no rows, or of
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
transaction is open, a count is refused (with a limit or without). On MySQL the limit is part of the count
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
the filter dialog.
