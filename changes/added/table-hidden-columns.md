- Columns can be hidden from a table tab's grid: `C` (or `Space r C`) on the column, "Hide
  column" in its menu, or `Space` / `C` on it in the filter dialog's column list, which shows
  them again. Hiding is a view only: every column is still read, and exports, the chart and the
  inspector see them all. The cursor, selections, copies, pastes and the search take the columns
  shown; a copy as SQL UPDATE or INSERT keeps the table's key columns. The results border says
  how many columns are hidden. With every column hidden the grid says how to show one again, and
  no key reads or changes a cell until then. They are kept per table across restarts, in the
  state directory.
