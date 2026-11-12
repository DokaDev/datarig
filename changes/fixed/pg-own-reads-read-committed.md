- PostgreSQL: a row count (`#`) outside your own transaction and an export no longer take part in
  other sessions' serialization failures on a server, role or database whose default isolation is
  SERIALIZABLE. They now read at READ COMMITTED in a read-only transaction of their own; inside your
  transaction they still read there, at its level.
