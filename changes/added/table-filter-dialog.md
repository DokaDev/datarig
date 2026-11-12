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
