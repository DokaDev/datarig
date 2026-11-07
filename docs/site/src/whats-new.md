# What's new

What changed in each release, newest first. For 0.36.0 and earlier, see the release notes on
[GitHub Releases](https://github.com/DokaDev/datarig/releases).

## 0.39.0 (2026-11-07)

datarig can tell you when a newer release is out, and shows what's new after an update.

### Added

- Once a day datarig asks GitHub whether a newer release exists (one HTTPS request to
  api.github.com, with nothing about you or your databases; through `HTTPS_PROXY` when one is
  set). A newer one is told about at the next start, with the command that updates your
  install; `Esc` closes it (it comes back after a week), `s` skips that version, `o` turns the
  checks off. `:update` says what the last check found. Turn it off with `[updates] check =
  "off"` (settings screen, Updates), `:set updates.check=off` or `DATARIG_NO_UPDATE_CHECK=1`;
  there is no check on CI (`CI` set).
- What's new: on the first start after an update, a dialog shows the notes of every version
  since the one that ran before (from the program itself, so it works offline). `:whatsnew`
  shows it any time; `[updates] whats_new = "off"` (settings screen, Updates) stops it from
  showing by itself.

## 0.38.3 (2026-11-06)

Screen text uses British spelling, and titles use a colon in place of a dash; the docs site shows new recordings of the editor, charts and plans.

### Changed

- Screen text uses British spelling (colour, materialised view; the connection form's row reads
  "Colour") and no longer uses the em dash: titles read "Changes: Result 1 · shop.users",
  "Apply changes: shop.users" and "Edit connection: name". Config keys, theme tokens and commands are unchanged (`color`, `[colors]`,
  `:explain analyze`).

## 0.38.2 (2026-11-05)

The grid shows where its cursor is and sums up a selected range on the status line; menus show what cannot run now, dimmed with why.

### Added

- While a range, rows or columns of the result grid are selected, the status line sums them
  up: their size, then for numbers the sum, average, least and greatest value (integers and
  decimals exactly), else how many distinct values, and how many are NULL. It takes the values
  the grid shows (pending changes too) and reads the rows in the background, so a big selection
  never holds up the keys.
- `:theme update [<name>]` adds to a theme file the tokens it does not list (such as tokens added
  after it was written), each under its description at the end of its section, commented out
  with the value it takes now. No line of the file changes, and nothing is written unless the
  file reads exactly as before.
- A theme file that does not list every token is said once in the status line when it is used,
  with how many and that `:theme update` adds them; again only when the tokens it does not list
  change.
- New setting `ui.background = "theme" | "terminal"` (settings screen, `:set
  ui.background=terminal`): with `terminal` the panes and the result grid's alternate rows leave
  the terminal's own background showing, so a translucent terminal shows through; text,
  selections and edit marks keep the theme's colours, and dialogs, menus and popups keep its
  surface. Turning it on with a theme that has a background of its own says so in one line.

### Changed

- The editor's completion popup opens by itself 60 ms after the last key (it waited 120 ms and
  then up to a tenth of a second more for the next timer tick), exactly at that moment. New
  setting `[editor] completion_delay_ms` (`60`, `0` for at once, `30`, `100`, `150`, `250`;
  settings screen and `:set`).
- The result grid shows where the cursor is: its row is tinted, its cell more strongly, and its
  column's header too. A selected range is drawn as one block (the separators between its
  columns included), with its rows' numbers and its columns' headers tinted. New theme tokens
  `grid_cursor_row` and `grid_cursor_header`; a theme file that sets its own background,
  surface or selection without them gets tints made of its own colours (when those are RGB).
- The action menu shows an action that would act there but can't now dimmed, with why in a few
  words where its key would be (a read-only environment, a result that can't be edited, an edit
  that doesn't fit the cell, a table tab's Save, paging that closed), instead of leaving it out.
  The keys and the wheel step over it; a click on it says the reason in full and keeps the menu
  open.

## 0.38.1 (2026-11-05)

Copy and paste inside the grid: `y` keeps the copied cells as they are, and `p` pastes them as new rows (again and again); previous page moved from `p` to `[`.

### Added

- The cells last copied with `y` or `Y` in the result grid paste back exactly as the grid
  showed them (NULL stays NULL), in any result in any tab, without reading any clipboard:
  `p` adds them as new rows (each press another copy), `g p` pastes them over the cells at
  the cursor. Each is one step `u` undoes.
- A range pasted into the grid skips a first line that names the columns it lands in (a copy
  with its header line), and says so; `Space d V` reads it again keeping that line.

### Changed

- Previous page moved from `p` to `[` in the result grid (and `]` is next page, as `n` still
  is); `p` now pastes the grid's copy as new rows.
- The Changes tab lists each problem under its row's line, without the row's name the line
  above gives, and names a row the same way wherever a problem is about it (the statements,
  the Apply's messages and failures, a paste's message, the cell editor and the inspector):
  `new 1`, `id=5` or `row 31`, not the row's number in the grid. A new row lists only the
  values set, then how many columns are left to their default (`DEFAULT ×2`). Control
  characters in values, column names and the table's name show as `\n`, `\r`, `\t` or
  `\x07`, and characters that turn the text's direction as `\u{202e}`, never raw; the
  statements copied are unchanged.
