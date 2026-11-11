- On a table tab's grid, `=` (or `Space r =`) keeps only the value of the cell under the cursor,
  in place of the column's own conditions; cells selected in one column keep any of their
  values, and a NULL cell keeps the rows whose value is NULL. `!` (or `Space r !`) keeps every
  row but those, the rows whose value is NULL included unless NULL is among the cells. `X` (or
  `Space r X`) clears the conditions and the order and keeps the limit. Each runs with the same
  checks as the filter dialog's `Enter`, and `u` in the dialog brings back what was applied
  before.
- `F` (or `Space r F`, or a right click on a column's header) opens the column's menu: sort it
  ascending or descending or take it out of the order, open the filter dialog on it, keep only
  or exclude the cursor's value, clear its own conditions, hide it. Entries that cannot apply
  are dimmed, with the reason.
- Values are written as the server reads them back: numbers bare, bytes in hexadecimal. A value
  not shown whole, binary data over 64 KiB, a type whose comparison datarig does not know or
  more than 1000 cells are refused, with the reason.
- A MySQL `TIMESTAMP` value (by value, or ticked in the filter dialog) is written as the instant
  it was read as, in the session's time zone when the filter runs (`CONVERT_TZ('…', '+00:00',
  @@session.time_zone)`): the same instant on every MySQL and MariaDB, after a change of the
  session's zone and when the filter runs again from the history.
- The headers of the columns a table tab filters by a condition of their own, or sorts by, are
  tinted.
