- On MySQL and MariaDB, a call of a built-in function that the server reads as the built-in only
  when its name is unquoted and directly followed by `(` (such as `NOW`, `COUNT` or `TRIM`) now
  counts as a call of the database's own function when written otherwise (`now ()`,
  `` `now`() ``), as the server reads it: a read-only profile refuses it and the table filter
  asks about it, as for any function that is not a built-in.
