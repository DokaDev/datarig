- A read-only PostgreSQL profile no longer refuses a cast of a quoted value written as a call
  to a built-in type, such as `uuid('…')` or `pg_catalog.jsonb('{}')`.
