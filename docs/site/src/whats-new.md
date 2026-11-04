# What's new

What changed in each release, newest first. For 0.36.0 and earlier, see the release notes on
[GitHub Releases](https://github.com/DokaDev/datarig/releases).

## 0.37.1 — 2026-11-04

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

## 0.37.0 — 2026-11-03

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

## 0.36.2 — 2026-10-31

### Changed

- The hint line, the menus, the welcome panel and the messages name the same key for an action
  as the tips do: a key of your own first, else one that needs no function key (`Space ?` for
  the keyboard help, `Space Space` for the action menu), and `F1` where `Space` types text.
- Startup SQL may set the server's idle limit (PostgreSQL `idle_session_timeout`, MySQL
  `wait_timeout`). Its value is then the idle disconnect of those sessions in place of the
  environment's `idle_disconnect`: the app closes the tab after that long, warns about an open
  transaction a minute before, and `:session` shows the limit with what it overrides. Behind a
  pooler PostgreSQL's is not sent and the environment's limit stays.

## 0.36.1 — 2026-10-30

The documentation now lives at <https://dokadev.github.io/datarig/>, and `datarig --help` names
MySQL next to PostgreSQL.
