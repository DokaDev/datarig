- PostgreSQL: a row count (`#`) outside your own transaction and an export no longer take part in
  other sessions' serialisation failures on a server, role or database whose default isolation is
  SERIALIZABLE, nor wait for a safe snapshot there. They now read at READ COMMITTED in a read-only
  transaction of their own; inside your transaction they still read there, at its level.
- PostgreSQL: when the session cannot tell whether a transaction is open, a row count without a
  time limit and an export are refused with a note, as a count with a time limit already was, so
  they never end or bypass a transaction of yours.