- The result grid marks every new row an Apply is refused for: a cell left to a default its
  column lacks (`NOT NULL`, no default) reads `! value needed` in the invalid-value colour
  instead of the dim `default`, its column widened to fit, and the row's gutter shows `!`
  instead of `+`, as for a value its column does not take; so does a new row of a table with
  such a column the result does not show. The badge counts these cells, and the cell editor
  and the inspector say why. Invalid values keep their colour under the cursor.
- A new row in the result grid is tinted as a whole (theme token `edit_inserted_row`) and
  numbered after the rows read, with `+` (or `!`) after its number; rows an Apply added are
  numbered too. Its cells left to their default show what the server will put there instead of
  a dim `default`: the default's value (`ACTIVE`, `0`), `<generated>` for a number the table
  draws or a generated column, `<default: now()>` for another default, `NULL` where the column
  takes it. The Apply still leaves them to the server.

## 0.38.0 (2026-11-04)

Pending changes get their own Changes tab, Apply asks on the status line, and a range copied elsewhere can be pasted into the result grid.

### Added

- A range pasted into the result grid with the terminal's own paste (`Cmd+V`, `Ctrl+Shift+V`)
  becomes pending changes from the cursor or the selection's corner: read as TSV, CSV, one
  column or one value (the status line names the format, and `Space d V` reads it again as the
  next one; text with commas and no tab asks first whether it is CSV, `c`, or one column, `1`), rows past the last added as new rows, one step `u` undoes. One value fills every
  selected cell. A paste that does not fit (too wide, a column that takes no value, a row marked
  for deletion, rows past the last row read while the server has more) is refused whole, with
  the reason; one of more than 1,000 rows asks first. `\N` pastes NULL (setting
  `edit.paste_null`); an empty field is an empty value. Values a column does not take are kept,
  marked `!` in the gutter (theme token `edit_invalid`) and listed first in the Changes tab, and
  the changes are not applied until they are fixed. `Space d v` pastes the system clipboard with
  `clipboard = "system"`, or `"auto"` outside SSH. For about a second after a status-line question or a
  waiting Apply ends without its answer, `y`, `n`, `c` and `1` do nothing in the results and say
  they were ignored, so a late `y` never copies a cell over the clipboard.

### Changed

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

## 0.37.1 (2026-11-04)

This patch adds the edits of a cell to the grid's right-click menu and makes the preview of
pending changes readable.

### Added

- The grid's right-click menu (and `Space Space`) lists the edits of the cell: edit it, set it
  to NULL or DEFAULT, add or copy a row, mark it to be deleted, and, with changes pending, undo,
  preview and discard. Only the edits that can act on the cell are listed.

### Changed

- The preview of pending changes shows each statement in the editor's syntax colors: a long one
  wraps over as many lines as it needs, its expected count on its first line, and a literal of
  more than a few hundred bytes shows its start and end around how many characters are left
  out. On MySQL, a statement that compares text by its bytes (`_utf8mb4 X'…'`) is followed by a
  dim `↳ set column = 'text' · where column = 'text'` line that reads the values back; that line is
  not SQL, and Copy SQL copies the statements whole without it.

### Fixed

- The keys on a dialog's bottom border no longer run into its corner: a border cell is left on
  each side, and keys too long for the dialog are cut with `…`.
- `:set ui.tips=on` is taken as `always`, as the config file already reads `tips = "on"`.

## 0.37.0 (2026-11-03)

This release adds editing a result's data in the grid: change cells, add, copy and delete rows,
then apply the changes after a preview of their exact statements.

### Added

- Edit a result's data in the grid: change cells (one line, or a box for values of several
  lines and JSON), add and clone rows, mark rows to delete, undo and redo. The changes stay
  pending until a preview shows the statements that apply them; `y` there runs them in one
  transaction, each row checked first. See Editing data in the guide.

### Changed

- MySQL: the rows affected by an `UPDATE` (and by `INSERT … ON DUPLICATE KEY UPDATE`) are the
  rows it matched, as with PostgreSQL, so a row set to the values it already had counts too;
  `ROW_COUNT()` counts them the same way. The server's info line still says how many changed.
- Tips keep showing: the tips of an empty place take turns and come back, and a key you have
  already pressed comes up less often instead of never again. `[ui] tips` is now `"always"`
  (the default), `"until_used"` (the earlier behaviour) or `"off"`; `"on"` in an existing
  config reads as `"always"`, and `true` and `false` read as `"always"` and `"off"`.

### Fixed

- MySQL: Copy as SQL no longer refuses text and JSON values of a tab's first result as "read
  across a change of the session's time zone or character set": the values a session reports as
  it connects no longer count as a change. An export of that result as SQL to a file is no
  longer refused for it either.

## 0.36.2 (2026-10-31)

### Changed

- The hint line, the menus, the welcome panel and the messages name the same key for an action
  as the tips do: a key of your own first, else one that needs no function key (`Space ?` for
  the keyboard help, `Space Space` for the action menu), and `F1` where `Space` types text.
- Startup SQL may set the server's idle limit (PostgreSQL `idle_session_timeout`, MySQL
  `wait_timeout`). Its value is then the idle disconnect of those sessions in place of the
  environment's `idle_disconnect`: the app closes the tab after that long, warns about an open
  transaction a minute before, and `:session` shows the limit with what it overrides. Behind a
  pooler PostgreSQL's is not sent and the environment's limit stays.

## 0.36.1 (2026-10-30)

The documentation now lives at <https://dokadev.github.io/datarig/>, and `datarig --help` names
MySQL next to PostgreSQL.
