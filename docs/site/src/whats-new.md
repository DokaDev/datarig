# What's new

What changed in each release, newest first. For 0.36.0 and earlier, see the release notes on
[GitHub Releases](https://github.com/DokaDev/datarig/releases).

## 0.41.4 (2026-11-18)

A patch release that reads MySQL and MariaDB function calls the way the server does on read-only profiles and in the table filter, and fixes how datarig lists and reads tables on MariaDB.

### Fixed

- On MariaDB an invisible column that is also generated, auto-numbered or set on update is now
  known as such: editing rows, and copying or exporting them as SQL, treat it as generated or
  auto-numbered as they do a visible one, and a table tab's filter and sort lists show it as
  invisible.
- On MariaDB a statement that is only a MySQL 5.7 or later executable comment (such as
  `/*!80000 SET TRANSACTION ... */`), which the server skips, no longer stops counts, exports and
  the check before a re-run until a transaction starts or ends.
- On MariaDB the checks datarig runs on its own before a count, a re-run, an export or an
  `EXPLAIN` of a change now wait at most 2 seconds behind another session's lock and stop after
  10 seconds, and each chunk of a table export read in chunks waits at most 5 seconds for a lock,
  instead of waiting without limit. A shorter limit the session already has is kept.
- On MariaDB a sequence is listed with the tables of its database, not with the views.
- Opening a table tab on MariaDB no longer opens the other tables of the server to read the
  table's keys.
- On MySQL and MariaDB, a call of a built-in function that the server reads as the built-in only
  when its name is directly followed by `(` (such as `NOW`, `COUNT` or `TRIM`) now counts as a
  call of the database's own function when a blank or a comment comes before the `(` (`now ()`),
  and so does a call of a built-in with its name in backticks (such as `now` or `coalesce`), as
  the server reads them: a read-only profile refuses it and the table filter asks about it, as
  for any function that is not a built-in. When the session's `sql_mode` has `IGNORE_SPACE` (as
  `ANSI` does), blanks before the `(` are fine and the call stays the built-in; a comment there
  still makes it the database's own.
- On MariaDB, a call of `lateral()` or `of()`, and a call of `JSON_TABLE` without its `COLUMNS`
  clause or with its name quoted, now count as calls of the database's own functions, as MariaDB
  reads them.
