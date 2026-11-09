- The server monitor no longer reads as idle when the account sees only some sessions: the
  longest running sessions are headed "this account" (MySQL without `PROCESS`) or "visible
  sessions" (PostgreSQL with other roles' sessions hidden), their empty text says the same, and
  a PostgreSQL sessions count with hidden sessions shows as the least there are.
- The server monitor names the server's role in words that cannot be taken for the profile's
  read-only mode: `primary` or `standby` on PostgreSQL, `server takes writes` or `server
  read-only` on MySQL.
