- Startup SQL refuses a setting that asks before it runs when typed (such as
  `foreign_key_checks = 0` or `session_replication_role = replica`), since Startup SQL runs
  without asking.
- On a read-only PostgreSQL profile a typed setting of an extension or a custom one (such as
  `SET app.tenant = '1'`) is refused; set it in the profile's Startup SQL, where it is still
  allowed.
- Typed `SET session_replication_role` and `set_config()` of it ask before they run.
- Behind a transaction pooler, a profile whose Startup SQL holds `SET ROLE` (or `SET SESSION
  AUTHORIZATION`) no longer connects, since the role could not hold for the session.
