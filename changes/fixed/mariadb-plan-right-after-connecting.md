- MariaDB: asking for a statement's plan right after a session connected no longer sends MySQL's
  `EXPLAIN FORMAT=TREE` (which MariaDB refuses); the session says it has no plan format before
  it says it is connected.
