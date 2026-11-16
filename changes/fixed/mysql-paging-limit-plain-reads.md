- MySQL and MariaDB: a last statement that is not a plain read, such as `SELECT … INTO OUTFILE`,
  `SELECT … INTO @variable` or a `SELECT` that calls a stored function, is no longer run under
  the row limit used to show the first page. That limit stopped the statement after a page of
  rows, so the file was written in part, or the function ran for those rows only, while the run
  reported success. Such a statement now runs to its end and its result is read whole.
