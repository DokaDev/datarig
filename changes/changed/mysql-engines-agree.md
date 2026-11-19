- On MySQL and MariaDB, every session now reads the server's own version line (`VERSION()`) before
  anything else and checks that it names the same engine as the server's greeting. When they
  disagree (a proxy in between may rewrite one of them), or the line gives no answer, the
  connection fails with an error that says both, and nothing is run on the session: not its
  settings, its read-only check or its start-up statements. The test connection checks the same.
