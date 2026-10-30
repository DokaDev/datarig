# Export

`Space r e`, the results menu, or `:export` alone opens the export dialog for the active tab's
result: which rows, the format, CSV's options, and the file.

```text
{{#include ../screens/export-dialog.txt}}
```

Without the dialog:

```text
:export [format] [all|fetched|selection] [delim= quote= header= null=] [path]
```

Every row is the default. With no format, the file's extension names it (`.xlsx` and
`.parquet` included). Examples:

```text
:export csv fetched delim=semicolon ~/Downloads/orders.csv
:export ~/Downloads/orders.parquet
```

An export runs in the background, one per tab, with its rows, size and time in the tab's
status.

## Formats

| Format | `:export` name | Options |
|---|---|---|
| CSV | `csv` | `delim=` (one character, or `tab`, `comma`, `semicolon`, `pipe`), `quote=` (`necessary`, `always`, `non_numeric`, `never`), `header=` (`on`/`off`), `null=` (the text written for NULL) |
| TSV | `tsv` | `header=` |
| Excel | `xlsx` (also `excel`) | `null=` |
| Parquet | `parquet` | none |
| JSON, indented JSON, JSON Lines | `json`, `json_pretty`, `jsonl` (also `ndjson`) | none |
| Markdown, HTML, XML | `markdown` (also `md`), `html`, `xml` | none |
| SQL `INSERT`, `UPDATE` | `insert` (also `sql`), `update` | none; only where the result's keys allow them, as with copy |

### Excel (.xlsx)

- The header row is bold and stays in view. A sheet holds 1,048,575 rows under its header; the
  rows after that go on in `Sheet2`, `Sheet3`, …, each with the header again.
- A value of a number column is a number only when Excel holds it exactly: at most 15
  significant digits, within a double's normal range. Anything else (a `bigint` past 15 digits,
  a `numeric` with more digits, `NaN`, `Infinity`) is text, every digit kept. Booleans are
  booleans. Everything else, dates and timestamps included, is the text the database wrote.
  NULL is an empty cell, or the `null=` text.
- A sheet holds 16,384 columns: a wider result is refused before anything is written. A cell
  holds 32,767 characters as Excel counts them (UTF-16 units: an emoji counts two); a longer
  value stops the export there, naming its column and row, and the workbook keeps the rows
  before it.
- Rows go to temporary files in the system's temporary folder (`TMPDIR`) as they come, so
  memory stays at about one row. The workbook is made from them at the end ("Making the
  workbook…" in the status). It is a ZIP64 container, so a workbook past 4 GiB is fine.

### Parquet

- Typed columns, every one nullable, compressed with Snappy. Integers are INT32 or INT64
  (MySQL's `bigint unsigned` an unsigned INT64), floats FLOAT or DOUBLE, booleans BOOLEAN, dates
  DATE, times without a zone TIME (a time with a zone is text) and timestamps TIMESTAMP in
  microseconds, binary data its bytes. PostgreSQL's `timestamptz` is converted to UTC; MySQL's
  `TIMESTAMP` is the session's local time and is not converted. Everything else, decimals
  included, is the text the database wrote, every digit kept.
- A column of a PostgreSQL domain is known by the domain's name only, so a domain over an
  integer is INT64 and one over a float DOUBLE.
- A value the column's type cannot hold (`infinity`, MySQL's zero date, `24:00:00`) stops the
  export there, naming its column and row; it is never written as NULL.
- Rows are written in row groups of at most 65,536 rows or 32 MiB of values, so memory stays
  at about one group.
- Two columns of one name are written as they are; some readers (Spark, some pandas paths)
  refuse or rename them, so give them aliases.

## Where the rows come from

- **Fetched rows** and **the selection** are read from what is already loaded; nothing is sent
  to the server.
- **All rows** of a result with more on the server need the statement to run again.
  - A table tab showing its whole table, with a primary key or a unique key on NOT NULL
    columns and no open transaction, is read in chunks by that key, each a read of its own that
    waits at most 5 s for a lock (rows changed meanwhile may be read in a later state).
  - Any other result runs its statement once and streams it: a plain `SELECT` that is safe to
    repeat goes ahead, anything else asks first. Read-only and prod-like environments ask first
    too, because the statement holds a read lock and a snapshot until the export finishes.
- An export only reads: a statement that writes or locks rows is refused unsent. Outside a
  transaction of yours each statement it runs is a read-only transaction of its own
  (`BEGIN READ ONLY` on PostgreSQL, `START TRANSACTION READ ONLY` on MySQL), so its rows are one
  snapshot and a function that writes fails. Inside your open transaction it reads there and
  sees what you changed; on PostgreSQL under a savepoint, so a failed or cancelled export does
  not abort your transaction. An aborted transaction refuses an export until you roll it back.
- A slow file holds the statement back instead of piling rows up in memory. A server may end a
  slow export (MySQL's `net_write_timeout`, 60 s by default; PostgreSQL's
  `idle_in_transaction_session_timeout` or a pooler's idle timeout).

## Choosing where to save

The path starts as `~/Downloads/<table or tab>-<YYYYMMDD-HHMMSS>.<ext>` (in the home folder when
there is no `~/Downloads`), and while it is that name a format change gives it the new
extension. `~` is the home folder; a relative path goes in `~/Downloads`.

- `Tab` in the path field completes a name: first the longest text the matching names share,
  then (`Tab` again) a list of them, then each in turn.
- `Ctrl+O` in the dialog (or its `[Browse…]` button) opens a tree of the file system. `Up` and
  `Down` (`j`/`k`) move, `Right` and `Left` (`l`/`h`) open and close a folder, `../` goes up.
  `Enter` on the top row ("Save here") or `Ctrl+S` anywhere puts the selected folder in the
  field; `Enter` on a file takes that file. `n` makes a folder, `.` shows hidden names, `/`
  filters, `Esc` leaves the field as it was. Files are dimmed: they are what a file of that
  name would replace.
- A folder that cannot be listed is never saved into. A path that names a folder is refused:
  name a file.

## When an export stops

- The file is written as `<path>.partial`, created new, and renamed to the path when the export
  succeeds. An existing file is never replaced without asking (`y` replaces it, `Enter` keeps
  it). A `.partial` file left by an earlier export is not written over; move or delete it.
- A cancel stops the statement on the server and keeps the `.partial` file with the rows
  written so far, ended so that it reads (a JSON array closed, an Excel workbook of those rows,
  a Parquet file with its footer), except while an Excel workbook is being made: a cancel then
  keeps nothing. An export that wrote no rows leaves no file.
- A failure says why and where the rows are:
  - text formats keep the `.partial` file; one that failed to be written (a full disk) may end
    in the middle of a row;
  - a Parquet value that does not fit, or an Excel value too long for a cell, keeps a file of
    the rows before it that reads, and so does an Excel or Parquet export stopped by the server
    or the session;
  - a Parquet file that could not be written, or an Excel workbook that could not be made, is
    removed.

## Limits

- Excel needs room in `TMPDIR` for the sheets' uncompressed XML until the workbook is made, as
  well as room for the workbook itself. Point `TMPDIR` at a larger disk for a very large export.
- Excel's dates and timestamps are text, not Excel dates. Parquet has no DECIMAL columns;
  decimals are exact text.
- A local export cannot be cancelled while the tab runs another statement.
- Exports are tested on macOS and Linux; running datarig on Windows is untested.