- On MariaDB, a `JSON_TABLE` call counts as the table function only with its real `COLUMNS`
  clause (the path's string, then `COLUMNS (`): a call with a column or a call named `columns` in
  its arguments (`JSON_TABLE(t.columns)`) now counts as a call of the database's own function, as
  MariaDB reads it.
- A column type with a precision in `JSON_TABLE`'s `COLUMNS` (`DATETIME(6)`, `NCHAR(10)`) is no
  longer taken for a function call, so such a query is a plain read on a read-only profile.
- On MySQL and MariaDB, text with a `$` is now checked both as a server that reads dollar quotes
  reads it and as one that does not, and the stricter reading counts, since the version a server
  reports (for example through a proxy) may not tell which it is.
- On MySQL, a `SET_VAR` optimiser hint that names its setting in backticks or quotes (such as
  `sql_mode` in backticks) now always asks before it runs, whatever the setting, since datarig
  does not read such a name.
- On MySQL, `STRING_TO_VECTOR`, `VECTOR_DIM` and `VECTOR_TO_STRING`, which only MySQL 9.0 and
  later have, now count as calls of the database's own functions on every server: a read-only
  profile refuses them.

## 0.41.3 (2026-11-17)

A patch release that lifts the refusal of extension functions on read-only PostgreSQL profiles that 0.41.2 brought in: an environment can now allow chosen functions with `read_only_functions` and every function of chosen extensions with `read_only_extensions`, for example `read_only_extensions = ["postgis"]` in its `[policy.<name>]` table. Read-only profiles also accept a cast written as a call with a quoted value (`uuid('…')`) and the PostgreSQL 18 built-ins when the call names `pg_catalog`.

### Added

- A read-only PostgreSQL environment can allow functions that are not built-ins with
  `read_only_functions` (such as `["st_x", "public.crypt"]`: a bare name allows that function
  in any schema, a name with its schema only calls written with that schema) and every function
  of chosen extensions with `read_only_extensions = ["postgis"]`, both set in the
  environment's `[policy.<name>]` table. An extension's function stays refused when a function
  outside the allowed extensions has the same name. A refused call says why, and names
  `read_only_functions` where adding the function would allow it; `:session` lists what is
  allowed. MySQL and MariaDB profiles ignore both settings.
- The functions of the extensions in `read_only_extensions` are read from the profile's own
  database when it connects. While they are being read, when the read failed, and in a tab
  working in another database, a call that only an extension would allow is refused with the
  reason. A refresh of the profile in the explorer reads them again after a failure.

### Fixed

- A read-only PostgreSQL profile no longer refuses a cast written as a call to a built-in type
  with one quoted value, such as `uuid('…')` or `pg_catalog.jsonb('{}')`. With anything else
  inside (a column, an expression) it is still a function call.
- A read-only PostgreSQL profile now runs the built-ins PostgreSQL 18 adds when the call
  names `pg_catalog`, such as `pg_catalog.uuidv7()`. Called without it (`uuidv7()`), they are
  refused with a hint saying how to call them, unless `read_only_functions` or an allowed
  extension lists them. The PostgreSQL 18 functions that write or clear statistics (such as
  `pg_restore_relation_stats`) and `pg_ls_summariesdir()`, which lists server files, stay
  refused however they are called.

## 0.41.2 (2026-11-17)

A patch release that makes several checks stricter, so read the list before upgrading. Read-only PostgreSQL profiles now refuse calls of functions that are not PostgreSQL built-ins, which includes extension functions such as PostGIS (`ST_*`) and pgcrypto; until a per-environment allow list ships in the next patch, use a profile that is not read-only for those queries. On MySQL and MariaDB, applying edits fails on any server warning and is undone where the table allows, `TIME` values are checked, the first-page row limit applies only to plain reads, counts and exports are refused inside your own SERIALIZABLE transaction, and a cancel is sent only when the server confirms the session (otherwise the tab's connection is closed). On PostgreSQL, a read-only profile also refuses typed settings of extensions, and a query holding a backslash or `U&` is not run again automatically. Startup SQL refuses settings that ask before they run, so a profile holding one stops connecting until it is removed.

### Changed

- Startup SQL now refuses any setting that asks before it runs when typed, whatever the value,
  since Startup SQL runs without asking: on MySQL and MariaDB for example `foreign_key_checks`,
  `unique_checks`, `sql_safe_updates`, `sql_log_bin`, `binlog_format` or `SET NAMES` of a
  character set other than UTF-8, on PostgreSQL `session_replication_role`. A profile whose
  Startup SQL (or preset) holds one no longer connects until it is removed; the profile form
  and `:session` say which statement. MySQL `sql_mode` stays allowed in Startup SQL.
- On a read-only PostgreSQL profile a typed setting of an extension or a custom one (such as
  `SET app.tenant = '1'`, `RESET pgaudit.log` or `set_config('app.tenant', …)`) is refused,
  and so are `RESET ALL` and `DISCARD ALL`; set such a setting in the profile's Startup SQL,
  where it is still allowed.
- Typed `SET session_replication_role`, `set_config()` of it and a `set_config()` whose
  setting is not named with a constant ask before they run (on a read-only profile they are
  refused).
- Behind a transaction pooler, a PostgreSQL profile whose Startup SQL holds `SET ROLE` (or
  `SET SESSION AUTHORIZATION`) no longer connects, since the role could not hold for the
  session; before, it connected as the login role. Connect directly, or log in as that role.

### Fixed

- MySQL and MariaDB: applying edits now fails when the server answers any step with a warning
  or a note, whatever the server's mode, and what it changed is undone; the message shows the
  server's warnings (up to five). Before, a server without strict mode could store a value it
  had changed (text cut to the column's length, a character the column cannot hold, a date
  made zero, a member of a SET dropped) and the Apply reported success. While the session's
  `sql_notes` is off (or cannot be read), edits are not sent, since the server would then not
  report a value it rounded: turn it on with `SET SESSION sql_notes = 1`. When undoing a failed
  change leaves some of it in place (a table that does not take part in transactions, such as
  MyISAM), the message says so instead of saying nothing was applied, and applying the same
  edits again asks first. Each Apply reads `sql_notes` first, one more round trip.
- MySQL and MariaDB: a value typed for a `TIME` column is checked before it is applied: `H:MM`
  or `H:MM:SS` (such as `12:30` or `8:3:2`; hours up to three digits, a leading `-` allowed)
  with at most as many digits of a second as the column keeps (the server would round the
  others away without a warning), or either of them with days first (`1 10:00:00`,
  `1 10:00`); further digits are accepted only when they are zero. Other forms, such as
  `123000` or `12:30.5`, are refused.
- MySQL and MariaDB: a cancel of a running statement is now sent only once the server confirms
  that it reaches the tab's own session (each datarig connection carries a `datarig_session`
  connection attribute, read back from `performance_schema.session_account_connect_attrs`).
  When that cannot be confirmed, datarig closes the tab's connection instead and says so; the
  statement may then still finish on the server and, outside a transaction block, commit what
  it changed, while an open transaction is rolled back. On MariaDB with `performance_schema`
  off (its default) this is always the case, and it can be through proxies and load balancers
  that share or reroute connections. A cancel that arrives before the statement is sent stops
  it and keeps the connection. Opening a MySQL session takes one more round trip.
- MySQL and MariaDB: a row count or an export inside your own open transaction is refused when
  the transaction is at SERIALIZABLE or at a level datarig cannot tell (for example after
  `SET TRANSACTION ISOLATION LEVEL …` for the next transaction, or after a stored procedure
  ran). Before, it read every row with a shared lock that other sessions' writes waited for
  until the transaction ended. End the transaction, or use another isolation level.
- MySQL Monitor: cancelling or ending a session is now sent only on a monitor connection the
  server confirms is direct, and only to a session that can be told apart from a later one given
  the same id. This needs the session's thread to be readable: grant the monitoring account
  `SELECT` on `performance_schema.threads`, then reopen the Monitor tab; otherwise nothing is sent
  and the Monitor says so. When the connection cannot be confirmed (for example with
  `performance_schema` off, or through a proxy or load balancer), the actions are shown dimmed
  and not sent. The check before the action also compares the session's account.
- MySQL and MariaDB: the row limit used to show the first page now applies only to plain reads.
  A last statement that is not one, such as `SELECT … INTO OUTFILE`, `SELECT … INTO @variable`,
  a `SELECT` that calls a stored or loadable function or a statement with an executable
  comment (`/*! … */`), now runs to its end, and its result is read whole into datarig's local
  spill, as a locking read already was. Before,
  the limit stopped it after a page of rows, so a file was written in part or a function ran
  for those rows only, while the run reported success. Your own `sql_select_limit` is kept.
- MySQL and MariaDB: a read-only profile on a read replica that does not keep a session's
  read-only setting (some cloud reader endpoints, such as some AWS Aurora readers) connects
  again. datarig's own metadata and monitor sessions run there without it, as they only send
  datarig's own reads, so the explorer and the Monitor work; `:session` shows this while they
  are open, and `errors.log` records the values the server gave. Query tabs of such a profile
  are still refused, before any Startup SQL runs, with "could not talk to the server (details
  in errors.log)"; the line in `errors.log` names the values the server gave. The FAQ now says
  where `errors.log` is.
- PostgreSQL: a read-only profile now refuses, before sending it, a statement that calls a
  function that is not one of PostgreSQL's own built-ins, including functions of extensions
  (such as PostGIS or pgcrypto) and any function named with a schema other than `pg_catalog`;
  `EXPLAIN` of such a call and `EXECUTE` of a statement prepared with one are refused too. A
  read-only MySQL profile already refused such calls. The refusal now names the function, on
  PostgreSQL and MySQL alike. A function reached through a view, a trigger, an operator, a cast
  or attribute notation (`t.f`) is not checked by this rule, and the rule reads a name as
  written, so an unqualified name that matches a built-in's passes it; the read-only
  transaction then stops only what it would write in the session's own database. Until a
  per-environment allow list ships in the next patch, run queries that need such functions
  (for example PostGIS `ST_*` or pgcrypto) from a profile that is not read-only. The same
  applies for now to some built-in calls the rule does not know: a type cast written as a
  function call (such as `pg_catalog.uuid(…)` or `jsonb(…)`) and functions newer than
  PostgreSQL 17 (such as `uuidv7()`).
