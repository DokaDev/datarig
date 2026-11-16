- MySQL: a connection to a read replica that does not keep a session's read-only setting (some
  cloud reader endpoints) opens again. datarig's own metadata and monitor sessions run there
  without it, as they only send datarig's own reads; `:session` and `errors.log` say so. A
  read-only profile is still refused there, and the message names the values the server gave.
