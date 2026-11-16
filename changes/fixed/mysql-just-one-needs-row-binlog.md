- MySQL: "just one" of identical rows is no longer applied while the session's binary log keeps
  statements (`binlog_format = STATEMENT`), where a replica could change a different one of the
  rows; the Apply sends nothing and says why. `ROW` and `MIXED` work as before.
