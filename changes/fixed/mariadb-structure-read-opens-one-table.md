- MariaDB: reading a table's structure no longer opens every table of the server to find that
  table's foreign keys, which was slow on servers with many tables and could wait behind another
  table's lock.
