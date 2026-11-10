- `F` (or `space r F`, or "Table → filter by value" in the grid's right-click menu) on a table
  tab filters by the value of the cell under the cursor: equal, not equal (with or without the
  rows whose value is NULL), less or greater, and for text "starts with" and "contains (any
  case)". A NULL cell offers `IS NULL` and `IS NOT NULL`; cells selected in one column offer
  `IN` and `NOT IN`, with a NULL among them tested on its own. The condition is added to the
  filter of the rows shown with `AND` (a filter with an `OR` at its top is put in parentheses
  first), written into the filter line and run as `Enter` runs it there.
- Values are written as the server reads them back: numbers bare, bytes in hexadecimal, a
  pattern's `%` and `_` escaped. A value not shown whole, binary data over 64 KiB, a type whose
  comparison datarig does not know or more than 1000 cells are offered dimmed, with the
  reason.
