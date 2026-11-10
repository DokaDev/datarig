- On PostgreSQL, the statements datarig sends itself now name their operators, functions and
  types with their schema, so a `search_path` listing a schema ahead of `pg_catalog` can no
  longer change what they do, including which rows an edit finds. The SQL shown for an edit
  therefore reads `OPERATOR(pg_catalog.=)` where it read `=`.