- PostgreSQL: copied and exported SQL, export chunks and DDL text write a string holding a
  backslash as `E'…'`, so it reads the same whatever `standard_conforming_strings` is. A query
  whose text holds a backslash anywhere (even in `E'…'` or a comment) or `U&`, which that
  setting may read differently, is not run again by datarig: a next page that would run it
  again, its row count and its value counts are refused (page it with `LIMIT` and `OFFSET`),
  and an export of the whole result asks first. Pages read from a result still held open are
  not affected. This includes a table tab whose filter holds a backslash. Startup SQL
  refuses such a statement. The safety checks read a national string (`N'…'`) with that
  setting off as the server does.
- On PostgreSQL, the explorer, completion, key, table structure, sort key and DDL reads now name
  every operator, function and type with its schema, so an operator of another schema on the
  session's `search_path` that matches their argument types more closely is no longer used in
  place of the server's own.
- On PostgreSQL, the statements datarig writes around a result or a table filter to count rows
  or count each value of a column now name their functions and operators with their schema
  (`pg_catalog.count(*)`), so they do the same whatever the session's `search_path` holds. The
  count shown in the run log reads `pg_catalog.count(*)` accordingly.

## 0.41.1 (2026-11-16)

Table tabs get a query bar with small popups for the conditions, the order, the columns and the rows. On PostgreSQL, date, time and interval text typed or pasted into the grid is now applied only in a form that reads the same under any `DateStyle`, `TimeZone` and `IntervalStyle` (ISO dates, timestamps with an explicit offset or `Z`, ISO 8601 intervals) and is otherwise refused with the reason; columns of composite types and of multiranges the database defines take no typed or pasted values from the grid for now (only NULL or DEFAULT); and a cancel the server does not confirm now shows a warning in Messages. MariaDB, now tested in CI, gets a series of fixes.

### Added

- A table tab has a query bar on the first line of its results: `WHERE` with each condition as a
  chip of the text that is sent, `ORDER BY`, `LIMIT`, how many columns are shown, and the rows of
  the result with its page arrows. A chip's `×` takes that condition out and runs the others as
  they were written. A click on a part opens a small popup under it: a condition opens its
  column's conditions, values and sort; `WHERE` and `+` the list of columns of a new condition;
  the count of conditions left out of the bar the list of them all; `ORDER BY` the order's keys;
  the columns those shown and hidden; the rows their pages and their counts. `Enter` applies,
  `Esc` or a click outside closes without applying, and `A` or `⋯ Advanced` turns the popup into
  Advanced, the whole filter dialog, keeping what you changed. `Space r w` puts the keys on the
  conditions (`x` takes one out), `Space r o` opens the order popup, `Space r l` types the limit
  in the bar, `Space r c` opens the columns popup, `Space r r` the rows popup and `Space r A`
  Advanced. The bar says less as the window narrows but always marks what is applied, and it
  says when the last run failed, was cancelled or was refused.

### Changed

- Advanced, the filter dialog (`⋯ Advanced` on the query bar, `A` in its popups, `f` on the
  grid), opens centred over the screen, the screen dimmed around it (on an 80 x 24 terminal it
  still takes the whole screen).
- `Q` (or `SQL ▸`) in Advanced shows the SQL in its right pane, in place of the column's
  conditions: first the statement as it is sent, read only; again, the `WHERE` and `ORDER BY`
  texts to edit; again, it closes. The footer keeps its three rows; the condition chips are
  hidden while the SQL is shown.
- The action menus (right click, `Space Space`) list their entries in groups with a rule between
  them, under short names without the area in front ("View value", "Export…", "Next result"
  in place of "Results: view cell value", "Results → export to a file…", "Result tabs → next").
  Typing still finds an entry by its long name too, which the key help and the command line
  keep. The cell menu's column entry is "Column…" (`F`).
- On a table tab a click on a column's header (its name or its type) opens that column's popup
  under the header: its conditions, values and sort. Whole columns are selected by dragging from
  the header; the sort zone at the header's right still sorts.
- `F` and a right click on a header open the same popup in place of the column menu, with "Hide
  column" and, on a cell, "Only" and "Exclude" the cell's value above it.
- On a table tab `Space r o` opens the order in a small popup under the query bar, as a click on
  the bar's `ORDER BY` does: its keys first, then the other columns (`Space` turns a key's
  direction or adds the column, `J`/`K` move a key, `x` takes it out), the clause that is sent
  underneath. `A` there opens Advanced on the order, as `Space r o` did before.
- On a table tab the result tabs and the connection are on the results pane's top border, and
  the filter and sort no longer show on its bottom border (the query bar says them); the grid
  shows as many rows as before.
