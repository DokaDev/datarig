- A SQLite profile written by hand before the SQLite driver existed, with its file in `database`,
  has the file moved to `path` when datarig starts, and `database` is left empty. Before the
  file is rewritten, a copy of it is kept as `config.toml.sqlite.bak` (an existing copy is never
  overwritten). Every other profile, and every other driver, is read and written as before.
