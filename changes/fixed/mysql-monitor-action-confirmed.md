- MySQL Monitor: cancelling or ending a session is now sent only on a monitor connection the
  server confirms is direct, and only to a session that can be told apart from a later one given
  the same id. This needs the session's thread to be readable: grant the monitoring account
  `SELECT` on `performance_schema.threads`, then reopen the Monitor tab; otherwise nothing is sent
  and the Monitor says so. When the connection cannot be confirmed, the actions are not offered.
  The check before the action also compares the session's account.
