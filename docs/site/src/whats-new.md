# What's new

What changed in each release, newest first. For 0.36.0 and earlier, see the release notes on
[GitHub Releases](https://github.com/DokaDev/datarig/releases).

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
