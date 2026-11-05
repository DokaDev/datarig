- A new row in the result grid is tinted as a whole (theme token `edit_inserted_row`) and
  numbered after the rows read, with `+` (or `!`) after its number; rows an Apply added are
  numbered too. Its cells left to their default show what the server will put there instead of
  a dim `default`: the default's value (`ACTIVE`, `0`), `<generated>` for a number the table
  draws or a generated column, `<default: now()>` for another default, `NULL` where the column
  takes it. The Apply still leaves them to the server.
