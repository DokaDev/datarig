- MySQL and MariaDB: a cancel of a running statement is now sent only once the server confirms it
  reaches the tab's own session. When that cannot be confirmed (for example on MariaDB with
  performance_schema off, or through some proxies), datarig closes the tab's connection instead and
  says so; the statement may then still finish on the server and, outside a transaction block,
  commit what it changed.
