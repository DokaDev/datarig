- On a table tab whose rows are filtered, `#` offers the planner's estimate of the matching
  rows (`page 3 / ~12 (planner)`, nothing read) or the exact count. On any result, a count now
  stops after 30 seconds or after waiting 2 seconds (1 on MySQL) for another session's lock,
  and says the number is not known (`[results] count_timeout`, `"off"` for neither limit); on
  PostgreSQL inside your own transaction it has no limit, as before.
