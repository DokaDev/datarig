- Table tabs filter and sort in a full-screen dialog: `f` (or `Space r f`) opens it on the
  cursor's column, `Space r o` on the order. It lists the table's columns from the catalogue
  (`/` searches them) and builds conditions on the one selected: pick an operator (those the
  column cannot take are dimmed, with the reason) and type a value, or tick values from the
  column's list, counted in the rows loaded (`~`), with values seen in earlier loads of the
  table and an exact count on the server with `#`. Long values are counted, not listed; mostly
  distinct numbers show their range.
- The dialog's footer shows the conditions as chips (one removed keeps the others as written),
  the order, the limit and the SQL section: the whole statement, with its WHERE and ORDER BY
  editable as text (`Ctrl+Space` completes) and `Open in editor`. A condition left alone is
  sent as typed. `Enter` applies with every check, a refusal keeps the dialog open with the
  reason and the refused part underlined, `Esc` drops the changes, and `u` undoes a step, then
  the last change applied. The mouse clicks, ticks and scrolls in it.
- A table tab's results border says its filter and sort at the bottom left (the first
  condition, how many more, the sort keys), spins while they run and says `failed` when the
  server rejected them; a click opens the dialog. The tab's result strip names its connection
  on the right, shorter as the window narrows.