- `Space r g` shows the chart (was `Space r c`, which now opens a table's columns in a popup);
  `c` in the grid is unchanged.

### Fixed

- Saving an environment under a name that profiles already use while no environment has it (a
  rename, a copy, a new one, a preset picked in a profile's form, or the presets) now asks
  first, naming those profiles and the rules they would take, instead of giving them its rules
  silently.
- A lock wait error no longer names the planned limit once you set the lock wait limit yourself
  in that tab (`SET lock_timeout`, `SET innodb_lock_wait_timeout`, a `RESET ALL`, a procedure).
- MariaDB: `EXPLAIN ANALYZE` of a write no longer fails with "Unknown system variable
  'partial_revokes'" from datarig's own check before it; MariaDB now answers the statement
  itself (it has no `EXPLAIN ANALYZE`; its `ANALYZE` statement is the equivalent).
- MariaDB: column attributes it lists after a comma are read. A column with `ON UPDATE
  CURRENT_TIMESTAMP` that is also `INVISIBLE` no longer gets a stray comma in its default (a
  table's definition built from its structure ran into a syntax error), and an invisible
  generated column (a system-versioned table's row start and end, too) shows as generated
  instead of as a plain column.
- MariaDB: asking for a statement's plan right after a session connected no longer sends MySQL's
  `EXPLAIN FORMAT=TREE` (which MariaDB refuses); the session says it has no plan format before
  it says it is connected.
- MariaDB: the read check knows which functions the server provides. A call of a function
  that MySQL has but MariaDB does not (`ANY_VALUE`, `REGEXP_LIKE`, `BIN_TO_UUID`, ...) names a
  stored function of the current database on MariaDB, so it is now treated as any unchecked
  function call there: a read-only profile refuses it, and a query that calls one is not run
  again for the next page. MySQL is unchanged, except for Startup SQL, which is checked before
  the server is known: there a call of one of these functions now counts as an unchecked
  function call on MySQL too, so such a Startup SQL is refused.
- MariaDB: reading a table's structure no longer opens every table of the server to find that
  table's foreign keys, which was slow on servers with many tables and could wait behind another
  table's lock.
- MariaDB: system-versioned tables are tables again. The explorer lists them with the tables
  (not with the views) and their estimates, and their table tabs read past the first page,
  count, filter and export as other tables do; a `FOR SYSTEM_TIME` read of one pages too.
- The Monitor tab on a MariaDB server now says the monitor does not read MariaDB, instead of
  calling the server an unsupported MySQL that needs MySQL 8.0.17 or newer.
- MySQL: for a user granted only some columns of a table, rows that look the same are no longer
  offered as identical ("just one" or all of them), since they may differ in columns the server
  does not list to that user; the change is refused, saying why, and the table's Columns group
  says some may not be listed.
- MySQL: a cancel that lands while the server looks up the definer of a `SQL SECURITY DEFINER`
  routine now ends the run as cancelled, instead of with error 1449 (the definer does not exist).
- MySQL: when a cancel sent again for a statement got no answer, a result whose rows were held
  for paging was cut after its first page and the run ended as a lost connection. Its rows and
  its end are now read as you page, and the session closes after that.
- MySQL: "just one" of identical rows is no longer applied while the session's binary log keeps
  statements (`binlog_format = STATEMENT`), where a replica could change a different one of the
  rows; the Apply sends nothing and says why. `ROW` and `MIXED` work as before.
- PostgreSQL: cancelling an Apply inside an open transaction no longer aborts the transaction
  when the cancel arrives before the Apply's savepoint is set; the Apply stops before its first
  statement and the transaction keeps what it held.
- PostgreSQL: the DDL shown for a view runs as written on PostgreSQL 17.12 and later. It no longer
  sets column options on the view's columns, which the server refuses, nor writes out options the
  server keeps for itself on any relation's columns.
- PostgreSQL: a statement that cannot run inside a transaction (`VACUUM`, `CREATE DATABASE`, …)
  and failed in one the app opened is blamed on the tab's schema only when the schema is what
  needed the transaction; otherwise the message says the app opened one for it.
- PostgreSQL: values the server sends as text and that hold dates, times or intervals (a range,
  a multirange, an array or a domain over one) are no longer pasted or applied as other dates
  when the session's `DateStyle`, `IntervalStyle` or `TimeZone` differs from the one they were
  read in (`[02/01/2026,05/01/2026)` copied in `SQL, DMY` and applied in `ISO, MDY` stored
  1 February for 2 January). `g p` into such a column is refused while the result was read in
  other settings than the session's now, and a copy of values still pending counts as of
  settings not known.
- PostgreSQL: text given to a column of dates, times or intervals, typed, pasted or cloned, is
  applied only in a form read the same in any `DateStyle`, `TimeZone` and `IntervalStyle` (ISO
  dates, times and timestamps with a zone written with an explicit offset or `Z`, ISO 8601
  intervals such as `P1DT2H`), since a trigger may change them while the changes are applied;
  other text is refused, saying which form to use.
- PostgreSQL: columns of a composite type, a pseudo-type or a multirange type the database
  defines take no typed, pasted or cloned text from the grid for now (NULL and DEFAULT still
  can be set), since datarig cannot tell the dates in them.
  Base types of extensions that hold no dates (`citext`, `hstore`, `ltree`, geometry) stay
  editable once the server has said what kind of type they are, which it is asked when their
  result comes.
- PostgreSQL: an `interval` copied in the grid or cloned with its row is written in ISO 8601
  from its exact value and applied, and a NULL pasted into a date column is never refused for
  the session's settings.
- PostgreSQL: cancelling a row count, a count of a column's values, a statement run again to
  page on, an export or an `EXPLAIN ANALYZE` inside an open transaction no longer aborts the
  transaction when the cancel arrives before the read's savepoint is set; the read ends
  cancelled and the transaction keeps what it held.
- PostgreSQL: a cancel the server does not confirm within 10 seconds (a slow tunnel) now shows a
  warning in Messages: it may still stop what runs next in the session, an open transaction
  included.
- Quitting or disconnecting with transactions open in several tabs, one of which failed, offered
  only to roll back every one. `c` now commits the healthy ones and rolls back the failed ones,
  which the question names.
- A lost metadata session, the metadata sessions of other databases and a Monitor tab's session
  now open only after the same checks as a connect (the profile's environment, its tunnel
  setting, its Startup SQL), so a profile changed since it connected no longer opens new
  sessions behind them.
- A Startup SQL that sets the server's idle limit (`idle_session_timeout`, `wait_timeout`) no
  longer races the server: datarig ends those sessions a little earlier than the server would
  (a tenth of the limit, from 1 to 30 seconds) and says why.

## 0.41.0 (2026-11-13)

Table tabs get a filter and sort dialog: pick conditions, tick values (counted exactly on the server if you like), sort, hide columns, and see the exact SQL before it runs; the grid filters with `=` and `!`, and a filtered count can be the planner's estimate. On PostgreSQL, the counts, exports and catalogue reads datarig runs on its own now read at READ COMMITTED, so a SERIALIZABLE default no longer drags them into other sessions' conflicts.

### Added

- Table tabs filter, sort and limit their rows on the server, in a full-screen filter dialog:
  `f` (or `Space r f`) opens it on the cursor's column, `Space r o` on the order, and a click on
  the filter in the results border opens it too. It lists the table's columns from the
  catalogue (`/` searches them) and builds conditions on the one selected: pick an operator
  (`≠` leaves NULL rows out, `≠ or NULL` keeps them; those the column cannot take are dimmed,
  with the reason) and type a value, or tick values from the column's list, counted in the rows
  loaded (`~`), with the values of earlier loads of the table to widen a condition. Mostly
  distinct numbers show their range; long values are counted, not listed.
- `#` in the dialog counts the column's values exactly on the server, under the other
  conditions only, with the time limit, lock rules and refusals of a row count (refused on
  MySQL when its read would lock rows, or while settings for the next transaction wait).
- The dialog's footer shows the condition as chips, the order, the limit and the SQL section:
  the whole statement as it would be sent, its WHERE and ORDER BY editable as text
  (`Ctrl+Space` completes) and `Open in editor`. A condition left alone is sent byte for byte;
  once changed, it is its terms joined with AND, those not touched as written. `Enter` applies with
  every check, a refusal keeps the dialog open with the reason and the refused part underlined,
  `Esc` drops the changes, and `u` or `Ctrl+Z` undoes a step, then the last change applied to the
  tab.
- A condition with a comment, a `;`, a row lock (FOR UPDATE, FOR SHARE) or another clause is
  refused before anything is sent, and one that calls functions datarig cannot check asks
  first (refused on a read-only connection); MySQL's `ROW_COUNT()` and `FOUND_ROWS()` are
  refused. The order is the table's columns with ASC or DESC (NULLS FIRST or LAST on
  PostgreSQL); the primary key (or, without one, the shortest unique key of `NOT NULL` columns)
  is added after it (`+ id`), and an order not known to be unique says so. `Space r v` opens the
  statement in a new console, `Space r x` clears the condition, order and limit, and from the
  command line `:where` and `:orderby` set the condition and the order and `:filter clear` clears
  them.
- On a table tab's grid, `=` (or `Space r =`) keeps only the value of the cell or of cells
  selected in one column, in place of the column's own conditions, and `!` (or `Space r !`)
  keeps every row but those, the NULL ones included unless NULL is among the cells; `X` (or
  `Space r X`) clears the conditions and the order. `s` (or `Space r s`) sorts by the cursor's
  column alone, going up, then down, then not at all, and `S` (or `Space r S`) adds it as one more
  key; a click at the right end of a header sorts too. `F` (or `Space r F`, or a right click on
  a header) opens the column's menu: sort, open the dialog on it, keep only or exclude the
  value, clear its conditions, hide it. Each runs with the checks of the dialog's `Enter`, and
  the headers of filtered or sorted columns are tinted and marked.
- The dialog's sort line and the column menu's sorts say where the server puts NULLs (`NULLs
  last`, `NULLs first`), and a MySQL `TIMESTAMP` ticked or taken from the grid is written as the
  instant it was read as (`CONVERT_TZ(…, '+00:00', @@session.time_zone)`), the same after the
  session's time zone changes.
- Columns can be hidden from a table tab's grid: `C` (or `Space r C`), "Hide column" in its
  menu, or `Space` / `C` in the dialog's column list, which shows them again. Hiding is a view
  only: every column is still read, and exports, the chart and the inspector see them all. The
  cursor, selections, copies, pastes and the search take the columns shown; a copy as SQL
  UPDATE or INSERT keeps the key columns. Hidden columns are kept per table across restarts, in
  the state directory.
- A table tab's results border says what is applied at the bottom left (the first condition,
  the sort, the hidden columns, shorter as the window narrows), spins while it runs and says
  `failed` when the server rejected it: the earlier rows stay, dimmed, and the dialog shows the
  server's message. The tab's result strip names its connection on the right.
- The LIMIT caps the rows the tab reads: the pages stop there, the results title says
  `limit 100`, and a count or an export of every row reads the same capped set. The applied
  condition, order and limit are kept with the workspace and with a closed tab brought back,
  and are checked again when the tab runs; a tab brought back with an order waits for the
  table's keys before its first run (`Ctrl+C` cancels the wait).
- On MySQL, a table tab asks before it runs when its reads would lock rows (isolation level
  SERIALIZABLE, or not known, with autocommit off or a transaction open), on the session the
  read is sent on; a page read again or a count in that state needs that answer first.
- A table tab keeps the conditions and orders that ran on each table: `Ctrl+R` in the filter
  dialog (or `Space r H` from the grid) lists them, most recent first, and fills the dialog
  with the one picked, which runs only with `Enter`. They stay on this machine, in a file of
  the state directory only you can read (never in the workspace file); `:set history.filters=off`
  stops keeping them and `:filter history clear [all]` forgets them.
- On a table tab whose rows are filtered, `#` offers the planner's estimate of the matching
  rows (`page 3 / ~12 (planner)`, nothing read) or the exact count. On any result, a count now
  stops after 30 seconds or after waiting 2 seconds (1 on MySQL) for another session's lock,
  and says the number is not known (`[results] count_timeout`, `"off"` for neither limit); on
  PostgreSQL inside your own transaction it has no limit, as before.

### Fixed

- On a PostgreSQL server whose role or database defaults to SERIALIZABLE, the explorer's and
  a table tab's reads of the catalogue no longer run as serializable transactions: they
  waited for a safe snapshot under `default_transaction_deferrable` and failed on a standby.
- PostgreSQL: a row count (`#`) outside your own transaction and an export no longer take part in
  other sessions' serialisation failures on a server, role or database whose default isolation is
  SERIALIZABLE, nor wait for a safe snapshot there. They now read at READ COMMITTED in a read-only
  transaction of their own; inside your transaction they still read there, at its level.
- PostgreSQL: when the session cannot tell whether a transaction is open, a row count without a
  time limit and an export are refused with a note, as a count with a time limit already was, so
  they never end or bypass a transaction of yours.

## 0.40.5 (2026-11-11)

Saving datarig's own files and running its own PostgreSQL statements are harder to redirect, and a `Ctrl+C` now cancels the run it was pressed for, including one still waiting for its turn.

### Fixed

- datarig's own files (the workspace, settings and other state) are now written through a
  temporary file created exclusively under an unpredictable name, so a symbolic link placed
  next to them can no longer redirect a write. A file name of the longest length a file
  system allows can now be saved too.
- A `Ctrl+C` pressed while a run is still waiting (for example while the connection is starting)
  now cancels that run, and a cancel no longer carries over to a later run.
- MySQL: cancelling a row count that waits behind a result still being paged now answers at
  once, instead of waiting a minute and closing the session.
- On PostgreSQL, the statements datarig sends itself now name their operators, functions and
  types with their schema, so a `search_path` listing a schema ahead of `pg_catalog` can no
  longer change what they do, including which rows an edit finds. The SQL shown for an edit
  therefore reads `OPERATOR(pg_catalog.=)` where it read `=`.

## 0.40.4 (2026-11-10)

MySQL exports and row counts no longer lock rows when the session's isolation level is SERIALIZABLE. After a `SET TRANSACTION` without `SESSION`, which sets your next transaction only, they are now refused with a message until a transaction starts or ends, so they never use that setting up. The install guide's Arch Linux command for building from source now upgrades the whole system.

### Fixed

- MySQL: an export, and a row count with autocommit off, no longer lock rows when the session's
  isolation level is `SERIALIZABLE`. They read one snapshot at `REPEATABLE READ` for their own
  transaction only, so they neither wait for nor hold up other sessions; the session's level is
  unchanged.
- MySQL: after `SET TRANSACTION` without `SESSION`, which sets your next transaction only, an
  export or a row count no longer uses that setting up. Until a transaction starts or ends they
  are refused with a note saying how to run them.

## 0.40.3 (2026-11-10)

MySQL now reads the structure and the DDL of objects in databases whose names contain braces, and the security policy explains how to report a vulnerability privately.

### Fixed

- MySQL: the structure and the DDL of objects whose schema or database name contains braces are
  read correctly.

## 0.40.2 (2026-11-09)

Install now lists the packages for building datarig from source on Debian, Ubuntu, Fedora and Arch Linux, and the Monitor tab's Locks view reads many MySQL metadata lock waits faster. Otherwise datarig works as before: its code is reorganised for easier development.

## 0.40.1 (2026-11-09)

The server monitor's Overview charts statements by type, rows by operation and sessions by state, says when it sees only some sessions, and shows its statements in syntax colours.

### Added

- The server monitor's Overview charts statements per second by type on MySQL (select,
  insert, update, delete), rows per second by operation on PostgreSQL (read, inserted,
  updated, deleted), and the sessions of each state on both, with the monitor's own statements
  taken out.
- The Monitor tab's session details show the statement in the editor's syntax colours and laid
  out for reading by the SQL formatter, for the eyes only: `f` shows it as the server sent it
  (or laid out again), and `y` and `o` always take the server's text. A statement known only in
  part (the list's first 256 characters, or cut by the server), one the formatter would change
  more than its blanks, or one over 64 KiB is shown as sent, and the details say why.

### Fixed

- The server monitor no longer reads as idle when the account sees only some sessions: the
  longest running sessions are headed "this account" (MySQL without `PROCESS`) or "visible
  sessions" (PostgreSQL with other roles' sessions hidden), their empty text says the same, and
  a PostgreSQL sessions count with hidden sessions shows as the least there are.
- The server monitor names the server's role in words that cannot be taken for the profile's
  read-only mode: `primary` or `standby` on PostgreSQL, `server takes writes` or `server
  read-only` on MySQL.
- The details of the monitor's own connection in the Monitor tab looked like any active session:
  they now name it ("this monitor") and say that its statement is the monitor reading the
  server and that it cannot be cancelled or ended.

## 0.40.0 (2026-11-08)

A Monitor tab per connection shows sessions, locks and replication, and can cancel or end a session safely.

### Added

- The Monitor tab (`M` on a connection, `:monitor`) shows a PostgreSQL or MySQL server's load
  as charts and its sessions as a list you can sort and filter, read by a connection of its own
  only while the tab is shown. What the account may not see is said, with the privilege that
  would show it.
- The Monitor tab's session details read a long statement in whole, and show it only while it
  is still that session's statement (on MySQL, once it has run for a few seconds); open in
  console and copy take the whole text too.
- The Monitor tab's Locks view (`3`) shows who waits for whom as a tree, root blockers first:
  what each session waits for and how long, what its blocker holds, prepared transactions
  that hold locks, and on MySQL the holders of metadata locks a request cannot be granted
  beside. `b` goes to the root blocker of the selected wait.
- The Monitor tab has a Replication view (`4`): whether the server takes changes from another and
  how far behind it is (said only as far as the server can tell), the servers that take
  changes from it, PostgreSQL replication slots with the log they keep and logical
  subscriptions, and MySQL group replication members. `i` shows what the server is in full,
  with the settings that decide what the monitor can read.
- The Monitor tab cancels a session's statement (`c`) or ends a session (`X`), asked first, never
  on a read-only environment; what was sent is checked by later reads and listed with `L`.

## 0.39.1 (2026-11-07)

Small fixes: explorer arrows open with a click, no opaque bands on a see-through terminal background, cursor tints that stay visible on 256 colours, and a copy format of your choice.

### Added

- `copy_format` sets the format `y` and `Y` copy in: TSV (the default), CSV, JSON, indented
  JSON, Markdown, HTML, XML or SQL `INSERT`. Choose it in the settings, with
  `:set copy_format=csv`, or with `Space r y d` (also in the grid's menu); the copy formats mark
  the current one, and the status bar names the format of each copy. SQL `INSERT` is used only
  for rows certainly of one table, else the copy is TSV and says why. `g p` still pastes the
  copied values exactly, whatever the clipboard got.
- `:theme update ` lists your theme files to pick from (never a built-in theme), the
  current theme's first.

### Changed

- The selection summary on the status line keeps its figures while a whole-column selection
  grows as every row is fetched (or a range is extended downwards): only the new rows are read
  and added, and the figures so far stay on the line, marked "summing…", instead of starting
  over with each page.

### Fixed

- On terminals of 256 colours (`TERM` such as `xterm-256color` without `COLORTERM=truecolor`)
  the result grid's cursor row, the cursor's line and the other cursor and selection tints no
  longer vanish into the background: each gets a palette colour of its own.
- A click on an explorer row's arrow opens or closes the row, for every kind of row:
  on a table it shows the table's structure. The arrow under the pointer is highlighted.
- With `ui.background = "terminal"` the editor no longer paints its current line and statement
  as opaque bands: the statement a run would take is an accent bar beside its lines with accent
  line numbers, and the cursor's line number is bold. The result grid's cursor row has no band
  either; its cell keeps its fill and its row number is bold.

## 0.39.0 (2026-11-07)

datarig can tell you when a newer release is out, and shows what's new after an update.

### Added

- Once a day datarig asks GitHub whether a newer release exists (one HTTPS request to
  api.github.com, with nothing about you or your databases; through `HTTPS_PROXY` when one is
  set). A newer one is told about at the next start, with the command that updates your
  install; `Esc` closes it (it comes back after a week), `s` skips that version, `o` turns the
  checks off. `:update` says what the last check found. Turn it off with `[updates] check =
  "off"` (settings screen, Updates), `:set updates.check=off` or `DATARIG_NO_UPDATE_CHECK=1`;
  there is no check on CI (`CI` set).
- What's new: on the first start after an update, a dialog shows the notes of every version
  since the one that ran before (from the program itself, so it works offline). `:whatsnew`
  shows it any time; `[updates] whats_new = "off"` (settings screen, Updates) stops it from
  showing by itself.

## 0.38.3 (2026-11-06)

Screen text uses British spelling, and titles use a colon in place of a dash; the docs site shows new recordings of the editor, charts and plans.

### Changed

- Screen text uses British spelling (colour, materialised view; the connection form's row reads
  "Colour") and no longer uses the em dash: titles read "Changes: Result 1 · shop.users",
  "Apply changes: shop.users" and "Edit connection: name". Config keys, theme tokens and commands are unchanged (`color`, `[colors]`,
  `:explain analyze`).

## 0.38.2 (2026-11-05)

The grid shows where its cursor is and sums up a selected range on the status line; menus show what cannot run now, dimmed with why.

### Added

- While a range, rows or columns of the result grid are selected, the status line sums them
  up: their size, then for numbers the sum, average, least and greatest value (integers and
  decimals exactly), else how many distinct values, and how many are NULL. It takes the values
  the grid shows (pending changes too) and reads the rows in the background, so a big selection
  never holds up the keys.
- `:theme update [<name>]` adds to a theme file the tokens it does not list (such as tokens added
  after it was written), each under its description at the end of its section, commented out
  with the value it takes now. No line of the file changes, and nothing is written unless the
  file reads exactly as before.
- A theme file that does not list every token is said once in the status line when it is used,
  with how many and that `:theme update` adds them; again only when the tokens it does not list
  change.
- New setting `ui.background = "theme" | "terminal"` (settings screen, `:set
  ui.background=terminal`): with `terminal` the panes and the result grid's alternate rows leave
  the terminal's own background showing, so a translucent terminal shows through; text,
  selections and edit marks keep the theme's colours, and dialogs, menus and popups keep its
  surface. Turning it on with a theme that has a background of its own says so in one line.

### Changed

- The editor's completion popup opens by itself 60 ms after the last key (it waited 120 ms and
  then up to a tenth of a second more for the next timer tick), exactly at that moment. New
  setting `[editor] completion_delay_ms` (`60`, `0` for at once, `30`, `100`, `150`, `250`;
  settings screen and `:set`).
- The result grid shows where the cursor is: its row is tinted, its cell more strongly, and its
  column's header too. A selected range is drawn as one block (the separators between its
  columns included), with its rows' numbers and its columns' headers tinted. New theme tokens
  `grid_cursor_row` and `grid_cursor_header`; a theme file that sets its own background,
  surface or selection without them gets tints made of its own colours (when those are RGB).
- The action menu shows an action that would act there but can't now dimmed, with why in a few
  words where its key would be (a read-only environment, a result that can't be edited, an edit
  that doesn't fit the cell, a table tab's Save, paging that closed), instead of leaving it out.
  The keys and the wheel step over it; a click on it says the reason in full and keeps the menu
  open.

## 0.38.1 (2026-11-05)

Copy and paste inside the grid: `y` keeps the copied cells as they are, and `p` pastes them as new rows (again and again); previous page moved from `p` to `[`.

### Added

- The cells last copied with `y` or `Y` in the result grid paste back exactly as the grid
  showed them (NULL stays NULL), in any result in any tab, without reading any clipboard:
  `p` adds them as new rows (each press another copy), `g p` pastes them over the cells at
  the cursor. Each is one step `u` undoes.
- A range pasted into the grid skips a first line that names the columns it lands in (a copy
  with its header line), and says so; `Space d V` reads it again keeping that line.

### Changed

- Previous page moved from `p` to `[` in the result grid (and `]` is next page, as `n` still
  is); `p` now pastes the grid's copy as new rows.
- The Changes tab lists each problem under its row's line, without the row's name the line
  above gives, and names a row the same way wherever a problem is about it (the statements,
  the Apply's messages and failures, a paste's message, the cell editor and the inspector):
  `new 1`, `id=5` or `row 31`, not the row's number in the grid. A new row lists only the
  values set, then how many columns are left to their default (`DEFAULT ×2`). Control
  characters in values, column names and the table's name show as `\n`, `\r`, `\t` or
  `\x07`, and characters that turn the text's direction as `\u{202e}`, never raw; the
  statements copied are unchanged.
- The result grid marks every new row an Apply is refused for: a cell left to a default its
  column lacks (`NOT NULL`, no default) reads `! value needed` in the invalid-value colour
  instead of the dim `default`, its column widened to fit, and the row's gutter shows `!`
  instead of `+`, as for a value its column does not take; so does a new row of a table with
  such a column the result does not show. The badge counts these cells, and the cell editor
  and the inspector say why. Invalid values keep their colour under the cursor.
- A new row in the result grid is tinted as a whole (theme token `edit_inserted_row`) and
  numbered after the rows read, with `+` (or `!`) after its number; rows an Apply added are
  numbered too. Its cells left to their default show what the server will put there instead of
  a dim `default`: the default's value (`ACTIVE`, `0`), `<generated>` for a number the table
  draws or a generated column, `<default: now()>` for another default, `NULL` where the column
  takes it. The Apply still leaves them to the server.

## 0.38.0 (2026-11-04)

Pending changes get their own Changes tab, Apply asks on the status line, and a range copied elsewhere can be pasted into the result grid.

### Added

- A range pasted into the result grid with the terminal's own paste (`Cmd+V`, `Ctrl+Shift+V`)
  becomes pending changes from the cursor or the selection's corner: read as TSV, CSV, one
  column or one value (the status line names the format, and `Space d V` reads it again as the
  next one; text with commas and no tab asks first whether it is CSV, `c`, or one column, `1`), rows past the last added as new rows, one step `u` undoes. One value fills every
  selected cell. A paste that does not fit (too wide, a column that takes no value, a row marked
  for deletion, rows past the last row read while the server has more) is refused whole, with
  the reason; one of more than 1,000 rows asks first. `\N` pastes NULL (setting
  `edit.paste_null`); an empty field is an empty value. Values a column does not take are kept,
  marked `!` in the gutter (theme token `edit_invalid`) and listed first in the Changes tab, and
  the changes are not applied until they are fixed. `Space d v` pastes the system clipboard with
  `clipboard = "system"`, or `"auto"` outside SSH. For about a second after a status-line question or a
  waiting Apply ends without its answer, `y`, `n`, `c` and `1` do nothing in the results and say
  they were ignored, so a late `y` never copies a cell over the clipboard.

### Changed

- A result with pending changes has a Changes tab in its result strip: one line per changed
  cell, every reason they can't be applied yet first, each with its row; `Enter` shows the cell
  in the grid, `s` switches to the statements that apply them, `c` copies them exactly as they
  are sent. `Space d p` and `:changes` show the tab. `ui.changes_view` picks the view the tab
  opens in.
- `Space d a` and `:apply` ask on the status line instead of opening a dialog: only `y`
  applies, any other key, a click or a paste cancels. Rows found by their values are counted
  again first (any key, a click or a paste meanwhile stops waiting); problems show in the Changes tab instead of a question. The dialog opens only for
  a choice among identical rows, an Apply whose result is unknown, and environments with
  `confirm = "writes"`. After an Apply the Changes tab shows what was applied and the statements
  sent until the query runs again, or why nothing was, the failing statement marked.

## 0.37.1 (2026-11-04)

This patch adds the edits of a cell to the grid's right-click menu and makes the preview of
pending changes readable.

### Added

- The grid's right-click menu (and `Space Space`) lists the edits of the cell: edit it, set it
  to NULL or DEFAULT, add or copy a row, mark it to be deleted, and, with changes pending, undo,
  preview and discard. Only the edits that can act on the cell are listed.

### Changed

- The preview of pending changes shows each statement in the editor's syntax colors: a long one
  wraps over as many lines as it needs, its expected count on its first line, and a literal of
  more than a few hundred bytes shows its start and end around how many characters are left
  out. On MySQL, a statement that compares text by its bytes (`_utf8mb4 X'…'`) is followed by a
  dim `↳ set column = 'text' · where column = 'text'` line that reads the values back; that line is
  not SQL, and Copy SQL copies the statements whole without it.

### Fixed

- The keys on a dialog's bottom border no longer run into its corner: a border cell is left on
  each side, and keys too long for the dialog are cut with `…`.
- `:set ui.tips=on` is taken as `always`, as the config file already reads `tips = "on"`.

## 0.37.0 (2026-11-03)

This release adds editing a result's data in the grid: change cells, add, copy and delete rows,
then apply the changes after a preview of their exact statements.

### Added

- Edit a result's data in the grid: change cells (one line, or a box for values of several
  lines and JSON), add and clone rows, mark rows to delete, undo and redo. The changes stay
  pending until a preview shows the statements that apply them; `y` there runs them in one
  transaction, each row checked first. See Editing data in the guide.

