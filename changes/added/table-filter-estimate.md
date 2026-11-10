- On a table tab whose rows are filtered, `#` offers the planner's estimate of the matching
  rows (`page 3 / ~12 (planner)`, nothing read) or the exact count. A count now stops after 30
  seconds (`[results] count_timeout`, `"off"` for no limit) or after waiting 2 seconds for
  another session's lock, and says the number is not known; on PostgreSQL inside your own
  transaction it has no limit, as before.
