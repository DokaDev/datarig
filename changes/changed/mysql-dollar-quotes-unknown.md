- On MySQL and MariaDB, a server whose greeting and `VERSION()` disagree on whether it reads
  dollar quotes (`$$…$$`; a proxy in between may write the greeting) no longer has them read
  as the greeting says. On such a session, text that could open one (a `$`, letters, digits or
  `_`, then a `$`, even inside a string or a comment) is not sent: the run fails with a message
  that says why. Text with a single `$`, and every session whose greeting and `VERSION()`
  agree, run as before.
