- `copy_format` sets the format `y` and `Y` copy in: TSV (the default), CSV, JSON, indented
  JSON, Markdown, HTML, XML or SQL `INSERT`. Choose it in the settings, with
  `:set copy_format=csv`, or with `Space r y d` (also in the grid's menu); the copy formats mark
  the current one, and the status bar names the format of each copy. SQL `INSERT` is used only
  for rows certainly of one table, else the copy is TSV and says why. `g p` still pastes the
  copied values exactly, whatever the clipboard got.
