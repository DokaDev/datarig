- MySQL: applying edits on a server without strict mode no longer reports success when the
  server changed what was sent and only warned (text cut to the column's length, a character
  the column cannot hold, a date made zero, a member of a SET dropped). Such a change fails with
  the server's warnings and nothing is applied. While the session's `sql_notes` is off, edits
  are not sent, since the server would then not report a value it rounded. When undoing a failed
  change leaves some of it in place (a table that does not take part in transactions, such as
  MyISAM), the message says so instead of saying nothing was applied.
