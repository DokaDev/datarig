- On MySQL and MariaDB, a session whose server's greeting names one engine and whose
  `VERSION()` names the other (a proxy in between may write the greeting) now has its
  statements checked by the rules of both engines, and the stricter answer counts. On such a
  session a read-only profile refuses a call either engine would look up as a stored function
  (`BIN_TO_UUID` on MariaDB, a quoted `` `DATABASE`() `` on MySQL), a statement either engine
  finds dangerous asks before it runs, results are run again or counted only when both
  engines allow it, the filter bar lets a call run only when both do, and a MySQL version
  comment (`/*!80000 … */`) counts as possibly setting the next transaction. Once any
  session of a connection found this, every statement of that connection is checked by both
  engines' rules until it connects again. A statement that waited (for its connection, or for
  an answer to a question about it) is checked again right before it is sent, and asks again
  for anything the earlier answer did not cover; waiting statements of several tabs are asked
  about one at a time.
  Sessions whose greeting and `VERSION()` name the same engine are checked as before.
