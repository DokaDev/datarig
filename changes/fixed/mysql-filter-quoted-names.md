- On MySQL and MariaDB, filters, filters by value, sorts and the column names completion
  offers in the filter dialog always quote column names, so a column named like a function
  (for example `current_role`) or a reserved word is read as the column. A condition typed
  with such a function's name bare is kept as written.
