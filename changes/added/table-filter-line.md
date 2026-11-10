- Table tabs have a filter line above the grid: a WHERE condition and an ORDER BY, run on the
  server. `f` (or `space r f`) edits the condition, `space r o` the order, Enter applies,
  `space r x` clears both and `space r v` opens the statement in a new console
  (`Ctrl+G` hands the same statement to your own editor). `:where`,
  `:orderby` and `:filter clear` do the same from the command line, and both fields complete
  column names with `ctrl+space` (Enter applies unless you moved through the list or asked
  for it). A filtered table tab keeps its mark in a shortened tab title.
- The condition is sent as typed, inside parentheses; the order is read as column names with
  ASC or DESC (NULLS FIRST or LAST on PostgreSQL) and shown normalised once applied; the
  primary key is added after it (`+ id`), and an order not known to be unique says so.
- A condition with a comment, a `;`, a row lock (FOR UPDATE, FOR SHARE) or another clause is
  refused before anything is sent, with the refused part underlined when datarig can point to
  it and the reason in full under the grid. A condition that calls functions datarig cannot check asks first, and is refused on a
  read-only connection.
- When the server rejects the condition, the line marks it failed (and where the error is,
  when the server says), and the earlier rows stay, dimmed, marked as not the filter's answer.
  A cancelled run or a lost connection leaves them the same way.
- A third field, LIMIT, caps the rows a table tab reads: the pages stop there and nothing is
  fetched past it, the results title says `limit 100`, and a count or an export of every row
  reads the same capped set. A limit that is not a whole number of rows is refused.
- The applied condition, order and limit are kept with the workspace and with a closed tab
  brought back; they return to the line without running and are checked again when the tab
  runs.
