- On MariaDB, the checks datarig runs before a count or a re-run, and each chunk of a table
  export, now give up after a short wait behind another session's lock instead of waiting
  without limit, and never wait longer than the session's own limits allow.