### Changed

- MySQL: the rows affected by an `UPDATE` (and by `INSERT … ON DUPLICATE KEY UPDATE`) are the
  rows it matched, as with PostgreSQL, so a row set to the values it already had counts too;
  `ROW_COUNT()` counts them the same way. The server's info line still says how many changed.
- Tips keep showing: the tips of an empty place take turns and come back, and a key you have
  already pressed comes up less often instead of never again. `[ui] tips` is now `"always"`
  (the default), `"until_used"` (the earlier behaviour) or `"off"`; `"on"` in an existing
  config reads as `"always"`, and `true` and `false` read as `"always"` and `"off"`.

### Fixed

- MySQL: Copy as SQL no longer refuses text and JSON values of a tab's first result as "read
  across a change of the session's time zone or character set": the values a session reports as
  it connects no longer count as a change. An export of that result as SQL to a file is no
  longer refused for it either.

## 0.36.2 (2026-10-31)

### Changed

- The hint line, the menus, the welcome panel and the messages name the same key for an action
  as the tips do: a key of your own first, else one that needs no function key (`Space ?` for
  the keyboard help, `Space Space` for the action menu), and `F1` where `Space` types text.
- Startup SQL may set the server's idle limit (PostgreSQL `idle_session_timeout`, MySQL
  `wait_timeout`). Its value is then the idle disconnect of those sessions in place of the
  environment's `idle_disconnect`: the app closes the tab after that long, warns about an open
  transaction a minute before, and `:session` shows the limit with what it overrides. Behind a
  pooler PostgreSQL's is not sent and the environment's limit stays.

## 0.36.1 (2026-10-30)

The documentation now lives at <https://dokadev.github.io/datarig/>, and `datarig --help` names
MySQL next to PostgreSQL.
