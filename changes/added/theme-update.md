- `:theme update [<name>]` adds to a theme file the tokens it does not list (such as tokens added
  after it was written), each under its description at the end of its section, commented out
  with the value it takes now. No line of the file changes, and nothing is written unless the
  file reads exactly as before.
- A theme file that does not list every token is said once in the status line when it is used,
  with how many and that `:theme update` adds them; again only when the tokens it does not list
  change.
