- PostgreSQL: the DDL shown for a view runs as written on PostgreSQL 17.12 and later. It no longer
  sets column options on the view's columns, which the server refuses, nor writes out options the
  server keeps for itself on any relation's columns.
