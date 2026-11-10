# Results

A run's results open in the results pane below the editor. Each row result gets a result tab
of its own, before the Plan, Chart and Messages tabs (Messages holds each statement's outcome). While
the shown result has pending changes, a Changes tab follows the result tabs (see
[The Changes tab](editing.md#the-changes-tab)). In the results pane `H` and `L` move between the
tabs; `Space r [` and `Space r ]` do it from anywhere.

```text
{{#include ../screens/grid-keys.txt}}
```

## Pages

The grid shows one page at a time (`page_size`, 500 rows by default): `n` (or `]`) and `[` move to the
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
- The cursor's row is tinted (its number in the accent), its cell more strongly, and so is its
  column's header (a selected range tints its columns' headers instead). A new row keeps its
  own tint and shows the cursor by its number. On the `terminal` theme the row keeps your terminal's background: the row's
  number and the underlined header show where the cursor is.
- `v` selects a range of cells, `V` whole rows, `Ctrl+A` every cell of the page; the mouse
  selects too. The selection is one tinted block, the separators between its columns included.
  The rows of a range of cells or of whole rows take the row tint and a tinted number; the
  selected columns' headers are tinted too (whole columns tint their headers and cells, not the
  row numbers). The theme tokens `grid_cursor_row` and `grid_cursor_header` set the tints.
- While a selection is there and the grid has the focus, the status line sums it up (a fresh
  message comes first; in a narrow window the least and greatest values, then NULL, then the
  average, are left out): its size,
  then for numbers (every selected column an integer, decimal or floating-point one) the sum,
  average, least and greatest value, else how many distinct values; and how many are NULL, as in
  `5 rows × 1 column · sum 1,013.00 · avg 253.25 · min 0.25 · max 1,000 · NULL 1`. Integers and
  decimals add up exactly. The values are those the grid shows, a pending change over the value
  read, as a copy takes them; a cell with no value yet (a pending DEFAULT) is counted apart, new
  rows are left out, and whole columns say "fetched rows only" when the server has more. The
  rows are read in the background (a big selection says "summing…" at first), so keys never
  wait for it. A selection that grows at its end (whole columns while every row is fetched, or
  a range extended downwards) keeps its figures: only the new rows are read and added, and the
  figures so far stay on the line, marked "summing…", until they are. Distinct values are
  counted up to 100,000.
- `z` (or `Space z`) zooms the results pane to the whole workspace and back; `+` and `-` make it
  taller or shorter. `Esc` or `q` goes back to the editor.

## Search

`/` searches forward and `?` backward: a prompt under the grid, the cursor on the first matching
cell as you type, `Enter` to search. Then `n` and `N` go from match to match, across pages and
around the ends with a warning, and `Esc` ends the search (`n` is the next page again; `]` is
the next page all along). `:noh` ends it too.

The search covers every row already fetched (the spilled ones too), each cell's full value as
the server sent it, not the cut the grid shows. NULL never matches, and nothing is fetched or
run again for it: when the server has more rows, the line under the grid says so. Patterns are
the editor's (Rust regular expressions, case sensitive unless they start with `(?i)`); matching
cells are tinted.

```text
{{#include ../screens/grid-search.txt}}
```

## Filter and sort a table tab

A table tab (opened from the explorer) has a line above its grid with a WHERE condition and an
ORDER BY, both run on the server. `f` (or `Space r f`) edits the condition and `Space r o` the
order; `Enter` applies them, `Tab` moves between the fields, `Ctrl+Space` completes column
names and `Esc` puts back what was applied. A completion list that opens by itself while you
type does not take `Enter`: once you move through it (arrows, `Ctrl+N`, `Ctrl+P`) or ask for it
with `Ctrl+Space`, `Enter` picks its entry. `Ctrl+E` always applies. `Space r x` clears them all and runs the table again,
and `Space r v` opens the statement in a new console. From the command line, `:where`,
`:orderby` and `:filter clear` do the same.

The condition is sent exactly as you typed it, inside parentheses. The order is read as column
names, each with `ASC` or `DESC` (and `NULLS FIRST` or `NULLS LAST` on PostgreSQL), and shown
in its normal form once applied. When the table has a primary key, its columns are added after
yours (shown dimmed as `+ id`) so that pages read again follow one order; until datarig knows
the order is unique (the table's keys are read from the catalog when the tab opens, without
locking the table), the line says "order not unique".

Before anything is sent, datarig refuses a condition that could do more than filter this
table's rows: a comment, a `;`, a row lock (`FOR UPDATE`, `FOR SHARE`), another clause or
another table. The refused part is underlined when it can be pointed to; the status line says
why, and the line under the grid repeats it in full while it stands. A condition that calls functions datarig cannot check for side effects asks first; on
a read-only connection it is refused.

When the server rejects the condition, the line marks it failed, underlined where the server
says the error is. The rows of the earlier query stay, dimmed, and the line under the grid says
they are not the filter's answer; a cancelled run or a lost connection leaves them the same way.

The third field, `LIMIT`, caps the rows the tab reads (`-` when there is none; a narrow line
shows it as `L100`). It takes a whole number of rows, written in digits, and a blank field means
no cap; `0`, or anything that is not a number, is refused before anything is sent. The cap is
not written into the statement the grid runs: the pages stop at it instead, the last one cut
there, and nothing is fetched past it (what the server held open for the result is closed). The
results title then says `limit 100`. A count (`#`) and an export of every row read the capped
statement (`… LIMIT 100`), so they see the same rows the pages would. A new limit runs the table
again from its first page.

The condition, order and limit you applied are kept with the workspace and with a closed tab
you bring back (`Space t u`): they return to the line without running, and are checked again
as above when the tab runs.

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

`y` copies the cell (a selected range) and `Y` the row (the selected rows), as TSV unless
`copy_format` says otherwise. The
`Space r y` group copies the selection, and `Space r a` every fetched row, as:

TSV with or without headers, CSV, JSON, indented JSON, Markdown, HTML, XML, an SQL `IN` list,
or SQL `INSERT`/`UPDATE` statements (only when the target table is certain). `:copy <format>`
does the same from the command line (for example `:copy csv all`).

- Copies go to the system clipboard, or through OSC 52 over SSH (`clipboard`: `auto`,
  `system` or `osc52`; `osc52_max_bytes` is the longest copy sent through OSC 52).
- The status bar names the format: `Copied 5 rows as CSV (OSC 52)`.
- The copied cells flash briefly and fade back (with the terminal's own colours the flash goes
  off at once); `[ui] yank_flash = "off"` turns that off.

### The format of y and Y

`copy_format` is the format `y` and `Y` copy in: `tsv` (the default), `csv`, `json`,
`json_pretty`, `markdown`, `html`, `xml` or `insert`, the same names as `:copy`. Set it in the
settings screen, with `:set copy_format=csv`, or from the grid: `Space r y d` (or the menu's
"Default format for y and Y") lists the formats with the current one marked, and picking one
makes it the default. The `Space r y` group and the menu's copy formats mark the one `y` and `Y`
use.

- One cell without a selection is always copied as its value, whatever the format.
- `insert` writes SQL `INSERT` statements only when the rows are certainly of one table (the
  same rule as Copy as SQL INSERT). Otherwise `y` and `Y` copy TSV and the status bar says why.
- `g p` and `p` paste the cells `y` and `Y` copied, value for value, whatever format the
  clipboard got.

### Column names in copies

`copy_header` says whether a copy starts with a row of column names:

- `auto` (the default): when more than one row is copied. `y` on one cell, `Y` on one row and a
  selection inside one row copy no header; a selection of two or more rows, or whole columns,
  starts with one.
- `on`: always, even for one cell.
- `off`: never.

It applies to TSV copies from `y` and `Y`. A CSV copy always starts with the column names
unless `copy_header = "off"`. Copy as TSV without or with headers ignores the setting, and JSON,
Markdown, HTML, XML and the SQL formats are written the same way whatever it says.

To write a result to a file instead, see [Export](export.md). To draw it, see
[Charts](charts.md).

## Editing

A result of one table can be edited in the grid: `e` edits a cell, `o` and `O` add rows, `D`
marks rows to be deleted, and the changes stay pending until you apply them. After the first
edit key that gets as far as the table, the result tab strip says whether the rows can be edited
(`editable · shop.users · key id`) or why not (`read-only · <reason>`; a reason about the
connection, such as a read-only environment, is said by the key only), then how many changes are
pending (`3 pending`) and, after an Apply, `applied · re-run to refresh`. See [Editing
data](editing.md).

## Transactions in the results

Results read inside an open transaction are labelled uncommitted. The tab bar and the status
bar show an open transaction, an aborted one, and "rollback required"; see
[Manual commit](manual-commit.md).
