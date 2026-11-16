- On PostgreSQL, the statements datarig writes around a result or a table filter to count rows
  or count each value of a column now name their functions and operators with their schema
  (`pg_catalog.count(*)`), so they do the same whatever the session's `search_path` holds. The
  count shown in the run log reads `pg_catalog.count(*)` accordingly.
