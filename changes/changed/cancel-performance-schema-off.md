- On MySQL and MariaDB with `performance_schema` off (MariaDB's default), cancelling a statement
  that is running now says in the run's Messages, when you cancel, that it closes the
  connection, as the server cannot stop the statement there.
