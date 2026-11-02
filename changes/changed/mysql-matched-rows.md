- MySQL: the rows affected by an `UPDATE` (and by `INSERT … ON DUPLICATE KEY UPDATE`) are the
  rows it matched, as with PostgreSQL, so a row set to the values it already had counts too;
  `ROW_COUNT()` counts them the same way. The server's info line still says how many changed.
