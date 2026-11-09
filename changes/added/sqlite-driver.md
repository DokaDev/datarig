- SQLite databases: a profile with the driver `sqlite` opens one database file, set by its `path`
  (an absolute path). Queries, the schema tree, the table and view definitions, cancelling a
  running statement and the test connection all work. A read-only profile opens the file
  read-only, so the database refuses every write; the file is never created, and `ATTACH` and
  `DETACH` are refused. Existing profile files read unchanged.
- Exporting a SQLite result writes every row, streamed to the file; the file is written as
  `<name>.partial` and renamed when the export ends. See the SQLite page of the guide.
