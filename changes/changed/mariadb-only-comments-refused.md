- On MariaDB, and on a session whose greeting and `VERSION()` name different engines, a statement
  whose text holds the characters `/*M!` (a MariaDB-only executable comment, with or without a
  version) is now refused before anything of it is sent, with a message that says why; so is
  such text in a string or a quoted name. Write the comment as `/*! … */` instead. Values datarig
  writes itself, such as an edited cell, are not affected. On MySQL such text runs as before,
  since MySQL reads it as a plain comment.
