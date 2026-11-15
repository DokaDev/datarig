- PostgreSQL: cancelling a row count, a count of a column's values, a statement run again to
  page on, an export or an `EXPLAIN ANALYZE` inside an open transaction no longer aborts the
  transaction when the cancel arrives before the read's savepoint is set; the read ends
  cancelled and the transaction keeps what it held.
