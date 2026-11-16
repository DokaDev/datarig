- On PostgreSQL a read-only profile now refuses a statement that calls a function by a name that
  is not a built-in's, naming the function, as a read-only MySQL profile already did. Functions
  reached through a view or a trigger are still stopped only by the read-only transaction.
