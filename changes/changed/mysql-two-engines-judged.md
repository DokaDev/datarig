- On MySQL and MariaDB, a session whose server's greeting names one engine and whose
  `VERSION()` names the other (a proxy in between may write the greeting) now has its
  statements checked by the rules of both engines, and the stricter answer counts. On such a
  session a read-only profile refuses a call either engine would look up as a stored function
  (`BIN_TO_UUID` on MariaDB, a quoted `` `DATABASE`() `` on MySQL), a statement either engine
  finds dangerous asks before it runs, results are run again or counted only when both
  engines allow it, the filter bar lets a call run only when both do, and a MySQL version
  comment (`/*!80000 … */`) counts as possibly setting the next transaction. Sessions whose
  greeting and `VERSION()` name the same engine are checked as before.
