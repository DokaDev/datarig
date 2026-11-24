- Copying rows as SQL and exporting them as SQL no longer write a TIMESTAMP with a time zone
  offset for a server that does not read one (MariaDB, MySQL before 8.0.19), or when the server
  version is not known (a proxy that rewrites the server's version line, or a version line
  datarig cannot read): such a value is refused with the reason (Aurora MySQL 3 reports a MySQL
  8.0 version, so it still writes the offset),
  and CSV or JSON export still works. An SQL export that reads the table again also stops when a
  column's type changes while it runs.
