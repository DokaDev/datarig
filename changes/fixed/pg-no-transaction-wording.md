- PostgreSQL: a statement that cannot run inside a transaction (`VACUUM`, `CREATE DATABASE`, …)
  and failed in one the app opened is blamed on the tab's schema only when the schema is what
  needed the transaction; otherwise the message says the app opened one for it.
