- PostgreSQL: `g p` no longer pastes a range of dates or times, or another value read as the
  server's text, as other dates when the session's `DateStyle`, `IntervalStyle` or `TimeZone`
  is not the one it was copied in (`[02/01/2026,05/01/2026)` copied in `SQL, DMY` and applied
  in `ISO, MDY` stored 1 February for 2 January); such a paste is refused, saying in which
  settings the cells were written and in which the session reads them. Text written in ISO
  pastes as before.
