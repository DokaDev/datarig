- MariaDB: `EXPLAIN ANALYZE` of a write no longer fails with "Unknown system variable
  'partial_revokes'" from datarig's own check before it; MariaDB now answers the statement
  itself (it has no `EXPLAIN ANALYZE`; its `ANALYZE` statement is the equivalent).
