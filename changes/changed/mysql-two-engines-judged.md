- On MySQL and MariaDB, a session whose server's greeting names one engine and whose
  `VERSION()` names the other (a proxy in between may write the greeting) now has its
  statements checked by the rules of both engines, and the stricter answer counts. On such a
  session a read-only profile refuses a call either engine would look up as a stored function
  (`BIN_TO_UUID` on MariaDB, a quoted `` `DATABASE`() `` on MySQL), a statement either engine
  finds dangerous asks before it runs, results are run again or counted only when both
  engines allow it, the filter bar lets a call run only when both do, and a MySQL version
  comment (`/*!80000 … */`) counts as possibly setting the next transaction. A statement
  that waited (for its connection, or for an answer to a question about it) is checked again
  right before it is sent, by the rules known then, and asks again if those rules ask about
  more; a tab whose session closed is checked by its connection's rules as they are now.
  Sessions whose greeting and `VERSION()` name the same engine are checked as before.
