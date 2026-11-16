- PostgreSQL: values the server sends as text and that hold dates, times or intervals (a range,
  a multirange, an array or a domain over one) are no longer pasted or applied as other dates
  when the session's `DateStyle`, `IntervalStyle` or `TimeZone` differs from the one they were
  read in (`[02/01/2026,05/01/2026)` copied in `SQL, DMY` and applied in `ISO, MDY` stored
  1 February for 2 January). `g p` into such a column is refused while the result was read in
  other settings than the session's now, and a copy of values still pending counts as of
  settings not known.
- PostgreSQL: text given to a column of dates, times or intervals, typed, pasted or cloned, is
  applied only in a form read the same in any `DateStyle`, `TimeZone` and `IntervalStyle` (ISO
  dates, times with a zone written with their offset, ISO 8601 intervals such as `P1DT2H`),
  since a trigger may change them while the changes are applied; other text is marked invalid,
  saying which form to use.
- PostgreSQL: columns of types whose dates the app cannot tell (a composite, a pseudo-type, a
  multirange of the database's own) take no text from the grid. Base types of extensions that
  hold no dates (`citext`, `hstore`, `ltree`, geometry) stay editable once the server has said
  what kind of type they are, which it is asked when their result comes.
- PostgreSQL: an `interval` copied in the grid or cloned with its row is written in ISO 8601
  from its exact value and applied, and a NULL pasted into a date column is never refused for
  the session's settings.
