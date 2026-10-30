# Results

A run's results open in the results pane below the editor. Each row result gets a result tab
of its own, after a Messages tab with each statement's outcome and before a Chart tab. In the
results pane `H` and `L` move between the tabs; `Space r [` and `Space r ]` do it from anywhere.

```text
{{#include ../screens/grid-keys.txt}}
```

## Pages

The grid shows one page at a time (`page_size`, 500 rows by default): `n` and `p` move to the
next and previous page. By default nothing stays open on the server between pages: the first
page ends its transaction, and the next page runs the statement again when it is safe to repeat
(said each time); otherwise you see the first page and why. An environment can keep the cursor
open instead; see [Paging and locks](environments.md#paging-and-locks).

The rows already fetched stay in a bounded memory window and spill to a private temporary file
past it. `#` counts every row on demand (it runs a count query), with a label that says whether
the count matches the pages you saw.

## Moving around

- `h` `j` `k` `l` or the arrows move the cell; `0` and `$` go to the first and last column,
  `gg` and `G` to the first and last row of the page, `Ctrl+D` / `Ctrl+U` scroll half a
  screen.
- The wheel scrolls the rows, three a notch. A trackpad's sideways swipe, a tilting wheel or
  Shift with the wheel scrolls sideways by character, four a step: the columns at the edges show
  the part that fits, and the headers scroll with the rows. A value longer than a column's 40
  characters is cut with `…`. The selected cell stays where it is until a key or a click moves
  it; then the view moves just enough to show that cell whole.
- `Enter` shows the cell's value whole in the cell viewer.
- `v` selects a range of cells, `V` whole rows, `Ctrl+A` every cell of the page; the mouse
  selects too.
- `z` (or `Space z`) zooms the results pane to the whole workspace and back; `+` and `-` make it
  taller or shorter. `Esc` or `q` goes back to the editor.

## Search

`/` searches forward and `?` backward: a prompt under the grid, the cursor on the first matching
cell as you type, `Enter` to search. Then `n` and `N` go from match to match, across pages and
around the ends with a warning, and `Esc` ends the search (`n` is the next page again). `:noh`
ends it too.

The search covers every row already fetched (the spilled ones too), each cell's full value as
the server sent it, not the cut the grid shows. NULL never matches, and nothing is fetched or
run again for it: when the server has more rows, the line under the grid says so. Patterns are
the editor's (Rust regular expressions, case sensitive unless they start with `(?i)`); matching
cells are tinted.

```text
{{#include ../screens/grid-search.txt}}
```

## The inspector

`i` shows or hides the inspector next to the grid; `I` switches between its Row and Cell tabs.
It starts on the Row tab, and the tab you pick stays for the session. JSON values are
pretty-printed.

- The Row tab follows the grid's column: it highlights that column and scrolls to it.
- After a click on the inspector, `j` and `k` (or a click on another column) move the grid's
  cell there, the grid scrolling sideways to it; `Enter` or a double-click shows the value
  whole.
- `detail_view = "statusbar"` (or `:set detail_view=statusbar`) shows a preview of the cell in
  the status bar instead of the panel.

```text
{{#include ../screens/inspector.txt}}
```

## Copy

`y` copies the cell (a selected range as TSV) and `Y` the row (the selected rows) as TSV. The
`Space r y` group copies the selection, and `Space r a` every fetched row, as:

TSV with or without headers, CSV, JSON, indented JSON, Markdown, HTML, XML, an SQL `IN` list,
or SQL `INSERT`/`UPDATE` statements (only when the target table is certain). `:copy <format>`
does the same from the command line (for example `:copy csv all`).

- Copies go to the system clipboard, or through OSC 52 over SSH (`clipboard`: `auto`,
  `system` or `osc52`; `osc52_max_bytes` is the longest copy sent through OSC 52).
- `copy_header` says whether a copied block starts with the column names.
- The copied cells flash briefly and fade back (with the terminal's own colours the flash goes
  off at once); `[ui] yank_flash = "off"` turns that off.

To write a result to a file instead, see [Export](export.md). To draw it, see
[Charts](charts.md).

## Transactions in the results

Results read inside an open transaction are labelled uncommitted. The tab bar and the status
bar show an open transaction, an aborted one, and "rollback required"; see
[Manual commit](manual-commit.md).
