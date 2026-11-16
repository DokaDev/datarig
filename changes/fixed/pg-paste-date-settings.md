- PostgreSQL: `g p` no longer pastes a range of dates or times, or another value read as the
  server's text, as other dates when the session's `DateStyle`, `IntervalStyle` or `TimeZone`
  is not the one it was copied in (`[02/01/2026,05/01/2026)` copied in `SQL, DMY` and applied
  in `ISO, MDY` stored 1 February for 2 January); such a paste is refused, saying in which
  settings the cells were written and in which the session reads them. Text written in ISO
  pastes as before.
- PostgreSQL: a value pasted with `g p` from such a column is no longer applied as other dates
  when the session's `DateStyle`, `IntervalStyle` or `TimeZone` changed between the paste and
  the Apply (a `RESET`, the end of a `SET LOCAL`); the cell is marked invalid, saying why. The
  settings of a result are known only for a plain read the server confirms, and a copy of
  values typed or written back by an Apply counts as of settings not known.
- PostgreSQL: a time with a time zone written with the zone's abbreviation (any `DateStyle`
  but ISO) pastes only when written in ISO, since the abbreviation is read in each session's
  `timezone_abbreviations`; behind a pooler, results of a session whose Startup SQL goes in
  each transaction are of settings not known; and a value pasted again into a cell keeps the
  settings of the last paste.
- PostgreSQL: a value copied while pending (not applied yet) no longer pastes into a column of
  dates or times, and `g p` into such a column of a result read in other settings than the
  session's now is refused, so the same text is never taken for an unchanged value.
- PostgreSQL: a range or other value read as text, pasted or cloned, is applied only when it was
  written in ISO (`DateStyle` ISO), which reads the same whatever a trigger of the Apply sets;
  text written in other settings can be pasted but its cell is invalid until pasted again in
  ISO.
- PostgreSQL: text given to a column of dates, times or intervals (also a range, an array or a
  domain over one), typed, pasted or cloned, is applied only in a form read the same in any
  `DateStyle`, `TimeZone` and `IntervalStyle` (ISO dates, times with their offset, ISO 8601
  intervals), since a trigger may change them while the changes are applied; other text is
  marked invalid, saying which form to use. A type whose dates the app cannot tell takes no
  text from the grid.
- PostgreSQL: columns of base types that hold no dates (`citext`, `hstore`, `ltree`, geometry)
  stay editable; text typed or pasted from the clipboard into a date column of a result read
  in other settings than the session's now is refused, as a paste from the grid is.
