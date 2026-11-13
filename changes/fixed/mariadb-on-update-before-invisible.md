- MariaDB: column attributes it lists after a comma are read. A column with `ON UPDATE
  CURRENT_TIMESTAMP` that is also `INVISIBLE` no longer gets a stray comma in its default (a
  table's definition built from its structure ran into a syntax error), and an invisible
  generated column (a system-versioned table's row start and end, too) shows as generated
  instead of as a plain column.
