- On PostgreSQL, the explorer, completion, key, table structure, sort key and DDL reads now name
  every operator, function and type with its schema, so an operator of another schema on the
  session's `search_path` that matches their argument types more closely is no longer used in
  place of the server's own.
