- On MariaDB, and on a session whose greeting and `VERSION()` name different engines, text that
  holds a MariaDB-only executable comment (`/*M! … */`, with or without a version) is now
  refused before anything of it is sent, with a message that says why. Write the comment as
  `/*! … */` instead. On MySQL such text runs as before, since MySQL reads it as a plain comment.
