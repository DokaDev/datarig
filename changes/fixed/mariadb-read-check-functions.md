- MariaDB: the read check knows which functions the server provides. A call of a function
  that MySQL has but MariaDB does not (`ANY_VALUE`, `REGEXP_LIKE`, `BIN_TO_UUID`, ...) names a
  stored function of the current database on MariaDB, so it is now treated as any unchecked
  function call there: a read-only profile refuses it, and a query that calls one is not run
  again for the next page. MySQL is unchanged.
