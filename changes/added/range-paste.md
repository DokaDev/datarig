- A range pasted into the result grid with the terminal's own paste (`Cmd+V`, `Ctrl+Shift+V`)
  becomes pending changes from the cursor or the selection's corner: read as TSV, CSV, one
  column or one value (the status line names the format, and `Space d V` reads it again as the
  next one), rows past the last added as new rows, one step `u` undoes. One value fills every
  selected cell. A paste that does not fit (too wide, a column that takes no value, a row marked
  for deletion, rows past the last row read while the server has more) is refused whole, with
  the reason; one of more than 1,000 rows asks first. `\N` pastes NULL (setting
  `edit.paste_null`); an empty field is an empty value. Values a column does not take are kept,
  marked `!` in the gutter (theme token `edit_invalid`) and listed first in the Changes tab, and
  the changes are not applied until they are fixed. `Space d v` pastes the system clipboard with
  `clipboard = "auto"` or `"system"`.
