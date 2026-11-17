- A read-only PostgreSQL profile now runs the built-ins PostgreSQL 18 adds when the call
  names `pg_catalog`, such as `pg_catalog.uuidv7()`. Called without it (`uuidv7()`), they are
  refused with a hint saying how to call them, unless `read_only_functions` lists them.
