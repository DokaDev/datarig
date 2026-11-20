- On MySQL and MariaDB, every session now reads the server's own version line (`VERSION()`)
  before anything that depends on the engine. When it gives no answer (no row, `NULL`, or a
  blank line), the connection fails with an error that names what the server's greeting says,
  and nothing else is run on the session: not its settings, its read-only check or its
  start-up statements. The test connection fails the same way. A server whose greeting and
  version line name different engines (a proxy in between may write the greeting) connects as
  before.
