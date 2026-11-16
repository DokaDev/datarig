- MySQL: for a user granted only some columns of a table, rows that look the same are no longer
  offered as identical ("just one" or all of them), since they may differ in columns the server
  does not list to that user; the change is refused, saying why, and the table's Columns group
  says some may not be listed.
