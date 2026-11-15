- PostgreSQL: cancelling an Apply inside an open transaction no longer aborts the transaction
  when the cancel arrives before the Apply's savepoint is set; the Apply stops before its first
  statement and the transaction keeps what it held.
