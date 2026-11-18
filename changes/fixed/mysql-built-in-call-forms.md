- On MySQL and MariaDB, a call of a built-in function that the server reads as the built-in only
  when its name is unquoted and directly followed by `(` (such as `NOW`, `COUNT` or `TRIM`) now
  counts as a call of the database's own function when written otherwise (`now ()`,
  `` `now`() ``), as the server reads it: a read-only profile refuses it and the table filter
  asks about it, as for any function that is not a built-in.
- On MariaDB, a call of `lateral()` or `of()`, and a `JSON_TABLE` call outside a `FROM` with its
  `COLUMNS` clause, now count as calls of the database's own functions, as MariaDB reads them.
- A column type with a precision in `JSON_TABLE`'s `COLUMNS` (`DATETIME(6)`, `NCHAR(10)`) is no
  longer taken for a function call, so such a query is a plain read again on a read-only profile.
