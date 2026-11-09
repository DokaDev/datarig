- The Monitor tab's session details show the statement in the editor's syntax colours and laid
  out for reading by the SQL formatter, for the eyes only: `f` shows it as the server sent it
  (or laid out again), and `y` and `o` always take the server's text. A statement known only in
  part (the list's first 256 characters, or cut by the server), one the formatter would change
  more than its blanks, or one over 64 KiB is shown as sent, and the details say why.
