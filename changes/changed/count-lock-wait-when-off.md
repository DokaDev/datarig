- On MySQL and MariaDB, a row count or a count of a column's values with
  `[results] count_timeout = "off"` no longer waits without end for a lock another session
  holds: it gives up after waiting 1 second for each lock, as a count with a time limit does.
  `"off"` still removes the time limit on the count itself.
