- Table tabs sort by a column from the grid: `s` (or `space r s`) sorts by the cursor's column
  alone, going up, then down, then not at all; `S` (or `space r S`) adds it as one more key, or
  turns or removes it where it is. A click at the right end of a column's header (marked on the
  cursor's column) sorts as `s` does; a click elsewhere on a header still selects the column.
  The new order is written into the filter line and runs on the server, asking first over
  pending changes.
- The grid's headers show how the rows are sorted (an arrow after the name, with the key's
  level from the second key on) and mark the columns the condition compares, as far as
  datarig reads it.
