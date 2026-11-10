- MySQL: an export, and a row count with autocommit off, no longer lock rows when the session's
  isolation level is `SERIALIZABLE`. They read one snapshot at `REPEATABLE READ` for their own
  transaction only, so they neither wait for nor hold up other sessions; the session's level is
  unchanged.
