- MySQL: the rows affected by an `UPDATE` are the rows it matched, as with PostgreSQL, so a row
  set to the values it already had counts too. The server's info line still says how many
  changed.
