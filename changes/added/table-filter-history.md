- A table tab keeps the filters and orders that ran on each table: `Ctrl+R` in the filter line
  (or `Space r H` from the grid) lists them, most recent first, and fills the line with the one
  picked, which runs only with `Enter`. They stay on this machine, in a file of the state
  directory only you can read (never in the workspace file); `:set history.filters=off` stops
  keeping them and `:filter history clear [all]` forgets them.
