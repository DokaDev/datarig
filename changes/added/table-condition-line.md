- Table tabs filter, sort and limit their rows on the server: a WHERE condition, an ORDER BY
  and a LIMIT, set in the filter dialog (`f` or `Space r f`, `Space r o` for the order).
  `Space r x` clears all three and `Space r v` opens the statement in a new console
  (`Ctrl+G` hands the same statement to your own editor). `:where`, `:orderby` and
  `:filter clear` do the same from the command line, and the condition and the order complete
  column names with `Ctrl+Space` (`Enter` applies unless you moved through the list or asked for
  it). A filtered table tab keeps its mark in a shortened tab title.
- The condition is sent as typed, inside parentheses; the order is read as column names with
  ASC or DESC (NULLS FIRST or LAST on PostgreSQL) and shown normalised once applied; the
  primary key is added after it (`+ id`), and an order not known to be unique says so.
- A condition with a comment, a `;`, a row lock (FOR UPDATE, FOR SHARE) or another clause is
  refused before anything is sent, with the refused part underlined when datarig can point to
  it and the reason in full under the grid. A condition that calls functions datarig cannot
  check asks first, and is refused on a read-only connection.
- When the server rejects the condition, the results border says `failed` and the filter
  dialog shows the server's message (and where the error is, when the server says), and the
  earlier rows stay, dimmed, marked as not the filter's answer. A cancelled run or a lost
  connection leaves them the same way.
