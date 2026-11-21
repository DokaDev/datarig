- On MariaDB, a session whose version datarig cannot be sure of (a proxy in between reports
  another number than the server, or the server's version line has none) now names its
  transaction settings `tx_read_only` and `tx_isolation`, which every supported MariaDB
  version has. Such a session to a MariaDB before 11.1 behind a greeting of 11.1 or later
  used to fail to connect. Sessions whose version is known, and sessions whose greeting names
  another engine than the server, are unchanged.
