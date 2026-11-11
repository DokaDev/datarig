- The LIMIT of a table tab's filter caps the rows the tab reads: the pages stop there and
  nothing is fetched past it, the results title says `limit 100`, and a count or an export of
  every row reads the same capped set. A limit of 0, or one that is not a whole number of rows,
  is refused.
- The applied condition, order and limit are kept with the workspace and with a closed tab
  brought back; they return to the tab without running and are checked again when the tab
  runs.
- On MySQL, a table tab asks before it runs when its reads would lock rows (isolation level
  SERIALIZABLE, or not known, with autocommit off or a transaction open, including one opened
  for the read with manual commit), on the session the read is sent on; a page read again or a
  count in that state needs that answer first. A condition may not call `ROW_COUNT()` or
  `FOUND_ROWS()`: in a table tab they would read the outcome of a statement the app sent.
- A table tab brought back with an order waits for the table's keys before its first run; the
  wait counts as a running query and `Ctrl+C` cancels it.
