- PostgreSQL: cancelling a row count, a count of a column's values, a statement run again to
  page on, an export or an `EXPLAIN ANALYZE` inside an open transaction no longer aborts the
  transaction when the cancel arrives before the read's savepoint is set; the read ends
  cancelled and the transaction keeps what it held.
- PostgreSQL: a cancel the server does not confirm within 10 seconds (a slow tunnel) is said in
  Messages: it may still stop what runs next in the session, an open transaction included.
