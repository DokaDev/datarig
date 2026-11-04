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
