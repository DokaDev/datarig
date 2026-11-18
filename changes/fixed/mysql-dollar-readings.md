- On MySQL, text with a `$` is now checked both as a server that reads dollar quotes reads it and
  as one that does not, and the stricter reading counts, since the version a server reports
  (for example through a proxy) may not tell which it is.
