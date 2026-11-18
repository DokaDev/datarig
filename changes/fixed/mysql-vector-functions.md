- On MySQL, `STRING_TO_VECTOR`, `VECTOR_DIM` and `VECTOR_TO_STRING`, which only MySQL 9.0 and
  later have, now count as calls of the database's own functions on every server: a read-only
  profile refuses them.
