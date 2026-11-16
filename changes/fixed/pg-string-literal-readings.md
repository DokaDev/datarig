- PostgreSQL: copied and exported SQL, export chunks and DDL text write a string holding a
  backslash as `E'…'`, so it reads the same whatever `standard_conforming_strings` is. A query
  whose strings read differently under that setting is not re-run or counted automatically, and
  Startup SQL refuses such a statement.
