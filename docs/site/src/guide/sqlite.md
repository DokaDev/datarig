# SQLite

**Versions.** The SQLite library is built into datarig, so there is no server and no version to
install. A file that SQLite cannot read is reported when the profile connects.

A SQLite profile has `driver = "sqlite"`, and its `path` is the database file, written as an
absolute path. It needs no host, port, login, TLS or SSH tunnel: those fields are not shown.

## What works

- **Connecting** to the file, and the test connection. A missing file is an error: datarig
  never creates it, and a relative path is refused.
- **Read-only and read-write profiles.** A read-only profile (a read-only [environment](environments.md))
  opens the file read-only, so the database itself refuses every write. A read-write profile
  asks before a statement that changes rows, a table or the schema.
- **Attached databases are not supported.** `ATTACH` and `DETACH` are refused, so one profile
  is one file.
- **Running statements**, several at once. Each statement is answered in turn, and a run is
  cancelled with the usual key: the running statement stops.
- **The schema tree**: the file's tables and views, their columns, and the `CREATE` text of a
  table, view or index.
- **Export** of a result, with the usual formats. A result is read in one piece, so a very large
  result takes memory; the rows of an export are streamed to the file.

## What is not there yet

- Schemas other than `main`, and the connection contexts.
- Table structure, the key view, and the explain plan.
- Manual commit: a `BEGIN` you run is kept by the database, but datarig does not show the open
  transaction.
- Edits of a result's rows.
- Start-up SQL presets for SQLite profiles: a preset for SQLite has no statements.
