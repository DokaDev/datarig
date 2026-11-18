- On MariaDB, a `JSON_TABLE` call counts as the table function only with its real `COLUMNS`
  clause (the path's string, then `COLUMNS (`): a call with a column or a call named `columns` in
  its arguments (`JSON_TABLE(t.columns)`) now counts as a call of the database's own function, as
  MariaDB reads it.
