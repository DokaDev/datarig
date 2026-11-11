- Groundwork for the table filter dialog: the server can count how many rows hold each value of
  a column under the table's other filter conditions, with the same time limit, lock rules and
  refusals as the row count, and a table's columns are read from the catalogue together with its
  keys, without locking the table.
