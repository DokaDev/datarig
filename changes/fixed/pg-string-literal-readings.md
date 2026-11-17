- PostgreSQL: copied and exported SQL, export chunks and DDL text write a string holding a
  backslash as `E'…'`, so it reads the same whatever `standard_conforming_strings` is. A query
  with a backslash or a `U&'…'` string, which that setting may read differently, is not re-run
  or counted automatically, and Startup SQL refuses such a statement. The safety checks read a
  national string (`N'…'`) with that setting off as the server does.
